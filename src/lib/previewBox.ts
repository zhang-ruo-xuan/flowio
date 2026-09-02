/**
 * 主预览图容器的尺寸计算 —— 目标是让容器形状**完全贴合**图片，
 * 使 `object-fit: contain` 不产生任何 letterbox 留白。
 *
 * ## 为什么只写 aspect-ratio 不够
 * 容器是 `width: 100%` + `max-height: 420px`。图片被 max-height 压住时
 * （宽图 + 宽容器，这是最常见的情况），盒子仍是满宽、只是高度停在 420，
 * 于是 contain 按高度缩放，左右各留一条灰底 —— 1920×1080 的截图在 800px
 * 容器里左右各留 27px，在 1200px 容器里左右各留 227px。
 *
 * 修法：既然高度被限死，就把**宽度**也按同一比例收窄到 `maxHeight × 宽高比`，
 * 盒子整体缩成图片的形状，contain 自然无缝。
 *
 * ## 为什么还要清掉 min-height
 * CSS 里原有 `min-height: 200px`。窄容器下的宽图算出来高度不足 200，
 * 会被强行撑高，于是又出现上下留白。既然现在宽高比由图片决定，
 * 下限就没有存在必要了（加载完成前仍是 CSS 里的默认值，不会塌陷）。
 */

/// 主预览图高度上限，需与 `.step-detail-image { max-height }` 保持一致
export const PREVIEW_MAX_HEIGHT = 420;

export interface PreviewBox {
  /** 写入 CSS 变量 `--step-img-aspect` 的宽高比（= 图片自然宽高比） */
  aspect: number;
  /** 容器 `max-width`（px）：`maxHeight × aspect` */
  maxWidth: number;
  /** 容器 `min-height`（px）：0 = 取消下限，避免重新引入上下留白 */
  minHeight: number;
}

/**
 * @param naturalWidth  图片自然宽度（`img.naturalWidth`）
 * @param naturalHeight 图片自然高度（`img.naturalHeight`）
 * @param maxHeight     高度上限，默认 [`PREVIEW_MAX_HEIGHT`]
 * @returns `null` 表示输入无效（图片未就绪 / 尺寸为 0），调用方应跳过设置
 */
export function previewBox(
  naturalWidth: number,
  naturalHeight: number,
  maxHeight: number = PREVIEW_MAX_HEIGHT,
): PreviewBox | null {
  if (!(naturalWidth > 0) || !(naturalHeight > 0) || !(maxHeight > 0)) {
    return null;
  }
  const aspect = naturalWidth / naturalHeight;
  if (!Number.isFinite(aspect) || aspect <= 0) {
    return null;
  }
  return {
    aspect,
    maxWidth: Math.round(maxHeight * aspect),
    minHeight: 0,
  };
}
