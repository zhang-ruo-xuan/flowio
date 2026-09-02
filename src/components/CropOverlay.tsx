import { useCallback, useEffect, useRef, useState } from 'react';
import {
  type CropHandle,
  type CropRectPx,
  clampRect,
  moveRect,
  newRect,
  resizeRect,
} from '../lib/cropDrag';
import { containScale } from '../lib/fitBox';

export type { CropRectPx };

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

interface Props {
  /** 全图 base64（不含 data: 前缀） */
  imageBase64: string;
  /** 初始选区（图坐标 px）。传入当前自动裁图结果，用户可在此基础上微调 */
  initialRect?: CropRectPx | null;
  onConfirm: (rect: CropRectPx) => void;
  onCancel: () => void;
}

const HANDLES: CropHandle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

const HANDLE_STYLE: Record<CropHandle, React.CSSProperties> = {
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
  mode: 'new' | 'move' | CropHandle;
  startX: number;
  startY: number;
  origin: CropRectPx;
}

export default function CropOverlay({ imageBase64, initialRect, onConfirm, onCancel }: Props) {
  const stageRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<DragState | null>(null);

  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const [scale, setScale] = useState(1);
  /** 内容区实测可用尺寸（已扣 padding），由 ResizeObserver 维护 */
  const [avail, setAvail] = useState<{ w: number; h: number } | null>(null);
  const [rect, setRect] = useState<CropRectPx | null>(null);
  const [dragging, setDragging] = useState(false);

  /* ── 加载图片 ───────────────────────────────────── */
  // ⚠️ 这里**不要**算 scale：可用尺寸由 ResizeObserver 实测，见下方 fit 效果。
  // 历史 bug：旧代码用 window.innerWidth*0.92 / innerHeight-150 硬编码可用区，
  // 与真实留白（panel maxHeight 94vh + 头部/底部/padding 约 128px）对不上，
  // 高度约束时反而溢出 → canvasWrap 出现滚动条，且图比能显示的小约 5%。
  useEffect(() => {
    const img = new Image();
    img.onload = () => {
      setImage(img);
      setStatus('ready');
    };
    img.onerror = () => setStatus('error');
    img.src = `data:image/jpeg;base64,${imageBase64}`;
  }, [imageBase64]);

  /* ── 实测内容区可用尺寸 ─────────────────────────── */
  // canvasWrap 是 flex-shrink 的（panel 有 maxHeight:94vh），所以它的尺寸
  // 就是「内容装得下的最大尺寸」——测量它无需硬编码任何留白常量。
  // contentRect 是 content box，天然已扣掉 padding:12。
  useEffect(() => {
    const el = wrapRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r && r.width > 0 && r.height > 0) setAvail({ w: r.width, h: r.height });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 图或可用区任一变化 → 重算等比缩放（永不放大）
  useEffect(() => {
    if (!image || !avail) return;
    setScale(
      containScale({
        availW: avail.w,
        availH: avail.h,
        imgW: image.naturalWidth,
        imgH: image.naturalHeight,
      }),
    );
  }, [image, avail]);

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
  const beginDrag = (e: React.MouseEvent, mode: 'new' | 'move' | CropHandle) => {
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

      // 三个分支各自保证「对角固定 / 尺寸恒定 / 不越界」，无需再外层 clamp
      const next: CropRectPx =
        d.mode === 'new'
          ? newRect(d.startX, d.startY, p.x, p.y, W, H)
          : d.mode === 'move'
            ? moveRect(d.origin, dx, dy, W, H)
            : resizeRect(d.origin, d.mode, dx, dy, W, H);
      setRect(next);
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

  // 旧实现把「还没加载完」也当成加载失败，data: URL 解码期间会闪一句「截图加载失败」
  if (status === 'error' || !image) {
    return (
      <div style={styles.backdrop}>
        <div style={{ color: '#fff' }}>{status === 'error' ? '截图加载失败' : '加载中…'}</div>
      </div>
    );
  }

  const W = image.naturalWidth;
  const H = image.naturalHeight;
  const dispW = Math.round(W * scale);
  const dispH = Math.round(H * scale);
  // 可用区必须先实测再定缩放，所以首帧图仍是原尺寸 —— 先藏起来，避免闪一下
  const fitted = avail !== null;

  return (
    <div style={styles.backdrop} onMouseDown={(e) => { if (e.target === e.currentTarget) onCancel(); }}>
      <div style={styles.panel} onMouseDown={(e) => e.stopPropagation()}>
        <div style={styles.header}>
          <strong style={{ fontSize: 14 }}>框选裁图区域</strong>
          <span style={styles.hint}>
            拖动空白处新建 · 拖动选区移动 · 拖八个控制点调整 · Enter 确认 / Esc 取消
          </span>
        </div>

        <div style={styles.canvasWrap} ref={wrapRef}>
          <div
            ref={stageRef}
            style={{
              ...styles.stage,
              width: dispW,
              height: dispH,
              visibility: fitted ? 'visible' : 'hidden',
            }}
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
    // 旧实现只限了高度。首帧图还是原尺寸（等实测可用区），若不限宽度，
    // 超宽截图会把面板撑到视口外、闪出横向滚动条。与 maxHeight 对称地限一下。
    maxWidth: '94vw',
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
