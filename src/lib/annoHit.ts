import { annoFont, annoTextAscent, contentScale } from './annoScale';

/**
 * 标注/裁剪框的命中检测几何。
 *
 * ## 为什么容差要按缩放换算
 * 鼠标坐标经 `getPos()` 换算成**图像像素**后再做命中判断，所以阈值如果直接写
 * `8` / `12` / `14` / `16`，那是**图像像素**而不是屏幕像素。屏幕上的实际容差
 * 要再乘回 scale：
 *
 *   3840×2160 截图显示在 1920×1080 窗口 → scale ≈ 0.44
 *   → 想给 12px 的屏幕容差，实际只有 12 × 0.44 ≈ 5.2px，箭头几乎点不中
 *
 * 正确做法：容差**按屏幕像素定义**，命中前除以 scale 换回图像像素。
 * 本模块所有 `slop*` 入参都是屏幕像素，内部统一换算。
 */

/** 命中容差（**屏幕像素**）——改这里就是改手感，与截图分辨率无关 */
export const HIT_SLOP = {
  /** 文字/高亮/模糊包围盒的外扩容差 */
  box: 8,
  /** 箭头线段的点到线距离容差 */
  arrow: 12,
  /** 裁剪框四边 */
  cropEdge: 14,
  /** 裁剪框四角（比边大，角优先） */
  cropCorner: 16,
} as const;

/**
 * 屏幕像素容差 → 图像像素容差。
 * scale <= 0 或非有限值时原样返回（此时坐标未就绪，宁可用原始值也不除零）。
 */
export function slopPx(screenPx: number, scale: number): number {
  if (!Number.isFinite(scale) || scale <= 0) return screenPx;
  if (!Number.isFinite(screenPx) || screenPx <= 0) return 0;
  // 下限 1 图像像素：极端缩小下容差不能塌到 0，否则永远点不中
  return Math.max(screenPx / scale, 1);
}

/**
 * 某条标注的命中容差（图像像素）——组件只该调这个，不要自己写数字。
 * 抽出来的意义：静态防线可以直接断言 `findAt` 函数体里出现了本函数，
 * 一旦有人把阈值写回去（哪怕写成三元表达式），函数体里就不再有 slopPx/annoSlop。
 */
export function annoSlop(a: { type: string }, scale: number): number {
  return slopPx(a.type === 'arrow' ? HIT_SLOP.arrow : HIT_SLOP.box, scale);
}

/** 裁剪框四边/四角的命中容差（图像像素） */
export function cropSlop(scale: number): { edge: number; corner: number } {
  return {
    edge: slopPx(HIT_SLOP.cropEdge, scale),
    corner: slopPx(HIT_SLOP.cropCorner, scale),
  };
}

export interface AnnoLike {
  type: string;
  x: number;
  y: number;
  width?: number;
  height?: number;
  endX?: number;
  endY?: number;
  cx?: number;
  cy?: number;
  r?: number;
  radius?: number;
  fontSize?: number;
}

/**
 * 点是否命中标注重叠区（图像坐标）。
 * @param m 图像像素容差（由 `slopPx(HIT_SLOP.box, scale)` 得到）
 */
export function hitAnnotation(a: AnnoLike, x: number, y: number, m: number, imgH?: number): boolean {
  if (a.type === 'arrow') {
    const dx = (a.endX ?? 0) - a.x;
    const dy = (a.endY ?? 0) - a.y;
    const len = Math.hypot(dx, dy);
    if (len === 0) return false;
    // 点到线段距离：把投影参数 t clamp 到 [0,1]，再算到该点的距离
    const t = Math.max(0, Math.min(1, ((x - a.x) * dx + (y - a.y) * dy) / (len * len)));
    return Math.hypot(x - (a.x + t * dx), y - (a.y + t * dy)) < m;
  }
  if (a.type === 'magnifier') {
    // 放大镜：点中透镜圆内（含圆周外扩 m 的容差）即可拖动
    const dx = x - (a.cx ?? 0);
    const dy = y - (a.cy ?? 0);
    const rr = a.r ?? 0;
    if (rr <= 0) return false;
    return Math.hypot(dx, dy) <= rr + m;
  }
  if (a.type === 'badge') {
    // 序号标注：点中实心圆内（含外扩 m 容差）即可命中；x/y 为圆心，radius 为半径
    const dx = x - a.x;
    const dy = y - a.y;
    const rr = a.radius ?? 10;
    if (rr <= 0) return false;
    return Math.hypot(dx, dy) <= rr + m;
  }
  if (a.type === 'text') {
    // 文字锚点是基线：命中框 = 框顶（a.y - asc）+ width/height（对齐插件 annoTextBox）。
    // asc 用与绘制/选中框相同的精确字体度量，三处共用同一基线。
    const k = contentScale(imgH);
    const fs = annoFont(a.fontSize || 24, k);
    const asc = annoTextAscent(fs);
    const w = a.width || 0;
    const h = a.height || 0;
    const ty = a.y - asc;
    return x >= a.x - m && x <= a.x + w + m && y >= ty - m && y <= ty + h + m;
  }
  const w = a.width || 0;
  const h = a.height || 0;
  // 高亮/模糊/矩形的包围盒：外扩 m 后做矩形包含判断
  return x >= a.x - m && x <= a.x + w + m && y >= a.y - m && y <= a.y + h + m;
}

export type CropHandleHit = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';

export interface RectLike {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * 裁剪框手柄命中检测（图像坐标）。
 * 判定顺序：**四角优先于四边**——角落区域同时满足角和边的条件，
 * 若先判边，角手柄永远抓不到。
 *
 * @param edge   四边容差（图像像素）
 * @param corner 四角容差（图像像素），通常 > edge
 */
export function hitCropHandle(
  r: RectLike,
  x: number,
  y: number,
  edge: number,
  corner: number,
): CropHandleHit | 'move' | null {
  const left = r.x;
  const top = r.y;
  const right = r.x + r.width;
  const bottom = r.y + r.height;
  const inX = x >= left && x <= right;
  const inY = y >= top && y <= bottom;

  if (Math.abs(x - left) <= corner && Math.abs(y - top) <= corner) return 'nw';
  if (Math.abs(x - right) <= corner && Math.abs(y - top) <= corner) return 'ne';
  if (Math.abs(x - left) <= corner && Math.abs(y - bottom) <= corner) return 'sw';
  if (Math.abs(x - right) <= corner && Math.abs(y - bottom) <= corner) return 'se';

  if (inX && Math.abs(y - top) <= edge) return 'n';
  if (inX && Math.abs(y - bottom) <= edge) return 's';
  if (inY && Math.abs(x - left) <= edge) return 'w';
  if (inY && Math.abs(x - right) <= edge) return 'e';

  // 框内 → 整体移动
  if (x > left && x < right && y > top && y < bottom) return 'move';
  return null;
}
