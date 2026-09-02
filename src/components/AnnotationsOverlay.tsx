// 截图标注编辑器（桌面端同步移植）
// 来源：flowio-site-next/components/AnnotationsOverlay.tsx
// 提供：选择/移动、文字、高亮框、箭头、模糊、裁剪、删除、撤销/重做、完成烧录

import { useState, useRef, useEffect, useCallback } from 'react';
import { Annotation, AnnoType, drawAnnotation, flattenAnnotations } from '../lib/annotate';
import { containScale, contentArea } from '../lib/fitBox';
import { annoSlop, cropSlop, hitAnnotation, hitCropHandle } from '../lib/annoHit';
import {
  annoFont, annoStroke, contentScale, uiPx, uiScale,
  UI_CROSS, UI_CROP_DASH, UI_CROP_LINE, UI_DASH, UI_HANDLE_R, UI_LINE, UI_LINE_STRONG, UI_PAD,
} from '../lib/annoScale';

export type ToolType = 'select' | 'text' | 'highlight' | 'arrow' | 'blur' | 'crop' | 'delete';

export interface CropRect { x: number; y: number; width: number; height: number; }

interface Props {
  imageUrl: string;
  annotations: Annotation[];
  onSave: (annos: Annotation[], croppedImageUrl?: string) => void;
  onClose: () => void;
}

const TOOLS: { id: ToolType; label: string; icon: string }[] = [
  { id: 'select', label: '选择', icon: '⇲' },
  { id: 'text', label: '文字', icon: 'T' },
  { id: 'highlight', label: '高亮', icon: '□' },
  { id: 'arrow', label: '箭头', icon: '↗' },
  { id: 'blur', label: '模糊', icon: '▦' },
  { id: 'crop', label: '裁剪', icon: '⌗' },
  { id: 'delete', label: '删除标注', icon: '✕' },
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

export default function AnnotationsOverlay({ imageUrl, annotations: initial, onSave, onClose }: Props) {
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
  // 裁剪框拖动/调整起点（用于整体移动的位移计算）
  const resizeOrigin = useRef<{ x: number; y: number }>({ x: 0, y: 0 });

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

  /** 根据 cropResize 方向 + 鼠标位移更新 cropRect（限制在图片范围内，最小 20px） */
  const updateCropByResize = (dir: NonNullable<typeof cropResize>, pos: { x: number; y: number }) => {
    if (!cropRect || !image) return;
    let { x, y, width: w, height: h } = cropRect;
    const min = 20;
    const clampX = (v: number) => Math.max(0, Math.min(image.width, v));
    const clampY = (v: number) => Math.max(0, Math.min(image.height, v));
    if (dir === 'move') {
      const dx = pos.x - resizeOrigin.current.x;
      const dy = pos.y - resizeOrigin.current.y;
      const nx = clampX(x + dx);
      const ny = clampY(y + dy);
      if (nx + w <= image.width) x = nx;
      if (ny + h <= image.height) y = ny;
    } else if (dir === 'n') {
      const ny = clampY(pos.y);
      if (y + h - ny >= min) { y = ny; h = y + h - ny; }
    } else if (dir === 's') {
      const nbottom = clampY(pos.y);
      if (nbottom - y >= min) h = nbottom - y;
    } else if (dir === 'w') {
      const nx = clampX(pos.x);
      if (x + w - nx >= min) { x = nx; w = x + w - nx; }
    } else if (dir === 'e') {
      const nright = clampX(pos.x);
      if (nright - x >= min) w = nright - x;
    } else if (dir === 'nw') {
      const nx = clampX(pos.x), ny = clampY(pos.y);
      if (x + w - nx >= min) { x = nx; w = x + w - nx; }
      if (y + h - ny >= min) { y = ny; h = y + h - ny; }
    } else if (dir === 'ne') {
      const nright = clampX(pos.x), ny = clampY(pos.y);
      if (nright - x >= min) w = nright - x;
      if (y + h - ny >= min) { y = ny; h = y + h - ny; }
    } else if (dir === 'sw') {
      const nx = clampX(pos.x), nbottom = clampY(pos.y);
      if (x + w - nx >= min) { x = nx; w = x + w - nx; }
      if (nbottom - y >= min) h = nbottom - y;
    } else if (dir === 'se') {
      const nright = clampX(pos.x), nbottom = clampY(pos.y);
      if (nright - x >= min) w = nright - x;
      if (nbottom - y >= min) h = nbottom - y;
    }
    setCropRect({ x, y, width: w, height: h });
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
          resizeOrigin.current = pos;
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

  const applyCrop = () => {
    if (!cropRect || !image) return null;
    const tc = document.createElement('canvas');
    tc.width = cropRect.width;
    tc.height = cropRect.height;
    const ctx = tc.getContext('2d');
    if (!ctx) return null;
    ctx.drawImage(image, cropRect.x, cropRect.y, cropRect.width, cropRect.height, 0, 0, cropRect.width, cropRect.height);
    const adjusted = annotations
      .map(a => ({
        ...a,
        x: a.x - cropRect.x,
        y: a.y - cropRect.y,
        endX: a.endX !== undefined ? a.endX - cropRect.x : undefined,
        endY: a.endY !== undefined ? a.endY - cropRect.y : undefined,
      }))
      .filter(a => {
        if (a.type === 'arrow') {
          const x1 = a.x, y1 = a.y, x2 = a.endX ?? 0, y2 = a.endY ?? 0;
          return (x1 >= 0 && x1 <= cropRect.width && y1 >= 0 && y1 <= cropRect.height) ||
                 (x2 >= 0 && x2 <= cropRect.width && y2 >= 0 && y2 <= cropRect.height);
        }
        return a.x + (a.width || 0) > 0 && a.x < cropRect.width &&
               a.y + (a.height || 0) > 0 && a.y < cropRect.height;
      });
    // ⚠️ dims 必须传【原图】尺寸：它是内容归一化的基准，传裁切尺寸会让标注粗细随裁切框突变
    for (const a of adjusted) drawAnnotation(ctx, a, { width: image.width, height: image.height });
    setAnnotations(adjusted);
    setHistory([adjusted]);
    setHistoryIndex(0);
    return tc.toDataURL('image/png');
  };

  const bake = (img: HTMLImageElement): string | undefined => {
    try {
      if (cropRect) return applyCrop() || undefined;
      return flattenAnnotations(img, annotations) || undefined;
    } catch (e) {
      console.error('标注烧录失败：', e);
      return undefined;
    }
  };

  const handleSave = () => {
    const doSave = (img: HTMLImageElement | null) => {
      const baked = img ? bake(img) : undefined;
      onSave(annotations, baked);
    };
    if (image) {
      doSave(image);
    } else {
      // image 未就绪时兜底从 imageUrl 重新加载，避免标注只存元数据没烧进图
      const img = new Image();
      if (imageSrc.startsWith('http://') || imageSrc.startsWith('https://')) {
        img.crossOrigin = 'anonymous';
      }
      img.onload = () => doSave(img);
      img.onerror = () => doSave(null);
      img.src = imageSrc;
    }
  };

  const cancelCrop = () => { setCropRect(null); setCropStart(null); setDrawCurrent(null); setIsCropping(false); setCropResize(null); setCropHover(null); };

  /** 生成默认裁剪框（完全贴合图片四边，向中间拉拽裁剪），进入裁剪工具时直接可用四边框拉动 */
  const initDefaultCrop = () => {
    if (!image) return;
    setCropRect({ x: 0, y: 0, width: image.width, height: image.height });
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
              title={t.label}
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
                    title={c}
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
          <button className="annotation-history-btn" onClick={handleStepBack} disabled={historyIndex <= 0} title="上一步">←</button>
          <button className="annotation-history-btn" onClick={handleStepForward} disabled={historyIndex >= history.length - 1} title="下一步">→</button>
          {cropRect && (
            <>
              <button className="annotation-crop-apply-btn" onClick={handleSave}>应用裁剪</button>
              <button className="annotation-crop-cancel-btn" onClick={cancelCrop}>取消裁剪</button>
            </>
          )}
          <button className="annotation-save-btn" onClick={handleSave}>完成</button>
          <button className="annotation-close-btn" onClick={onClose}>✕</button>
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
                <button className="annotation-text-popover-confirm" onClick={confirmText} title="确认">✓</button>
                <button className="annotation-text-popover-cancel" onClick={() => { setTextPlacePos(null); setTextInput(''); }} title="取消">✕</button>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
