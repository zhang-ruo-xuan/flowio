// 标注绘制与扁平化工具：把标注（文字/高亮/箭头/模糊）绘制到 canvas，
// 并在保存时把标注烧录进图片形成 composite，供编辑器与展示复用。
// 来源：flowio-site-next/lib/annotate.ts（桌面端同步移植）

import { ANNO_FONT, annoArrowHead, annoBlurDash, annoBlurRadius, annoFont, annoStroke, annoTextMetrics, contentScale } from './annoScale';

export type AnnoType = 'text' | 'highlight' | 'arrow' | 'blur' | 'ellipse' | 'magnifier' | 'badge' | 'rect' | 'pen';

export interface Annotation {
  id: string;
  type: AnnoType;
  x: number;
  y: number;
  width?: number;
  height?: number;
  endX?: number;
  endY?: number;
  /** 放大镜圆心（图像像素，与 x/y 同坐标系；r 为半径像素，zoom 为放大倍数） */
  cx?: number;
  cy?: number;
  r?: number;
  zoom?: number;
  content?: string;
  color?: string;
  strokeWidth?: number;
  fontSize?: number;
  /** 序号标注（badge）：编号 n 从 1 递增；x/y 为圆心，radius 为半径（图像像素） */
  n?: number;
  radius?: number;
  /** 画笔（pen）：扁平点数组 [x0,y0,x1,y1,...]，x/y/w/h 为其包围盒 */
  points?: number[];
  /** 矩形/画笔是否虚线（对齐插件 dashedRef） */
  dashed?: boolean;
}

type Dims = { width: number; height: number };

const DEFAULT_COLOR = '#ff4d4f';

/**
 * 放大镜内外圈半径（对齐插件 shared/magnifierRing.ts）。
 * 内圈＝外圈半径的 55%（钳制在 [6, 外圈-3]），用于在白底内容上也能看清分界。
 * sizeScale 为内容归一化系数（图高/1080），内圈下限随图等比放大。
 */
export function magnifierRingRadii(r: number, sizeScale: number): { outer: number; inner: number } {
  const outer = r;
  const inner = Math.min(Math.max(r * 0.55, 6 * sizeScale), Math.max(r - 3, 0));
  return { outer, inner };
}

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
  dims?: Dims,
  img?: CanvasImageSource | null,
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
      const { asc, desc } = annoTextMetrics(fs);
      ctx.font = ANNO_FONT(fs);
      a.width = Math.max(ctx.measureText(a.content).width, fs * 0.5);
      a.height = asc + desc;
      // 文字锚点是基线（对齐插件 annoTextBox：py = y - asc，textBaseline=alphabetic）。
      // 绘制 / 命中 / 选中框共用同一基线，保证提交瞬间文字不跳。
      ctx.fillStyle = color;
      ctx.textBaseline = 'alphabetic';
      ctx.fillText(a.content, a.x, a.y + asc);
      break;
    }
    case 'ellipse': {
      // 圆（只描边不遮住内容）：外接框内切椭圆。对齐插件 shape-ellipse 绘制语义。
      const w = a.width || 0;
      const h = a.height || 0;
      if (w <= 0 || h <= 0) break;
      ctx.strokeStyle = a.color || DEFAULT_COLOR;
      ctx.lineWidth = annoStroke(a.strokeWidth || 4, k);
      ctx.lineJoin = 'round';
      ctx.beginPath();
      ctx.ellipse(a.x + w / 2, a.y + h / 2, Math.abs(w) / 2, Math.abs(h) / 2, 0, 0, Math.PI * 2);
      ctx.stroke();
      break;
    }
    case 'badge': {
      // 序号标注：实心圆 + 白色描边 + 白色序号数字。对齐插件 annotate.tsx::badge 绘制语义。
      // 注意 radius 是创建时按原图短边算好的【图像像素】半径，canvas 坐标系就是图像像素，直接绘制。
      const cx = a.x, cy = a.y;
      const r = Math.max(3, a.radius || 10);
      ctx.fillStyle = a.color || DEFAULT_COLOR;
      ctx.beginPath(); ctx.arc(cx, cy, r, 0, Math.PI * 2); ctx.fill();
      ctx.strokeStyle = 'rgba(255,255,255,0.9)';
      ctx.lineWidth = Math.max(1, r * 0.12);
      ctx.stroke();
      const fs = r * 1.15;
      ctx.fillStyle = '#fff';
      ctx.font = `700 ${fs}px -apple-system, "Segoe UI", "Microsoft YaHei", sans-serif`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(String(a.n ?? 1), cx, cy + fs * 0.02);
      ctx.textAlign = 'start';
      ctx.textBaseline = 'alphabetic';
      break;
    }
    case 'rect': {
      // 矩形描边（不遮住内容）：对齐插件 shape-rect 绘制语义（round join + 可选虚线）。
      const w = a.width || 0;
      const h = a.height || 0;
      if (w <= 0 || h <= 0) break;
      const sw = annoStroke(a.strokeWidth || 4, k);
      ctx.strokeStyle = a.color || DEFAULT_COLOR;
      ctx.lineWidth = sw;
      ctx.lineJoin = 'round';
      if (a.dashed) ctx.setLineDash([sw * 2.2, sw * 1.6]);
      ctx.beginPath();
      ctx.rect(a.x, a.y, w, h);
      ctx.stroke();
      ctx.setLineDash([]);
      break;
    }
    case 'pen': {
      // 画笔自由手绘：points 扁平数组逐段连线（round cap/join + 可选虚线）。对齐插件 pen 绘制语义。
      const pts = a.points;
      if (!pts || pts.length < 4) break;
      const sw = annoStroke(a.strokeWidth || 4, k);
      ctx.strokeStyle = a.color || DEFAULT_COLOR;
      ctx.lineWidth = sw;
      ctx.lineCap = 'round';
      ctx.lineJoin = 'round';
      if (a.dashed) ctx.setLineDash([sw * 2.2, sw * 1.6]);
      ctx.beginPath();
      for (let i = 0; i < pts.length; i += 2) {
        i === 0 ? ctx.moveTo(pts[i], pts[i + 1]) : ctx.lineTo(pts[i], pts[i + 1]);
      }
      ctx.stroke();
      ctx.setLineDash([]);
      break;
    }
    case 'magnifier': {
      // 放大镜：透镜内把底图放大 zoom 倍，外圈描边 + 内圈白色分界圈。
      // 对齐插件 magnifier 绘制语义（圆心保持不动点，clip 圆内放大重绘底图）。
      const cx = a.cx ?? 0;
      const cy = a.cy ?? 0;
      const r = a.r ?? 0;
      if (!(r > 0)) break;
      const zoom = Math.max(1.2, a.zoom || 2);
      const { outer, inner } = magnifierRingRadii(r, k);
      ctx.save();
      ctx.beginPath();
      ctx.arc(cx, cy, outer, 0, Math.PI * 2);
      ctx.clip();
      // 以 (cx, cy) 为不动点放大：平移补回缩放偏移后 drawImage(0,0)
      ctx.translate(cx - zoom * cx, cy - zoom * cy);
      ctx.scale(zoom, zoom);
      if (img) ctx.drawImage(img, 0, 0);
      ctx.restore();
      ctx.beginPath();
      ctx.arc(cx, cy, outer, 0, Math.PI * 2);
      ctx.strokeStyle = a.color || DEFAULT_COLOR;
      ctx.lineWidth = Math.max(2, 3 * k);
      ctx.stroke();
      if (inner > 0) {
        ctx.beginPath();
        ctx.arc(cx, cy, inner, 0, Math.PI * 2);
        ctx.strokeStyle = 'rgba(255,255,255,0.85)';
        ctx.lineWidth = Math.max(1, k);
        ctx.stroke();
      }
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
  for (const a of annotations) drawAnnotation(ctx, a, { width: img.width, height: img.height }, img instanceof HTMLImageElement ? img : null);
  return c.toDataURL('image/png');
}
