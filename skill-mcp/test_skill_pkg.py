#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""skill-mcp 自测：构造 fixture + 真实包双验证 + stdio JSON-RPC 冒烟。

运行（走 managed python，离线可用）：
    python test_skill_pkg.py
退出码 0 = 全绿；非 0 = 有失败。
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REAL_PKG = Path(r"D:/tmp/skill_fixed")  # 真实导出包（未改动，作为黄金 fixture）

# 若真实包不在，跳过真实包断言（fixture 仍覆盖核心逻辑）
HAVE_REAL = REAL_PKG.is_dir()

sys.path.insert(0, str(HERE))
from skill_pkg import (  # noqa: E402
    SkillPackageError,
    get_step,
    list_skills,
    read_skill_package,
    search_steps,
)


def _fail(msg: str) -> None:
    print(f"  ✗ {msg}")
    raise AssertionError(msg)


def _ok(msg: str) -> None:
    print(f"  ✓ {msg}")


def build_fixture(root: Path) -> Path:
    """构造一个最小但合法的 skill 包，返回其目录。"""
    pkg = root / "demo-2026-01-01-aaaaaaaa"
    pkg.mkdir()
    (pkg / "SKILL.md").write_text("# 演示 skill\n\n1. 点击\n2. 输入\n", encoding="utf-8")
    (pkg / "screenshots").mkdir()
    (pkg / "screenshots" / "step-01.jpg").write_bytes(b"\xff\xd8\xff\xe0demo")
    manifest = {
        "schema": "flowio.skill.manifest/v1",
        "name": "demo-2026-01-01-aaaaaaaa",
        "title": "演示录制",
        "app": "记事本",
        "recorded_at": "2026-01-01 09:00:00",
        "files": {"skill": "SKILL.md", "screenshots": "screenshots/", "steps": "steps.json"},
        "quality": {
            "total": 2,
            "intent_vague": 1,
            "with_point": 2,
            "with_anchor": 1,
            "with_screenshot": 2,
        },
    }
    steps = {
        "skill": "demo-2026-01-01-aaaaaaaa",
        "title": "演示录制",
        "app": "记事本",
        "recorded_at": "2026-01-01 09:00:00",
        "steps": [
            {
                "n": 1,
                "action": "click",
                "intent": "点击",
                "intent_vague": True,
                "window": "",
                "point": [10, 20],
                "anchor": {"cls": "Edit", "fx": 0.1, "fy": 0.2},
                "screenshot": "screenshots/step-01.jpg",
            },
            {
                "n": 2,
                "action": "type",
                "intent": "在「记事本」中输入",
                "intent_vague": False,
                "window": "记事本",
                "text": "hello",
                "screenshot": None,
            },
        ],
    }
    (pkg / "skill.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    (pkg / "steps.json").write_text(
        json.dumps(steps, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    return pkg


def test_fixture() -> None:
    print("[1] fixture 包读包 / 校验 / summary")
    with tempfile.TemporaryDirectory() as tmp:
        pkg_dir = build_fixture(Path(tmp))
        pkg = read_skill_package(str(pkg_dir))
        s = pkg.summary()
        assert s["name"] == "demo-2026-01-01-aaaaaaaa", _fail("name 不符")
        assert s["step_count"] == 2, _fail("step_count 不符")
        assert s["quality"]["total"] == 2, _fail("quality.total 不符")
        assert s["quality"]["intent_vague"] == 1, _fail("quality.intent_vague 不符")
        assert s["quality"]["with_point"] == 2, _fail("quality.with_point 不符")
        assert s["quality"]["with_anchor"] == 1, _fail("quality.with_anchor 不符")
        assert s["quality"]["with_shot"] == 2, _fail("quality.with_shot 不符")
        assert s["steps_file"] == str(pkg_dir / "steps.json"), _fail("steps_file 不符")
        _ok("summary 字段与 Rust 同构")

        # 坏包：缺 SKILL.md
        bad = Path(tmp) / "bad"
        bad.mkdir()
        (bad / "skill.json").write_text(
            json.dumps(
                {"schema": "flowio.skill.manifest/v1", "files": {"skill": "X.md", "screenshots": "shots/"}},
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        try:
            read_skill_package(str(bad))
            _fail("坏包（缺 SKILL.md）应抛错却通过")
        except SkillPackageError:
            _ok("坏包（缺 SKILL.md）被正确拒绝")

        # 坏 schema
        bad2 = Path(tmp) / "bad2"
        bad2.mkdir()
        (bad2 / "skill.json").write_text(
            json.dumps({"schema": "wrong/v1"}, ensure_ascii=False), encoding="utf-8"
        )
        try:
            read_skill_package(str(bad2))
            _fail("坏 schema 应抛错却通过")
        except SkillPackageError:
            _ok("坏 schema 被正确拒绝")

        # list_skills 扫描父目录，应只发现 demo 包
        summaries = list_skills(tmp)
        assert len(summaries) == 1, _fail("list_skills 应发现 1 个包")
        assert summaries[0]["name"].startswith("demo"), _fail("list_skills 返回名不符")
        _ok("list_skills 只返回合法包")

        # get_step 按 n 命中 + 截图绝对路径
        st = get_step(str(pkg_dir), 1)
        assert st is not None and st["action"] == "click", _fail("get_step(1) 不符")
        assert Path(st["screenshot_abs"]).is_file(), _fail("screenshot_abs 未解析/不存在")
        assert get_step(str(pkg_dir), 99) is None, _fail("get_step(99) 应返回 None")
        _ok("get_step 按 n 命中且解析截图路径")

        # search_steps 关键词（大小写不敏感）：仅第 2 步含「记事本」
        hits = search_steps(str(pkg_dir), "记事本")
        assert len(hits) == 1, _fail("search 记事本 应命中 1 步")
        hits2 = search_steps(str(pkg_dir), "hello")
        assert len(hits2) == 1 and hits2[0]["n"] == 2, _fail("search hello 应命中第 2 步")
        assert search_steps(str(pkg_dir), "") == [], _fail("空关键词应返回 []")
        _ok("search_steps 命中正确")


def test_real() -> None:
    print("[2] 真实导出包 /d/tmp/skill_fixed 断言")
    if not HAVE_REAL:
        print("  ⊘ 真实包不在，跳过（fixture 已覆盖核心逻辑）")
        return
    pkg = read_skill_package(str(REAL_PKG))
    s = pkg.summary()
    expect = {
        "name": "2026-09-08-183123-0d518f2b",
        "app": "函数参数",
        "step_count": 19,
    }
    assert s["name"] == expect["name"], _fail(f"name 应为 {expect['name']}")
    assert s["app"] == expect["app"], _fail(f"app 应为 {expect['app']}")
    assert s["step_count"] == 19, _fail("真实包步数应为 19")
    q = s["quality"]
    assert q["total"] == 19, _fail("quality.total 应为 19")
    assert q["intent_vague"] == 14, _fail("quality.intent_vague 应为 14")
    assert q["with_point"] == 18, _fail("quality.with_point 应为 18")
    assert q["with_anchor"] == 14, _fail("quality.with_anchor 应为 14")
    assert q["with_shot"] == 19, _fail("quality.with_shot 应为 19")
    _ok("真实包 summary 与 Rust 写侧产物逐字段一致")

    # get_step: 第 7 步是 type，text 应为 "sum"
    st = get_step(str(REAL_PKG), 7)
    assert st is not None and st.get("text") == "sum", _fail("第 7 步 text 应为 sum")
    assert Path(st["screenshot_abs"]).is_file(), _fail("第 7 步截图不存在")
    _ok("get_step(7) 返回 type 文本 sum + 截图")

    # search: WPS 应命中窗口名为 WPS 的步骤
    hits = search_steps(str(REAL_PKG), "WPS")
    assert len(hits) >= 1, _fail("search WPS 应至少命中 1 步")
    _ok(f"search WPS 命中 {len(hits)} 步")


def test_stdio_smoke() -> None:
    print("[3] stdio JSON-RPC 冒烟（启动 server.py 走完整握手）")
    if not HAVE_REAL:
        print("  ⊘ 真实包不在，跳过 get_step 截图断言（握手仍验证）")
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    ]
    if HAVE_REAL:
        requests += [
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "flowio_load_skill",
                    "arguments": {"dir": str(REAL_PKG)},
                },
            },
            {
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "flowio_get_step",
                    "arguments": {"dir": str(REAL_PKG), "n": 7},
                },
            },
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "flowio_search_steps",
                    "arguments": {"dir": str(REAL_PKG), "keyword": "WPS"},
                },
            },
            {
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "flowio_list_skills",
                    "arguments": {"base_dir": str(REAL_PKG.parent)},
                },
            },
        ]

    proc = subprocess.Popen(
        [sys.executable, str(HERE / "server.py")],
        cwd=str(HERE),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    stdin_data = "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in requests)
    out, err = proc.communicate(input=stdin_data, timeout=30)
    if proc.returncode != 0:
        raise AssertionError(f"server 退出码 {proc.returncode}; stderr={err[:500]}")

    lines = [ln for ln in out.splitlines() if ln.strip()]
    assert len(lines) == len(requests), _fail(
        f"响应行数 {len(lines)} != 请求数 {len(requests)}"
    )
    responses = {}
    for ln in lines:
        msg = json.loads(ln)
        responses[msg["id"]] = msg

    # initialize
    init = responses[1]["result"]
    assert init["protocolVersion"] == "2024-11-05", _fail("protocolVersion 不符")
    assert init["capabilities"]["tools"]["listChanged"] is True, _fail("capabilities 不符")
    assert init["serverInfo"]["name"] == "flowio-skill-mcp", _fail("serverInfo 不符")
    _ok("initialize 握手正常")

    # tools/list
    tools = responses[2]["result"]["tools"]
    names = {t["name"] for t in tools}
    assert names == {
        "flowio_list_skills",
        "flowio_load_skill",
        "flowio_get_step",
        "flowio_search_steps",
    }, _fail(f"工具清单不符: {names}")
    _ok("tools/list 返回 4 个工具")

    if HAVE_REAL:
        # load_skill
        load_res = responses[3]["result"]
        assert load_res["isError"] is False, _fail("load_skill 不应报错")
        load_text = load_res["content"][0]["text"]
        load_json = json.loads(load_text)
        assert load_json["step_count"] == 19, _fail("load_skill 步数不符")
        _ok("tools/call flowio_load_skill 返回 19 步摘要")

        # get_step（含 image 块）
        gs = responses[4]["result"]
        assert gs["isError"] is False, _fail("get_step 不应报错")
        types = [c["type"] for c in gs["content"]]
        assert "image" in types, _fail("get_step 应含 image 块")
        _ok("tools/call flowio_get_step 返回 text + image")

        # search_steps
        sr = responses[5]["result"]
        assert sr["isError"] is False, _fail("search 不应报错")
        sr_hits = json.loads(sr["content"][0]["text"])
        assert len(sr_hits) >= 1, _fail("search WPS 应命中")
        _ok(f"tools/call flowio_search_steps 命中 {len(sr_hits)} 步")

        # list_skills
        ls = responses[6]["result"]
        assert ls["isError"] is False, _fail("list 不应报错")
        ls_pkgs = json.loads(ls["content"][0]["text"])
        assert any(p["name"].startswith("2026-09-08") for p in ls_pkgs), _fail("list 未发现真实包")
        _ok("tools/call flowio_list_skills 发现真实包")


def main() -> None:
    print("=== skill-mcp 自测 ===")
    test_fixture()
    test_real()
    test_stdio_smoke()
    print("\n✅ 全部通过")


if __name__ == "__main__":
    main()
