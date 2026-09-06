// 截图标注编辑器（桌面端同步移植）
// 来源：flowio-site-next/components/AnnotationsOverlay.tsx
// 提供：选择/移动、文字、高亮框、箭头、模糊、裁剪、删除、撤销/重做、完成烧录

import { useState, useRef, useEffect, useCallback } from 'react';
import { Annotation, AnnoType, drawAnnotation, flattenAnnotations } from '../lib/annotate';
import { rectToCropAnchor, type CropAnchor } from '../lib/cropAnchor';
import { containScale, contentArea } from '../lib/fitBox';
import { annoSlop, cropSlop, hitAnnotation, hitCropHandle } from '../lib/annoHit';
import { moveRect, resizeRect } from '../lib/cropDrag';
import {
  annoFont, annoStroke, contentScale, uiPx, uiScale,
  UI_CROSS, UI_CROP_DASH, UI_CROP_LINE, UI_DASH, UI_HANDLE_R, UI_LINE, UI_LINE_STRONG, UI_PAD,
} from '../lib/annoScale';

export type ToolType = 'select' | 'text' | 'highlight' | 'arrow' | 'blur' | 'crop' | 'delete';

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
  /** 步骤 action_type：手动框选的裁剪矩形反算回锚点时，与自动裁图同口径（type 步骤左侧多留 1/3） */
  actionType?: string;
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
  { id: 'select', label: '选择', icon: '⇲', tip: '选择\n点选已画好的标注，再拖动调整位置' },
  { id: 'text', label: '文字', icon: 'T', tip: '文字\n在图上点一下，输入要写的说明文字' },
  { id: 'highlight', label: '高亮', icon: '□', tip: '高亮\n拖出半透明色块强调某块区域，底下的内容仍看得见' },
  { id: 'arrow', label: '箭头', icon: '↗', tip: '箭头\n从起点拖到终点，指向要操作的位置' },
  { id: 'blur', label: '模糊', icon: '▦', tip: '模糊\n把要遮住的区域涂成马赛克，用来给敏感信息打码' },
  { id: 'crop', label: '裁剪', icon: '⌗', tip: '裁剪\n框出要保留的区域，点「应用裁剪」后其余部分被裁掉' },
  { id: 'delete', label: '删除标注', icon: '✕', tip: '删除标注\n点一下要删的那条标注就删掉，可以连续删' },
];

const COLOR_PRESETS = ['#FF4235', '#FF6B35', '#FFC107', '#4CAF50', '#2196F3', '#9C27B0', '#FFFFFF', '#24292E'];
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

export default function AnnotationsOverlay({ imageUrl, annotations: initial, actionType, initialCropRect, onSave, onClose }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [annotations, setAnnotations] = useState<Annotation[]>(initial);
  const [activeTool, setActiveTool] = useState<ToolType>('select');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [isDrawing, setIsDrawing] = useState(false);
  const [drawStart, setDrawStart] = useState<{ x: number; y: number } | null>(null);
  const [drawCurrent, setDrawCurrent] = useState<{ x: number; y: number } | null>(null);
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
  const [currentColor, setCurrentColor] = useState('#FF4235');
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
    canvas.style.width = (image.width * scale) + 'px';
    canvas.style.height = (image.height * scale) + 'px';

    ctx.clearRect(0, 0, image.width, image.height);
    ctx.drawImage(image, 0, 0);

    // 两种缩放系数：内容随图等比（vk），UI 装饰按屏幕像素恒定（uk）
    const vk = contentScale(image.height);
    const uk = uiScale(scale);

    for (const a of annotations) {
      drawAnnotation(ctx, a, { width: image.width, height: image.height });
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
      }
    }

    // Text placement crosshair（UI 装饰：屏幕像素恒定）
    if (textPlacePos) {
      const cr = uiPx(UI_CROSS, scale);
      ctx.strokeStyle = '#FF6B35';
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

    // Selection highlight（UI 装饰：手柄/虚线在屏幕上的大小与命中容差同基准）
    if (selectedId && activeTool === 'select') {
      const sel = annotations.find(a => a.id === selectedId);
      if (sel) {
        const handleR = uiPx(UI_HANDLE_R, scale);
        if (sel.type === 'arrow') {
          ctx.strokeStyle = '#999';
          ctx.lineWidth = UI_LINE * uk;
          ctx.setLineDash([UI_DASH * uk, UI_DASH * uk]);
          ctx.beginPath();
          ctx.moveTo(sel.x, sel.y);
          ctx.lineTo(sel.endX ?? 0, sel.endY ?? 0);
          ctx.stroke();
          ctx.setLineDash([]);
          const handles = [[sel.x, sel.y], [sel.endX ?? 0, sel.endY ?? 0]];
          for (const [px, py] of handles) {
            ctx.fillStyle = '#fff';
            ctx.beginPath(); ctx.arc(px, py, handleR, 0, Math.PI * 2); ctx.fill();
            ctx.strokeStyle = '#999';
            ctx.lineWidth = UI_LINE_STRONG * uk;
            ctx.stroke();
          }
        } else {
          // 灰色虚线框 + 四角圆形手柄（微信截图风格）
          const pad = uiPx(UI_PAD, scale);
          const bx = sel.x, by = sel.y, bw = sel.width || 0, bh = sel.height || 0;
          ctx.strokeStyle = '#999';
          ctx.lineWidth = UI_LINE * uk;
          ctx.setLineDash([UI_DASH * uk, UI_DASH * uk]);
          ctx.strokeRect(bx - pad, by - pad, bw + pad * 2, bh + pad * 2);
          ctx.setLineDash([]);
          const corners = [
            [bx - pad, by - pad],
            [bx + bw + pad, by - pad],
            [bx - pad, by + bh + pad],
            [bx + bw + pad, by + bh + pad],
          ];
          for (const [cx, cy] of corners) {
            ctx.fillStyle = '#fff';
            ctx.beginPath(); ctx.arc(cx, cy, handleR, 0, Math.PI * 2); ctx.fill();
            ctx.strokeStyle = '#999';
            ctx.lineWidth = UI_LINE_STRONG * uk;
            ctx.stroke();
          }
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
  }, [annotations, image, scale, activeTool, selectedId, isDrawing, drawStart, drawCurrent, isCropping, cropStart, cropRect, textPlacePos]);

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
      if (next.length > 20) next.shift();
      return next;
    });
    setHistoryIndex(prev => Math.min(prev + 1, 19));
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

  /** Confirm text annotation from toolbar input */
  const confirmText = () => {
    if (!textPlacePos || !textInput.trim()) {
      setTextPlacePos(null);
      setTextInput('');
      return;
    }
    const canvas = canvasRef.current;
    let tw = 80, th = 28;
    if (canvas) {
      const ctx = canvas.getContext('2d');
      if (ctx) {
        // 必须用与 drawAnnotation 同一个归一化字号测量，否则存进 width/height 的命中框会偏小
        const k = contentScale(image?.height);
        const fs = annoFont(currentFontSize, k);
        ctx.font = `bold ${fs}px "Noto Sans SC", "Microsoft YaHei", sans-serif`;
        tw = ctx.measureText(textInput.trim()).width;
        th = fs + annoStroke(4, k);
      }
    }
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
    setTextPlacePos(null);
    setTextInput('');
  };

  const onMouseDown = (e: React.MouseEvent) => {
    const pos = getPos(e);

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
      // Click on canvas to place text anchor; user types in toolbar input
      setTextPlacePos(pos);
      setTextInput('');
    } else if (activeTool === 'crop') {
      // 已有裁剪框：命中手柄 → 调整；框内 → 整体移动；框外 → 重置默认裁剪框
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
      // 无框时直接生成默认裁剪框
      initDefaultCrop();
    } else if (activeTool === 'delete') {
      const hit = findAt(pos.x, pos.y);
      if (hit) {
        const na = annotations.filter(a => a.id !== hit.id);
        setAnnotations(na);
        setSelectedId(null);
        pushHistory(na);
      }
    } else {
      // highlight / arrow / blur
      setIsDrawing(true);
      setDrawStart(pos);
      setDrawCurrent(pos);
    }
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const pos = getPos(e);

    if (isDragging && selectedId && activeTool === 'select') {
      const newX = pos.x - dragOffset.current.dx;
      const newY = pos.y - dragOffset.current.dy;
      setAnnotations(prev => prev.map(a => {
        if (a.id !== selectedId) return a;
        if (a.type === 'arrow') {
          const ddx = newX - a.x, ddy = newY - a.y;
          return { ...a, x: newX, y: newY, endX: (a.endX ?? 0) + ddx, endY: (a.endY ?? 0) + ddy };
        }
        return { ...a, x: newX, y: newY };
      }));
      return;
    }

    if (isDrawing) setDrawCurrent(pos);
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
  };

  const onMouseUp = () => {
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
    if (activeTool === 'highlight' || activeTool === 'blur') {
      newA.x = Math.min(drawStart.x, drawCurrent.x);
      newA.y = Math.min(drawStart.y, drawCurrent.y);
      newA.width = Math.abs(drawCurrent.x - drawStart.x);
      newA.height = Math.abs(drawCurrent.y - drawStart.y);
    } else if (activeTool === 'arrow') {
      newA.x = drawStart.x; newA.y = drawStart.y;
      newA.endX = drawCurrent.x; newA.endY = drawCurrent.y;
    }
    let valid = false;
    if (newA.width !== undefined && newA.height !== undefined) valid = newA.width > 5 && newA.height > 5;
    else if (newA.endX !== undefined && newA.endY !== undefined) valid = Math.hypot((newA.endX as number) - newA.x, (newA.endY as number) - newA.y) > 10;
    if (valid) {
      newA.color = currentColor;
      newA.strokeWidth = currentStrokeWidth;
      const na = [...annotations, newA];
      setAnnotations(na);
      setSelectedId(newA.id);
      pushHistory(na);
    }
    setIsDrawing(false);
    setDrawStart(null);
    setDrawCurrent(null);
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

  /** 生成默认裁剪框：传入了 initialCropRect（当前裁图框）就沿用它，否则贴合图片四边 */
  const initDefaultCrop = () => {
    if (!image) return;
    if (initialCropRect && initialCropRect.width > 0 && initialCropRect.height > 0) {
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
      if (e.key === 'Delete' || e.key === 'Backspace') {
        if (selectedId) {
          const na = annotations.filter(a => a.id !== selectedId);
          setAnnotations(na);
          setSelectedId(null);
          pushHistory(na);
        }
      } else if (e.key === 'Escape') {
        if (isCropping || isDrawing) { cancelCrop(); setIsDrawing(false); }
        else if (textPlacePos) { setTextPlacePos(null); setTextInput(''); }
        else onClose();
      }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [selectedId, annotations, isDrawing, isCropping, textPlacePos, textInput]);

  const cropCursor = (() => {
    const d = cropResize || cropHover;
    if (!d) return 'crosshair';
    if (d === 'move') return 'move';
    if (d === 'n' || d === 's') return 'ns-resize';
    if (d === 'e' || d === 'w') return 'ew-resize';
    if (d === 'nw' || d === 'se') return 'nwse-resize';
    return 'nesw-resize';
  })();

  const cursor = activeTool === 'select' ? (isDragging ? 'grabbing' : 'default')
    : activeTool === 'delete' ? 'not-allowed'
    : activeTool === 'text' ? 'text'
    : activeTool === 'crop' ? cropCursor
    : 'crosshair';

  return (
    <div className="annotation-overlay">
      <div className="annotation-toolbar">
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
                  initDefaultCrop();
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

        {/* 颜色与大小设置 */}
        {activeTool !== 'select' && activeTool !== 'crop' && activeTool !== 'delete' && (
          <div className="annotation-settings">
            <div className="annotation-setting-group">
              <span className="annotation-setting-label">颜色</span>
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
            </div>
            {activeTool === 'text' ? (
              <div className="annotation-setting-group">
                <span className="annotation-setting-label">字号</span>
                <div className="annotation-sizes">
                  {FONT_SIZES.map(s => (
                    <button
                      key={s}
                      className={`annotation-size-btn ${currentFontSize === s ? 'active' : ''}`}
                      onClick={() => setCurrentFontSize(s)}
                      data-tip={`字号 ${s}px`}
                    >
                      {s}
                    </button>
                  ))}
                </div>
              </div>
            ) : (
              <div className="annotation-setting-group">
                <span className="annotation-setting-label">粗细</span>
                <div className="annotation-sizes">
                  {SIZE_PRESETS.map(s => (
                    <button
                      key={s.val}
                      className={`annotation-size-btn ${currentStrokeWidth === s.val ? 'active' : ''}`}
                      onClick={() => setCurrentStrokeWidth(s.val)}
                      data-tip={`线条粗细 ${s.label}（${s.val}px）`}
                    >
                      {s.label}
                    </button>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}

        <div className="annotation-actions">
          <button className="annotation-history-btn" onClick={handleStepBack} disabled={historyIndex <= 0} data-tip="撤销上一步">←</button>
          <button className="annotation-history-btn" onClick={handleStepForward} disabled={historyIndex >= history.length - 1} data-tip="重做（撤销多了往回走）">→</button>
          {cropRect && (
            <>
              <button className="annotation-crop-apply-btn" onClick={handleSave} data-tip="按当前选框裁掉其余部分">应用裁剪</button>
              <button className="annotation-crop-cancel-btn" onClick={cancelCrop} data-tip="放弃这次裁剪，保留整张图">取消裁剪</button>
            </>
          )}
          <button className="annotation-save-btn" onClick={handleSave} data-tip="保存标注，覆盖这一步的截图">完成</button>
          <button className="annotation-close-btn" onClick={onClose} data-tip="放弃所有改动并关闭">✕</button>
        </div>
      </div>
      <div className="annotation-canvas-area" ref={areaRef}>
        <div
          className="annotation-canvas-wrapper"
          style={{ width: image ? image.width * scale : 0, height: image ? image.height * scale : 0 }}
        >
          <canvas ref={canvasRef}
            onMouseDown={onMouseDown} onMouseMove={onMouseMove} onMouseUp={onMouseUp} onMouseLeave={onMouseUp}
            style={{ cursor, display: 'block' }}
          />
          {activeTool === 'text' && textPlacePos && image && (
            <div
              className="annotation-text-popover"
              style={{
                left: Math.min(textPlacePos.x * scale, image.width * scale - 260),
                top: Math.min(textPlacePos.y * scale, image.height * scale - 100),
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
                    if (e.key === 'Escape') { setTextPlacePos(null); setTextInput(''); }
                  }}
                />
                <button className="annotation-text-popover-confirm" onClick={confirmText} data-tip="把这行文字放到图上">✓</button>
                <button className="annotation-text-popover-cancel" onClick={() => { setTextPlacePos(null); setTextInput(''); }} data-tip="不写了">✕</button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
