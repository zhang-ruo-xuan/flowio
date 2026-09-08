#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""run_skill.py 自动化执行桥的自测（零依赖，走 managed python 即可跑）。

覆盖：
  - classify / _coord_of 三类判定（exact / vision / manual）
  - build_plan_from_steps / build_plan / check_report 纯函数
  - 四种 CLI 模式（默认 / --json / --check / --mcp-hint）的端到端输出
  - 坏包（目录不存在 / skill.json 缺失）被正确拒绝（退出码 2）

fixture 在 tempdir 内构造，不入库；不依赖外部真实导出包。
"""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent

sys.path.insert(0, str(HERE))
from run_skill import (  # noqa: E402
    _coord_of,
    build_plan,
    build_plan_from_steps,
    check_report,
    classify,
    main,
)
from skill_pkg import SkillPackageError  # noqa: E402


def _fail(msg: str) -> None:
    print(f"  ✗ {msg}")
    raise AssertionError(msg)


def _ok(msg: str) -> None:
    print(f"  ✓ {msg}")


def build_fixture(root: Path) -> Path:
    """构造含 exact/vision/manual 三类的合法 skill 包，返回目录。"""
    pkg = root / "demo-2026-01-01-aaaaaaaa"
    pkg.mkdir()
    (pkg / "SKILL.md").write_text(
        "# 演示 skill\n\n1. 点击坐标点\n2. 视觉点击\n3. 人工决断\n", encoding="utf-8"
    )
    (pkg / "screenshots").mkdir()
    (pkg / "screenshots" / "step-01.jpg").write_bytes(b"\xff\xd8\xff\xe0demo")
    (pkg / "screenshots" / "step-02.jpg").write_bytes(b"\xff\xd8\xff\xe0demo")
    manifest = {
        "schema": "flowio.skill.manifest/v1",
        "name": "demo-2026-01-01-aaaaaaaa",
        "title": "演示录制",
        "app": "记事本",
        "recorded_at": "2026-01-01 09:00:00",
        "files": {
            "skill": "SKILL.md",
            "screenshots": "screenshots/",
            "steps": "steps.json",
        },
        "quality": {
            "total": 3,
            "intent_vague": 1,
            "with_point": 1,
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
            {  # exact：有 point
                "n": 1,
                "action": "click",
                "intent": "点击坐标点",
                "window": "",
                "point": [10, 20],
                "anchor": {"cls": "Edit", "fx": 0.1, "fy": 0.2},
                "screenshot": "screenshots/step-01.jpg",
            },
            {  # vision：仅截图无坐标
                "n": 2,
                "action": "click",
                "intent": "视觉点击",
                "window": "记事本",
                "screenshot": "screenshots/step-02.jpg",
            },
            {  # manual：无坐标无截图
                "n": 3,
                "action": "type",
                "intent": "人工决断",
                "text": "hello",
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


def test_unit() -> None:
    print("[1] run_skill 纯函数：classify / _coord_of / plan / report")
    with tempfile.TemporaryDirectory() as tmp:
        pkg_dir = build_fixture(Path(tmp))
        steps = json.loads((pkg_dir / "steps.json").read_text(encoding="utf-8"))["steps"]

        # _coord_of
        assert _coord_of(steps[0]) == (10, 20), _fail("_coord_of 步1 应返回 (10,20)")
        assert _coord_of(steps[1]) is None, _fail("_coord_of 步2 应返回 None")
        _ok("_coord_of 抽点正确")

        # classify
        assert classify(steps[0])[0] == "exact", _fail("步1 应为 exact")
        assert classify(steps[1])[0] == "vision", _fail("步2 应为 vision")
        assert classify(steps[2])[0] == "manual", _fail("步3 应为 manual")
        _ok("classify 三类判定正确")

        # build_plan_from_steps
        plan = build_plan_from_steps(steps, str(pkg_dir))
        assert len(plan) == 3, _fail("plan 长度应为 3")
        assert plan[0]["executable"] == "exact" and plan[0]["coord"] == [10, 20], _fail("plan[0] 不符")
        assert plan[1]["executable"] == "vision" and plan[1]["has_screenshot"], _fail("plan[1] 不符")
        assert plan[2]["executable"] == "manual", _fail("plan[2] 不符")
        assert "screenshot_abs" in plan[0] and "screenshot_abs" in plan[1], _fail("截图绝对路径缺失")
        _ok("build_plan_from_steps 标注完整")

        # build_plan（含 summary）
        summary, plan2 = build_plan(str(pkg_dir))
        assert summary["step_count"] == 3, _fail("summary.step_count 应为 3")
        assert len(plan2) == 3, _fail("build_plan 返回 plan 长度不符")
        _ok("build_plan 返回 summary + plan")

        # check_report
        rep = check_report(plan)
        assert rep["total"] == 3, _fail("report.total 应为 3")
        assert rep["exact"] == 1 and rep["vision"] == 1 and rep["manual"] == 1, _fail("report 计数不符")
        assert rep["auto_rate"] == 66.7, _fail(f"report.auto_rate 应为 66.7，实得 {rep['auto_rate']}")
        _ok("check_report 统计正确（auto_rate=66.7%）")


def test_cli() -> None:
    print("[2] run_skill CLI 四种模式端到端")
    with tempfile.TemporaryDirectory() as tmp:
        pkg_dir = build_fixture(Path(tmp))
        py = sys.executable
        script = str(HERE / "run_skill.py")
        d = str(pkg_dir)

        # 默认人类可读
        out = subprocess.run([py, script, d], capture_output=True, text=True)
        assert out.returncode == 0, _fail(f"默认模式退出码 {out.returncode}: {out.stderr}")
        assert "步  1  [ exact]" in out.stdout, _fail("默认模式缺 exact 行")
        assert "步  2  [vision]" in out.stdout, _fail("默认模式缺 vision 行")
        assert "步  3  [manual]" in out.stdout, _fail("默认模式缺 manual 行")
        _ok("默认模式打印三行计划")

        # --json
        out = subprocess.run([py, script, d, "--json"], capture_output=True, text=True)
        assert out.returncode == 0, _fail(f"--json 退出码 {out.returncode}: {out.stderr}")
        obj = json.loads(out.stdout)
        assert obj["summary"]["step_count"] == 3, _fail("--json summary 不符")
        assert len(obj["plan"]) == 3, _fail("--json plan 不符")
        _ok("--json 输出可解析")

        # --check
        out = subprocess.run([py, script, d, "--check"], capture_output=True, text=True)
        assert out.returncode == 0, _fail(f"--check 退出码 {out.returncode}: {out.stderr}")
        rep = json.loads(out.stdout)
        assert rep["exact"] == 1 and rep["manual"] == 1 and rep["auto_rate"] == 66.7, _fail("--check 统计不符")
        _ok("--check 输出统计")

        # --mcp-hint
        out = subprocess.run([py, script, d, "--mcp-hint"], capture_output=True, text=True)
        assert out.returncode == 0, _fail(f"--mcp-hint 退出码 {out.returncode}: {out.stderr}")
        assert "flowio_get_step" in out.stdout, _fail("--mcp-hint 缺 flowio_get_step 示意")
        assert "manual" in out.stdout.lower(), _fail("--mcp-hint 应含 manual 步说明")
        _ok("--mcp-hint 输出每步工具调用示意")


def test_bad() -> None:
    print("[3] 坏包拒绝（退出码 2）")
    py = sys.executable
    script = str(HERE / "run_skill.py")

    # 目录不存在
    out = subprocess.run([py, script, "D:/no/such/pkg"], capture_output=True, text=True)
    assert out.returncode == 2, _fail(f"目录不存在应退出 2，实得 {out.returncode}")
    assert "技能包不合法" in out.stderr or "技能包不合法" in out.stdout, _fail("应提示技能包不合法")
    _ok("目录不存在被拒绝")

    # skill.json 缺失
    with tempfile.TemporaryDirectory() as tmp:
        empty = Path(tmp) / "empty"
        empty.mkdir()
        out = subprocess.run([py, script, str(empty)], capture_output=True, text=True)
        assert out.returncode == 2, _fail(f"skill.json 缺失应退出 2，实得 {out.returncode}")
        _ok("skill.json 缺失被拒绝")


def main() -> None:
    print("=== run_skill.py 自测 ===")
    test_unit()
    test_cli()
    test_bad()
    print("\n✅ 全部通过")


if __name__ == "__main__":
    main()
