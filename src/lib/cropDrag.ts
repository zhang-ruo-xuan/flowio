/**
 * 手动框选裁图的纯几何 —— 从 CropOverlay 抽出来，便于独立量化校验。
 *
 * 三条不变式（拖拽时用户的心智模型）：
 *   1. resize：拖某个手柄时，**对角**必须钉死不动；
 *   2. new  ：按下点是矩形的对角之一，随鼠标移动的那个角跟随指针；
 *   3. move ：尺寸恒定，只改位置（贴边即停，不缩放）。
 *
 * 坐标一律是**原图像素坐标**，不含显示缩放。
 */

export interface CropRectPx {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type CropHandle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';

/** 选区最小边长（图坐标 px） */
export const MIN_CROP_SIZE = 20;

/** lo/hi 反转时以 lo 为准，保证返回值 >= lo */
function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), Math.max(lo, hi));
}

function roundRect(r: CropRectPx): CropRectPx {
  return {
    x: Math.round(r.x),
    y: Math.round(r.y),
    width: Math.round(r.width),
    height: Math.round(r.height),
  };
}

/** 图片尺寸合法性兜底，避免除零/负尺寸把 clamp 上界算反 */
function safeSize(v: number): number {
  return Number.isFinite(v) && v > 0 ? v : MIN_CROP_SIZE;
}

/**
 * 把矩形收进图片范围内，并保证最小边长。
 *
 * 语义：**优先保尺寸、位置让路**。适用于 move / 初始化这类「尺寸不该变」的场景。
 * ⚠️ resize 场景**不能**直接用它 —— 位置被截断后宽高不变会让对角漂移，见 resizeRect。
 */
export function clampRect(r: CropRectPx, W: number, H: number): CropRectPx {
  const w = safeSize(W);
  const h = safeSize(H);
  const width = Math.min(Math.max(r.width, MIN_CROP_SIZE), w);
  const height = Math.min(Math.max(r.height, MIN_CROP_SIZE), h);
  const x = clamp(r.x, 0, w - width);
  const y = clamp(r.y, 0, h - height);
  return roundRect({ x, y, width, height });
}

/**
 * 按控制点方向调整矩形（dx/dy 为图坐标位移）。
 *
 * 关键：**先 clamp 被拖动的边，再用「对角固定」反算宽高**，而不是让宽高自由增长
 * 后交给 clampRect 截断。后者的后果是：把左上角手柄往图外拖时，右/下边界会跟着
 * 往外跑（越界越深漂移越大），用户看到的选区会莫名"变胖"。
 */
export function resizeRect(
  o: CropRectPx,
  handle: CropHandle,
  dx: number,
  dy: number,
  W: number,
  H: number,
): CropRectPx {
  const w = safeSize(W);
  const h = safeSize(H);
  // 起始矩形先归一化进图内：调用方传的 rect 本来就该是合法的，这里兜底，
  // 免得外部传入越界矩形时「对角固定」把越界状态一路带下去。
  const base = clampRect(o, w, h);
  let left = base.x;
  let top = base.y;
  let right = base.x + base.width;
  let bottom = base.y + base.height;

  if (handle.includes('w')) left = clamp(o.x + dx, 0, right - MIN_CROP_SIZE);
  if (handle.includes('e')) right = clamp(o.x + o.width + dx, left + MIN_CROP_SIZE, w);
  if (handle.includes('n')) top = clamp(o.y + dy, 0, bottom - MIN_CROP_SIZE);
  if (handle.includes('s')) bottom = clamp(o.y + o.height + dy, top + MIN_CROP_SIZE, h);

  return roundRect({ x: left, y: top, width: right - left, height: bottom - top });
}

/**
 * 拖拽新建：按下点 (ax, ay) 与当前指针 (bx, by) 构成矩形。
 * 指针先 clamp 进图内，保证起点的对角始终跟随到边界而不是「宽高饱和成整图」。
 */
export function newRect(
  ax: number,
  ay: number,
  bx: number,
  by: number,
  W: number,
  H: number,
): CropRectPx {
  const w = safeSize(W);
  const h = safeSize(H);
  const sx = clamp(ax, 0, w);
  const sy = clamp(ay, 0, h);
  const px = clamp(bx, 0, w);
  const py = clamp(by, 0, h);

  let x0 = Math.min(sx, px);
  let x1 = Math.max(sx, px);
  let y0 = Math.min(sy, py);
  let y1 = Math.max(sy, py);

  // 不足最小边长时，朝指针方向补足
  if (x1 - x0 < MIN_CROP_SIZE) {
    if (px >= sx) {
      x1 = Math.min(w, x0 + MIN_CROP_SIZE);
      x0 = Math.max(0, x1 - MIN_CROP_SIZE);
    } else {
      x0 = Math.max(0, x1 - MIN_CROP_SIZE);
      x1 = Math.min(w, x0 + MIN_CROP_SIZE);
    }
  }
  if (y1 - y0 < MIN_CROP_SIZE) {
    if (py >= sy) {
      y1 = Math.min(h, y0 + MIN_CROP_SIZE);
      y0 = Math.max(0, y1 - MIN_CROP_SIZE);
    } else {
      y0 = Math.max(0, y1 - MIN_CROP_SIZE);
      y1 = Math.min(h, y0 + MIN_CROP_SIZE);
    }
  }

  return roundRect({ x: x0, y: y0, width: x1 - x0, height: y1 - y0 });
}

/** 平移：尺寸恒定，位置跟随（贴边即停） */
export function moveRect(o: CropRectPx, dx: number, dy: number, W: number, H: number): CropRectPx {
  return clampRect({ ...o, x: o.x + dx, y: o.y + dy }, W, H);
}
