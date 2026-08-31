// 截图标注编辑器（桌面端同步移植）
// 来源：flowio-site-next/components/AnnotationsOverlay.tsx
// 提供：选择/移动、文字、高亮框、箭头、模糊、裁剪、删除、撤销/重做、完成烧录

import { useState, useRef, useEffect, useCallback } from 'react';
import { Annotation, AnnoType, drawAnnotation, flattenAnnotations } from '../lib/annotate';

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
    img.onload = () => {
      setImage(img);
      const maxW = Math.min(window.innerWidth * 0.995, img.width);
      const maxH = window.innerHeight - 56;
      const s = Math.min(maxW / img.width, maxH / img.height, 1);
      setScale(s);
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

  // 窗口尺寸变化时重新计算缩放，截图跟随放大
  useEffect(() => {
    const onResize = () => {
      if (!image) return;
      const maxW = Math.min(window.innerWidth * 0.995, image.width);
      const maxH = window.innerHeight - 56;
      const s = Math.min(maxW / image.width, maxH / image.height, 1);
      setScale(s);
    };
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [image]);

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
        ctx.lineWidth = currentStrokeWidth;
        ctx.setLineDash([5, 5]);
        ctx.strokeRect(rx, ry, rw, rh);
        ctx.setLineDash([]);
      } else if (activeTool === 'arrow') {
        ctx.beginPath();
        ctx.moveTo(drawStart.x, drawStart.y);
        ctx.lineTo(drawCurrent.x, drawCurrent.y);
        ctx.strokeStyle = currentColor;
        ctx.lineWidth = currentStrokeWidth;
        ctx.setLineDash([5, 5]);
        ctx.stroke();
        ctx.setLineDash([]);
      } else if (activeTool === 'blur') {
        const rx = Math.min(drawStart.x, drawCurrent.x), ry = Math.min(drawStart.y, drawCurrent.y);
        const rw = Math.abs(drawCurrent.x - drawStart.x), rh = Math.abs(drawCurrent.y - drawStart.y);
        ctx.fillStyle = 'rgba(255,255,255,0.3)';
        ctx.fillRect(rx, ry, rw, rh);
        ctx.strokeStyle = currentColor;
        ctx.lineWidth = 1;
        ctx.setLineDash([4, 4]);
        ctx.strokeRect(rx, ry, rw, rh);
        ctx.setLineDash([]);
      }
    }

    // Text placement crosshair
    if (textPlacePos) {
      ctx.strokeStyle = '#FF6B35';
      ctx.lineWidth = 1;
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.moveTo(textPlacePos.x - 10, textPlacePos.y);
      ctx.lineTo(textPlacePos.x + 10, textPlacePos.y);
      ctx.moveTo(textPlacePos.x, textPlacePos.y - 10);
      ctx.lineTo(textPlacePos.x, textPlacePos.y + 10);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // Selection highlight
    if (selectedId && activeTool === 'select') {
      const sel = annotations.find(a => a.id === selectedId);
      if (sel) {
        if (sel.type === 'arrow') {
          ctx.strokeStyle = '#999';
          ctx.lineWidth = 1;
          ctx.setLineDash([3, 3]);
          ctx.beginPath();
          ctx.moveTo(sel.x, sel.y);
          ctx.lineTo(sel.endX ?? 0, sel.endY ?? 0);
          ctx.stroke();
          ctx.setLineDash([]);
          const handles = [[sel.x, sel.y], [sel.endX ?? 0, sel.endY ?? 0]];
          for (const [px, py] of handles) {
            ctx.fillStyle = '#fff';
            ctx.beginPath(); ctx.arc(px, py, 5, 0, Math.PI * 2); ctx.fill();
            ctx.strokeStyle = '#999';
            ctx.lineWidth = 1.5;
            ctx.stroke();
          }
        } else {
          // 灰色虚线框 + 四角圆形手柄（微信截图风格）
          const pad = 4;
          const bx = sel.x, by = sel.y, bw = sel.width || 0, bh = sel.height || 0;
          ctx.strokeStyle = '#999';
          ctx.lineWidth = 1;
          ctx.setLineDash([3, 3]);
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
            ctx.beginPath(); ctx.arc(cx, cy, 5, 0, Math.PI * 2); ctx.fill();
            ctx.strokeStyle = '#999';
            ctx.lineWidth = 1.5;
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
      ctx.lineWidth = 1.5;
      ctx.setLineDash([6, 3]);
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

  const findAt = (x: number, y: number): Annotation | null => {
    for (let i = annotations.length - 1; i >= 0; i--) {
      const a = annotations[i];
      if (a.type === 'arrow') {
        const dx = (a.endX ?? 0) - a.x, dy = (a.endY ?? 0) - a.y;
        const len = Math.hypot(dx, dy);
        if (len === 0) continue;
        const t = Math.max(0, Math.min(1, ((x - a.x) * dx + (y - a.y) * dy) / (len * len)));
        if (Math.hypot(x - (a.x + t * dx), y - (a.y + t * dy)) < 12) return a;
      } else {
        const m = 8;
        const w = a.width || 0, h = a.height || 0;
        const bx = a.x;
        const by = a.y;
        if (x >= bx - m && x <= bx + w + m &&
            y >= by - m && y <= by + h + m) return a;
      }
    }
    return null;
  };

  /** 裁剪框手柄命中检测（图像像素坐标） */
  const hitTestCrop = (x: number, y: number): null | 'move' | 'n' | 's' | 'e' | 'w' | 'nw' | 'ne' | 'sw' | 'se' => {
    if (!cropRect) return null;
    const { x: cx, y: cy, width: cw, height: ch } = cropRect;
    const hit = 14; // 边手柄命中半径
    const corner = 16; // 角手柄命中范围
    const inX = x >= cx && x <= cx + cw;
    const inY = y >= cy && y <= cy + ch;

    // 四角优先
    if (Math.abs(x - cx) <= corner && Math.abs(y - cy) <= corner) return 'nw';
    if (Math.abs(x - (cx + cw)) <= corner && Math.abs(y - cy) <= corner) return 'ne';
    if (Math.abs(x - cx) <= corner && Math.abs(y - (cy + ch)) <= corner) return 'sw';
    if (Math.abs(x - (cx + cw)) <= corner && Math.abs(y - (cy + ch)) <= corner) return 'se';

    // 四边
    if (inX && Math.abs(y - cy) <= hit) return 'n';
    if (inX && Math.abs(y - (cy + ch)) <= hit) return 's';
    if (inY && Math.abs(x - cx) <= hit) return 'w';
    if (inY && Math.abs(x - (cx + cw)) <= hit) return 'e';

    // 框内 → 整体移动
    if (x > cx && x < cx + cw && y > cy && y < cy + ch) return 'move';
    return null;
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
        ctx.font = `bold ${currentFontSize}px "Noto Sans SC", "Microsoft YaHei", sans-serif`;
        tw = ctx.measureText(textInput.trim()).width;
        th = currentFontSize + 4;
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
    for (const a of adjusted) drawAnnotation(ctx, a, { width: cropRect.width, height: cropRect.height });
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
      <div className="annotation-canvas-area">
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
