/**
 * 全局悬浮提示层（事件委托版）。
 *
 * 用法：给任意元素加 `data-tip="说明文字"` 即可，鼠标悬浮自动出气泡。
 * 支持 `\n` 换行（CSS 用 pre-line 渲染）。
 *
 * 为什么不用原生 `title`：
 * ① 要等约 1 秒才出现，交互很迟钝；
 * ② 样式是系统灰黄框，和深色 UI 不搭；
 * ③ 无法在文案里做换行排版。
 *
 * 为什么用事件委托而不是包一层 <Tooltip> 组件：
 * 本项目大量按钮位于 `overflow: hidden / auto` 容器里（侧栏、步骤条、
 * 弹窗 body），包一层组件做绝对定位会被父容器裁掉；用 portal + fixed
 * 定位则完全不受影响。且委托模式下给按钮加提示只是加一个属性，
 * 不用改 JSX 结构，也不会破坏 flex 布局。
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

type Pos = { x: number; y: number; placement: "top" | "bottom" };

/** 气泡距目标元素的间隙（px） */
const GAP = 8;
/** 距视口边缘的安全距离（px），超出则夹回来 */
const EDGE = 8;
/** 点击后若鼠标仍停在原处，多久重新浮出提示（ms） */
const RESHOW_MS = 700;

export default function Tooltip() {
  const [tip, setTip] = useState("");
  const [pos, setPos] = useState<Pos | null>(null);
  /** 当前命中的元素，避免同一元素内移动时反复重算/闪烁 */
  const curRef = useRef<HTMLElement | null>(null);
  /** 鼠标最后位置，用于点击后判断「是否还停在原按钮上」 */
  const mouseRef = useRef({ x: 0, y: 0 });
  const tipRef = useRef<HTMLDivElement | null>(null);
  /**
   * 贴边校正后的中心 x。气泡是 translateX(-50%) 居中，靠边时会被视口切掉，
   * 需要把中心点夹回安全区。夹多少取决于气泡**实际**宽度 —— 短文案不该
   * 按 max-width 去夹（否则气泡会明显偏离按钮），所以渲染后量一次再校正。
   */
  const [clampedX, setClampedX] = useState<number | null>(null);

  const hide = useCallback(() => {
    curRef.current = null;
    setTip("");
    setPos(null);
  }, []);

  const showFor = useCallback((el: HTMLElement) => {
    const text = el.getAttribute("data-tip")?.trim();
    // 空文案 / 元素已不可见 → 不显示（disabled 时按钮通常仍可见，故只看尺寸）
    if (!text) return hide();
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return hide();

    const cx = r.left + r.width / 2;
    // 默认放上方；顶部空间不够（距顶 < 56px）就翻到下方
    const placement: Pos["placement"] = r.top > 56 ? "top" : "bottom";
    const y = placement === "top" ? r.top - GAP : r.bottom + GAP;
    setPos({ x: cx, y, placement });
    setTip(text);
  }, [hide]);

  useEffect(() => {
    const onOver = (e: MouseEvent) => {
      const el = (e.target as HTMLElement | null)?.closest?.("[data-tip]") as
        | HTMLElement
        | null;
      if (!el) {
        // 移到没有 tip 的区域 → 收起
        if (curRef.current) hide();
        return;
      }
      if (el === curRef.current) return;
      curRef.current = el;
      showFor(el);
    };

    const onOut = (e: MouseEvent) => {
      const from = (e.target as HTMLElement | null)?.closest?.("[data-tip]") as
        | HTMLElement
        | null;
      if (!from) return;
      // 仍在同一个 tip 元素内部（移到它的子元素上）→ 不收起
      const to = e.relatedTarget as Node | null;
      if (to && from.contains(to)) return;
      hide();
    };

    const onMove = (e: MouseEvent) => {
      mouseRef.current = { x: e.clientX, y: e.clientY };
    };

    // 点击后收起：按钮往往会在点击后消失/禁用/弹窗，气泡留在原位会很怪。
    // 但如果鼠标没移开（比如点了「设为默认」后按钮还在），过一会儿重新浮出，
    // 否则用户会觉得"点完就看不到了"。
    let reshowTimer: ReturnType<typeof setTimeout> | null = null;
    const onDown = () => {
      hide();
      if (reshowTimer) clearTimeout(reshowTimer);
      reshowTimer = setTimeout(() => {
        const { x, y } = mouseRef.current;
        const under = document.elementFromPoint(x, y) as HTMLElement | null;
        const el = under?.closest?.("[data-tip]") as HTMLElement | null;
        if (el) {
          curRef.current = el;
          showFor(el);
        }
      }, RESHOW_MS);
    };

    // 捕获阶段：防止被业务组件的 stopPropagation 吃掉
    document.addEventListener("mouseover", onOver, true);
    document.addEventListener("mouseout", onOut, true);
    document.addEventListener("mousemove", onMove, true);
    document.addEventListener("mousedown", onDown, true);
    // 滚动时位置会失效（fixed 定位不跟随）→ 直接收起
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    window.addEventListener("blur", hide);
    return () => {
      if (reshowTimer) clearTimeout(reshowTimer);
      document.removeEventListener("mouseover", onOver, true);
      document.removeEventListener("mouseout", onOut, true);
      document.removeEventListener("mousemove", onMove, true);
      document.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("blur", hide);
    };
  }, [hide, showFor]);

  // 渲染后按气泡真实宽度做贴边校正（同一帧完成，肉眼看不到跳动）。
  useLayoutEffect(() => {
    if (!pos || !tipRef.current) {
      setClampedX(null);
      return;
    }
    const w = tipRef.current.offsetWidth;
    const half = w / 2;
    const minX = EDGE + half;
    const maxX = Math.max(minX, window.innerWidth - EDGE - half);
    const next = Math.min(Math.max(pos.x, minX), maxX);
    setClampedX(Math.abs(next - pos.x) < 0.5 ? pos.x : next);
  }, [pos, tip]);

  if (!pos || !tip) return null;

  return createPortal(
    <div
      ref={tipRef}
      className={`ui-tip ui-tip-${pos.placement}`}
      style={{ left: clampedX ?? pos.x, top: pos.y }}
      role="tooltip"
    >
      {tip}
    </div>,
    document.body,
  );
}
