#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Flowio 最小 MCP 消费端（零依赖，纯标准库 stdio JSON-RPC 2.0 server）。

把本地导出的 Flowio skill 包（SKILL.md + screenshots/ + steps.json + skill.json）暴露成
AI agent 可真正调用 / 查看的工具，闭合 route ②「导出 ⇄ 注册 ⇄ 执行」全链路。

传输：MCP stdio 传输采用「换行分隔的 JSON-RPC 消息」，每条消息独占一行。
协议：initialize / tools/list / tools/call（+ ping、notifications/initialized）。

暴露 4 个工具：
  - flowio_list_skills  列出某目录下所有已导出的 skill 包摘要
  - flowio_load_skill   读取并校验单个 skill 包（坏包报错而非半截数据）
  - flowio_get_step     取回指定步号详情 + 对应截图（image 块）
  - flowio_search_steps 按关键词检索步骤（intent/window/action/text/hotkey）

零外部依赖，可在离线环境直接运行：
    python server.py
MCP 客户端配置（以 mcp.json / claude_desktop_config.json 为例）：
    {"mcpServers": {"flowio-skill": {"command": "python",
      "args": ["/绝对路径/flowio/skill-mcp/server.py"],
      "env": {"FLOWIO_SKILL_DIR": "/默认/skill/包父目录"}}}}
"""
from __future__ import annotations

import json
import os
import sys
from typing import Any, Dict, List, Optional

from skill_pkg import (
    SkillPackageError,
    get_step,
    list_skills,
    read_image_block,
    read_skill_package,
    search_steps,
)

PROTOCOL_VERSION = "2024-11-05"
SERVER_NAME = "flowio-skill-mcp"
SERVER_VERSION = "1.0.0"


# ── 工具清单 ──────────────────────────────────────────────────────

TOOLS: List[Dict[str, Any]] = [
    {
        "name": "flowio_list_skills",
        "description": (
            "列出某个目录下所有已导出的 Flowio skill 包（每个子目录一个包）。"
            "返回每个包的规范摘要：name / title / app / recorded_at / 步数 step_count / "
            "质量统计 quality{total,intent_vague,with_point,with_anchor,with_shot}。"
            "用于在消费端发现可用的录屏教程。坏包会被跳过（不返回）。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "base_dir": {
                    "type": "string",
                    "description": (
                        "包含若干 skill 包子目录的父目录绝对路径。"
                        "缺省时读取环境变量 FLOWIO_SKILL_DIR，再缺省则用当前工作目录。"
                    ),
                }
            },
            "required": [],
        },
    },
    {
        "name": "flowio_load_skill",
        "description": (
            "读取并校验单个 Flowio skill 包目录，返回其规范摘要（同 list 的单条）。"
            "会校验 schema（flowio.skill.manifest/v1）与引用文件完整性（SKILL.md / "
            "screenshots/ / steps.json）。坏包会返回错误而非半截数据，便于 MCP host 拒绝登记。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "dir": {
                    "type": "string",
                    "description": "skill 包目录绝对路径（内含 skill.json / SKILL.md / screenshots/）",
                }
            },
            "required": ["dir"],
        },
    },
    {
        "name": "flowio_get_step",
        "description": (
            "取回某 skill 包中指定步号 n 的详细步骤：动作类型 action、意图 intent、"
            "目标窗口 window、落点坐标 point、控件锚点 anchor、待输入文本 text / 组合键 hotkey，"
            "以及对应截图（作为 image 块返回，供视觉定位）。"
            "intent_vague=true 的步骤意图含糊，应以截图为准而非文字描述。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "dir": {
                    "type": "string",
                    "description": "skill 包目录绝对路径",
                },
                "n": {
                    "type": "integer",
                    "description": "步号，对应 SKILL.md 的编号与 screenshots/step-NN",
                },
            },
            "required": ["dir", "n"],
        },
    },
    {
        "name": "flowio_search_steps",
        "description": (
            "在 skill 包的 steps.json 中按关键词检索步骤。匹配字段：intent（意图标题）、"
            "window（目标窗口）、action（动作类型）、text（输入文本）、hotkey（组合键）。"
            "大小写不敏感。返回命中步骤的概要（n / action / intent / window / 截图路径）。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "dir": {"type": "string", "description": "skill 包目录绝对路径"},
                "keyword": {"type": "string", "description": "检索关键词"},
            },
            "required": ["dir", "keyword"],
        },
    },
]


# ── 工具分发 ──────────────────────────────────────────────────────

def _default_base_dir() -> str:
    return os.environ.get("FLOWIO_SKILL_DIR") or os.getcwd()


def _call_tool(name: str, args: Dict[str, Any]) -> Dict[str, Any]:
    """执行工具，返回 MCP `tools/call` 的 result 体（content + isError）。"""
    try:
        if name == "flowio_list_skills":
            base_dir = args.get("base_dir") or _default_base_dir()
            summaries = list_skills(base_dir)
            text = json.dumps(summaries, ensure_ascii=False, indent=2)
            return {
                "content": [{"type": "text", "text": text}],
                "isError": False,
            }

        if name == "flowio_load_skill":
            d = args.get("dir")
            if not d:
                raise SkillPackageError("缺少参数 dir")
            pkg = read_skill_package(d)
            text = json.dumps(pkg.summary(), ensure_ascii=False, indent=2)
            return {
                "content": [{"type": "text", "text": text}],
                "isError": False,
            }

        if name == "flowio_get_step":
            d = args.get("dir")
            n = args.get("n")
            if not d or n is None:
                raise SkillPackageError("缺少参数 dir / n")
            step = get_step(d, int(n))
            if step is None:
                raise SkillPackageError(f"包 {d} 中不存在步号 {n}")
            text = json.dumps(step, ensure_ascii=False, indent=2)
            content: List[Dict[str, Any]] = [{"type": "text", "text": text}]
            shot = step.get("screenshot_abs")
            if shot:
                img = read_image_block(shot)
                if img is not None:
                    content.append(img)
            return {"content": content, "isError": False}

        if name == "flowio_search_steps":
            d = args.get("dir")
            kw = args.get("keyword", "")
            if not d:
                raise SkillPackageError("缺少参数 dir")
            hits = search_steps(d, kw)
            text = json.dumps(hits, ensure_ascii=False, indent=2)
            return {
                "content": [{"type": "text", "text": text}],
                "isError": False,
            }

        raise SkillPackageError(f"未知工具: {name}")

    except SkillPackageError as e:
        return {
            "content": [{"type": "text", "text": f"错误: {e}"}],
            "isError": True,
        }
    except Exception as e:  # noqa: BLE001 - 兜底，避免整个 server 崩
        return {
            "content": [{"type": "text", "text": f"内部错误: {e}"}],
            "isError": True,
        }


# ── JSON-RPC 2.0 调度 ─────────────────────────────────────────────

def _handle(msg: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    method = msg.get("method")
    msg_id = msg.get("id")  # 有 id = 请求；无 id = 通知

    # 通知：无需回复
    if method == "notifications/initialized":
        return None
    if method == "notifications/cancelled":
        return None

    if method == "ping":
        return _resp(msg_id, result={})

    if method == "initialize":
        return _resp(
            msg_id,
            result={
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": True}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
            },
        )

    if method == "tools/list":
        return _resp(msg_id, result={"tools": TOOLS})

    if method == "tools/call":
        params = msg.get("params", {}) or {}
        name = params.get("name", "")
        arguments = params.get("arguments", {}) or {}
        result = _call_tool(name, arguments)
        return _resp(msg_id, result=result)

    # 未知方法
    if msg_id is not None:
        return {
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": f"方法未实现: {method}"},
        }
    return None


def _resp(msg_id: Any, result: Any) -> Dict[str, Any]:
    return {"jsonrpc": "2.0", "id": msg_id, "result": result}


def main() -> None:
    # 日志走 stderr，绝不污染 stdout 的 JSON-RPC 通道
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        try:
            out = _handle(msg)
        except Exception as e:  # noqa: BLE001
            sys.stderr.write(f"[flowio-mcp] handle error: {e}\n")
            out = None
        if out is not None:
            sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()
