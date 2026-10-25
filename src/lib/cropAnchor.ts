/**
 * 裁图锚点 ↔ 裁图矩形（插件对齐：恒 50% 窗口）。
 *
 * 5b 对齐改造：几何口径从「元素 + PAD 外扩」切换为插件端
 * `shared/cropRegion.ts` 的 computeCropRect —— 中心 + FOCUS_VISIBLE_FRAC=0.5
 * （倍率 2×）恒比例窗口 + 贴边平移夹回图内。
 *
 * - 锚点 (x, y) = 元素矩形中心（像素）；crop_w/crop_h = 元素矩形尺寸，
 *   **不再参与几何**，仅作为「有元素」标志（>0 即走锚点分支）。
 * - 裁图矩形恒为 50% × 50% 截图尺寸，以锚点为中心，越界贴边平移（不缩小窗口）。
 * - 历史 CROP_PAD=8 外扩 / type 左扩 1/3 / drag extraH 体系已随 5b 删除。
 *
 * 消费端同步清单（同一几何，任何一端改动必须同步其余端）：
 *   1. 本文件 cropBoxFromAnchor（前端）
 *   2. `src-tauri/src/export/commands.rs::crop_region_50`（导出端唯一纯函数）
 *   3. `src-tauri/src/ai/pipeline.rs::ai_crop_region`（AI 端）
 * 录制端 `recorder/commands.rs` 仍落库「中心 + 尺寸」，尺寸仅作标志。
 * 一致性由 `npm run check:crop`（黄金值）与 cargo 侧 `both_consumers_*` 测试兜底。
 */

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
 * forward：锚点 → 裁图矩形（与插件 computeCropRect 同一口径）。
 *
 * 以锚点为中心、截图尺寸 50%×50% 的恒比例窗口；窗口越界时整体贴边平移
 * 夹回图内（不缩小窗口）。actionType 不再影响几何（type/drag 与 click 相同，
 * 与插件一致 —— 插件对 scroll/navigate/mark 走整图、其余类型同用 computeCropRect）。
 * imgW/imgH 为截图尺寸，调用方必须传入。
 */
export function cropBoxFromAnchor(
  anchor: CropAnchor,
  _actionType?: string,
  imgW = 0,
  imgH = 0,
): CropRectPx {
  return cropBoxFromAnchorFrac(anchor, 0.5, imgW, imgH);
}

/**
 * 泛化版：以锚点为中心、露出边长 frac（归一化；1 = 整图）的取景框。
 * 对齐插件 shared/crop.ts 的 shotScaleFrac / focusBoxSized —— 顶栏「图片 X%」
 * （统一改变截图大小）批量调比例时用它算裁框。窗口越界整体贴边夹回图内。
 */
export function cropBoxFromAnchorFrac(
  anchor: { x: number; y: number },
  frac: number,
  imgW = 0,
  imgH = 0,
): CropRectPx {
  const f = Math.min(1, Math.max(0.05, frac));
  const width = Math.max(1, Math.round(imgW * f));
  const height = Math.max(1, Math.round(imgH * f));
  return {
    x: clamp(anchor.x - Math.round(width / 2), 0, Math.max(0, imgW - width)),
    y: clamp(anchor.y - Math.round(height / 2), 0, Math.max(0, imgH - height)),
    width,
    height,
  };
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), Math.max(lo, hi));
}

/**
 * inverse：裁图矩形 → 锚点（解析解）。
 *
 * 消费端几何恒为 50% 窗口，锚点即矩形中心；cropW/cropH 记矩形尺寸，
 * 仅作为「有元素」标志（不再需要迭代收敛）。
 */
export function rectToCropAnchor(rect: CropRectPx, _actionType?: string): CropAnchor {
  return {
    x: Math.round(rect.x + rect.width / 2),
    y: Math.round(rect.y + rect.height / 2),
    cropW: Math.max(1, Math.round(rect.width)),
    cropH: Math.max(1, Math.round(rect.height)),
  };
}
