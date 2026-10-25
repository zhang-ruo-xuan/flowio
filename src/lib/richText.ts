/* ── 大标题富文本：白名单内联 HTML 子集（2026-09-19 第十四轮）──────────
 *
 * 这一份规则与 Rust 端 `export::commands::sanitize_inline_html` **逐条对齐**：
 *   - 标签只认 <b> <i> <u> <s> <br> 和 <span style="...">
 *   - style 只认 color / background-color（必须是 hex）、font-style、
 *     font-weight、text-decoration，且取值落在固定集合内
 *   - 正文里出现的 < > 必须转义成 &lt; / &gt;
 *
 * 为什么要有这一层：
 *   1) contenteditable 的产物五花八门（<font color> <div> <em> 内联 rgb() 颜色），
 *      直接落库的话导出侧的白名单会「不完全匹配 → 整体退回纯文本」，格式全丢；
 *   2) 落库前先归一化，导出侧那道校验就只是个兜底，而不是主路径。
 *
 * 安全性：所有解析都在**游离的 div** 里做（`innerHTML` 到游离节点不会执行脚本、
 * 不会发起图片请求），输出只可能包含上面那几个标签，因此可以安全地
 * `dangerouslySetInnerHTML` 到预览里。
 */

export interface RichColor {
  label: string;
  value: string;
}

/** 文字颜色：与编辑器既有配色同一套色板（值域必须是 hex） */
export const RICH_TEXT_COLORS: RichColor[] = [
  { label: "深灰", value: "#1a1a2e" },
  { label: "红", value: "#c0392b" },
  { label: "橙", value: "#ba7517" },
  { label: "绿", value: "#1a7f37" },
  { label: "蓝", value: "#0c447c" },
  { label: "紫", value: "#6b3fa0" },
  { label: "灰", value: "#6b7280" },
];

/** 高亮底色：沿用「标注 / 警告」那两个已有底色，视觉不外扩 */
export const RICH_HIGHLIGHTS: RichColor[] = [
  { label: "淡黄", value: "#faeeda" },
  { label: "淡蓝", value: "#e6f1fb" },
  { label: "淡绿", value: "#e6f4ea" },
  { label: "淡粉", value: "#fbe7ef" },
];

/** 标签映射：把浏览器的等价标签压成白名单里的那一个 */
const TAG_MAP: Record<string, string> = {
  B: "b",
  STRONG: "b",
  I: "i",
  EM: "i",
  U: "u",
  S: "s",
  STRIKE: "s",
  DEL: "s",
};

/** 这些标签当块处理：内容后面补一个 <br>（粘贴多行文本时不至于糊成一行） */
const BLOCK_TAGS = new Set([
  "DIV", "P", "LI", "TR", "H1", "H2", "H3", "H4", "H5", "H6",
  "BLOCKQUOTE", "SECTION", "ARTICLE", "UL", "OL", "TABLE", "PRE",
]);

/** 落库前的文本转义：正文里的这三个字符必须转义，否则会被当成标签 */
export function escapeText(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** 把 CSS 颜色写法转成 hex（Chromium 的 el.style.color 返回的是 rgb(...)）；
 *  认不出来就返回空串（效果 = 这个属性不落库，而不是把整段格式废掉）。 */
function cssColorToHex(v: string): string {
  const s = v.trim().toLowerCase();
  if (/^#[0-9a-f]{3}$/.test(s) || /^#[0-9a-f]{6}$/.test(s)) return s;
  const m = s.match(/^rgba?\(\s*([0-9.]+)[,\s]+([0-9.]+)[,\s]+([0-9.]+)/);
  if (!m) return "";
  const hex = (x: string) =>
    Math.max(0, Math.min(255, Math.round(parseFloat(x))))
      .toString(16)
      .padStart(2, "0");
  return `#${hex(m[1])}${hex(m[2])}${hex(m[3])}`;
}

/** 收集一个元素上可落库的样式（只取白名单里的属性，且只取能认出来的取值） */
function collectStyle(el: HTMLElement): string {
  const st = el.style;
  const out: string[] = [];

  const color = st.color ? cssColorToHex(st.color) : "";
  if (color) out.push(`color:${color}`);

  const bg = st.backgroundColor ? cssColorToHex(st.backgroundColor) : "";
  if (bg) out.push(`background-color:${bg}`);

  const fs = (st.fontStyle || "").toLowerCase();
  if (fs === "italic" || fs === "normal") out.push(`font-style:${fs}`);

  if (st.fontWeight) {
    if (st.fontWeight === "bold") out.push("font-weight:bold");
    else {
      const n = parseInt(st.fontWeight, 10);
      if (!Number.isNaN(n) && n >= 600) out.push("font-weight:bold");
    }
  }

  const tl = (st.textDecorationLine || "").toLowerCase();
  const hasU = tl.includes("underline");
  const hasS = tl.includes("line-through");
  if (hasU && hasS) out.push("text-decoration:underline line-through");
  else if (hasU) out.push("text-decoration:underline");
  else if (hasS) out.push("text-decoration:line-through");

  return out.join(";");
}

/** DOM → 规范化 HTML 片段 */
function walk(node: Node, out: string[]): void {
  if (node.nodeType === Node.TEXT_NODE) {
    out.push(escapeText(node.nodeValue ?? ""));
    return;
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return;

  const el = node as HTMLElement;
  const tag = el.tagName.toUpperCase();

  if (tag === "BR") {
    out.push("<br>");
    return;
  }

  if (tag === "FONT") {
    // execCommand 不开 styleWithCSS 时的产物，转成 <span style="color:...">
    const c = cssColorToHex(el.getAttribute("color") || "");
    if (c) out.push(`<span style="color:${c}">`);
    el.childNodes.forEach((c2) => walk(c2, out));
    if (c) out.push("</span>");
    return;
  }

  const mapped = TAG_MAP[tag];
  if (mapped) {
    out.push(`<${mapped}>`);
    el.childNodes.forEach((c) => walk(c, out));
    out.push(`</${mapped}>`);
    return;
  }

  if (tag === "SPAN") {
    const style = collectStyle(el);
    if (style) out.push(`<span style="${style}">`);
    el.childNodes.forEach((c) => walk(c, out));
    if (style) out.push("</span>");
    return;
  }

  // 其余标签一律「透明化」：只留内容，块级标签后面补个换行
  el.childNodes.forEach((c) => walk(c, out));
  if (BLOCK_TAGS.has(tag)) out.push("<br>");
}

/** 收尾：把重复/首尾的 <br> 去掉（大标题只有一行，不需要空行） */
function tidy(s: string): string {
  return s.replace(/(<br>)+/g, "<br>").replace(/^(<br>)+/, "").replace(/(<br>)+$/, "");
}

/**
 * 把一段真实 DOM（contenteditable）归一化成白名单 HTML 子集。
 * 输入是**活动 DOM 元素**，只读不写 —— 不动光标位置。
 */
export function normalizeInlineHtml(el: HTMLElement): string {
  const out: string[] = [];
  el.childNodes.forEach((c) => walk(c, out));
  return tidy(out.join(""));
}

/**
 * 把库里存的 HTML 归一化（预览/导出前用）。
 * 解析在游离 div 里做：不可信输入（手改库、粘贴的陌生 HTML）不会被执行。
 */
export function sanitizeStoredHtml(html: string): string {
  if (!html.trim()) return "";
  const box = document.createElement("div");
  box.innerHTML = html;
  return tidy(normalizeInlineHtml(box));
}

/** HTML → 纯文本（落库的 `title` 恒等于它：导出/搜索/SKILL.md 都读这个字段） */
export function htmlToPlainText(html: string): string {
  const box = document.createElement("div");
  box.innerHTML = html;
  const t = box.textContent ?? "";
  return t.replace(/\u00a0/g, " ").replace(/\s+/g, " ").trim();
}
