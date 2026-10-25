// ★ 成片体检（对齐插件 v0.65.244 `lib/preview.ts` + `lib/verify.ts` 的只读静态口径）。
//
// 定位：录制停止后先进**只读成片预览**（浏览 / 删步 / 排序），点「编辑」才进完整编辑器。
// 预览页顶部一条「成片体检条」，把「这一步会不会让成片读起来断掉」的问题一次性摊开，
// 点一下跳到那一步。
//
// 🔴 单一口径：结构性问题的 code / 级别与插件 verify.ts 完全一致（NO_SHOT / BAD_ACTION /
//    NO_TITLE / BAD_URL / BAD_COORD / LONG_DESC），桌面端仅按本地 Step 字段做映射：
//   - 桌面端无 element/xpath 概念（录制的是桌面应用不是网页）⇒ 不报 NO_TARGET；
//   - 桌面端坐标为**屏幕像素**（x/y 为 Option<i32>），非插件 0..1 归一化 ⇒ BAD_COORD 仅当
//     点击类步骤坐标缺失或为负时才报（越界上界无法在体检层知道屏幕尺寸）；
//   - 剪贴板/快捷键步骤（copy/paste/hotkey/key）录制时不截图 ⇒ 不报 NO_SHOT（同插件 v371 口径）。
//
// 第二十二轮：桌面端已带 step.url（浏览器/导入场景），BAD_URL 与插件同语义——
// url 非空但解析失败（new URL 抛错）时报 warn。
//
// ⚠️ 别用「有没有截图路径」当内容块判断：内容块（heading/note/warning）天然无截图，
//    与插件 isContentBlock 的 BLOCK_IRRELEVANT_CODES 豁免逻辑一致 —— 对 block_kind 内容块
//    不报 NO_SHOT / BAD_COORD。

import type { Step } from "../types";

export type VerifyLevel = "error" | "warn";

export interface HealthIssue {
  /** 步骤序号（1-based，与 step.step_number 对齐） */
  step: number;
  level: VerifyLevel;
  code: string;
  message: string;
  /** 体检条 chip 上的短标签（2~4 字） */
  label: string;
}

export interface HealthSummary {
  issues: HealthIssue[];
  /** error 条数（成片会断在这里） */
  errorCount: number;
  /** warn 条数（能读，但建议修） */
  warnCount: number;
  /** 涉及的步骤序号（1-based，升序去重）—— 「一键跳转问题步骤」按这个列表走 */
  problemSteps: number[];
  /** 完全没有问题时为 true（体检条显示「通过」态） */
  ok: boolean;
}

/** 描述长度上限：超过就认为成片里会挤成一大段。正文式内容块（标注/流程）更容易写长。 */
export const LONG_DESC_LIMIT = 120;

/** 合法操作类型（与 StepEditor ACTION_LABELS 同源） */
const VALID_ACTIONS = new Set([
  "click", "type", "input", "hotkey", "copy", "paste", "drag", "key", "scroll", "select", "nav", "screenshot", "other",
]);

/** 剪贴板/编辑快捷键步骤：录制时不截图，不报 NO_SHOT（同插件 v371 口径） */
const CLIPBOARD_ACTIONS = new Set(["copy", "paste", "hotkey", "key"]);

/** 需要点击坐标才能回放/定位的操作类型 */
const COORD_ACTIONS = new Set(["click", "dblclick", "drag", "scroll"]);

/** 内容块类型（heading/note/warning/flow）：无截图、无坐标概念 */
const CONTENT_BLOCK_KINDS = new Set(["heading", "note", "warning", "flow"]);

/** code → 体检条短标签。未知 code 直接显示 code 本身（不静默吞掉）。 */
const CODE_LABELS: Record<string, string> = {
  NO_SHOT: "缺截图",
  NO_TITLE: "缺标题",
  BAD_COORD: "定位不明",
  BAD_ACTION: "未知操作",
  BAD_URL: "地址异常",
  LONG_DESC: "描述过长",
};

export function healthLabel(code: string): string {
  return CODE_LABELS[code] || code;
}

function isContentBlock(step: Step): boolean {
  return CONTENT_BLOCK_KINDS.has(step.block_kind || "");
}

/** 对齐插件 verify.ts 的 BAD_URL：url 非空但 new URL() 抛错即视为无法解析 */
function isValidUrl(raw: string): boolean {
  try {
    new URL(raw);
    return true;
  } catch {
    return false;
  }
}

/** 校验单步：返回该步的全部问题（error 表示成片会断，warn 表示建议修） */
export function verifyStep(step: Step): HealthIssue[] {
  const issues: HealthIssue[] = [];
  const add = (level: VerifyLevel, code: string, message: string) =>
    issues.push({ step: step.step_number, level, code, message, label: healthLabel(code) });

  const isBlock = isContentBlock(step);

  // 截图：操作后画面是教程的本体，缺失则无法证明这一步真的发生过
  // 剪贴板/编辑快捷键步骤录制时不截图 ⇒ 不报 NO_SHOT（插件 v371 口径）
  if (!isBlock && !step.screenshot_path && !step.after_screenshot_path && !CLIPBOARD_ACTIONS.has(step.action_type)) {
    add("error", "NO_SHOT", "缺少操作后的截图（教程会断在这里）");
  }
  // 操作类型必须是已知类型
  if (!VALID_ACTIONS.has(step.action_type)) {
    add("error", "BAD_ACTION", `未知操作类型：${step.action_type}`);
  }
  // 标题：缺标题虽不影响复现，但导出/阅读体验差
  if (!step.title || !step.title.trim()) {
    add("warn", "NO_TITLE", "步骤缺少标题（建议补一句操作说明）");
  }
  // 来源地址：url 非空但解析失败（对齐插件 verify.ts 的 BAD_URL —— 桌面端本机步骤恒为空，
  // 只有浏览器/导入场景有值，空串不报）
  if (step.url && !isValidUrl(step.url)) {
    add("warn", "BAD_URL", `步骤来源地址无法解析：${step.url}`);
  }
  // 点击坐标：桌面端为屏幕像素（Option<i32>），仅当需要坐标的操作缺失或为负时报
  if (!isBlock && COORD_ACTIONS.has(step.action_type)) {
    const x = step.x;
    const y = step.y;
    if (x == null || y == null || x < 0 || y < 0) {
      add("warn", "BAD_COORD", "缺少有效的点击坐标（回放定位可能不准）");
    }
  }
  // 成片阅读体验：描述过长（插件 preview.ts 独有的那一类）
  const descLength = (step.description || "").trim().length;
  if (descLength > LONG_DESC_LIMIT) {
    add("warn", "LONG_DESC", `步骤描述偏长（${descLength} 字，建议 ${LONG_DESC_LIMIT} 字以内），成片里会挤成一大段文字`);
  }
  return issues;
}

/**
 * 成片体检：整篇教程 → 按步骤升序的问题清单。
 * 排序：先按步骤号，同一步内 error 排在 warn 前面（用户从上往下修）。
 */
export function healthOfRecording(steps: Step[]): HealthIssue[] {
  const out: HealthIssue[] = [];
  for (const s of steps) {
    for (const iss of verifyStep(s)) {
      out.push(iss);
    }
  }
  out.sort((a, b) => a.step - b.step || levelRank(a.level) - levelRank(b.level));
  return out;
}

function levelRank(level: VerifyLevel): number {
  return level === "error" ? 0 : 1;
}

/** 汇总：给体检条用（总条数 / 通过态 / 问题步骤列表） */
export function summarizeHealth(issues: HealthIssue[]): HealthSummary {
  let errorCount = 0;
  let warnCount = 0;
  const steps = new Set<number>();
  for (const i of issues) {
    steps.add(i.step);
    if (i.level === "error") errorCount++;
    else warnCount++;
  }
  return {
    issues,
    errorCount,
    warnCount,
    problemSteps: [...steps].sort((a, b) => a - b),
    ok: issues.length === 0,
  };
}

/** 一步到位：整篇步骤 → 汇总（预览页只调这一个就够） */
export function healthSummaryOf(steps: Step[]): HealthSummary {
  return summarizeHealth(healthOfRecording(steps));
}
