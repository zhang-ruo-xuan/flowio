// 截图标注编辑器（桌面端同步移植）
// 来源：flowio-site-next/components/AnnotationsOverlay.tsx
// 提供：选择/移动、文字、高亮框、箭头、模糊、裁剪、删除、撤销/重做、完成烧录

import { useState, useRef, useEffect, useCallback } from 'react';
import { Annotation, AnnoType, drawAnnotation, flattenAnnotations } from '../lib/annotate';
import { rectToCropAnchor, cropBoxFromAnchor, type CropAnchor } from '../lib/cropAnchor';
import { containScale, contentArea } from '../lib/fitBox';
import { annoSlop, cropSlop, hitAnnotation, hitCropHandle } from '../lib/annoHit';
import { moveRect, resizeRect } from '../lib/cropDrag';
import {
  ANNO_FONT, annoFont, annoStroke, annoTextAscent, annoTextMetrics, contentScale, uiPx, uiScale,
  UI_CROSS, UI_CROP_DASH, UI_CROP_LINE, UI_DASH, UI_HANDLE_R, UI_LINE, UI_LINE_STRONG, UI_PAD,
} from '../lib/annoScale';

export type ToolType = 'select' | 'text' | 'highlight' | 'arrow' | 'badge' | 'rect' | 'pen' | 'blur' | 'ellipse' | 'magnifier' | 'crop' | 'delete';

export interface CropRect { x: number; y: number; width: number; height: number; }

/** 把一张 dataURL 按图坐标矩形裁出，返回 JPEG dataURL（用于「全图视图裁剪」顶替全图） */
function cropDataUrl(dataUrl: string, rect: CropRect): Promise<string> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const w = Math.max(1, Math.round(rect.width));
      const h = Math.max(1, Math.round(rect.height));
      const c = document.createElement('canvas');
      c.width = w;
      c.height = h;
      const ctx = c.getContext('2d');
      if (!ctx) {
        reject(new Error('canvas 2d context 不可用'));
        return;
      }
      ctx.drawImage(img, rect.x, rect.y, rect.width, rect.height, 0, 0, w, h);
      try {
        resolve(c.toDataURL('image/jpeg', 0.92));
      } catch (e) {
        reject(e);
      }
    };
    img.onerror = () => reject(new Error('裁剪源图加载失败'));
    img.src = dataUrl;
  });
}

interface Props {
  imageUrl: string;
  annotations: Annotation[];
  /** 步骤 action_type：5b 后不再影响裁图几何（恒 50% 窗口，type/drag 与 click 相同） */
  actionType?: string;
  /**
   * 进入标注时的自动裁图锚点（落库值：中心 + 尺寸，尺寸仅作标志）。
   * 存在时默认裁剪框 = cropBoxFromAnchor 的恒 50% 窗口（贴边夹回），
   * 即「重置为自动」的语义 —— 手动框选只落 cropRect、不改锚点（对齐插件）。
   * 历史 bug：调用方曾把锚点中心 (x, y) 当矩形左上角传入 initialCropRect，
   * 导致初始选区偏移半个框，已改由本组件用 anchor 正确换算。
   */
  initialAnchor?: CropAnchor | null;
  /**
   * 进入标注时的当前视图：'full'=全图，'crop'=裁图。
   * 决定保存时裁切结果落到「全图」还是「裁图」槽位（见 StepEditor.handleAnnotateSave）。
   */
  initialCropRect?: CropRect | null;
  /**
   * 标注完成回调。
   * - full  ：把注释烧录进【完整图】的 base64（顶替「全图」视图，不裁切）；
   * - anchor：若裁剪了，框反算成的裁图锚点（顶替「裁图」视图）；
   * - cropped：若裁剪了，注释烧录后再按框裁出的【裁切图】base64（全图视图裁剪时顶替全图用）。
   * 三者按当前视图独立落槽，互不影响。
   */
  onSave: (annos: Annotation[], result: { full: string | undefined; anchor: CropAnchor | null; cropped?: string | undefined }) => void;
  onClose: () => void;
}

const TOOLS: { id: ToolType; label: string; icon: string; tip: string }[] = [
  // 顺序与图标对齐插件版顶栏：文字T/序号1/高亮▢/矩形▭/圆◯/箭头↗/模糊◍/放大镜🔍/画笔✎/裁剪✂；
  // select（桌面端默认）置首、delete 置尾（插件无此二工具）。
  { id: 'select', label: '选择', icon: '⇲', tip: '选择\n点选已画好的标注，再拖动调整位置' },
  { id: 'text', label: '文字', icon: 'T', tip: '文字\n在图上点一下，输入要写的说明文字' },
  { id: 'badge', label: '序号', icon: '1', tip: '序号\n在图上点一下，自动按 1、2、3… 递增编号标注位置' },
  { id: 'highlight', label: '高亮', icon: '▢', tip: '高亮\n拖出半透明色块强调某块区域，底下的内容仍看得见' },
  { id: 'rect', label: '矩形', icon: '▭', tip: '矩形\n拖出描边矩形框，圈住要强调的区域' },
  { id: 'ellipse', label: '圆', icon: '◯', tip: '圆\n画正圆（只描边不遮住内容）：按下的点钉在圆周上，拖到对侧圆周' },
  { id: 'arrow', label: '箭头', icon: '↗', tip: '箭头\n从起点拖到终点，指向要操作的位置' },
  { id: 'blur', label: '模糊', icon: '◍', tip: '模糊\n把要遮住的区域涂成马赛克，用来给敏感信息打码' },
  { id: 'magnifier', label: '放大镜', icon: '🔍', tip: '放大镜\n点一下是点，拖拽后放大（按下的点钉在透镜圆周上）' },
  { id: 'pen', label: '画笔', icon: '✎', tip: '画笔\n按住拖动，自由手绘划线' },
  { id: 'crop', label: '裁剪', icon: '✂', tip: '裁剪\n框出要保留的区域，点「应用裁剪」后其余部分被裁掉' },
  { id: 'delete', label: '删除标注', icon: '✕', tip: '删除标注\n点一下要删的那条标注就删掉，可以连续删' },
];

// ── 选中标记（对齐插件 annotate.tsx annoSelFrame / circleFromHandle）────────
// 选中框几何（图像像素坐标）：矩形类 8 把手；箭头 p1/p2 端点；放大镜/圆 8 向；
// 序号/文字/画笔 仅 move（插件语义：看得见就一定拖得着）。
function selFramePx(a: Annotation, imgH?: number): { x: number; y: number; w: number; h: number; handles: { handle: string; x: number; y: number }[] } {
  let x = a.x, y = a.y, w = 0, h = 0;
  const handles: { handle: string; x: number; y: number }[] = [];
  const push8 = () => {
    handles.push(
      { handle: 'nw', x, y },
      { handle: 'n', x: x + w / 2, y },
      { handle: 'ne', x: x + w, y },
      { handle: 'e', x: x + w, y: y + h / 2 },
      { handle: 'se', x: x + w, y: y + h },
      { handle: 's', x: x + w / 2, y: y + h },
      { handle: 'sw', x, y: y + h },
      { handle: 'w', x, y: y + h / 2 },
    );
  };
  if (a.type === 'arrow') {
    const ex = a.endX ?? a.x, ey = a.endY ?? a.y;
    x = Math.min(a.x, ex); y = Math.min(a.y, ey);
    w = Math.abs(ex - a.x); h = Math.abs(ey - a.y);
    handles.push({ handle: 'p1', x: a.x, y: a.y }, { handle: 'p2', x: ex, y: ey });
  } else if (a.type === 'text') {
    // 文字锚点是基线：选中框顶 = 基线 - asc（对齐插件 annoTextBox）
    const k = contentScale(imgH);
    const fs = annoFont(a.fontSize || 24, k);
    const asc = annoTextAscent(fs);
    y = a.y - asc;
    w = a.width || 0; h = a.height || 0;
    handles.push({ handle: 'move', x: x + w / 2, y: y + h / 2 });
  } else if (a.type === 'badge') {
    const r = Math.max(3, a.radius || 10);
    x = a.x - r; y = a.y - r; w = r * 2; h = r * 2;
    handles.push({ handle: 'move', x: a.x, y: a.y });
  } else if (a.type === 'magnifier') {
    const cx = a.cx ?? a.x, cy = a.cy ?? a.y, r = a.r ?? 0;
    x = cx - r; y = cy - r; w = r * 2; h = r * 2;
    push8();
  } else if (a.type === 'pen') {
    w = a.width || 0; h = a.height || 0;
    handles.push({ handle: 'move', x: x + w / 2, y: y + h / 2 });
  } else {
    w = a.width || 0; h = a.height || 0;
    push8();
  }
  return { x, y, w, h, handles };
}

// 圆 / 放大镜「边固定缩放」（对齐插件 circleFromHandle）：对边 / 对角固定不动，恒为正圆。
function circleFromHandlePx(
  handle: string, fx: number, fy: number,
  x1: number, y1: number, x2: number, y2: number, minR: number,
) {
  const e = handle.includes('e'), w = handle.includes('w'), s = handle.includes('s'), n = handle.includes('n');
  const cands: number[] = [];
  if (e) cands.push(fx - x1);
  if (w) cands.push(x2 - fx);
  if (s) cands.push(fy - y1);
  if (n) cands.push(y2 - fy);
  const r = Math.max(minR, ...(cands.length ? cands : [minR]));
  const cx = e ? x1 + r : w ? x2 - r : (x1 + x2) / 2;
  const cy = s ? y1 + r : n ? y2 - r : (y1 + y2) / 2;
  return { cx, cy, r };
}

// 选中把手命中（屏幕容差，入参为图像坐标）
function hitSelHandlePx(a: Annotation, px: number, py: number, scale: number, imgH?: number): string | null {
  const fr = selFramePx(a, imgH);
  const slop = 10 / scale;
  for (const hd of fr.handles) {
    if (Math.hypot(px - hd.x, py - hd.y) <= slop) return hd.handle;
  }
  return null;
}

// 选中把手方向光标（对齐插件 annoHandleCursor）
function selHandleCursor(handle: string): string | null {
  if (handle === 'n' || handle === 's') return 'ns-resize';
  if (handle === 'e' || handle === 'w') return 'ew-resize';
  if (handle === 'nw' || handle === 'se') return 'nwse-resize';
  if (handle === 'ne' || handle === 'sw') return 'nesw-resize';
  return null;
}

const COLOR_PRESETS = ['#ff4d4f', '#fa8c16', '#fadb14', '#52c41a', '#1890ff', '#722ed1', '#ffffff', '#595959'];
const SIZE_PRESETS = [
  { label: 'S', val: 2 },
  { label: 'M', val: 4 },
  { label: 'L', val: 6 },
  { label: 'XL', val: 10 },
];
const FONT_SIZES = [14, 20, 24, 28, 36, 48];

/**
 * 标注弹窗的布局常量，**必须与 styles.css 保持一致**（改 CSS 时同步改这里）：
 *   .annotation-toolbar      height: 56px
 *   .annotation-canvas-area  padding: 40px
 * 只在 ResizeObserver 还没给出实测值前用作兜底，之后一律以实测内容区为准。
 */
const TOOLBAR_H = 56;
const CANVAS_AREA_PAD = 40;

/** ResizeObserver 不可用时的兜底：按窗口尺寸估算内容区（须自己扣掉工具栏与 padding） */
function windowFallbackArea(): { w: number; h: number } {
  const c = contentArea(
    window.innerWidth,
    window.innerHeight - TOOLBAR_H,
    CANVAS_AREA_PAD,
    CANVAS_AREA_PAD,
  );
  return { w: c.availW, h: c.availH };
}

export default function AnnotationsOverlay({ imageUrl, annotations: initial, actionType, initialAnchor, initialCropRect, onSave, onClose }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [annotations, setAnnotations] = useState<Annotation[]>(initial);
  const [activeTool, setActiveTool] = useState<ToolType>('select');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [isDrawing, setIsDrawing] = useState(false);
  const [drawStart, setDrawStart] = useState<{ x: number; y: number } | null>(null);
  const [drawCurrent, setDrawCurrent] = useState<{ x: number; y: number } | null>(null);
  /** 画笔（pen）轨迹点累积（图像坐标）；onMouseUp 时扁平化成 points + 包围盒 */
  const [penPoints, setPenPoints] = useState<{ x: number; y: number }[] | null>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [scale, setScale] = useState(1);
  /** 内容区实测可用尺寸（已扣 padding），由 ResizeObserver 维护 */
  const [avail, setAvail] = useState<{ w: number; h: number } | null>(null);
  const areaRef = useRef<HTMLDivElement>(null);
  const [imageSrc, setImageSrc] = useState(imageUrl);
  const [cropRect, setCropRect] = useState<CropRect | null>(null);
  const [isCropping, setIsCropping] = useState(false);
  const [cropStart, setCropStart] = useState<{ x: number; y: number } | null>(null);
  // 裁剪框四边框拉动调整（n/s/e/w 四边 + nw/ne/sw/se 四角 + move 整体移动）
  const [cropResize, setCropResize] = useState<null | 'move' | 'n' | 's' | 'e' | 'w' | 'nw' | 'ne' | 'sw' | 'se'>(null);
  const [cropHover, setCropHover] = useState<null | 'move' | 'n' | 's' | 'e' | 'w' | 'nw' | 'ne' | 'sw' | 'se'>(null);
  // 裁剪方式（对齐插件 cropMode）：drag=按住拖选选区；frame=框选手柄调整边角。
  const [cropMode, setCropMode] = useState<'drag' | 'frame'>('drag');
  // 整图缩放 1~4（对齐插件 zoom 缩放控件）
  const [zoom, setZoom] = useState(1);
  // 缩放百分比按钮记忆的原档位：非 100% 时点击切 100% 并记住；100% 时点击切回该档位（null=尚未记忆）
  const [preZoom, setPreZoom] = useState<number | null>(null);
  // 虚线开关（对齐插件 dashedRef：箭头/矩形/圆/画笔可选虚线）
  const [dashed, setDashed] = useState(false);
  // 文字就地编辑：点已有文字时记录被编辑的 id（对齐插件 textDragging 语义）
  const [editTextId, setEditTextId] = useState<string | null>(null);
  // 选中标注把手缩放状态（任意工具下拖动选中框 8 把手 / 端点 / move）
  const selResize = useRef<{ id: string; handle: string; startX: number; startY: number; origin: Annotation } | null>(null);
  // 当前 hover 到哪个选中把手（用于光标样式，对齐插件 annoHandleCursor）
  const [selHover, setSelHover] = useState<string | null>(null);

  // Text tool: click canvas to place, then type in toolbar input
  const [textPlacePos, setTextPlacePos] = useState<{ x: number; y: number } | null>(null);
  const [textInput, setTextInput] = useState('');
  const textInputRef = useRef<HTMLInputElement>(null);

  // Drag state for select tool
  const [isDragging, setIsDragging] = useState(false);
  const dragOffset = useRef<{ dx: number; dy: number }>({ dx: 0, dy: 0 });
  // 裁剪框拖动/调整起点：按下时的鼠标位置 + 当时矩形快照。
  // ★ 必须以「按下时的矩形」为 base、用全程位移反算，不能拿每帧被 setCropRect
  //   更新过的当前 rect 再叠加位移（会重复累加 → 一拉就冲到边界）。对齐 CropOverlay 的 d.origin 范式。
  const cropDragOrigin = useRef<{ startX: number; startY: number; rect: CropRect } | null>(null);

  const [history, setHistory] = useState<Annotation[][]>([initial]);
  const [historyIndex, setHistoryIndex] = useState(0);

  // Color and size settings
  const [currentColor, setCurrentColor] = useState('#ff4d4f');
  const [currentStrokeWidth, setCurrentStrokeWidth] = useState(4);
  const [currentFontSize, setCurrentFontSize] = useState(24);

  // Load image
  useEffect(() => {
    const img = new Image();
    // 同源（blob:/data:）无需跨域；仅对 http(s) 远程图启用 anonymous，避免画布被污染
    if (imageSrc.startsWith('http://') || imageSrc.startsWith('https://')) {
      img.crossOrigin = 'anonymous';
    }
    // ⚠️ 这里**不要**再算 scale：可用尺寸由 ResizeObserver 实测，见下方 fit 效果
    img.onload = () => {
      setImage(img);
    };
    img.onerror = () => {
      // 图片加载失败（如远程图无 CORS）也允许继续，只是无法烧录
      setImage(null);
    };
    img.src = imageSrc;
  }, [imageSrc]);

  // Auto-focus text input when placed
  useEffect(() => {
    if (textPlacePos && textInputRef.current) {
      textInputRef.current.focus();
    }
  }, [textPlacePos]);

  // 实测内容区可用尺寸。
  // 历史 bug：旧代码用 window.innerWidth*0.995 / innerHeight-56 当可用区，
  // 但 .annotation-canvas-area 有 padding:40px，工具栏还占 56px —— 全都没扣。
  // 结果只要图比屏幕大，canvas 就溢出，被 overflow:hidden 上下各裁 40px。
  // ResizeObserver 的 contentRect 是 content box（天然已扣 padding），比手算可靠。
  useEffect(() => {
    const el = areaRef.current;
    if (!el) return;
    if (typeof ResizeObserver === 'undefined') {
      // 兜底：老环境退回窗口估算（此时必须自己扣掉 padding 与工具栏）
      setAvail(windowFallbackArea());
      return;
    }
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r && r.width > 0 && r.height > 0) setAvail({ w: r.width, h: r.height });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 图或可用区任一变化 → 重算等比缩放（永不放大）
  useEffect(() => {
    if (!image) return;
    const box = avail ?? windowFallbackArea();
    setScale(containScale({ availW: box.w, availH: box.h, imgW: image.width, imgH: image.height }));
  }, [image, avail]);

  // Draw
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !image) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    canvas.width = image.width;
    canvas.height = image.height;
    canvas.style.width = (image.width * scale * zoom) + 'px';
    canvas.style.height = (image.height * scale * zoom) + 'px';

    ctx.clearRect(0, 0, image.width, image.height);
    ctx.drawImage(image, 0, 0);

    // 两种缩放系数：内容随图等比（vk），UI 装饰按屏幕像素恒定（uk）
    const vk = contentScale(image.height);
    const uk = uiScale(scale);

    for (const a of annotations) {
      drawAnnotation(ctx, a, { width: image.width, height: image.height }, image);
    }

    // Drawing preview
    if (isDrawing && drawStart && drawCurrent) {
      if (activeTool === 'highlight') {
        const rx = Math.min(drawStart.x, drawCurrent.x), ry = Math.min(drawStart.y, drawCurrent.y);
        const rw = Math.abs(drawCurrent.x - drawStart.x), rh = Math.abs(drawCurrent.y - drawStart.y);
        ctx.fillStyle = currentColor + '20';
        ctx.fillRect(rx, ry, rw, rh);
        ctx.strokeStyle = currentColor;
        ctx.lineWidth = annoStroke(currentStrokeWidth, vk);
        ctx.setLineDash([5 * uk, 5 * uk]);
        ctx.strokeRect(rx, ry, rw, rh);
        ctx.setLineDash([]);
      } else if (activeTool === 'arrow') {
        ctx.beginPath();
        ctx.moveTo(drawStart.x, drawStart.y);
        ctx.lineTo(drawCurrent.x, drawCurrent.y);
        ctx.strokeStyle = currentColor;
        ctx.lineWidth = annoStroke(currentStrokeWidth, vk);
        ctx.setLineDash([5 * uk, 5 * uk]);
        ctx.stroke();
        ctx.setLineDash([]);
      } else if (activeTool === 'rect') {
        const rx = Math.min(drawStart.x, drawCurrent.x), ry = Math.min(drawStart.y, drawCurrent.y);
        const rw = Math.abs(drawCurrent.x - drawStart.x), rh = Math.abs(drawCurrent.y - drawStart.y);
        ctx.strokeStyle = currentColor;
        ctx.lineWidth = annoStroke(currentStrokeWidth, vk);
        ctx.setLineDash([5 * uk, 5 * uk]);
        ctx.strokeRect(rx, ry, rw, rh);
        ctx.setLineDash([]);
      } else if (activeTool === 'pen' && penPoints && penPoints.length >= 2) {
        ctx.beginPath();
        penPoints.forEach((pt, i) => (i === 0 ? ctx.moveTo(pt.x, pt.y) : ctx.lineTo(pt.x, pt.y)));
        ctx.strokeStyle = currentColor;
        ctx.lineWidth = annoStroke(currentStrokeWidth, vk);
        ctx.lineCap = 'round';
        ctx.lineJoin = 'round';
        ctx.setLineDash([5 * uk, 5 * uk]);
        ctx.stroke();
        ctx.setLineDash([]);
      } else if (activeTool === 'blur') {
        const rx = Math.min(drawStart.x, drawCurrent.x), ry = Math.min(drawStart.y, drawCurrent.y);
        const rw = Math.abs(drawCurrent.x - drawStart.x), rh = Math.abs(drawCurrent.y - drawStart.y);
        ctx.fillStyle = 'rgba(255,255,255,0.3)';
        ctx.fillRect(rx, ry, rw, rh);
        ctx.strokeStyle = currentColor;
        ctx.lineWidth = UI_LINE * uk;
        ctx.setLineDash([4 * uk, 4 * uk]);
        ctx.strokeRect(rx, ry, rw, rh);
        ctx.setLineDash([]);
      } else if (activeTool === 'ellipse') {
        // 圆预览：与创建共用「直径式」几何（按下点与光标＝直径两端）
        const r = Math.hypot(drawCurrent.x - drawStart.x, drawCurrent.y - drawStart.y) / 2;
        if (r > 0) {
          const cx = (drawStart.x + drawCurrent.x) / 2, cy = (drawStart.y + drawCurrent.y) / 2;
          ctx.strokeStyle = currentColor;
          ctx.lineWidth = annoStroke(currentStrokeWidth, vk);
          ctx.setLineDash([5 * uk, 5 * uk]);
          ctx.beginPath();
          ctx.ellipse(cx, cy, r, r, 0, 0, Math.PI * 2);
          ctx.stroke();
          ctx.setLineDash([]);
        }
      } else if (activeTool === 'magnifier') {
        // 放大镜预览：虚线圆 + 圆心十字
        const r = Math.hypot(drawCurrent.x - drawStart.x, drawCurrent.y - drawStart.y) / 2;
        if (r > 0) {
          const cx = (drawStart.x + drawCurrent.x) / 2, cy = (drawStart.y + drawCurrent.y) / 2;
          ctx.strokeStyle = currentColor;
          ctx.lineWidth = UI_LINE * uk;
          ctx.setLineDash([5 * uk, 5 * uk]);
          ctx.beginPath();
          ctx.arc(cx, cy, r, 0, Math.PI * 2);
          ctx.stroke();
          const cr = uiPx(UI_CROSS, scale);
          ctx.beginPath();
          ctx.moveTo(cx - cr, cy); ctx.lineTo(cx + cr, cy);
          ctx.moveTo(cx, cy - cr); ctx.lineTo(cx, cy + cr);
          ctx.stroke();
          ctx.setLineDash([]);
        }
      }
    }

    // Text placement crosshair（UI 装饰：屏幕像素恒定）
    if (textPlacePos) {
      const cr = uiPx(UI_CROSS, scale);
      ctx.strokeStyle = '#fa8c16';
      ctx.lineWidth = UI_LINE * uk;
      ctx.setLineDash([UI_DASH * uk, UI_DASH * uk]);
      ctx.beginPath();
      ctx.moveTo(textPlacePos.x - cr, textPlacePos.y);
      ctx.lineTo(textPlacePos.x + cr, textPlacePos.y);
      ctx.moveTo(textPlacePos.x, textPlacePos.y - cr);
      ctx.lineTo(textPlacePos.x, textPlacePos.y + cr);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // Selection highlight（对齐插件 drawSelection：蓝色虚线框 + 蓝色实心把手）
    // 任意工具下，只要存在选中标注就显示；矩形类 8 把手可缩放，箭头两端点，圆/放大镜 8 向，序号/文字/画笔仅 move。
    if (selectedId) {
      const sel = annotations.find(a => a.id === selectedId);
      if (sel) {
        const fr = selFramePx(sel);
        const handleR = uiPx(UI_HANDLE_R, scale);
        ctx.strokeStyle = '#1890ff';
        ctx.lineWidth = UI_LINE * uk;
        ctx.setLineDash([UI_DASH * uk, UI_DASH * uk]);
        ctx.strokeRect(fr.x, fr.y, fr.w, fr.h);
        ctx.setLineDash([]);
        for (const hd of fr.handles) {
          ctx.fillStyle = '#1890ff';
          ctx.beginPath(); ctx.arc(hd.x, hd.y, handleR, 0, Math.PI * 2); ctx.fill();
          ctx.strokeStyle = '#fff';
          ctx.lineWidth = UI_LINE_STRONG * uk;
          ctx.stroke();
        }
      }
    }

    // Crop overlay
    if (cropRect || (isCropping && cropStart && drawCurrent)) {
      const c = cropRect || {
        x: Math.min(cropStart!.x, drawCurrent!.x),
        y: Math.min(cropStart!.y, drawCurrent!.y),
        width: Math.abs(drawCurrent!.x - cropStart!.x),
        height: Math.abs(drawCurrent!.y - cropStart!.y),
      };
      ctx.fillStyle = 'rgba(0,0,0,0.5)';
      ctx.fillRect(0, 0, image.width, c.y);
      ctx.fillRect(0, c.y + c.height, image.width, image.height - c.y - c.height);
      ctx.fillRect(0, c.y, c.x, c.height);
      ctx.fillRect(c.x + c.width, c.y, image.width - c.x - c.width, c.height);
      ctx.strokeStyle = '#fff';
      ctx.lineWidth = UI_CROP_LINE * uk;
      ctx.setLineDash([UI_CROP_DASH[0] * uk, UI_CROP_DASH[1] * uk]);
      ctx.strokeRect(c.x, c.y, c.width, c.height);
      ctx.setLineDash([]);
      // 四边整条边可拖拽，不显示中心点/角手柄（仅框线）
    }
  }, [annotations, image, scale, zoom, activeTool, selectedId, isDrawing, drawStart, drawCurrent, penPoints, isCropping, cropStart, cropRect, textPlacePos]);

  /**
   * Mouse → image pixel coordinates.
   */
  const getPos = (e: React.MouseEvent): { x: number; y: number } => {
    const canvas = canvasRef.current;
    if (!canvas) return { x: 0, y: 0 };
    const rect = canvas.getBoundingClientRect();
    const rx = canvas.width / rect.width;
    const ry = canvas.height / rect.height;
    return {
      x: (e.clientX - rect.left) * rx,
      y: (e.clientY - rect.top) * ry,
    };
  };

  const pushHistory = useCallback((a: Annotation[]) => {
    setHistory(prev => {
      const next = prev.slice(0, historyIndex + 1);
      next.push(a);
      if (next.length > 100) next.shift();
      return next;
    });
    setHistoryIndex(prev => Math.min(prev + 1, 99));
  }, [historyIndex]);

  const handleStepBack = () => {
    if (historyIndex > 0) {
      const i = historyIndex - 1;
      setHistoryIndex(i);
      setAnnotations(history[i]);
      setSelectedId(null);
    }
  };

  const handleStepForward = () => {
    if (historyIndex < history.length - 1) {
      const i = historyIndex + 1;
      setHistoryIndex(i);
      setAnnotations(history[i]);
      setSelectedId(null);
    }
  };

  /**
   * 命中检测（入参为**图像坐标**）。
   * 容差按**屏幕像素**定义后除以 scale —— 直接写 8/12 会被当成图像像素，
   * 4K 截图缩小显示时屏幕容差只剩 3.5px，箭头基本点不中。
   */
  const findAt = (x: number, y: number): Annotation | null => {
    for (let i = annotations.length - 1; i >= 0; i--) {
      const a = annotations[i];
      const m = annoSlop(a, scale);
      if (hitAnnotation(a, x, y, m)) return a;
    }
    return null;
  };

  // 坐标钳制到图片范围（move/resize 时防止拖出图外）
  const clampX = (v: number) => (image ? Math.max(0, Math.min(image.width, v)) : v);
  const clampY = (v: number) => (image ? Math.max(0, Math.min(image.height, v)) : v);

  /** 裁剪框手柄命中检测（入参为**图像坐标**，容差同样按屏幕像素换算） */
  const hitTestCrop = (x: number, y: number): null | 'move' | 'n' | 's' | 'e' | 'w' | 'nw' | 'ne' | 'sw' | 'se' => {
    if (!cropRect) return null;
    const { edge, corner } = cropSlop(scale);
    return hitCropHandle(cropRect, x, y, edge, corner);
  };

  /** 根据 cropResize 方向 + 鼠标位移更新 cropRect。
   *  ★ 几何全部委托 cropDrag.ts 纯函数（check:crop 3.8 万例覆盖）——
   *    旧内联实现有代数 bug：`y = ny; h = y + h - ny` 里第二行的 y 已被改写，
   *    h 恒等于旧值 → 拖上/左边变成整体平移；框平移后超出图界，move 的
   *    `nx + w <= image.width` 守卫永远不成立 → 框中间也拖不动。 */
  const updateCropByResize = (dir: NonNullable<typeof cropResize>, pos: { x: number; y: number }) => {
    const o = cropDragOrigin.current;
    if (!o || !image) return;
    // dx/dy 是从「按下点」起的全程位移；base 用按下时的矩形快照 rect，
    // 每帧都用同一 base 反算 → 框跟手移动，不会越拖越快冲到边界。
    const dx = pos.x - o.startX;
    const dy = pos.y - o.startY;
    const next =
      dir === 'move'
        ? moveRect(o.rect, dx, dy, image.width, image.height)
        : resizeRect(o.rect, dir, dx, dy, image.width, image.height);
    setCropRect(next);
  };

  /** Confirm text annotation from toolbar input（支持就地编辑：editTextId 存在时更新原标注） */
  const confirmText = () => {
    if (!textPlacePos) {
      setTextPlacePos(null);
      setTextInput('');
      return;
    }
    // 清空 → 对齐插件「就地编辑清空即删除」语义
    if (!textInput.trim()) {
      if (editTextId) {
        const na = annotations.filter(a => a.id !== editTextId);
        setAnnotations(na);
        setSelectedId(null);
        pushHistory(na);
      }
      setTextPlacePos(null);
      setTextInput('');
      setEditTextId(null);
      return;
    }
    const canvas = canvasRef.current;
    let tw = 80, th = 28;
    if (canvas) {
      const ctx = canvas.getContext('2d');
      if (ctx) {
        // 必须用与 drawAnnotation 同一个归一化字号测量（对齐插件 annoTextMetrics/annoTextBox），
        // 否则存进 width/height 的命中框会偏小；asc+desc 精确框高，提交瞬间文字不跳。
        const k = contentScale(image?.height);
        const fs = annoFont(currentFontSize, k);
        const { asc, desc } = annoTextMetrics(fs);
        ctx.font = ANNO_FONT(fs);
        tw = Math.max(ctx.measureText(textInput.trim()).width, fs * 0.5);
        th = asc + desc;
      }
    }
    if (editTextId) {
      const na = annotations.map(a =>
        a.id === editTextId
          ? { ...a, x: textPlacePos.x, y: textPlacePos.y, content: textInput.trim(), width: tw, height: th, color: currentColor, fontSize: currentFontSize }
          : a,
      );
      setAnnotations(na);
      setSelectedId(editTextId);
      pushHistory(na);
    } else {
      const na: Annotation = {
        id: `a_${Date.now()}`,
        type: 'text',
        x: textPlacePos.x,
        y: textPlacePos.y,
        content: textInput.trim(),
        width: tw,
        height: th,
        color: currentColor,
        fontSize: currentFontSize,
      };
      const annos = [...annotations, na];
      setAnnotations(annos);
      setSelectedId(na.id);
      pushHistory(annos);
    }
    setTextPlacePos(null);
    setTextInput('');
    setEditTextId(null);
  };

  const onMouseDown = (e: React.MouseEvent) => {
    const pos = getPos(e);

    // 0) 选中把手 resize（任意工具下拖动选中标注的 8 把手 / 端点 / move）
    //    对齐插件 annoSelFrame + annoHandleCursor 语义：看得见手柄就一定拖得着。
    const sel = selectedId ? annotations.find(a => a.id === selectedId) : null;
    if (sel) {
      const h = hitSelHandlePx(sel, pos.x, pos.y, scale, image?.height);
      if (h) {
        selResize.current = { id: sel.id, handle: h, startX: pos.x, startY: pos.y, origin: { ...sel } };
        return;
      }
    }

    if (activeTool === 'select') {
      const hit = findAt(pos.x, pos.y);
      if (hit) {
        setSelectedId(hit.id);
        setIsDragging(true);
        dragOffset.current = { dx: pos.x - hit.x, dy: pos.y - hit.y };
      } else {
        setSelectedId(null);
      }
    } else if (activeTool === 'text') {
      // 点已有文字 → 就地编辑（对齐插件 textDragging：重开输入并预填内容）
      const hit = findAt(pos.x, pos.y);
      if (hit && hit.type === 'text') {
        setEditTextId(hit.id);
        setSelectedId(hit.id);
        setTextPlacePos({ x: hit.x, y: hit.y });
        setTextInput(hit.content || '');
        return;
      }
      // 否则在点击处放置锚点，用户从工具栏输入
      setEditTextId(null);
      setTextPlacePos(pos);
      setTextInput('');
    } else if (activeTool === 'crop') {
      if (cropMode === 'frame') {
        // 框选模式：命中手柄 → 调整；框内 → 整体移动；框外 → 重置默认裁剪框
        if (cropRect) {
          const dir = hitTestCrop(pos.x, pos.y);
          if (dir) {
            // 记录按下时的鼠标位置与矩形快照，作为后续每帧位移反算的固定 base
            cropDragOrigin.current = { startX: pos.x, startY: pos.y, rect: { ...cropRect } };
            setCropResize(dir);
            return;
          }
          initDefaultCrop();
          return;
        }
        initDefaultCrop();
      } else {
        // 拖拽模式：无框时按住拖选（有框可拖手柄/移动，拖选新框则先重置）
        if (cropRect) {
          const dir = hitTestCrop(pos.x, pos.y);
          if (dir) {
            cropDragOrigin.current = { startX: pos.x, startY: pos.y, rect: { ...cropRect } };
            setCropResize(dir);
            return;
          }
        }
        setIsCropping(true);
        setCropStart(pos);
        setDrawCurrent(pos);
      }
    } else if (activeTool === 'delete') {
      const hit = findAt(pos.x, pos.y);
      if (hit) {
        const na = annotations.filter(a => a.id !== hit.id);
        setAnnotations(na);
        setSelectedId(null);
        pushHistory(na);
      }
    } else if (activeTool === 'badge') {
      // 序号标注：单击即创建（对齐插件 pointerdown 即生成、不拖拽）。
      // 序号 = 现有 badge 最大 n + 1；半径按原图短边 3.2%（下限 10px，对齐插件 badgeRadiusFor）。
      if (!image) return;
      const maxN = annotations.reduce((mx, a) => (a.type === 'badge' ? Math.max(mx, a.n || 0) : mx), 0);
      const na: Annotation = {
        id: `a_${Date.now()}`,
        type: 'badge',
        x: pos.x,
        y: pos.y,
        n: maxN + 1,
        radius: Math.max(10, Math.min(image.width, image.height) * 0.032),
        color: currentColor,
      };
      const annos = [...annotations, na];
      setAnnotations(annos);
      setSelectedId(na.id);
      pushHistory(annos);
    } else {
      // 任意工具先命中已画标注 → 就地拖动 + 选中（对齐插件 hitAnnoId 上移语义）
      const hit = findAt(pos.x, pos.y);
      if (hit) {
        setSelectedId(hit.id);
        setIsDragging(true);
        dragOffset.current = { dx: pos.x - hit.x, dy: pos.y - hit.y };
        return;
      }
      // highlight / arrow / rect / pen / blur / ellipse / magnifier：拖拽创建
      setIsDrawing(true);
      setDrawStart(pos);
      setDrawCurrent(pos);
      if (activeTool === 'pen') setPenPoints([pos]);
    }
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const pos = getPos(e);

    // 选中把手缩放（任意工具下）：以按下时快照为 base 反算，避免重复累加
    if (selResize.current) {
      const rs = selResize.current;
      const dx = pos.x - rs.startX;
      const dy = pos.y - rs.startY;
      setAnnotations(prev => prev.map(a => {
        if (a.id !== rs.id) return a;
        const o = rs.origin;
        if (rs.handle === 'move') {
          if (a.type === 'arrow') {
            const ddx = dx, ddy = dy;
            return { ...a, x: clampX(o.x + ddx), y: clampY(o.y + ddy), endX: (o.endX ?? 0) + ddx, endY: (o.endY ?? 0) + ddy };
          }
          if (a.type === 'magnifier') {
            const ncx = clampX((o.cx ?? o.x) + dx), ncy = clampY((o.cy ?? o.y) + dy);
            return { ...a, x: ncx, y: ncy, cx: ncx, cy: ncy };
          }
          if (a.type === 'pen' && o.points) {
            return { ...a, x: clampX(o.x + dx), y: clampY(o.y + dy), points: o.points.map((v, i) => (i % 2 === 0 ? v + dx : v + dy)) };
          }
          return { ...a, x: clampX(o.x + dx), y: clampY(o.y + dy) };
        }
        if (a.type === 'arrow') {
          if (rs.handle === 'p1') return { ...a, x: clampX(pos.x), y: clampY(pos.y) };
          return { ...a, endX: clampX(pos.x), endY: clampY(pos.y) };
        }
        if (a.type === 'magnifier') {
          const cx = o.cx ?? o.x, cy = o.cy ?? o.y, r = o.r ?? 0;
          const { cx: ncx, cy: ncy, r: nr } = circleFromHandlePx(rs.handle, pos.x, pos.y, cx - r, cy - r, cx + r, cy + r, Math.max(4, r * 0.1));
          return { ...a, cx: ncx, cy: ncy, r: nr, x: ncx, y: ncy };
        }
        if (a.type === 'ellipse') {
          const x1 = o.x, y1 = o.y, x2 = o.x + (o.width || 0), y2 = o.y + (o.height || 0);
          const { cx, cy, r } = circleFromHandlePx(rs.handle, pos.x, pos.y, x1, y1, x2, y2, Math.max(4, (o.strokeWidth || 4)));
          return { ...a, x: cx - r, y: cy - r, width: 2 * r, height: 2 * r };
        }
        // 矩形类（highlight / blur / rect）
        const x1 = o.x, y1 = o.y, x2 = o.x + (o.width || 0), y2 = o.y + (o.height || 0);
        let nx1 = x1, ny1 = y1, nx2 = x2, ny2 = y2;
        if (rs.handle.includes('w')) nx1 = Math.min(pos.x, nx2 - 2);
        if (rs.handle.includes('e')) nx2 = Math.max(pos.x, nx1 + 2);
        if (rs.handle.includes('n')) ny1 = Math.min(pos.y, ny2 - 2);
        if (rs.handle.includes('s')) ny2 = Math.max(pos.y, ny1 + 2);
        const nx = Math.min(nx1, nx2), ny = Math.min(ny1, ny2);
        return { ...a, x: nx, y: ny, width: Math.abs(nx2 - nx1), height: Math.abs(ny2 - ny1) };
      }));
      return;
    }

    // 任意工具下拖动已画标注（对齐插件 hitAnnoId 上移后移动）
    if (isDragging && selectedId) {
      const newX = pos.x - dragOffset.current.dx;
      const newY = pos.y - dragOffset.current.dy;
      setAnnotations(prev => prev.map(a => {
        if (a.id !== selectedId) return a;
        if (a.type === 'arrow') {
          const ddx = newX - a.x, ddy = newY - a.y;
          return { ...a, x: newX, y: newY, endX: (a.endX ?? 0) + ddx, endY: (a.endY ?? 0) + ddy };
        }
        if (a.type === 'magnifier') {
          const ddx = newX - a.x, ddy = newY - a.y;
          return { ...a, x: newX, y: newY, cx: (a.cx ?? 0) + ddx, cy: (a.cy ?? 0) + ddy };
        }
        if (a.type === 'pen' && a.points) {
          // 画笔整体移动：points 每个点同步平移（对齐插件 pen move 语义）
          const ddx = newX - a.x, ddy = newY - a.y;
          return { ...a, x: newX, y: newY, points: a.points.map((v, i) => (i % 2 === 0 ? v + ddx : v + ddy)) };
        }
        return { ...a, x: newX, y: newY };
      }));
      return;
    }

    if (isDrawing) {
      setDrawCurrent(pos);
      if (activeTool === 'pen') {
        setPenPoints(prev => (prev ? [...prev, pos] : [pos]));
      }
    }
    if (isCropping) setDrawCurrent(pos);

    // 裁剪框四边框拉动调整
    if (cropResize) {
      updateCropByResize(cropResize, pos);
      return;
    }
    // 未在调整时更新 hover 状态（用于光标样式）
    if (activeTool === 'crop' && cropRect) {
      setCropHover(hitTestCrop(pos.x, pos.y));
    } else if (cropHover) {
      setCropHover(null);
    }
    // 选中把手 hover（任意工具下命中已选标注的手柄 → 对应缩放光标）
    const seld = selectedId ? annotations.find(a => a.id === selectedId) : null;
    const sh = seld ? hitSelHandlePx(seld, pos.x, pos.y, scale, image?.height) : null;
    if (sh !== selHover) setSelHover(sh);
  };

  const onMouseUp = () => {
    // 选中把手缩放结束（任意工具）
    if (selResize.current) {
      selResize.current = null;
      pushHistory(annotations);
    }
    if (cropResize) {
      setCropResize(null);
      cropDragOrigin.current = null;
      return;
    }
    if (isDragging) {
      setIsDragging(false);
      pushHistory(annotations);
      return;
    }

    if (isCropping && cropStart && drawCurrent) {
      const rx = Math.min(cropStart.x, drawCurrent.x);
      const ry = Math.min(cropStart.y, drawCurrent.y);
      const rw = Math.abs(drawCurrent.x - cropStart.x);
      const rh = Math.abs(drawCurrent.y - cropStart.y);
      if (rw > 10 && rh > 10) setCropRect({ x: rx, y: ry, width: rw, height: rh });
      setIsCropping(false);
      setCropStart(null);
      setDrawCurrent(null);
      return;
    }

    if (!isDrawing || !drawStart || !drawCurrent) return;
    const newA: Annotation = { id: `a_${Date.now()}`, type: activeTool as AnnoType, x: 0, y: 0 };
    if (activeTool === 'highlight' || activeTool === 'blur' || activeTool === 'rect') {
      newA.x = Math.min(drawStart.x, drawCurrent.x);
      newA.y = Math.min(drawStart.y, drawCurrent.y);
      newA.width = Math.abs(drawCurrent.x - drawStart.x);
      newA.height = Math.abs(drawCurrent.y - drawStart.y);
    } else if (activeTool === 'pen' && penPoints && penPoints.length >= 2) {
      // 画笔：扁平点数组 + 包围盒（对齐插件 PenAnno 结构）
      const pts: number[] = [];
      let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
      for (const p of penPoints) {
        pts.push(p.x, p.y);
        if (p.x < minX) minX = p.x;
        if (p.y < minY) minY = p.y;
        if (p.x > maxX) maxX = p.x;
        if (p.y > maxY) maxY = p.y;
      }
      newA.x = minX; newA.y = minY;
      newA.width = Math.max(0, maxX - minX);
      newA.height = Math.max(0, maxY - minY);
      newA.points = pts;
    } else if (activeTool === 'arrow') {
      newA.x = drawStart.x; newA.y = drawStart.y;
      newA.endX = drawCurrent.x; newA.endY = drawCurrent.y;
    } else if (activeTool === 'ellipse' || activeTool === 'magnifier') {
      // 直径式：按下点与光标＝直径两端，圆心＝中点，半径＝两点距离一半
      const r = Math.hypot(drawCurrent.x - drawStart.x, drawCurrent.y - drawStart.y) / 2;
      const cx = (drawStart.x + drawCurrent.x) / 2;
      const cy = (drawStart.y + drawCurrent.y) / 2;
      if (activeTool === 'ellipse') {
        newA.x = cx - r; newA.y = cy - r;
        newA.width = 2 * r; newA.height = 2 * r;
      } else {
        // x/y 作为锚点（= 圆心），拖动时以它算位移；真正绘制用 cx/cy/r
        newA.x = cx; newA.y = cy;
        newA.cx = cx; newA.cy = cy; newA.r = r; newA.zoom = 2;
      }
    }
    let valid = false;
    if (newA.width !== undefined && newA.height !== undefined) valid = newA.width > 5 && newA.height > 5;
    else if (newA.endX !== undefined && newA.endY !== undefined) valid = Math.hypot((newA.endX as number) - newA.x, (newA.endY as number) - newA.y) > 10;
    else if (activeTool === 'magnifier' && newA.r !== undefined) valid = newA.r > 5;
    else if (activeTool === 'pen' && newA.points) valid = newA.points.length >= 4 && (newA.width || 0) > 2 && (newA.height || 0) > 2;
    if (valid) {
      newA.color = currentColor;
      newA.strokeWidth = currentStrokeWidth;
      // 虚线落库（对齐插件 dashed 语义：arrow/rect/ellipse/pen 可勾选虚线）
      if (dashed && (activeTool === 'arrow' || activeTool === 'rect' || activeTool === 'ellipse' || activeTool === 'pen')) newA.dashed = true;
      const na = [...annotations, newA];
      setAnnotations(na);
      setSelectedId(newA.id);
      pushHistory(na);
    }
    setIsDrawing(false);
    setDrawStart(null);
    setDrawCurrent(null);
    setPenPoints(null);
  };

  /**
   * 烧录结果：
   * - full  ：把注释（箭头/高亮/文字/模糊）烧录进【完整图】，用于顶替「全图」视图；
   *           ★ 注意**不裁切**——裁图交给锚点派生，全图与裁图互不影响。
   * - anchor：若当前有裁剪框，把框反算成裁图锚点，用于顶替「裁图」视图
   *          （cropAdaptiveBase64 会用全图 + 锚点重新裁出裁图）。
   * - cropped：若当前有裁剪框，把「full（含注释）」按框裁出的【裁切图】，
   *          供「全图视图裁剪」时直接顶替全图槽位（原全图不保留，应用内可撤销）。
   * 三者按 StepEditor 传入的当前视图独立落槽，互不影响。
   */
  const bake = async (
    img: HTMLImageElement,
  ): Promise<{ full: string | undefined; anchor: CropAnchor | null; cropped: string | undefined }> => {
    try {
      const full = flattenAnnotations(img, annotations) || undefined;
      const anchor = cropRect
        ? rectToCropAnchor(
            { x: cropRect.x, y: cropRect.y, width: cropRect.width, height: cropRect.height },
            actionType,
          )
        : null;
      let cropped: string | undefined;
      if (cropRect && full) {
        cropped = await cropDataUrl(full, {
          x: cropRect.x,
          y: cropRect.y,
          width: cropRect.width,
          height: cropRect.height,
        });
      }
      return { full, anchor, cropped };
    } catch (e) {
      console.error('标注烧录失败：', e);
      return { full: undefined, anchor: null, cropped: undefined };
    }
  };

  const handleSave = () => {
    const doSave = async (img: HTMLImageElement | null) => {
      const r = img
        ? await bake(img)
        : { full: undefined, anchor: null, cropped: undefined };
      onSave(annotations, r);
    };
    if (image) {
      void doSave(image);
    } else {
      // image 未就绪时兜底从 imageUrl 重新加载，避免标注只存元数据没烧进图
      const img = new Image();
      if (imageSrc.startsWith('http://') || imageSrc.startsWith('https://')) {
        img.crossOrigin = 'anonymous';
      }
      img.onload = () => void doSave(img);
      img.onerror = () => onSave(annotations, { full: undefined, anchor: null, cropped: undefined });
      img.src = imageSrc;
    }
  };

  const cancelCrop = () => { setCropRect(null); setCropStart(null); setDrawCurrent(null); setIsCropping(false); setCropResize(null); setCropHover(null); };

  /** 生成默认裁剪框：
   *  1. 有自动锚点（initialAnchor）→ 恒 50% 窗口（cropBoxFromAnchor，贴边夹回）＝「重置为自动」；
   *  2. 否则沿用在用的 initialCropRect（兼容旧调用方）；
   *  3. 都没有 → 贴合图片四边。
   */
  const initDefaultCrop = () => {
    if (!image) return;
    if (initialAnchor && (initialAnchor.cropW ?? 0) > 0 && (initialAnchor.cropH ?? 0) > 0) {
      setCropRect(cropBoxFromAnchor(initialAnchor, actionType, image.width, image.height));
    } else if (initialCropRect && initialCropRect.width > 0 && initialCropRect.height > 0) {
      setCropRect({
        x: initialCropRect.x,
        y: initialCropRect.y,
        width: initialCropRect.width,
        height: initialCropRect.height,
      });
    } else {
      setCropRect({ x: 0, y: 0, width: image.width, height: image.height });
    }
    setIsCropping(false);
    setCropStart(null);
    setDrawCurrent(null);
    setCropResize(null);
    setCropHover(null);
  };

  // Keyboard
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      // If text input is focused, let it handle keys
      if (document.activeElement === textInputRef.current) {
        if (e.key === 'Enter') { e.preventDefault(); confirmText(); }
        if (e.key === 'Escape') { setTextPlacePos(null); setTextInput(''); }
        return;
      }
      // 就地编辑/下拉等输入控件聚焦时不抢快捷键（避免 Ctrl+Z 与输入框撤销冲突）
      const ae = document.activeElement as HTMLElement | null;
      if (ae && (ae.tagName === 'INPUT' || ae.tagName === 'TEXTAREA' || ae.tagName === 'SELECT')) return;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') {
        // 撤销 Ctrl+Z（对齐插件；桌面端重做用 Ctrl+Y，不用 Ctrl+Shift+Z）
        e.preventDefault();
        if (historyIndex > 0) {
          const i = historyIndex - 1;
          setHistoryIndex(i);
          setAnnotations(history[i]);
          setSelectedId(null);
        }
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'y') {
        e.preventDefault();
        if (historyIndex < history.length - 1) {
          const i = historyIndex + 1;
          setHistoryIndex(i);
          setAnnotations(history[i]);
          setSelectedId(null);
        }
      } else if (e.key === 'Delete' || e.key === 'Backspace') {
        if (selectedId) {
          const na = annotations.filter(a => a.id !== selectedId);
          setAnnotations(na);
          setSelectedId(null);
          pushHistory(na);
        }
      } else if (e.key === 'Escape') {
        if (isCropping || isDrawing) { cancelCrop(); setIsDrawing(false); setPenPoints(null); }
        else if (textPlacePos) { setTextPlacePos(null); setTextInput(''); }
        else onClose();
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [selectedId, annotations, isDrawing, isCropping, textPlacePos, textInput, historyIndex, history]);

  const cropCursor = (() => {
    const d = cropResize || cropHover;
    if (!d) return 'crosshair';
    if (d === 'move') return 'move';
    if (d === 'n' || d === 's') return 'ns-resize';
    if (d === 'e' || d === 'w') return 'ew-resize';
    if (d === 'nw' || d === 'se') return 'nwse-resize';
    return 'nesw-resize';
  })();

  const cursor = selHover ? (selHandleCursor(selHover) ?? 'move')
    : activeTool === 'select' ? (isDragging ? 'grabbing' : 'default')
    : activeTool === 'delete' ? 'not-allowed'
    : activeTool === 'text' ? 'text'
    : activeTool === 'crop' ? cropCursor
    : 'crosshair';

  return (
    <div className="annotation-overlay">
      <div className="annotation-toolbar">
        {/* 缩放控件：全局常驻顶栏最左（对齐插件：0.25 步进、范围 1~4、百分比实时显示、当前档位↔100% 双向切换） */}
        <div className="annotation-zoom">
          <button className="annotation-zoom-btn" onClick={() => setZoom(z => Math.max(1, Math.round((z - 0.25) * 100) / 100))} disabled={zoom <= 1} data-tip="缩小（0.25 步进）">−</button>
          <button
            className="annotation-zoom-pct"
            onClick={() => {
              if (zoom !== 1) {
                setPreZoom(zoom);
                setZoom(1);
              } else {
                setZoom(preZoom !== null && preZoom !== 1 ? preZoom : 1);
              }
            }}
            data-tip={
              zoom !== 1
                ? '点击切回完整显示（100%）'
                : preZoom !== null && preZoom !== 1
                  ? `点击切回 ${Math.round(preZoom * 100)}%`
                  : '当前已是完整显示（100%）'
            }
          >{Math.round(zoom * 100)}%</button>
          <button className="annotation-zoom-btn" onClick={() => setZoom(z => Math.min(4, Math.round((z + 0.25) * 100) / 100))} disabled={zoom >= 4} data-tip="放大（0.25 步进）">＋</button>
        </div>
        <div className="annotation-toolbar-sep" />
        <div className="annotation-tools">
          {TOOLS.map(t => (
            <button key={t.id}
              className={`annotation-tool ${activeTool === t.id ? 'annotation-tool-active' : ''}`}
              onClick={() => {
                setActiveTool(t.id);
                setSelectedId(null);
                setTextPlacePos(null);
                setTextInput('');
                if (t.id === 'crop') {
                  // 两种裁剪：drag 拖选区（无默认框，按下即选）；frame 框选手柄（进来自动生成默认框）
                  if (cropMode === 'frame' && !cropRect) initDefaultCrop();
                } else {
                  cancelCrop();
                }
              }}
              data-tip={t.tip}
            >
              <span className="annotation-tool-icon">{t.icon}</span>
            </button>
          ))}
        </div>

        {/* Text 输入浮层渲染在图片点击处（见 canvas-wrapper 内） */}

        {/* 属性面板（对齐插件：颜色块 → 虚线开关 → 线宽 → 字号下拉；裁剪时显示裁剪方式） */}
        {activeTool === 'crop' ? (
          <div className="annotation-settings">
            <div className="annotation-setting-group">
              <span className="annotation-setting-label">裁剪方式</span>
              <div className="annotation-sizes">
                <button className={`annotation-size-btn ${cropMode === 'drag' ? 'active' : ''}`} onClick={() => setCropMode('drag')} data-tip="按住鼠标拖选裁剪区域（对齐插件 cropMode=drag）">拖拽选区</button>
                <button className={`annotation-size-btn ${cropMode === 'frame' ? 'active' : ''}`} onClick={() => setCropMode('frame')} data-tip="用默认框后拖动四边四角调整（对齐插件 cropMode=frame）">框选手柄</button>
              </div>
            </div>
          </div>
        ) : (
          <div className="annotation-settings">
            <div className="annotation-colors">
              {COLOR_PRESETS.map(c => (
                <button
                  key={c}
                  className={`annotation-color-swatch ${currentColor === c ? 'active' : ''}`}
                  style={{ background: c }}
                  onClick={() => setCurrentColor(c)}
                  data-tip={`画笔颜色 ${c}`}
                />
              ))}
            </div>
            {(activeTool === 'arrow' || activeTool === 'rect' || activeTool === 'ellipse' || activeTool === 'pen') && (
              <button className={`annotation-dashed-toggle ${dashed ? 'active' : ''}`} onClick={() => setDashed(!dashed)} data-tip="线条是否使用虚线（对齐插件 dashed）">╌</button>
            )}
            {(activeTool === 'highlight' || activeTool === 'arrow' || activeTool === 'rect' || activeTool === 'ellipse' || activeTool === 'pen' || activeTool === 'blur') && (
              <div className="annotation-thick">
                {SIZE_PRESETS.map(s => (
                  <button key={s.val} className={`annotation-thick-btn ${currentStrokeWidth === s.val ? 'active' : ''}`} onClick={() => setCurrentStrokeWidth(s.val)} data-tip={`线条粗细 ${s.label}（${s.val}px）`}>{s.label}</button>
                ))}
              </div>
            )}
            {activeTool === 'text' && (
              <select className="annotation-font" value={currentFontSize} onChange={(e) => setCurrentFontSize(Number(e.target.value))} data-tip="字号档位（14/20/24/28/36/48px）">
                {FONT_SIZES.map(s => <option key={s} value={s}>{s}px</option>)}
              </select>
            )}
          </div>
        )}

        <div className="annotation-toolbar-spacer" />

        <div className="annotation-actions">
          <button className="annotation-history-btn" onClick={handleStepBack} disabled={historyIndex <= 0} data-tip="撤销（Ctrl+Z）">↩</button>
          <button className="annotation-history-btn" onClick={handleStepForward} disabled={historyIndex >= history.length - 1} data-tip="重做（Ctrl+Y）">↪</button>
          {cropRect && (
            <>
              <button className="annotation-crop-apply-btn" onClick={handleSave} data-tip="按当前选框裁掉其余部分">应用裁剪</button>
              <button className="annotation-crop-cancel-btn" onClick={cancelCrop} data-tip="放弃这次裁剪，保留整张图">取消裁剪</button>
            </>
          )}
          <button className="annotation-save-btn" onClick={handleSave} data-tip="保存标注，覆盖这一步的截图">✓ 完成</button>
          <button className="annotation-close-btn" onClick={onClose} data-tip="放弃所有改动并关闭">✕</button>
        </div>
      </div>
      <div className="annotation-canvas-area" ref={areaRef}>
        <div
          className="annotation-canvas-wrapper"
          style={{ width: image ? image.width * scale * zoom : 0, height: image ? image.height * scale * zoom : 0 }}
        >
          <canvas ref={canvasRef}
            onMouseDown={onMouseDown} onMouseMove={onMouseMove} onMouseUp={onMouseUp} onMouseLeave={onMouseUp}
            style={{ cursor, display: 'block' }}
          />
          {activeTool === 'text' && textPlacePos && image && (
            <div
              className="annotation-text-popover"
              style={{
                left: Math.min(textPlacePos.x * scale * zoom, image.width * scale * zoom - 260),
                top: Math.min(textPlacePos.y * scale * zoom, image.height * scale * zoom - 100),
              }}
            >
              <div className="annotation-text-popover-row">
                <input
                  ref={textInputRef}
                  className="annotation-text-popover-input"
                  value={textInput}
                  onChange={(e) => setTextInput(e.target.value)}
                  placeholder="输入文字"
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') { e.preventDefault(); confirmText(); }
                    if (e.key === 'Escape') { setTextPlacePos(null); setTextInput(''); setEditTextId(null); }
                  }}
                />
                <button className="annotation-text-popover-confirm" onClick={confirmText} data-tip="把这行文字放到图上">✓</button>
                <button className="annotation-text-popover-cancel" onClick={() => { setTextPlacePos(null); setTextInput(''); setEditTextId(null); }} data-tip="不写了">✕</button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
