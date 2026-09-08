#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Flowio skill 包读取 / 校验内核（纯标准库，零外部依赖）。

镜像 Rust 侧 `read_skill_package` + `SkillPackage::summary()`（src-tauri/src/export/commands.rs
约 1474–1588 行），保证导出 ⇄ 注册 ⇄ 消费三端同口径。任一引用缺失 / schema 不符都抛出
`SkillPackageError`，对应 Rust 的 `return Err(...)`，让消费方拒绝登记坏包，而非带着半截
文件去执行。

同时附带 `list_skills` / `load_steps` / `get_step` / `search_steps` 辅助，供 MCP server、
CLI、单测复用。

用法（库）：
    from skill_pkg import read_skill_package, list_skills, get_step, search_steps
    pkg = read_skill_package("/path/to/2026-09-08-183123-0d518f2b")
    print(pkg.summary())
    print(get_step(pkg.skill_md.parent, 7))
"""
from __future__ import annotations

import base64
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional

SCHEMA = "flowio.skill.manifest/v1"

# 截图扩展名 → MIME 类型（MCP image 块需要）
_MIME_BY_EXT = {
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".png": "image/png",
    ".webp": "image/webp",
    ".gif": "image/gif",
    ".bmp": "image/bmp",
}


class SkillPackageError(Exception):
    """skill 包不合法（schema 不符 / 引用文件缺失）时抛出，对应 Rust `Err(...)`。"""


@dataclass
class SkillPackage:
    name: str
    title: str
    app: str
    recorded_at: str
    skill_md: str
    screenshots_dir: str
    steps_file: Optional[str]
    step_count: int
    quality: Dict[str, int]

    def summary(self) -> Dict[str, Any]:
        """与 Rust `SkillPackage::summary()` 同构的 dict。"""
        return {
            "name": self.name,
            "title": self.title,
            "app": self.app,
            "recorded_at": self.recorded_at,
            "skill_md": self.skill_md,
            "screenshots_dir": self.screenshots_dir,
            "steps_file": self.steps_file,
            "step_count": self.step_count,
            "quality": {
                "total": self.quality.get("total", 0),
                "intent_vague": self.quality.get("intent_vague", 0),
                "with_point": self.quality.get("with_point", 0),
                "with_anchor": self.quality.get("with_anchor", 0),
                "with_shot": self.quality.get("with_shot", 0),
            },
        }


def _quality_from_manifest(manifest: Dict[str, Any]) -> Dict[str, int]:
    """从 manifest 的 quality 字段抽 5 项计数，缺省为 0（对应 Rust `#[serde(default)]`）。"""
    q = manifest.get("quality", {}) or {}
    return {
        "total": int(q.get("total", 0) or 0),
        # Rust 用 `#[serde(rename = "intent_vague")]` → JSON key 是 intent_vague
        "intent_vague": int(q.get("intent_vague", 0) or 0),
        "with_point": int(q.get("with_point", 0) or 0),
        "with_anchor": int(q.get("with_anchor", 0) or 0),
        # Rust 用 `#[serde(rename = "with_screenshot")]` → JSON key 是 with_screenshot
        "with_shot": int(q.get("with_screenshot", 0) or 0),
    }


def read_skill_package(dir_: str) -> SkillPackage:
    """读取并校验一个导出的 skill 包目录。

    逻辑逐行对齐 Rust `read_skill_package`（commands.rs:1545）：
      - 解析 skill.json，校验 schema == SCHEMA
      - 校验 files.skill（SKILL.md）存在且为文件
      - 校验 files.screenshots（目录）存在且为目录
      - 若 files.steps 非空，校验 steps.json 存在并读回步数
    返回 `SkillPackage`；任一校验失败抛 `SkillPackageError`。
    """
    d = Path(dir_)
    if not d.is_dir():
        raise SkillPackageError(f"skill 包目录不存在: {dir_}")

    manifest_path = d / "skill.json"
    if not manifest_path.is_file():
        raise SkillPackageError(f"skill.json 不存在: {manifest_path}")
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise SkillPackageError(f"解析 skill.json 失败: {e}")

    if manifest.get("schema") != SCHEMA:
        got = manifest.get("schema", "<缺失>")
        raise SkillPackageError(
            f"skill.json schema 不符，期望 {SCHEMA}，实得 {got}"
        )

    files = manifest.get("files", {}) or {}
    skill_rel = files.get("skill", "") or ""
    skill_path = d / skill_rel
    if not skill_rel or not skill_path.is_file():
        raise SkillPackageError(f"skill.json 引用的 SKILL.md（{skill_rel or '<空>'}）不存在")

    ss_rel = files.get("screenshots", "") or ""
    ss_dir = d / ss_rel
    if not ss_rel or not ss_dir.is_dir():
        raise SkillPackageError(f"screenshots 目录（{ss_rel or '<空>'}）不存在")

    steps_rel = files.get("steps", "") or ""
    steps_file: Optional[str] = None
    step_count = 0
    if steps_rel:
        sp = d / steps_rel
        if not sp.is_file():
            raise SkillPackageError(f"skill.json 引用的 steps.json（{steps_rel}）不存在")
        try:
            obj = json.loads(sp.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            raise SkillPackageError(f"解析 steps.json 失败: {e}")
        steps = obj.get("steps", []) or []
        step_count = len(steps)
        steps_file = str(sp)

    return SkillPackage(
        name=manifest.get("name", "") or "",
        title=manifest.get("title", "") or "",
        app=manifest.get("app", "") or "",
        recorded_at=manifest.get("recorded_at", "") or "",
        skill_md=str(skill_path),
        screenshots_dir=str(ss_dir),
        steps_file=steps_file,
        step_count=step_count,
        quality=_quality_from_manifest(manifest),
    )


# ── 辅助：步骤级读取 ──────────────────────────────────────────────

def _steps_path(dir_: str) -> Optional[Path]:
    """从 skill.json 推断 steps.json 绝对路径（无则 None）。"""
    d = Path(dir_)
    manifest_path = d / "skill.json"
    if not manifest_path.is_file():
        return None
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    steps_rel = (manifest.get("files", {}) or {}).get("steps", "") or ""
    if not steps_rel:
        return None
    sp = d / steps_rel
    return sp if sp.is_file() else None


def load_steps(dir_: str) -> List[Dict[str, Any]]:
    """读取 steps.json 的全部步骤数组；无 steps 文件返回空列表。"""
    sp = _steps_path(dir_)
    if sp is None:
        return []
    try:
        obj = json.loads(sp.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise SkillPackageError(f"解析 steps.json 失败: {e}")
    return obj.get("steps", []) or []


def get_step(dir_: str, n: int) -> Optional[Dict[str, Any]]:
    """取回指定步号 `n` 的步骤；命中则附带 `screenshot_abs`（截图绝对路径）。

    步号以 steps.json 中每步的 `n` 字段为准（与 SKILL.md 编号、screenshots/step-NN 对应），
    而非数组下标 —— 录制端偶发步号不连续，必须按字段匹配。
    """
    for s in load_steps(dir_):
        if int(s.get("n", -1)) == n:
            out = dict(s)
            shot_rel = s.get("screenshot")
            if shot_rel:
                out["screenshot_abs"] = str((Path(dir_) / shot_rel).resolve())
            return out
    return None


def search_steps(dir_: str, keyword: str) -> List[Dict[str, Any]]:
    """在 steps.json 中按关键词检索；匹配字段：intent / window / action / text / hotkey。

    大小写不敏感；keyword 为空返回空列表。返回命中步骤的概要（n/action/intent/window +
    截图绝对路径）。
    """
    kw = (keyword or "").strip().lower()
    if not kw:
        return []
    hits: List[Dict[str, Any]] = []
    for s in load_steps(dir_):
        hay = " ".join(
            str(s.get(k, "") or "")
            for k in ("intent", "window", "action", "text", "hotkey")
        ).lower()
        if kw in hay:
            item = {
                "n": s.get("n"),
                "action": s.get("action"),
                "intent": s.get("intent"),
                "window": s.get("window"),
                "intent_vague": s.get("intent_vague", False),
            }
            shot_rel = s.get("screenshot")
            if shot_rel:
                item["screenshot_abs"] = str((Path(dir_) / shot_rel).resolve())
            hits.append(item)
    return hits


# ── 辅助：批量发现 ────────────────────────────────────────────────

def list_skills(base_dir: str, include_errors: bool = False) -> List[Dict[str, Any]]:
    """扫描 `base_dir` 的直接子目录，对每个尝试 `read_skill_package`。

    仅返回校验通过的包摘要；`include_errors=True` 时同时返回形如
    `{"dir":..., "error":...}` 的失败项（便于诊断破损包）。
    """
    base = Path(base_dir)
    if not base.is_dir():
        raise SkillPackageError(f"base_dir 不存在: {base_dir}")
    out: List[Dict[str, Any]] = []
    for child in sorted(base.iterdir()):
        if not child.is_dir():
            continue
        try:
            pkg = read_skill_package(str(child))
        except SkillPackageError as e:
            if include_errors:
                out.append({"dir": str(child), "error": str(e)})
            continue
        out.append(pkg.summary())
    return out


def read_image_block(screenshot_abs: str) -> Optional[Dict[str, str]]:
    """读截图文件为 MCP image 内容块（base64 + MIME），文件不存在/不可读返回 None。"""
    p = Path(screenshot_abs)
    if not p.is_file():
        return None
    mime = _MIME_BY_EXT.get(p.suffix.lower())
    if mime is None:
        return None
    try:
        data = base64.b64encode(p.read_bytes()).decode("ascii")
    except OSError:
        return None
    return {"type": "image", "data": data, "mimeType": mime}
