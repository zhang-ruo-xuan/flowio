// 标注绘制的「单位」工具 —— 区分两种像素，避免分辨率一变观感就漂移。
//
// 背景：canvas 的 backing store 是【图像分辨率】，再用 CSS 缩到 scale 显示。
//   1) 内容（箭头粗细 / 字号 / 马赛克 / 高亮边框）被写死成图像像素 →
//      屏幕观感 = 值 × scale。4K 截图上箭头线宽只剩 0.88px，几乎看不见。
//      正确语义：内容应【按图高等比】—— 两张图在屏幕上显示为同一高度时，
//      标注的粗细/字号必须看起来一样大。
//   2) UI 装饰（选中框 / 手柄 / 十字线 / 裁剪框线）画在同一张图上，
//      但它们是界面元素，应【屏幕像素恒定】—— 4K 图上也要看得见、点得中。
//      （与 src/lib/annoHit.ts 的命中容差同一基准，否则「看到的」和「点得中的」对不上）
//
// 基准：内容取 1080p（ANNO_REF_H），该分辨率下观感零变化。

/** 内容归一化的基准高度（1080p）。改这个值会让所有分辨率的观感一起变。 */
export const ANNO_REF_H = 1080;

/** 内容缩放系数：图高相对 1080p 的倍数。缺省/非法返回 1（= 按 1080p 处理）。 */
export function contentScale(imgH?: number): number {
  if (!imgH || !Number.isFinite(imgH) || imgH <= 0) return 1;
  return imgH / ANNO_REF_H;
}

/** UI 装饰：屏幕像素 → 图像像素。scale 非法时返回 1，防除零/放大。 */
export function uiScale(displayScale?: number): number {
  if (!displayScale || !Number.isFinite(displayScale) || displayScale <= 0) return 1;
  return 1 / displayScale;
}

/* ═══ 内容侧：随图等比 ══════════════════════════════════════ */

/** 箭头头部的基础长度与「每单位线宽的增加量」（图像像素，1080p 基准）。 */
const HEAD_BASE = 14;
const HEAD_PER_SW = 2;
/** 马赛克采样半径（图像像素，1080p 基准）。 */
const BLUR_R = 8;
/** 模糊框虚线段长（图像像素，1080p 基准）。 */
const BLUR_DASH = 4;

export const annoStroke = (v: number, k: number): number => v * k;

export const annoFont = (v: number, k: number): number => v * k;

export const annoArrowHead = (sw: number, k: number): number =>
  (HEAD_BASE + sw * HEAD_PER_SW) * k;

export const annoBlurRadius = (k: number): number => BLUR_R * k;

export const annoBlurDash = (k: number): number[] => [BLUR_DASH * k, BLUR_DASH * k];

/* ═══ UI 装饰侧：屏幕像素恒定 ═══════════════════════════════ */

/** 选中手柄半径（屏幕 px）。 */
export const UI_HANDLE_R = 5;
/** 选中虚线宽（屏幕 px）。 */
export const UI_LINE = 1;
/** 手柄描边宽（屏幕 px）。 */
export const UI_LINE_STRONG = 1.5;
/** 选中虚线段长（屏幕 px）。 */
export const UI_DASH = 3;
/** 选中框相对标注的内缩（屏幕 px）。 */
export const UI_PAD = 4;
/** 文字落点十字线的半长（屏幕 px）。 */
export const UI_CROSS = 10;
/** 裁剪框虚线（屏幕 px）。 */
export const UI_CROP_DASH: readonly [number, number] = [6, 3];
/** 裁剪框线宽（屏幕 px）。 */
export const UI_CROP_LINE = 1.5;

/** 把「屏幕像素」换算成绘制时要用的图像像素。 */
export const uiPx = (screenPx: number, displayScale?: number): number =>
  screenPx * uiScale(displayScale);
