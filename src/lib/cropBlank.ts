/**
 * 空白裁剪守卫 —— 裁出的区域几乎纯色时毫无信息量，调用方应回退**整图**。
 *
 * 背景（2026-09-05 用户实测）：WPS「正在打开文档」加载过程中录制时 UIA 元素树
 * 不可用，录制端把「Program Manager」桌面宿主窗口的伪元素矩形（x=237,y=790,
 * 420×280）记成裁图锚点，而用户实际看到的加载动画（spinner+文件名）在画面中心
 * —— 裁出的区域是纯灰空白。矩形在几何上"正确"，内容上无用；因为锚点本身就
 * 指向空白区，退回 40% 点击点兜底照样空白，**唯一有意义的回退是整图**。
 *
 * ⚠️ 同步清单（阈值/算法改动必须三处同步，黄金值互相锁死）：
 *   - 本文件（编辑器 / 发布 / 封面）
 *   - src-tauri/src/export/commands.rs::is_blank_crop（导出）
 *   - src-tauri/src/ai/pipeline.rs::crop_and_encode_typed（AI 裁图，调 is_blank_crop）
 *   黄金值 BLANK_CROP_DELTA=24 锁在 scripts/check-crop.mjs 第 7 段 +
 *   Rust `blank_crop_falls_back_to_full_bytes` 等测试。
 *
 * 判据（宽松即可）：按步长采样 RGB 三通道的 min/max，极差全部 ≤
 * BLANK_CROP_DELTA 判为纯色。JPEG 压缩在平坦区的噪声 ±6 左右，24 有充分余量；
 * 区域内只要有任何文字/图标/线条，某通道极差必然远超 24。
 * 采样步长偏大漏掉的小特征在整图里仍然存在 —— 回退整图永远不丢信息，故宁松勿严。
 */

export const BLANK_CROP_DELTA = 24;

/** 目标采样数上限（约 2 万），步长按面积推算 */
const TARGET_SAMPLES = 20_000;

export function isUniformCropPixels(
  data: Uint8ClampedArray | number[],
  w: number,
  h: number,
): boolean {
  if (w <= 0 || h <= 0) return false;
  const stride = Math.max(1, Math.floor(Math.sqrt((w * h) / TARGET_SAMPLES)));
  let minR = 255, maxR = 0, minG = 255, maxG = 0, minB = 255, maxB = 0;
  for (let y = 0; y < h; y += stride) {
    for (let x = 0; x < w; x += stride) {
      const i = (y * w + x) * 4;
      const r = data[i], g = data[i + 1], b = data[i + 2];
      // ⚠️ min/max 必须各自独立判断，不能 else-if（首像素会让 max 停在 0）
      if (r < minR) minR = r;
      if (r > maxR) maxR = r;
      if (g < minG) minG = g;
      if (g > maxG) maxG = g;
      if (b < minB) minB = b;
      if (b > maxB) maxB = b;
    }
  }
  return (
    maxR - minR <= BLANK_CROP_DELTA &&
    maxG - minG <= BLANK_CROP_DELTA &&
    maxB - minB <= BLANK_CROP_DELTA
  );
}
