#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Flowio 技能包 → 自动化执行计划（纯标准库，零外部依赖）。

这是 route ②「录制 ⇄ 双教程 ⇄ 自动化」链条的**最后一截桥**：把已导出的技能包
（SKILL.md 人读教程 + skill.json/steps.json 机器数据）转成一份**逐步骤可执行计划**，
让 MCP host / AI agent 能够按步消费并真正驱动桌面端重放。

- 对每步标注执行方式（classify）：
  * exact  —— 有精确屏幕坐标（point / anchor.ax,ay / anchor.wx,wy），可直接下发点击/输入；
  * vision —— 只有截图没有坐标，需结合截图（视觉定位）执行；
  * manual —— 既无坐标也无截图，需人工介入或 LLM 临场决策。
  这恰好对应「双教程」的两面：人读 SKILL.md、机器读 steps.json 拿坐标/截图。

用法：
    python run_skill.py <skill_dir>              # 打印人类可读执行计划
    python run_skill.py <skill_dir> --json       # 打印 JSON 计划（供程序消费）
    python run_skill.py <skill_dir> --check      # 仅统计可执行性（exact/vision/manual 占比）
    python run_skill.py <skill_dir> --mcp-hint   # 打印每步对应的 MCP 工具调用示意

不依赖 Tauri / Rust，可在任意环境跑（含本沙箱），用于验证「导出 ⇄ 注册 ⇄ 执行」闭环
是否真的可消费；真机执行时由 MCP host 拿 plan + 截图去驱动桌面端。
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from skill_pkg import SkillPackageError, load_steps, read_skill_package


# 动作类型 → 人类可读动词（仅用于计划展示，不影响执行判定）
_VERB = {
    "click": "点击",
    "double_click": "双击",
    "right_click": "右键",
    "type": "输入文字",
    "input": "输入文字",
    "hotkey": "组合键",
    "copy": "复制",
    "paste": "粘贴",
    "drag": "拖拽",
    "scroll": "滚动",
    "wait": "等待",
    "screenshot": "截图",
}


def _coord_of(step: Dict[str, Any]) -> Optional[Tuple[int, int]]:
    """抽取步骤的屏幕坐标（绝对像素），返回 (x, y) 或 None。

    对齐 Rust 导出侧实际 schema（见 D:/tmp/skill_fixed/steps.json）：
      1. point —— 导出侧为 [x, y] 数组（绝对屏幕像素），最优先；
      2. point —— 兼容 {x, y} 字典形态；
      3. anchor.ax / anchor.ay —— 绝对像素（导出侧 anchor 同时带 ax/ay 与归一化 wx/wy）。
    注意：anchor.wx / anchor.wy 是归一化比例（0..1），须配窗口矩形才能还原成像素，
    这里**不**返回它们，避免把比例误当作像素坐标下发给桌面端重放。
    """
    pt = step.get("point")
    if isinstance(pt, (list, tuple)) and len(pt) == 2:
        try:
            x, y = int(pt[0]), int(pt[1])
            if x >= 0 and y >= 0:
                return (x, y)
        except (TypeError, ValueError):
            pass
    if isinstance(pt, dict) and "x" in pt and "y" in pt:
        try:
            x, y = int(pt["x"]), int(pt["y"])
            if x >= 0 and y >= 0:
                return (x, y)
        except (TypeError, ValueError):
            pass
    anchor = step.get("anchor") or {}
    ax = anchor.get("ax")
    ay = anchor.get("ay")
    if isinstance(ax, (int, float)) and isinstance(ay, (int, float)) and ax >= 0 and ay >= 0:
        return (int(ax), int(ay))
    return None


def classify(step: Dict[str, Any]) -> Tuple[str, str]:
    """返回 (mode, reason)。

    mode ∈ {"exact", "vision", "manual"}：
      exact  —— 有精确坐标，可直接下发；
      vision —— 仅截图，需视觉定位；
      manual —— 既无坐标也无截图，需人工/LLM 临场决策。
    """
    if _coord_of(step) is not None:
        return "exact", "有精确屏幕坐标，可直接下发"
    if step.get("screenshot"):
        return "vision", "仅有截图，需结合截图视觉定位"
    return "manual", "既无坐标也无截图，需人工介入或 LLM 临场决策"


def _verb_of(step: Dict[str, Any]) -> str:
    action = step.get("action") or "unknown"
    return _VERB.get(action, action)


def step_plan(step: Dict[str, Any], dir_: str) -> Dict[str, Any]:
    """把单步转成计划条目。"""
    n = step.get("n")
    mode, reason = classify(step)
    coord = _coord_of(step)
    entry: Dict[str, Any] = {
        "n": n,
        "action": step.get("action"),
        "verb": _verb_of(step),
        "window": step.get("window"),
        "intent": step.get("intent"),
        "text": step.get("text"),
        "hotkey": step.get("hotkey"),
        "has_screenshot": bool(step.get("screenshot")),
        "coord": list(coord) if coord else None,
        "executable": mode,  # exact / vision / manual
        "exec_note": reason,
    }
    shot = step.get("screenshot")
    if shot:
        entry["screenshot_abs"] = str((Path(dir_) / shot).resolve())
    return entry


def build_plan_from_steps(steps: List[Dict[str, Any]], dir_: str) -> List[Dict[str, Any]]:
    """纯函数：步骤数组 → 计划列表（不依赖完整包，便于单测）。"""
    return [step_plan(s, dir_) for s in steps]


def build_plan(dir_: str) -> Tuple[Dict[str, Any], List[Dict[str, Any]]]:
    """读取技能包并返回 (summary, plan)。抛 SkillPackageError 表示坏包。"""
    pkg = read_skill_package(dir_)
    steps = load_steps(dir_)
    return pkg.summary(), build_plan_from_steps(steps, dir_)


def check_report(plan: List[Dict[str, Any]]) -> Dict[str, Any]:
    """统计可执行性。"""
    counts = {"exact": 0, "vision": 0, "manual": 0}
    for e in plan:
        counts[e["executable"]] = counts.get(e["executable"], 0) + 1
    total = len(plan) or 1
    return {
        "total": len(plan),
        "exact": counts["exact"],
        "vision": counts["vision"],
        "manual": counts["manual"],
        "auto_rate": round((counts["exact"] + counts["vision"]) / total * 100, 1),
    }


def _mcp_hint(plan: List[Dict[str, Any]]) -> List[str]:
    """生成每步对应的 MCP 工具调用示意（给 agent 看），并标注执行方式。"""
    lines: List[str] = []
    for e in plan:
        if e["executable"] == "manual":
            lines.append(
                f"# 步 {e['n']} [manual]（{e['verb']}）：无坐标无截图 → "
                f"flowio_get_step(n={e['n']}) 取 SKILL.md 描述，由 LLM 临场决策"
            )
            continue
        extra = ""
        if e["text"]:
            extra = f" text={e['text']!r}"
        if e["hotkey"]:
            extra = f" combo={e['hotkey']!r}"
        coord_note = f" coord={e['coord']}" if e["coord"] else " (视觉定位)"
        mode = "exact" if e["coord"] else "vision"
        lines.append(
            f"# 步 {e['n']} [{mode}]（{e['verb']}）：flowio_get_step(n={e['n']}){coord_note}{extra} → 下发 {e['action']}"
        )
    return lines


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="Flowio 技能包 → 自动化执行计划")
    ap.add_argument("dir", help="导出的技能包目录")
    ap.add_argument("--json", action="store_true", help="以 JSON 输出计划")
    ap.add_argument("--check", action="store_true", help="仅统计可执行性")
    ap.add_argument("--mcp-hint", action="store_true", help="打印每步 MCP 工具调用示意")
    args = ap.parse_args(argv)

    try:
        summary, plan = build_plan(args.dir)
    except SkillPackageError as e:
        print(f"[错误] 技能包不合法，拒绝生成计划：{e}", file=sys.stderr)
        return 2

    if args.check:
        print(json.dumps(check_report(plan), ensure_ascii=False, indent=2))
        return 0

    if args.mcp_hint:
        for line in _mcp_hint(plan):
            print(line)
        return 0

    if args.json:
        print(json.dumps({"summary": summary, "plan": plan}, ensure_ascii=False, indent=2))
        return 0

    # 默认：人类可读计划
    rep = check_report(plan)
    print(f"# 技能包：{summary['title']}（{summary['name']}）")
    print(f"# 应用：{summary.get('app') or '<未知>'}  步数：{summary['step_count']}  "
          f"可执行率：{rep['auto_rate']}%  (exact={rep['exact']} vision={rep['vision']} manual={rep['manual']})")
    print()
    for e in plan:
        coord = e["coord"] if e["coord"] else ("(视觉定位)" if e["executable"] == "vision" else "(需人工)")
        detail = ""
        if e["text"]:
            detail = f" 文本={e['text']!r}"
        if e["hotkey"]:
            detail = f" 组合键={e['hotkey']!r}"
        if e["window"]:
            detail += f" 窗口={e['window']!r}"
        print(f"步 {e['n']:>2}  [{e['executable']:>6}] {e['verb']:<4} {coord}{detail}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
