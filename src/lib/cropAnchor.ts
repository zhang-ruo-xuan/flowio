/**
 * 裁图锚点 ↔ 裁图矩形 的互转。
 *
 * Flowio 的自动裁图管线（StepEditor 的 cropAdaptiveBase64）是：
 *   锚点 (x, y) = 元素矩形中心点；crop_w/crop_h = 元素矩形尺寸
 *   → 外扩 CROP_PAD 得到裁图矩形。
 *
 * ⚠️ 实际落库的 crop_w/crop_h 已是**录制端外扩过 PAD=24 的框**
 * （recorder/commands.rs 调 centered_crop_box 时传了 `w + CROP_PAD*2`，
 * CROP_PAD 在那边是 24），不是元素原始尺寸。本文件（CROP_PAD=8）和
 * 导出端（EXPORT_CROP_PAD=8）会**再外扩一次 8*2**，所以元素实际留白约为
 * 32px/侧。前后端、导出端用同一套 stored 值、都再外扩同样 8*2，因此
 * 编辑器预览与导出的裁图框完全一致 —— 只是比「仅 8px 留白」设计值更松。
 * 改这里时务必保持三者（前端/录制端/导出端）对 crop_w/h 的语义理解一致。
 *
 * 手动「重新裁图」的功能需要它的**逆运算**：用户在图上框一个矩形，
 * 我们要反推出锚点，让自动管线重跑时精确复现这个矩形 ——
 * 而不是把裁剪结果烧进原图（那样原图就丢了，无法再调整）。
 *
 * 这里两个函数互为逆运算，rectToCropAnchor 内部跑 forward 迭代校正，
 * 消除 round() 与 type 步骤「左扩 1/3」斜率带来的 ±1~2px 误差。
 */

/** 元素矩形外扩 padding（px）—— 必须与自动裁图管线取值一致 */
export const CROP_PAD = 8;

/** 图坐标矩形（左上角 + 宽高） */
export interface CropRectPx {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 落库的裁图锚点：中心点 + 元素矩形尺寸 */
export interface CropAnchor {
  x: number;
  y: number;
  cropW: number;
  cropH: number;
}

/**
 * forward：锚点 → 裁图矩形。
 *
 * 与 cropAdaptiveBase64 的矩形计算保持一致（不含最后 clamp 到图边界那一步，
 * 因为手动框选的矩形本来就在图内，clamp 不生效）。
 *
 * type 步骤特殊：矩形左侧多留原始宽度的 1/3，把左侧 label（如"用户名"）带进裁图。
 *
 * ⚠️ 注意是 `- extra / 2` 而不是 `- extra`：框宽 `width` 里已经含了 `extra`，
 * 居中项 `x - width/2` 天然在左右各分到 `extra/2`；要让左侧**总共**多出 `extra`，
 * 只需再左移 `extra/2`。写成 `- extra` 会过冲 `extra/2`，导致右侧留白变成
 * `PAD - extra/2` —— 元素宽度 > 48px 时即为负值，右边缘被切掉（300px 的输入框
 * 会被切 42px）。
 */
export function cropBoxFromAnchor(anchor: CropAnchor, actionType?: string): CropRectPx {
  const { x, y, cropW, cropH } = anchor;
  const height = cropH + CROP_PAD * 2;
  if (actionType === 'type') {
    const extra = Math.round(cropW / 3);
    const width = cropW + CROP_PAD * 2 + extra;
    return {
      x: Math.round(x - width / 2 - extra / 2),
      y: Math.round(y - height / 2),
      width,
      height,
    };
  }
  if (actionType === 'drag') {
    // DRAG 步骤高度方向多留上下文（与 cropAdaptiveBase64 一致）
    const extraH = Math.max(Math.round(cropH * 0.6), 100);
    const dragHeight = cropH + CROP_PAD * 2 + extraH;
    const dragWidth = cropW + CROP_PAD * 2;
    return {
      x: Math.round(x - dragWidth / 2),
      y: Math.round(y - dragHeight / 2),
      width: dragWidth,
      height: dragHeight,
    };
  }
  const width = cropW + CROP_PAD * 2;
  return {
    x: Math.round(x - width / 2),
    y: Math.round(y - height / 2),
    width,
    height,
  };
}

/**
 * inverse：裁图矩形 → 锚点。
 *
 * 先用解析解起步，再迭代跑 forward 收敛到 0 误差。
 * 每轮记录当前最优解，保证即使不收敛也不会比解析解更差。
 */
export function rectToCropAnchor(rect: CropRectPx, actionType?: string): CropAnchor {
  const isType = actionType === 'type';
  const isDrag = actionType === 'drag';

  // 解析解：非 type/drag 时 rect 宽高 = crop + 2*PAD；type 时还要多出 extra = crop/3
  // drag 时要多出 extraH = cropH * 1.5
  let cropW: number;
  let cropH: number;
  if (isType) {
    cropW = Math.round(((rect.width - CROP_PAD * 2) * 3) / 4);
    cropH = rect.height - CROP_PAD * 2;
  } else if (isDrag) {
    cropW = rect.width - CROP_PAD * 2;
    // Forward: displayH = cropH + PAD*2 + max(cropH*0.6, 100)
    // 粗略初值（迭代 8 轮会收敛到精确解）
    cropH = Math.max(1, Math.round((rect.height - CROP_PAD * 2 - 100) / 1.8));
  } else {
    cropW = rect.width - CROP_PAD * 2;
    cropH = rect.height - CROP_PAD * 2;
  }
  // forward 的居中项已在左右各分到 extra/2，故中心点只需再右移 extra/2
  let cx = rect.x + rect.width / 2 + (isType ? Math.round(cropW / 3) / 2 : 0);
  let cy = rect.y + rect.height / 2;

  let best: CropAnchor = { x: cx, y: cy, cropW, cropH };
  let bestErr = Number.POSITIVE_INFINITY;

  for (let i = 0; i < 8; i++) {
    const f = cropBoxFromAnchor({ x: cx, y: cy, cropW, cropH }, actionType);
    const dx = rect.x - f.x;
    const dy = rect.y - f.y;
    const dw = rect.width - f.width;
    const dh = rect.height - f.height;
    const err = Math.abs(dx) + Math.abs(dy) + Math.abs(dw) + Math.abs(dh);

    if (err < bestErr) {
      bestErr = err;
      best = { x: cx, y: cy, cropW, cropH };
    }
    if (err === 0) break;

    cx += dx;
    cy += dy;
    cropW += dw;
    cropH += dh;
  }

  return {
    x: Math.round(best.x),
    y: Math.round(best.y),
    // 锚点尺寸必须 > 0，否则自动管线会退回"以点击点为中心"的兜底逻辑
    cropW: Math.max(1, Math.round(best.cropW)),
    cropH: Math.max(1, Math.round(best.cropH)),
  };
}
