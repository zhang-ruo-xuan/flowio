// 标注绘制与扁平化工具：把标注（文字/高亮/箭头/模糊）绘制到 canvas，
// 并在保存时把标注烧录进图片形成 composite，供编辑器与展示复用。
// 来源：flowio-site-next/lib/annotate.ts（桌面端同步移植）

import { annoArrowHead, annoBlurDash, annoBlurRadius, annoFont, annoStroke, contentScale } from './annoScale';

export type AnnoType = 'text' | 'highlight' | 'arrow' | 'blur';

export interface Annotation {
  id: string;
  type: AnnoType;
  x: number;
  y: number;
  width?: number;
  height?: number;
  endX?: number;
  endY?: number;
  content?: string;
  color?: string;
  strokeWidth?: number;
  fontSize?: number;
}

type Dims = { width: number; height: number };

const DEFAULT_COLOR = '#FF4235';

/**
 * 把单条标注画到 ctx 上（不含选中框/预览等 UI 装饰）。
 *
 * `dims` 是【原始图像尺寸】，仅用于内容归一化（见 annoScale.contentScale）：
 * 所有写死的像素值（线宽 / 字号 / 箭头头 / 马赛克半径）都按 图高/1080 缩放，
 * 使得两张图在屏幕上显示为同一高度时，标注看起来一样粗。
 *
 * ⚠️ 不要用「当前 canvas 的尺寸」当 dims —— 裁图后 canvas 变成裁切尺寸，
 * 会让剩余标注的粗细跟着裁切框大小突变。模糊的裁剪边界读 ctx.canvas，不依赖 dims。
 */
export function drawAnnotation(
  ctx: CanvasRenderingContext2D,
  a: Annotation,
  dims?: Dims
) {
  // 内容归一化的基准：原始图高相对 1080p 的倍数（未传 dims 时按 1080p 处理）
  const k = contentScale(dims?.height);
  switch (a.type) {
    case 'highlight': {
      const color = a.color || DEFAULT_COLOR;
      const sw = annoStroke(a.strokeWidth || 2, k);
      ctx.fillStyle = color + '20';
      ctx.fillRect(a.x, a.y, a.width || 0, a.height || 0);
      ctx.strokeStyle = color;
      ctx.lineWidth = sw;
      ctx.strokeRect(a.x, a.y, a.width || 0, a.height || 0);
      break;
    }
    case 'arrow': {
      const color = a.color || DEFAULT_COLOR;
      const sw = annoStroke(a.strokeWidth || 3, k);
      const ex = a.endX ?? 0;
      const ey = a.endY ?? 0;
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(ex, ey);
      ctx.strokeStyle = color;
      ctx.lineWidth = sw;
      ctx.lineCap = 'round';
      ctx.stroke();
      const angle = Math.atan2(ey - a.y, ex - a.x);
      // 头部长度按【未归一化】的线宽算，再整体乘 k（否则会被缩放两次）
      const hl = annoArrowHead(a.strokeWidth || 3, k);
      ctx.beginPath();
      ctx.moveTo(ex, ey);
      ctx.lineTo(ex - hl * Math.cos(angle - Math.PI / 6), ey - hl * Math.sin(angle - Math.PI / 6));
      ctx.lineTo(ex - hl * Math.cos(angle + Math.PI / 6), ey - hl * Math.sin(angle + Math.PI / 6));
      ctx.closePath();
      ctx.fillStyle = color;
      ctx.fill();
      break;
    }
    case 'blur': {
      const bw = Math.ceil(a.width || 0);
      const bh = Math.ceil(a.height || 0);
      if (bw < 2 || bh < 2) break;
      const sx = Math.max(0, Math.floor(a.x));
      const sy = Math.max(0, Math.floor(a.y));
      // 裁剪边界取当前 canvas（裁图后 canvas 就是裁切尺寸），不用 dims
      const maxW = ctx.canvas.width - sx;
      const maxH = ctx.canvas.height - sy;
      const sw = Math.min(bw, maxW);
      const sh = Math.min(bh, maxH);
      if (sw < 2 || sh < 2) break;
      const id = ctx.getImageData(sx, sy, sw, sh);
      const d = id.data;
      // 半径随图等比；采样步长同比例放大，保证「块感」一致且采样点数不变（性能不退化）
      const r = Math.max(1, Math.round(annoBlurRadius(k)));
      const step = Math.max(1, Math.round(r / 4));
      const tmp = new Uint8ClampedArray(d);
      for (let py = 0; py < sh; py++) {
        for (let px = 0; px < sw; px++) {
          let cr = 0, cg = 0, cb = 0, cnt = 0;
          for (let dy = -r; dy <= r; dy += step) {
            for (let dx = -r; dx <= r; dx += step) {
              const ny = py + dy, nx = px + dx;
              if (ny >= 0 && ny < sh && nx >= 0 && nx < sw) {
                const i = (ny * sw + nx) * 4;
                cr += tmp[i]; cg += tmp[i + 1]; cb += tmp[i + 2]; cnt++;
              }
            }
          }
          const i = (py * sw + px) * 4;
          d[i] = cr / cnt; d[i + 1] = cg / cnt; d[i + 2] = cb / cnt;
        }
      }
      ctx.putImageData(id, sx, sy);
      ctx.strokeStyle = a.color || DEFAULT_COLOR;
      ctx.lineWidth = annoStroke(a.strokeWidth || 1, k);
      ctx.setLineDash(annoBlurDash(k));
      ctx.strokeRect(a.x, a.y, bw, bh);
      ctx.setLineDash([]);
      break;
    }
    case 'text': {
      if (!a.content) break;
      const color = a.color || DEFAULT_COLOR;
      const fs = annoFont(a.fontSize || 24, k);
      ctx.font = `bold ${fs}px "Noto Sans SC", "Microsoft YaHei", sans-serif`;
      const m = ctx.measureText(a.content);
      a.width = m.width;
      a.height = fs + annoStroke(4, k);
      // 纯文字，无背景（微信截图风格）
      ctx.fillStyle = color;
      ctx.textBaseline = 'top';
      ctx.fillText(a.content, a.x, a.y);
      break;
    }
  }
}

/** 把原图 + 标注烧录成一张 composite dataURL（PNG）。 */
export function flattenAnnotations(
  img: HTMLImageElement | { width: number; height: number },
  annotations: Annotation[]
): string {
  const c = document.createElement('canvas');
  c.width = img.width;
  c.height = img.height;
  const ctx = c.getContext('2d');
  if (!ctx) return '';
  if (img instanceof HTMLImageElement) {
    ctx.drawImage(img, 0, 0);
  }
  for (const a of annotations) drawAnnotation(ctx, a, { width: img.width, height: img.height });
  return c.toDataURL('image/png');
}
