import { useCallback, useEffect, useRef, useState } from 'react';

/**
 * 手动框选裁图 —— 让用户把「自动裁图没对准」的步骤一框修好。
 *
 * 与标注编辑器的裁剪工具的区别（关键）：
 * - 标注裁剪会**烧录并覆盖原图**（crop_step_screenshot），原图丢失，之后无法再放大重裁；
 * - 本组件只输出一个**矩形（图坐标 px）**，由调用方反算成 crop 锚点落库，
 *   原图完整保留，可以反复重裁、随时放大缩小。
 *
 * 输出坐标 = 原图像素坐标（未受显示缩放影响），调用方负责换算/落库。
 */

/** 图坐标矩形 */
export interface CropRectPx {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Props {
  /** 全图 base64（不含 data: 前缀） */
  imageBase64: string;
  /** 初始选区（图坐标 px）。传入当前自动裁图结果，用户可在此基础上微调 */
  initialRect?: CropRectPx | null;
  onConfirm: (rect: CropRectPx) => void;
  onCancel: () => void;
}

type Handle = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w';

/** 选区最小边长（图坐标 px） */
const MIN_SIZE = 20;

const HANDLES: Handle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

const HANDLE_STYLE: Record<Handle, React.CSSProperties> = {
  nw: { left: 0, top: 0, cursor: 'nwse-resize' },
  n: { left: '50%', top: 0, cursor: 'ns-resize' },
  ne: { left: '100%', top: 0, cursor: 'nesw-resize' },
  e: { left: '100%', top: '50%', cursor: 'ew-resize' },
  se: { left: '100%', top: '100%', cursor: 'nwse-resize' },
  s: { left: '50%', top: '100%', cursor: 'ns-resize' },
  sw: { left: 0, top: '100%', cursor: 'nesw-resize' },
  w: { left: 0, top: '50%', cursor: 'ew-resize' },
};

interface DragState {
  mode: 'new' | 'move' | Handle;
  startX: number;
  startY: number;
  origin: CropRectPx;
}

export default function CropOverlay({ imageBase64, initialRect, onConfirm, onCancel }: Props) {
  const stageRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<DragState | null>(null);

  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [scale, setScale] = useState(1);
  const [rect, setRect] = useState<CropRectPx | null>(null);
  const [dragging, setDragging] = useState(false);

  /* ── 加载图片并计算显示缩放 ─────────────────────── */
  useEffect(() => {
    const img = new Image();
    img.onload = () => {
      setImage(img);
      const maxW = window.innerWidth * 0.92;
      const maxH = window.innerHeight - 150;
      const s = Math.min(maxW / img.naturalWidth, maxH / img.naturalHeight, 1);
      setScale(s);
    };
    img.onerror = () => setImage(null);
    img.src = `data:image/jpeg;base64,${imageBase64}`;
  }, [imageBase64]);

  /** 图片就绪后：用传入的初始选区，或默认取画面中央 60% */
  useEffect(() => {
    if (!image || rect) return;
    const W = image.naturalWidth;
    const H = image.naturalHeight;
    if (initialRect && initialRect.width > 0 && initialRect.height > 0) {
      setRect(clampRect(initialRect, W, H));
    } else {
      const w = Math.round(W * 0.6);
      const h = Math.round(H * 0.6);
      setRect({ x: Math.round((W - w) / 2), y: Math.round((H - h) / 2), width: w, height: h });
    }
  }, [image, initialRect, rect]);

  /* ── 坐标换算 ───────────────────────────────────── */
  const toImageCoords = useCallback(
    (clientX: number, clientY: number) => {
      const el = stageRef.current;
      if (!el) return { x: 0, y: 0 };
      const b = el.getBoundingClientRect();
      return { x: (clientX - b.left) / scale, y: (clientY - b.top) / scale };
    },
    [scale],
  );

  /* ── 拖拽 ───────────────────────────────────────── */
  const beginDrag = (e: React.MouseEvent, mode: 'new' | 'move' | Handle) => {
    if (!image || !rect) return;
    e.preventDefault();
    e.stopPropagation();
    const p = toImageCoords(e.clientX, e.clientY);
    dragRef.current = { mode, startX: p.x, startY: p.y, origin: { ...rect } };
    setDragging(true);
  };

  useEffect(() => {
    if (!dragging) return;

    const onMove = (e: MouseEvent) => {
      const d = dragRef.current;
      if (!d || !image) return;
      const p = toImageCoords(e.clientX, e.clientY);
      const dx = p.x - d.startX;
      const dy = p.y - d.startY;
      const W = image.naturalWidth;
      const H = image.naturalHeight;

      let next: CropRectPx;
      if (d.mode === 'new') {
        next = {
          x: Math.min(d.startX, p.x),
          y: Math.min(d.startY, p.y),
          width: Math.abs(dx),
          height: Math.abs(dy),
        };
      } else if (d.mode === 'move') {
        next = { ...d.origin, x: d.origin.x + dx, y: d.origin.y + dy };
      } else {
        next = resizeRect(d.origin, d.mode, dx, dy);
      }
      setRect(clampRect(next, W, H));
    };

    const onUp = () => {
      dragRef.current = null;
      setDragging(false);
    };

    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    return () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
  }, [dragging, image, scale, toImageCoords]);

  /* ── 键盘：Enter 确认 / Esc 取消 ───────────────── */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onCancel();
      } else if (e.key === 'Enter' && rect) {
        e.preventDefault();
        onConfirm(rect);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [rect, onCancel, onConfirm]);

  if (!image) {
    return (
      <div style={styles.backdrop}>
        <div style={{ color: '#fff' }}>截图加载失败</div>
      </div>
    );
  }

  const W = image.naturalWidth;
  const H = image.naturalHeight;
  const dispW = Math.round(W * scale);
  const dispH = Math.round(H * scale);

  return (
    <div style={styles.backdrop} onMouseDown={(e) => { if (e.target === e.currentTarget) onCancel(); }}>
      <div style={styles.panel} onMouseDown={(e) => e.stopPropagation()}>
        <div style={styles.header}>
          <strong style={{ fontSize: 14 }}>框选裁图区域</strong>
          <span style={styles.hint}>
            拖动空白处新建 · 拖动选区移动 · 拖八个控制点调整 · Enter 确认 / Esc 取消
          </span>
        </div>

        <div style={styles.canvasWrap}>
          <div
            ref={stageRef}
            style={{ ...styles.stage, width: dispW, height: dispH }}
            onMouseDown={(e) => {
              if (e.target === e.currentTarget || (e.target as HTMLElement).tagName === 'IMG') {
                beginDrag(e, 'new');
              }
            }}
          >
            <img
              src={`data:image/jpeg;base64,${imageBase64}`}
              alt="截图"
              style={{ width: dispW, height: dispH, display: 'block', userSelect: 'none' }}
              draggable={false}
            />

            {rect && (
              <div
                style={{
                  position: 'absolute',
                  left: rect.x * scale,
                  top: rect.y * scale,
                  width: rect.width * scale,
                  height: rect.height * scale,
                  boxShadow: '0 0 0 9999px rgba(0, 0, 0, 0.55)',
                  outline: '1px solid #fff',
                  cursor: 'move',
                }}
                onMouseDown={(e) => beginDrag(e, 'move')}
              >
                {HANDLES.map((h) => (
                  <div
                    key={h}
                    style={{
                      position: 'absolute',
                      width: 10,
                      height: 10,
                      marginLeft: -5,
                      marginTop: -5,
                      background: '#fff',
                      border: '1px solid #333',
                      borderRadius: 2,
                      ...HANDLE_STYLE[h],
                    }}
                    onMouseDown={(e) => beginDrag(e, h)}
                  />
                ))}
              </div>
            )}
          </div>
        </div>

        <div style={styles.footer}>
          <span style={styles.size}>
            {rect ? `${rect.width} × ${rect.height} px @ (${rect.x}, ${rect.y})` : ''}
          </span>
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="btn" onClick={onCancel}>
              取消
            </button>
            <button
              className="btn"
              onClick={() => setRect({ x: 0, y: 0, width: W, height: H })}
              title="选区铺满整张截图"
            >
              全选
            </button>
            <button
              className="btn btn-primary"
              disabled={!rect}
              onClick={() => rect && onConfirm(rect)}
            >
              应用裁图
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

/* ── 纯函数 helpers ────────────────────────────────── */

/** 把矩形限制在图片范围内，并保证最小边长 */
function clampRect(r: CropRectPx, W: number, H: number): CropRectPx {
  const w = Math.min(Math.max(r.width, MIN_SIZE), W);
  const h = Math.min(Math.max(r.height, MIN_SIZE), H);
  const x = Math.min(Math.max(r.x, 0), Math.max(0, W - w));
  const y = Math.min(Math.max(r.y, 0), Math.max(0, H - h));
  return { x: Math.round(x), y: Math.round(y), width: Math.round(w), height: Math.round(h) };
}

/** 按控制点方向调整矩形（dx/dy 为图坐标位移） */
function resizeRect(o: CropRectPx, h: Handle, dx: number, dy: number): CropRectPx {
  let { x, y, width: w, height: ht } = o;
  if (h.includes('w')) {
    x = o.x + dx;
    w = o.width - dx;
  }
  if (h.includes('e')) {
    w = o.width + dx;
  }
  if (h.includes('n')) {
    y = o.y + dy;
    ht = o.height - dy;
  }
  if (h.includes('s')) {
    ht = o.height + dy;
  }
  // 负宽/负高归零，交由 clampRect 兜到最小边长
  return { x, y, width: Math.max(0, w), height: Math.max(0, ht) };
}

/* ── 样式 ──────────────────────────────────────────── */

const styles: Record<string, React.CSSProperties> = {
  backdrop: {
    position: 'fixed',
    inset: 0,
    background: 'rgba(15, 18, 22, 0.75)',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    zIndex: 3000,
  },
  panel: {
    background: '#fff',
    borderRadius: 10,
    boxShadow: '0 12px 40px rgba(0,0,0,0.35)',
    display: 'flex',
    flexDirection: 'column',
    maxHeight: '94vh',
    overflow: 'hidden',
  },
  header: {
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
    padding: '12px 16px',
    borderBottom: '1px solid #e5e7eb',
  },
  hint: { fontSize: 12, color: '#6b7280' },
  canvasWrap: {
    overflow: 'auto',
    padding: 12,
    background: '#f3f4f6',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
  },
  stage: { position: 'relative', flexShrink: 0 },
  footer: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
    padding: '10px 16px',
    borderTop: '1px solid #e5e7eb',
  },
  size: { fontSize: 12, color: '#374151', fontVariantNumeric: 'tabular-nums' },
};
