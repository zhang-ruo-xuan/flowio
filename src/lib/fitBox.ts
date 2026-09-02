/**
 * 等比缩放（contain）：把图完整放进可用区域，且**永不放大**（上限 1）。
 *
 * 供标注弹窗（AnnotationsOverlay）决定 canvas 的 CSS 显示尺寸。
 * 抽成纯函数是为了能批量量化「溢出多少像素」——这类问题靠肉眼看只能看到
 * 「好像被切了一条」，说不清是几像素、由哪个约束方向引起。
 */

export interface FitInput {
  /** 可用区域宽（**已扣掉 padding**） */
  availW: number;
  /** 可用区域高（**已扣掉 padding 与固定工具栏**） */
  availH: number;
  /** 图的原始宽（canvas 的像素宽） */
  imgW: number;
  /** 图的原始高（canvas 的像素高） */
  imgH: number;
}

/** 等比 contain 的缩放系数，恒 > 0 且 <= 1。非法输入回落 1（按原尺寸显示，不放大）。 */
export function containScale(i: FitInput): number {
  const { availW, availH, imgW, imgH } = i;
  if (!(imgW > 0) || !(imgH > 0)) return 1;
  if (!(availW > 0) || !(availH > 0)) return 1;
  const s = Math.min(availW / imgW, availH / imgH, 1);
  return s > 0 ? s : 1;
}

/**
 * 缩放后 canvas 的 CSS 显示尺寸。
 * 调用方用它设 style.width / style.height，两者必须与图同宽高比，否则图会变形。
 */
export function fitDisplaySize(i: FitInput): { w: number; h: number; scale: number } {
  const scale = containScale(i);
  return { w: i.imgW * scale, h: i.imgH * scale, scale };
}

/**
 * 用「整块内容区（含 padding）」计算可用区域。
 *
 * ⚠️ 历史 bug 就出在这里：标注弹窗的 `.annotation-canvas-area` 有 `padding: 40px`，
 * 而旧代码直接拿 `window.innerWidth * 0.995` / `window.innerHeight - 56` 当可用区
 * ——padding 从来没被扣掉过。所以只要图比屏幕大（s < 1），canvas 就必然溢出，
 * 被 `overflow: hidden` 上下/左右对称裁掉一条。
 *
 * 更稳的做法是让调用方用 ResizeObserver 实测容器的 contentRect（天然已扣 padding），
 * 本函数只负责「从外框尺寸减去四周留白」这层换算。
 */
export function contentArea(
  boxW: number,
  boxH: number,
  paddingX: number,
  paddingY: number,
): { availW: number; availH: number } {
  const availW = boxW - paddingX * 2;
  const availH = boxH - paddingY * 2;
  return {
    availW: availW > 0 ? availW : 0,
    availH: availH > 0 ? availH : 0,
  };
}
