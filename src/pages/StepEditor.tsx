import { useState, useEffect, useCallback, useRef, useMemo, Fragment, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import {
  ArrowLeft,
  Download,
  ChevronDown,
  ChevronRight,
  Check,
  Loader2,
  X,
  ChevronLeft,
  ChevronRight as ChevronRightIcon,
  Sparkles,
  Eye,
  Plus,
  Minus,
  ChevronUp,
  Trash2,
  FileCode,
  FileText,
  File,
  ImageIcon,
  ShieldCheck,
  ScanSearch,
  Minimize2,
  Maximize2,
  Settings as SettingsIcon,
  Wrench,
  Upload,
  Copy,
  Languages,
  PencilLine,
  Wand2,
  Undo2,
  Redo2,
  Hash,
  Camera,
  Lightbulb,
  TriangleAlert,
  FolderInput,
  Highlighter,
  /* 第十九轮：简介块图标 + 整页缩放三键 */
  AlignLeft,
  ZoomIn,
  ZoomOut,
  RotateCcw,
  /* 第二十轮：目录行的拖拽把手 */
  GripVertical,
  /* 第二十四轮：目录批量删除入口（缩略图网格已按用户要求整块移除） */
  ListChecks,
  CheckCircle2,
  XCircle,
  AlertTriangle,
  /* 视频步骤自绘控制条：替换原生 controls 的 −/+（原生倍速步进 0.21x 之类太怪，且不可控） */
  Play,
  Pause,
} from "lucide-react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import ExportDialog, { type ExportOptions } from "../components/ExportDialog";
import { AUTOMATION_ENABLED } from "../config/automation";
import AnnotationsOverlay from "../components/AnnotationsOverlay";
import LoginSaveModal from "../components/LoginSaveModal";
import type { Annotation } from "../lib/annotate";
import { type CropAnchor, cropBoxFromAnchorFrac } from "../lib/cropAnchor";
import { previewBox, type PreviewBox } from "../lib/previewBox";
// ★ v0.65.244 对齐：成片预览体检（静态校验纯函数，口径同插件 lib/preview.ts + verify.ts）
import { healthSummaryOf, healthLabel, type HealthIssue } from "../lib/previewHealth";
// ★ T9：团队库的过滤口径与措辞口径单点（纯函数）+ 网络层
import {
  TEAM_PUBLISH_LABEL,
  TEAM_PUBLISH_NONE,
  TEAM_PUBLISH_NONE_LABEL,
  TEAM_PUBLISH_EMPTY_HINT,
  teamWorkspaceLabel,
  composePublishToast,
  type TeamWorkspace,
} from "../lib/teamPublish";
import { listMyWorkspaces, attachTutorialToWorkspace } from "../lib/teamApi";
// ★ r65「在网页编辑」（对齐 Scribe）：URL 形态与措辞的口径单点
import { SITE_ORIGIN, webEditUrl, webSlugKey, WEB_EDIT_BUTTON, WEB_EDIT_HINT, WEB_EDIT_NEED_LOGIN, WEB_EDIT_OPEN_FAILED } from "../lib/webEdit";
import type { Recording, Step } from "../types";
import { useAppStore } from "../store/appStore";
import {
  normalizeInlineHtml,
  sanitizeStoredHtml,
  htmlToPlainText,
  escapeText,
  RICH_TEXT_COLORS,
  RICH_HIGHLIGHTS,
} from "../lib/richText";

/* ── 文档块 / 补充说明类型（2026-09-19 第十三轮）────────────── */

/** 大标题块：不占步骤编号、不计入步骤总数 */
const isHeadingBlock = (s: Step) => (s.block_kind ?? "step") === "heading";

/** 流程摘要块：录制结束时自动生成、固定在正文最前，同样不占编号（2026-09-19 第十六轮） */
const isSummaryBlock = (s: Step) => (s.block_kind ?? "step") === "summary";

/** 简介块：用户在正文里自己写的引子（第十九轮）。同样是文档块 —— 正文存 description */
const isIntroBlock = (s: Step) => (s.block_kind ?? "step") === "intro";

/** 文档块 = 大标题 / 流程摘要 / 简介：都不参与步骤编号，也不算进「N 个步骤」 */
const isDocBlock = (s: Step) => isHeadingBlock(s) || isSummaryBlock(s) || isIntroBlock(s);

/* ── 第二十轮：步骤媒体（图片 / GIF 动图 / 视频）──────────────────── */

/** 这一步是视频吗（mp4 / webm）。录制截图与手传图片都返回 false。 */
const isVideoStep = (s: Step) => (s.media_kind ?? "image") === "video";

/** 视频的播放地址 —— 走 Tauri 的 asset 协议直接读本地文件（base64 那一份是首帧，不是视频）。 */
const stepVideoSrc = (s: Step) =>
  s.screenshot_path ? convertFileSrc(s.screenshot_path) : "";

/** 允许手传的媒体扩展名（真正的闸门在 Rust 侧 `normalize_media_ext`，这里只管别选错文件）。 */
const MEDIA_EXT_RE = /\.(png|jpe?g|webp|gif|mp4|webm)$/i;
/** 单个媒体文件大小上限（MB）。视频为主 —— 再大 base64 走一遍 IPC 会明显卡。 */
const MEDIA_MAX_MB = 64;

/** 从文件名取小写扩展名（无扩展名返回空串）。 */
const extOf = (name: string) => {
  const m = /\.([A-Za-z0-9]+)$/.exec(name);
  return m ? m[1].toLowerCase() : "";
};

/**
 * 摘要正文 → 概述段 + 要点列表。
 * 与 Rust 端 `export::commands::parse_summary` 是**同一套口径**（改一处要同步另一处）：
 * 以 `-`/`•`/`*` 开头的行算要点，要点里第一对 `**…**` 是名字、其后到行尾是说明；
 * 其余非空行拼成概述段。
 */
const parseSummary = (text: string): { intro: string; bullets: Array<{ name: string; detail: string }> } => {
  const intro: string[] = [];
  const bullets: Array<{ name: string; detail: string }> = [];
  for (const raw of (text ?? "").split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) continue;
    const head = line[0];
    if (head === "-" || head === "•" || head === "*") {
      const item = line.slice(1).trim();
      const m = /\*\*(.+?)\*\*/.exec(item);
      if (m && m.index !== undefined) {
        const name = m[1].trim();
        const detail = item.slice(m.index + m[0].length).trim().replace(/^[：:]\s*/, "").trim();
        bullets.push({ name, detail });
      } else {
        bullets.push({ name: "", detail: item });
      }
    } else {
      intro.push(line);
    }
  }
  return { intro: intro.join(""), bullets };
};

/** 步骤「补充说明」的类型：'' 普通描述 / 'mark' 标注 / 'warn' 警告 */
const noteKindOf = (s: Step): "" | "mark" | "warn" =>
  s.note_kind === "mark" || s.note_kind === "warn" ? s.note_kind : "";

/** 三种说明类型：进描述框才浮出的那三个小图标 */
/** 补充说明的类型。第十五轮起用文字 chip（描述 / 标注 / 警告）替掉原来的三个图标方块：
    文字比图标好认，标签常显也省掉了「悬停才敢点」的猜谜。 */
const NOTE_KINDS: Array<{ id: "" | "mark" | "warn"; label: string; tip: string }> = [
  { id: "", label: "描述", tip: "描述：普通补充说明" },
  { id: "mark", label: "标注", tip: "标注：这一步要注意的点" },
  { id: "warn", label: "警告", tip: "警告：做错了会出什么问题" },
];

const NOTE_PLACEHOLDER: Record<"" | "mark" | "warn", string> = {
  "": "补充描述（可留空）",
  mark: "标注：这一步要注意什么",
  warn: "警告：做错了会怎样",
};

/** 大标题层级 1..4（与参考产品的 H1~H4、导出侧 heading_level 一致） */
const HEADING_LEVELS = [1, 2, 3, 4] as const;

/** 每步草稿：就地改过但还没落库的标题/描述/大标题富文本 */
interface StepDraft {
  title: string;
  description: string;
  /** 大标题富文本正文；undefined = 这次没动富文本（落库时 COALESCE 保留旧值） */
  titleHtml?: string;
}

/** 大标题的初始 HTML：有富文本用富文本，否则是转义后的纯文本 */
const headingInitialHtml = (s: Step): string => {
  const rich = sanitizeStoredHtml(s.title_html ?? "");
  if (rich.trim()) return rich;
  return escapeText(s.title ?? "");
};

/* ── Export format options ─────────────────────────────────── */

const EXPORT_FORMATS = [
  { value: "html", label: "HTML", ext: "", icon: FileCode },
  { value: "docx", label: "Word", ext: "", icon: FileText },
  { value: "pdf", label: "PDF", ext: "", icon: File },
  { value: "markdown", label: "Markdown", ext: "md", icon: FileCode },
  { value: "skill", label: "SKILL.md", ext: "md", icon: Wand2 },
  { value: "txt", label: "TXT", ext: "txt", icon: FileText },
  { value: "long", label: "长图", ext: "png", icon: ImageIcon },
] as const;

/** 各导出格式的悬浮说明（说清「导出来能干嘛」，而不是重复格式名） */
const EXPORT_TIPS: Record<string, string> = {
  html: "导出单个 HTML 文件，带目录；双击即可打开，也能直接放到网站上",
  docx: "导出 Word 文档，方便交给同事继续编辑排版",
  pdf: "导出 PDF，适合直接发给别人看或打印",
  markdown: "导出纯 Markdown 文本文件（不含图片），方便贴到笔记/博客或交给 AI 继续编辑",
  skill: "导出 Agent Skills 标准的 SKILL.md（截图一并打包到 screenshots/ 目录），交给 CodeBuddy / Claude 等 AI 助手看图执行；标记了变量的步骤会变成可填参数",
  txt: "导出纯文本，只保留步骤标题和说明，适合快速复制粘贴",
  long: "把所有步骤的截图拼成一张长图，适合发长微博 / 朋友圈或直接查看",
};

/* ── Backend response types ────────────────────────────────── */

interface RecordingWithSteps {
  recording: Recording;
  steps: Step[];
}

/* ── Translation types（与 Rust 端 translations JSON 结构对应）── */

interface TranslatedGuide {
  title: string;
  steps: Record<string, { title: string; description: string }>;
}
/** key = 语言代码（en/ja/ko/es），见 Rust 端 TARGET_LANGS */
type TranslationsMap = Record<string, TranslatedGuide>;

/** 语言代码 → 显示名（与 Rust 端 TARGET_LANGS 保持一致） */
const LANG_LABELS: Record<string, string> = {
  zh: "中文",
  en: "English",
  ja: "日本語",
  ko: "한국어",
  es: "Español",
};

/** 原文快照在 translations 里的专用键（首次翻译前由 Rust 端写入，勿上传网站） */
const ORIG_KEY = "__orig";

/* ── Publish form constants ───────────────────────────────── */

// 分类兜底列表（网站 API 拉取失败时使用）
const FALLBACK_CATEGORIES = [
  { id: "software", name: "软件教程", slug: "software" },
  { id: "office", name: "办公软件", slug: "office" },
  { id: "design", name: "设计工具", slug: "design" },
  { id: "dev", name: "开发工具", slug: "dev" },
  { id: "browser", name: "浏览器与插件", slug: "browser" },
  { id: "system", name: "系统设置", slug: "system" },
  { id: "flowio", name: "录步 Flowio", slug: "flowio" },
  { id: "other", name: "其他", slug: "other" },
];

/* ── 左右两栏宽度（第十八轮） ─────────────────────────────── */

/**
 * 目录栏宽度可拖拽调节。
 *
 * 背景：第十七轮把正文列固定成 900px 居中，结果窗口一宽，正文就只剩中间一条、
 * 两侧全是留白（用户原话「不要在局部变小，要在整体想办法」）。
 * 现在改成**整体分配**：把两栏之间那条缝做成可拖的分隔条 —— 目录栏拖窄，
 * 右栏连同里面的截图就整体跟着变小。
 */
const SIDE_W_KEY = "flowio_editor_sidebar_w";
const SIDE_W_DEFAULT = 320;
const SIDE_W_MIN = 200;
/** 第三十二轮：400 → 272。用户圈着左栏右边界那条分隔条说「向左移动 10%」。
   真机截图实测量化（物理 px，窗口 1375 / scale 1.255 ⇒ 窗口 1096 CSS）：
     分隔条在物理 x=479 ⇒ 左栏占整窗 34.8%（CSS 值 380，与 localStorage 一致）
     「向左移 10%」= 整窗宽度的 10% ⇒ 目标 24.8% ⇒ 380 × 0.713 ≈ 271。
   ⚠️ 第三十轮把 760→400 是空操作：真机存的 380 本来就在 400 以内。
   这次 272 < 380，clampSideWidth 初始化时就会把它收回来，用户不用手动拖。 */
const SIDE_W_MAX = 272;
/** 右栏至少留出这么宽，否则「拖到最左」会把正文挤没 */
const SIDE_W_RESERVE = 420;

const clampSideWidth = (w: number): number => {
  const winW = typeof window === "undefined" ? 1440 : window.innerWidth;
  const upper = Math.max(SIDE_W_MIN, Math.min(SIDE_W_MAX, winW - SIDE_W_RESERVE));
  return Math.round(Math.min(Math.max(w, SIDE_W_MIN), upper));
};

/* ── 整页缩放（第十九轮） ───────────────────────────────────
   目录标题右边那组「－ ＋ ⟲」调的是**整页比例**：目录栏与正文一起缩放，
   像浏览器的 Ctrl+±（字号、内边距、控件都按比例走；顶栏那排工具键在外壳里，不动）。
   实现用 CSS `zoom` 作用在包住两栏的 .editor-zoom 上（不是 webview 缩放）：
   ① 布局会真的重排（transform: scale 不会）；② 无头实测台能量到真实几何。
   ⚠️ zoom 会放大长度值 ⇒ 分隔条拖拽的鼠标位移必须除以 zoom，否则拖 100px 会跑 100*zoom。
   档位收到 0.7~1.5：再往上目录栏会宽到把正文挤掉一块（正文是弹性宽度）。 */
const ZOOM_KEY = "flowio_editor_zoom";
const ZOOM_STEPS = [0.7, 0.8, 0.9, 1, 1.1, 1.25, 1.5];
const ZOOM_DEFAULT = 1;
const ZOOM_MIN = ZOOM_STEPS[0];
const ZOOM_MAX = ZOOM_STEPS[ZOOM_STEPS.length - 1];

/** 收进合法区间，并吸附到最近的档位（历史值/手改 localStorage 都不会跑飞） */
const normalizeZoom = (v: number): number => {
  if (!Number.isFinite(v) || v <= 0) return ZOOM_DEFAULT;
  let best = ZOOM_DEFAULT;
  for (const s of ZOOM_STEPS) {
    if (Math.abs(s - v) < Math.abs(best - v)) best = s;
  }
  return best;
};

/** 从任意当前值算「上一档 / 下一档」（值可能不在档位表里，先夹到区间内） */
const stepZoom = (cur: number, dir: 1 | -1): number => {
  const v = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, cur));
  if (dir > 0) {
    for (const s of ZOOM_STEPS) if (s > v + 1e-6) return s;
    return ZOOM_MAX;
  }
  for (let i = ZOOM_STEPS.length - 1; i >= 0; i -= 1) {
    if (ZOOM_STEPS[i] < v - 1e-6) return ZOOM_STEPS[i];
  }
  return ZOOM_MIN;
};

/* ── 重点图 helper ───────────────────────────────────────── */

/** 默认显示倍率：1 = 图片完整填满截图框（100% 显示），超出画面的边缘被容器裁掉。
    −/+ 以此为基准按 10% 档位缩放；用户主动放大（>1）时才进入「重点图」取景。 */
const SHOT_ZOOM_DEFAULT = 1;

/**
 * 把整图裁成「重点图」—— 也就是前端 CSS `transform: scale(z)` +
 * `transform-origin: 操作落点` 放大后读者真正看见的那块区域。
 * 发布/封面上传走这条；导出由 Rust 端 `focus_shot_box` 等价实现（公式必须一致）。
 *
 * 推导：整图 W×H、倍率 z、落点 (cx, cy)。可见区域宽高 W/z、H/z；
 * 落点在区域内的相对位置与其在整图内一致 ⇒ left = cx·(1 − 1/z)。
 * 倍率 ≤ 1 或落点缺失 → 原样返回整图（不重编码、不丢信息）。
 */
function focusShotBase64(
  base64: string,
  x: number | null,
  y: number | null,
  zoom: number,
): Promise<string> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      try {
        const W = img.naturalWidth;
        const H = img.naturalHeight;
        if (W <= 0 || H <= 0 || !Number.isFinite(zoom) || zoom <= 1) { resolve(base64); return; }
        if (x == null || y == null || x < 0 || y < 0 || x > W || y > H) { resolve(base64); return; }
        const z = Math.min(zoom, 4);
        const cw = Math.max(1, Math.min(W, Math.round(W / z)));
        const ch = Math.max(1, Math.min(H, Math.round(H / z)));
        const k = 1 - 1 / z;
        const left = Math.min(Math.max(0, Math.round(x * k)), W - cw);
        const top = Math.min(Math.max(0, Math.round(y * k)), H - ch);
        const canvas = document.createElement("canvas");
        canvas.width = cw;
        canvas.height = ch;
        const ctx = canvas.getContext("2d");
        if (!ctx) { resolve(base64); return; }
        ctx.drawImage(img, left, top, cw, ch, 0, 0, cw, ch);
        resolve(canvas.toDataURL("image/jpeg", 0.85).split(",")[1] || base64);
      } catch { resolve(base64); }
    };
    img.onerror = () => resolve(base64);
    img.src = `data:image/jpeg;base64,${base64}`;
  });
}

/**
 * ★ 复刻插件「统一改变截图大小」：按锚点中心裁出 frac 比例的取景窗（JPEG base64）。
 * 无锚点（scroll/navigate 整窗截图或已手动裁切）时以图中心兜底 —— 与插件
 * 批量放宽到"所有有截图的步"同口径；裁切失败（载图失败等）返回 null，调用方跳过。
 */
function cropBase64ByAnchorFrac(
  base64: string,
  anchor: { x: number; y: number } | null,
  frac: number,
): Promise<string | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      try {
        const W = img.naturalWidth;
        const H = img.naturalHeight;
        if (W <= 0 || H <= 0) { resolve(null); return; }
        const center = anchor ?? { x: Math.round(W / 2), y: Math.round(H / 2) };
        const r = cropBoxFromAnchorFrac(center, frac, W, H);
        const canvas = document.createElement("canvas");
        canvas.width = r.width;
        canvas.height = r.height;
        const ctx = canvas.getContext("2d");
        if (!ctx) { resolve(null); return; }
        ctx.drawImage(img, r.x, r.y, r.width, r.height, 0, 0, r.width, r.height);
        resolve(canvas.toDataURL("image/jpeg", 0.9).split(",")[1] || null);
      } catch { resolve(null); }
    };
    img.onerror = () => resolve(null);
    img.src = `data:image/jpeg;base64,${base64}`;
  });
}

/** 读取某教程的重点图倍率偏好（localStorage 首屏同步缓存；SQLite 才是权威存储） */
function readInitialZoom(recordingId: string | undefined): Record<string, number> {
  if (!recordingId) return {};
  try {
    const raw = localStorage.getItem(`flowio_view_${recordingId}`);
    if (raw) {
      const z = JSON.parse(raw)?.shotZoom;
      if (z && typeof z === "object" && !Array.isArray(z)) return z as Record<string, number>;
    }
  } catch { /* 坏数据忽略 → 全部走默认 125% */ }
  return {};
}

/* ── Lightbox ──────────────────────────────────────────────── */

interface LightboxProps {
  steps: Step[];
  currentIndex: number;
  onClose: () => void;
  onPrev: () => void;
  onNext: () => void;
  /** 主截图解析：返回该步骤应显示的 base64（裁剪图或全图） */
  getMainSrc: (step: Step) => string | null;
  /** When true the lightbox shows the "marked" (annotated) variant */
  showMarked: boolean;
  onToggleMarked: () => void;
}

function Lightbox({
  steps,
  currentIndex,
  onClose,
  onPrev,
  onNext,
  getMainSrc,
  showMarked,
  onToggleMarked,
}: LightboxProps) {
  const step = steps[currentIndex];
  if (!step) return null;

  // 跟随主截图：该步骤为裁剪图且有缓存时显示裁剪图，否则显示全图
  const srcBase64 = getMainSrc(step);
  if (!srcBase64) return null;

  return (
    <div className="lightbox-overlay" onClick={onClose}>
      <div className="lightbox-content" onClick={(e) => e.stopPropagation()}>
        {/* Top bar: 关闭叉（右上角） */}
        <div className="lightbox-topbar">
          <button className="lightbox-close" onClick={onClose} data-tip="关闭大图（也可按 Esc）">
            <X size={24} />
          </button>
        </div>

        {/* Navigation */}
        {steps.length > 1 && (
          <>
            <button
              className="lightbox-nav lightbox-prev"
              onClick={onPrev}
              disabled={currentIndex === 0}
              data-tip="上一张（←）"
            >
              <ChevronLeft size={28} />
            </button>
            <button
              className="lightbox-nav lightbox-next"
              onClick={onNext}
              disabled={currentIndex === steps.length - 1}
              data-tip="下一张（→）"
            >
              <ChevronRightIcon size={28} />
            </button>
          </>
        )}

        {/* Image */}
        <img
          className="lightbox-image"
          src={`data:image/jpeg;base64,${srcBase64}`}
          alt={`步骤 ${step.step_number}`}
        />

        {/* Footer bar */}
        <div className="lightbox-footer">
          <span>
            {/* 这里收到的是已经滤掉文档块的列表（调用方传 realSteps） */}
            {step.step_number} / {steps.length}
          </span>
        </div>
      </div>
    </div>
  );
}

/* ── Component ─────────────────────────────────────────────── */

interface StepEditorProps {
  siteToken?: string | null;
  siteUser?: { id: string; email?: string; display_name?: string; bio?: string; avatar?: string } | null;
  voiceTranscribing?: boolean;
  /** 「续录」：从这个位置往后接着录（录制归 App 管，编辑器只负责发起） */
  onResumeInto?: (recordingId: string, insertAfter: number) => void;
}

// 把 base64 截图压缩为更小的 JPEG，降低上传体积，避免“保存草稿/发布”因请求体过大而失败
/** 评估截图"信息丰富度"（0~1）。用于智能选封面：空白/纯色加载页得分低，内容丰富的页面得分高。 */
async function scoreScreenshotRichness(b64: string): Promise<number> {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      const w = 24, h = 24;
      const canvas = document.createElement("canvas");
      canvas.width = w; canvas.height = h;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      if (!ctx) { resolve(0.5); return; }
      ctx.drawImage(img, 0, 0, w, h);
      try {
        const d = ctx.getImageData(0, 0, w, h).data;
        const n = w * h;
        let sum = 0, sumSq = 0;
        for (let i = 0; i < n; i++) {
          const l = (d[i*4] + d[i*4+1] + d[i*4+2]) / 3;
          sum += l; sumSq += l * l;
        }
        const mean = sum / n;
        const variance = sumSq / n - mean * mean;
        let edge = 0;
        for (let y = 1; y < h; y++) {
          for (let x = 1; x < w; x++) {
            const i1 = (y*w+x)*4, i2 = (y*w+(x-1))*4, i3 = ((y-1)*w+x)*4;
            const l1 = (d[i1]+d[i1+1]+d[i1+2])/3;
            const l2 = (d[i2]+d[i2+1]+d[i2+2])/3;
            const l3 = (d[i3]+d[i3+1]+d[i3+2])/3;
            edge += Math.abs(l1-l2) + Math.abs(l1-l3);
          }
        }
        resolve(Math.min(1, variance / 3000 + edge / 4000));
      } catch {
        resolve(0.5);
      }
    };
    img.onerror = () => resolve(0.5);
    img.src = b64;
  });
}

/** 发布弹窗预览用：raw base64 → 可直接给 <img src> 的 dataURL（上传 API 两种格式都认，但 <img> 必须带前缀） */
function toPreviewSrc(b64: string): string {
  return b64.startsWith("data:") ? b64 : `data:image/jpeg;base64,${b64}`;
}

async function downscaleBase64Image(dataUrl: string, maxWidth = 1280, quality = 0.82): Promise<string> {
  return new Promise((resolve) => {
    try {
      const img = new Image();
      img.onload = () => {
        const scale = Math.min(1, maxWidth / img.width);
        const w = Math.max(1, Math.round(img.width * scale));
        const h = Math.max(1, Math.round(img.height * scale));
        const canvas = document.createElement("canvas");
        canvas.width = w;
        canvas.height = h;
        const ctx = canvas.getContext("2d");
        if (!ctx) { resolve(dataUrl); return; }
        ctx.drawImage(img, 0, 0, w, h);
        try {
          resolve(canvas.toDataURL("image/jpeg", quality));
        } catch {
          resolve(dataUrl);
        }
      };
      img.onerror = () => resolve(dataUrl);
      img.src = dataUrl;
    } catch {
      resolve(dataUrl);
    }
  });
}

export default function StepEditor({ siteToken, siteUser, voiceTranscribing, onResumeInto }: StepEditorProps) {
  const { id } = useParams<{ id: string }>();
  const recordingId = id!;
  const navigate = useNavigate();

  /* Data state */
  const [recording, setRecording] = useState<Recording | null>(null);
  const [steps, setSteps] = useState<Step[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  /* UI state */
  const [selectedStepId, setSelectedStepId] = useState<string>("");
  /* ── 通篇滚动编辑（2026-09-19）───────────────────────────────
     编辑态右栏把所有步骤排成一条可滚动的文档流，滑到哪改到哪。
     每步一份未落库草稿 drafts：改动 800ms 防抖落库，落库成功后清掉草稿
     （输入框随即回到"以 steps 为准"，值相同所以无感）。
     防抖回调里用 stepsRef 取最新步骤，避免闭包拿到旧数组。 */
  const [drafts, setDrafts] = useState<Record<string, StepDraft>>({});
  const draftTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  /* 每步「提示」展开态（默认收起，别让 AI 说明把通篇流撑满） */
  const [openTips, setOpenTips] = useState<Record<string, boolean>>({});
  /* 左栏颗粒 ＋ 的插入菜单（第十三轮）：步骤 / 大标题 / 导入已有教程 / 续录 */
  const [insertMenu, setInsertMenu] = useState<{ index: number; x: number; y: number } | null>(null);

  /* ── 流程摘要（2026-09-19 第十六轮）─────────────────────────── */
  /** 正在重新生成摘要的那一步（按钮转圈 + 防连点） */
  const [summaryBusy, setSummaryBusy] = useState<string | null>(null);
  /** 哪张摘要卡切到了「原文编辑」态（默认看排版好的卡片，点编辑才变输入框） */
  const [summaryEditId, setSummaryEditId] = useState<string | null>(null);

  /* ── 整页缩放（2026-09-19 第十九轮）───────────────────────────
     用户点的是目录标题旁那组「－ ＋ ⟲」：比例恢复原样 = 回到 100%。 */
  const [pageZoom, setPageZoom] = useState<number>(() => {
    try {
      const raw = localStorage.getItem(ZOOM_KEY);
      return raw == null ? ZOOM_DEFAULT : normalizeZoom(Number(raw));
    } catch {
      return ZOOM_DEFAULT;
    }
  });

  /* ── 大标题富文本 + 目录栏（2026-09-19 第十四轮）────────────── */
  /** 正在编辑哪个大标题（工具条跟着它走）；null = 没在编辑 */
  const [activeHeadingId, setActiveHeadingId] = useState<string | null>(null);  /** 工具条里的取色下拉：null / 'text' 字色 / 'hl' 高亮 */
  const [colorMenu, setColorMenu] = useState<null | "text" | "hl">(null);
  /** 目录栏里折起来的大标题 id（只影响左栏，不动右侧文档流） */
  const [collapsedHeadings, setCollapsedHeadings] = useState<Set<string>>(new Set());

  /* ── 目录批量删除（2026-09-20 第二十三轮加，第二十四轮保留）──────
     缩略图网格形态已按用户要求整块移除（2026-09-20），目录只保留大纲列表。
     批量态：目录步骤项出勾选框（标题/摘要/简介块不参与）+ 目录底部操作条
     「已选 N 步 · 删除 · 取消」，0 选中禁用删除。 */
  const [batchMode, setBatchMode] = useState(false);
  const [batchChecked, setBatchChecked] = useState<Set<string>>(new Set());
  const [batchBusy, setBatchBusy] = useState(false);
  const toggleBatchMode = () => {
    setBatchMode((b) => !b);
    setBatchChecked(new Set());
  };
  const toggleBatchCheck = (id: string) =>
    setBatchChecked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  /** 目录栏拖拽排序：正在拖谁 / 准备插到谁的前后 */
  const [dragStepId, setDragStepId] = useState<string | null>(null);
  const [dropAt, setDropAt] = useState<{ id: string; after: boolean } | null>(null);
  /* 「导入已有教程」选择器 */
  const [importFor, setImportFor] = useState<{ index: number } | null>(null);
  const [importList, setImportList] = useState<
    Array<{ id: string; title: string; total_steps: number; created_at: string }>
  >([]);
  const [importBusy, setImportBusy] = useState(false);
  /* ── 重点图 · 图片显示倍率（2026-09-19 第十一轮，2026-10-06 更新口径）──
     框显示图片的比例：100% = 整图全貌填满框（最小），+ 放大到 175%（最大），
     每档 10%；以操作落点（点击坐标 x/y）为原点缩放，超出卡片的边缘由容器裁掉。
     纯前端展示态，不落库、不影响导出。 */
  /* 重点图倍率（按步）：默认 100%，用户用 −/+ 或圆钮调过之后必须记住 ——
     否则刷新/重进又被默认 100% 吞掉，等于白调。落库见下方持久化 effect。 */
  const [shotZoom, setShotZoom] = useState<Record<string, number>>(
    () => readInitialZoom(recordingId)
  );
  /* 重点图缩放原点模式（按步）：true = 以截图中心为缩放中心（−/+ 放大镜微调）；
     false = 以操作落点（点击坐标）为缩放中心（准星「放大至点击位置」）。
     纯 UI 显示语义，不落库、不改烧录数据；刷新后回落默认的落点模式。 */
  const [shotZoomCenter, setShotZoomCenter] = useState<Record<string, boolean>>({});
  /* 单步「查看全图」开关（按步）：true = 显示 before 全图（100%，scale 1）；
     false = 显示当前主截图（当前图片比例，含倍率与落点缩放）。
     由图片右下角定位圆钮切换（当前比例 ↔ 100%），纯 UI 显示态，不落库。 */
  const [showFullShot, setShowFullShot] = useState<Record<string, boolean>>({});
  const isFullShot = (stepId: string) => !!showFullShot[stepId];
  const exitFullShot = (stepId: string) =>
    setShowFullShot((prev) => (prev[stepId] ? { ...prev, [stepId]: false } : prev));
  /* 单步「顶栏全局比例态」开关（按步）：true = 显示当前主图（真实比例裁剪图，
     如 50%）原样填满框（scale 100/disp，以操作落点为原点）；
     false = 显示整图全貌 / 放大微调态。
     由图片右下角右边按钮切换（100% 整图 ↔ 顶栏全局比例），纯 UI 显示态，
     不落库；−/+ 不进入该区间，从顶栏态起步时先回到 [100,175] 体系。 */
  const [shotAtGlobalPct, setShotAtGlobalPct] = useState<Record<string, boolean>>({});
  /* ── 视频步骤自绘播放器（替换原生 controls）──────────────────
     原生 controls 的 −/+ 是浏览器内部倍速控件，步进诡异（点几下变成 0.21x），
     外观与档位都不可控。自绘控制条后：−/+ 走 VIDEO_RATE_STEPS 档位
     （0.5/0.75/1/1.25/1.5/1.75/2），播放/暂停、进度条、时间、全屏一并接管。 */
  const [videoRate, setVideoRate] = useState<Record<string, number>>({});
  const [videoPlaying, setVideoPlaying] = useState<Record<string, boolean>>({});
  const [videoTime, setVideoTime] = useState<Record<string, number>>({});
  const [videoDur, setVideoDur] = useState<Record<string, number>>({});
  const videoEls = useRef<Record<string, HTMLVideoElement | null>>({});
  const [exportOpen, setExportOpen] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [translateOpen, setTranslateOpen] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [executing, setExecuting] = useState(false);
  const [exportProgress, setExportProgress] = useState<string | null>(null);
  const [redacting, setRedacting] = useState(false);
  // ★ v310: 脱敏结果弹窗（带 ×，不点不消失；与插件端对齐）
  const [redactResult, setRedactResult] = useState<{ title: string; lines: string[] } | null>(null);
  // Stage 3：执行审批闸门——计划含敏感步骤（快捷键/复制/粘贴/拖拽）时弹出计划确认
  const [approvalPlan, setApprovalPlan] = useState<{
    recordingId: string;
    total: number;
    requiresApproval: boolean;
    steps: { stepNumber: number; actionType: string; target: string; sensitive: boolean }[];
  } | null>(null);
  const ACTION_LABELS: Record<string, string> = {
    click: "点击", type: "输入", input: "输入", hotkey: "快捷键", copy: "复制", paste: "粘贴",
    drag: "拖拽", key: "按键", scroll: "滚动", select: "选择", nav: "导航",
    screenshot: "截图", other: "其它",
  };
  const [scanRedacting, setScanRedacting] = useState(false);
  const [scanProgress, setScanProgress] = useState<{ done: number; total: number; currentStepId: string }>({ done: 0, total: 0, currentStepId: "" });
  const [scanToast, setScanToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const scanToastTimerRef = useRef<number | null>(null);
  const [toast, setToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const [voiceBanner, setVoiceBanner] = useState(voiceTranscribing ?? false); // 录制跳编辑界面时，若语音未好则顶部常驻提示
  const [aiGenerating, setAiGenerating] = useState(false);
  const [domainMode, setDomainMode] = useState<"auto" | "spreadsheet" | "browser" | "code" | "terminal">("auto");

  /* ── 编辑器双模式（简洁 / 复杂）─────────────────────────────
     简洁（默认）：纯本地编辑/渲染，不调任何模型、不耗积分，零门槛零成本。
     复杂：开放 AI 增强功能（AI 精简 / AI 生成 / 扫描脱敏 / 翻译 / 摘要重生成），按调用耗积分。
     持久化在 localStorage（与 ZOOM_KEY/SIDE_W_KEY 同套路）；SQLite 落地留待 P1 之后。 */
  const EDITOR_MODE_KEY = "flowio_editor_mode";
  const [editorMode, setEditorMode] = useState<"simple" | "complex">(() => {
    try {
      const raw = localStorage.getItem(EDITOR_MODE_KEY);
      return raw === "complex" ? "complex" : "simple";
    } catch {
      return "simple";
    }
  });
  const isComplex = editorMode === "complex";
  const switchEditorMode = (mode: "simple" | "complex") => {
    if (mode === editorMode) return;
    setEditorMode(mode);
    try { localStorage.setItem(EDITOR_MODE_KEY, mode); } catch { /* 隐私模式等，忽略 */ }
    if (mode === "complex") {
      setToast({ type: "success", message: "已切换到复杂模式：AI 功能会调用模型，按用量消耗积分" });
    }
  };

  /* Translation state */
  const [translations, setTranslations] = useState<TranslationsMap>({});
  const [translating, setTranslating] = useState(false);
  // 当前生效的语言（"orig" | 语言代码 | null），用于下拉高亮
  const [appliedLang, setAppliedLang] = useState<string | null>(null);

  // ★ v0.65.108: appliedLang 持久化——重启桌面端后重进教程时自动恢复上次生效的翻译语言
  const APPLIED_LANG_KEY = (rid: string) => `flowio_applied_lang_${rid}`;
  const saveAppliedLang = (rid: string, lang: string | null) => {
    try {
      if (lang) localStorage.setItem(APPLIED_LANG_KEY(rid), lang);
      else localStorage.removeItem(APPLIED_LANG_KEY(rid));
    } catch { /* storage full */ }
  };
  const loadAppliedLang = (rid: string): string | null => {
    try { return localStorage.getItem(APPLIED_LANG_KEY(rid)); } catch { return null; }
  };

  const loadTranslations = useCallback(async (): Promise<TranslationsMap> => {
    try {
      const t = await invoke<TranslationsMap | null>("get_recording_translations", { recordingId });
      setTranslations(t ?? {});
      return t ?? {};
    } catch {
      return {};
    }
  }, [recordingId]);

  // 未登录 + 录制完成后自动弹出「保存到云端」登录引导弹窗（LoginSaveModal）
  const [showLoginSaveModal, setShowLoginSaveModal] = useState(false);
  useEffect(() => {
    if (siteToken) return;
    if (sessionStorage.getItem("flowio_show_login_prompt") !== "1") return;
    if (sessionStorage.getItem("flowio_login_prompt_dismissed") === "1") return;
    // 延迟弹出，避免与编辑页加载/导航冲突
    const timer = setTimeout(() => setShowLoginSaveModal(true), 400);
    return () => clearTimeout(timer);
  }, [siteToken]);

  const handleLoginSaveModalClose = () => {
    setShowLoginSaveModal(false);
    sessionStorage.setItem("flowio_login_prompt_dismissed", "1");
  };

  const handleLoginSaveModalLogin = () => {
    setShowLoginSaveModal(false);
    sessionStorage.setItem("flowio_login_prompt_dismissed", "1");
    invoke("open_external_url", { url: "https://flowio.cn/author/login?redirect=flowio://auth" }).catch((err) => {
      setToast({ type: "error", message: "打开登录页失败: " + String(err) });
    });
  };

  // Publish state
  const [publishing, setPublishing] = useState(false);
  const [categories, setCategories] = useState<{ id: string; name: string; slug: string }[]>([]);
  const [publishedGuide, setPublishedGuide] = useState<{ id: string; title: string; url: string } | null>(null);

  // 发布弹窗（与浏览器插件同步：先选 发布/存草稿，再进表单）
  const [showPublishDialog, setShowPublishDialog] = useState(false);
  const [pubStep, setPubStep] = useState<"pick" | "form">("pick");

  // 发布表单（与浏览器插件同步）
  const [pubTitle, setPubTitle] = useState("");
  const [pubApplications, setPubApplications] = useState<string[]>([]);
  const [pubAppInput, setPubAppInput] = useState("");
  const [pubCategory, setPubCategory] = useState("");
  // ★ T9：发布到团队库（可选）。
  //   🔴 哪些空间算「可发布」不在这里判 —— 一律用服务端回传的 can_publish。
  const [pubWorkspaceId, setPubWorkspaceId] = useState("");
  const [pubWorkspaces, setPubWorkspaces] = useState<TeamWorkspace[]>([]);
  const [pubTags, setPubTags] = useState<string[]>([]);
  const [pubTagInput, setPubTagInput] = useState("");
  const [pubDownloadable, setPubDownloadable] = useState<"yes" | "no">("yes");
  const [pubCoverBase64, setPubCoverBase64] = useState<string | null>(null);
    const [pubCoverPreview, setPubCoverPreview] = useState<string | null>(null);
    const [smartLoading, setSmartLoading] = useState(false);
    // 场景描述（与浏览器插件同步）
    const [pubSummary, setPubSummary] = useState("");

  // 发布弹窗内联样式
  const pubDialogStyles = {
    label: { display: "block", margin: "10px 0 4px", fontSize: 13, color: "#8b949e" },
    input: { width: "100%", boxSizing: "border-box" as const, padding: "7px 9px", borderRadius: 6, border: "1px solid #30363d", background: "#0d1117", color: "#e6edf3", fontSize: 13, fontFamily: "inherit" },
    addBtn: { padding: "7px 12px", borderRadius: 6, border: "1px solid #30363d", background: "#21262d", color: "#e6edf3", cursor: "pointer", fontSize: 13 },
    seg: { padding: "6px 14px", borderRadius: 6, border: "1px solid #30363d", background: "transparent", color: "#8b949e", cursor: "pointer", fontSize: 13 },
  };

  // 分类列表：优先从网站 API 拉取，失败时用内置兜底
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch("https://flowio.cn/api/categories");
        const data = await res.json();
        if (!cancelled && data.ok && Array.isArray(data.categories) && data.categories.length > 0) {
          setCategories(data.categories);
        } else if (!cancelled) {
          setCategories(FALLBACK_CATEGORIES);
        }
      } catch {
        if (!cancelled) setCategories(FALLBACK_CATEGORIES);
      }
    })();
    return () => { cancelled = true; };
  }, []);

  // Auto-dismiss toast（★ 10s：错误文案长，2s 看不完会误以为功能坏了）
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 10000);
    return () => clearTimeout(timer);
  }, [toast]);

  /* ── 翻译：选择语言后直接翻译，完成后同步到编辑器步骤 ── */
  const handleTranslate = useCallback(
    async (lang: string) => {
      if (steps.length === 0) {
        setToast({ type: "error", message: "没有可翻译的步骤" });
        return;
      }
      setTranslating(true);
      try {
        await invoke("translate_recording", { recordingId, lang });
        // 完成后由 translate://done 事件收尾（同步译文到编辑器）
      } catch (err) {
        setTranslating(false);
        setToast({ type: "error", message: "翻译失败: " + String(err) });
      }
    },
    [recordingId, steps.length]
  );

  /* Preview mode */
  const [previewMode, setPreviewMode] = useState(false);
  /* ★ v0.65.244 对齐：录制停止带 ?preview=1 进「成片预览」只读态（进预览 = 先看一遍成品）。
     退出预览（点「编辑」）时清掉 URL 参数 —— 刷新/分享不会又落回预览态。 */
  const [searchParams, setSearchParams] = useSearchParams();
  const [previewLanding, setPreviewLanding] = useState(() => searchParams.get("preview") === "1");
  const exitPreviewLanding = () => {
    setPreviewMode(false);
    setPreviewLanding(false);
    if (searchParams.has("preview")) {
      const next = new URLSearchParams(searchParams);
      next.delete("preview");
      setSearchParams(next, { replace: true });
    }
  };

  /* ── 左右两栏宽度：拖分隔条调节（第十八轮） ───────────────── */
  const [sideWidth, setSideWidth] = useState<number>(() => {
    try {
      const raw = localStorage.getItem(SIDE_W_KEY);
      const v = raw == null ? NaN : Number(raw);
      return Number.isFinite(v) && v > 0 ? clampSideWidth(v) : clampSideWidth(SIDE_W_DEFAULT);
    } catch {
      return SIDE_W_DEFAULT;
    }
  });
  const [splitDragging, setSplitDragging] = useState(false);
  /** startX/startW 固定拖拽基准，w 记最近一次的值（松手时才落库，免得每移动 1px 写一次）；
   *  zoom 记按下那一刻的整页比例（拖拽位移要换算回未缩放的 CSS 像素） */
  const splitDragRef = useRef<{ startX: number; startW: number; w: number; zoom: number } | null>(null);

  useEffect(() => {
    if (!splitDragging) return;
    const onMove = (e: MouseEvent) => {
      const d = splitDragRef.current;
      if (!d) return;
      // CSS zoom 会放大长度值：鼠标在视口里走 dx，栏宽只能加 dx/zoom，
      // 否则缩放到 125% 时拖 100px 会跑 125px（手感像"越拖越快"）。
      d.w = clampSideWidth(d.startW + (e.clientX - d.startX) / d.zoom);
      setSideWidth(d.w);
    };
    const onUp = () => {
      setSplitDragging(false);
      const d = splitDragRef.current;
      splitDragRef.current = null;
      if (d) {
        try { localStorage.setItem(SIDE_W_KEY, String(d.w)); } catch { /* 隐私模式等，忽略 */ }
      }
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    // 拖动时鼠标可能移出 7px 命中区，全局兜住光标与选区
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
  }, [splitDragging]);

  /* 窗口变窄时把已存的宽度收回到合法区间内（否则正文会被挤没） */
  useEffect(() => {
    const onResize = () => setSideWidth((w) => clampSideWidth(w));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  /* ── 第十八/十九轮：栏宽与整页比例的「恢复原样」────────────────
     栏宽：双击分隔条 → 回到 SIDE_W_DEFAULT（拖动是隐式的，双击是它的复位键）。
     比例：目录标题旁那组键里的 ⟲ → 回到 100%。两者都即刻落库。 */
  const resetSideWidth = useCallback(() => {
    setSideWidth(SIDE_W_DEFAULT);
    try { localStorage.setItem(SIDE_W_KEY, String(SIDE_W_DEFAULT)); } catch { /* 忽略 */ }
  }, []);

  const applyPageZoom = useCallback((z: number) => {
    setPageZoom(z);
  }, []);

  /** 按档位表放大/缩小一档。
   *  🔴 必须走函数式更新：两个 setState 在同一个 tick 里读到的都是**闭包里的旧值**，
   *     连点两次 ＋ 只会走一档（无头实测 PROBE62 抓到过：z1=1.1 而不是 1.25）。 */
  const nudgePageZoom = useCallback((dir: 1 | -1) => {
    setPageZoom((prev) => stepZoom(prev, dir));
  }, []);

  /* 比例落库：状态一变就写（首屏从 localStorage 读回来时写的是同一个值，无副作用） */
  useEffect(() => {
    try { localStorage.setItem(ZOOM_KEY, String(pageZoom)); } catch { /* 隐私模式等，忽略 */ }
  }, [pageZoom]);

  /* ── 步骤精简：自动合并（规则）/ AI 精简（撤销上移为顶栏全局 ↶↷）── */
  const [simplifying, setSimplifying] = useState<null | "rules" | "ai">(null);
  const [simplifyOpen, setSimplifyOpen] = useState(false);
  // 跨页面保留的「AI 精简进行中」标记：离开教程页也不丢，回来仍显示「AI 精简中…」
  const globalAiBusy = useAppStore((s) => s.simplifyingId === recordingId);
  const aiBusy = simplifying === "ai" || globalAiBusy;
  // handleAutoMerge / handleAiSimplify 定义在 reloadRecording
  // 之后（它们的 deps 引用 reloadRecording，放在这里会 TDZ 崩溃）

  /* ── 全局撤销/重做（WPS 式 ↶↷，后端 edit_history 两栈）──
     快照由后端在每个写命令入口自动捕获，前端只负责：
     ① 按钮显隐（edit_history_sizes）② 触发 edit_undo/edit_redo 后刷新。
     histBusy 防连点：undo/redo 涉及全表重写，串行执行。 */
  const [hist, setHist] = useState<{ undo: number; redo: number }>({ undo: 0, redo: 0 });
  const [histBusy, setHistBusy] = useState(false);
  const refreshHist = useCallback(() => {
    invoke<{ undo: number; redo: number }>("edit_history_sizes", { recordingId })
      .then(setHist)
      .catch(() => setHist({ undo: 0, redo: 0 }));
  }, [recordingId]);
  // 自动保存定时器（声明提前：applyHist 里要在撤销前掐掉它，deps 引用不能 TDZ）
  const autoSaveRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  /* ── 把一份译文（或原文快照）同步到编辑器步骤（translate://done 与秒切共用）── */
  const syncStepsFromGuide = useCallback(
    async (trans: TranslatedGuide) => {
      for (const s of steps) {
        const st = trans.steps[String(s.order_index ?? steps.indexOf(s))];
        if (!st) continue;
        const newTitle = st.title ?? s.title;
        const newDesc = st.description ?? s.description;
        if (newTitle === s.title && newDesc === s.description) continue;
        try {
          await invoke("update_step", {
            stepId: s.id,
            title: newTitle,
            description: newDesc,
            actionType: s.action_type,
            tip: s.tip || "",
          });
        } catch {}
        setSteps((prev) =>
          prev.map((p) =>
            p.id === s.id ? { ...p, title: newTitle, description: newDesc } : p
          )
        );
      }
      if (trans.title && recording && trans.title !== recording.title) {
        try { await invoke("update_recording_title", { id: recordingId, title: trans.title }); } catch {}
        setRecording((prev) => prev ? { ...prev, title: trans.title } : prev);
      }
      refreshHist();
    },
    [steps, recording, recordingId, refreshHist] // eslint-disable-line react-hooks/exhaustive-deps
  );

  /* ── 翻译下拉点击：有缓存译文 → 秒切不重译；无缓存 → 走 LLM 翻译 ── */
  const handleLangClick = useCallback(
    async (code: string) => {
      if (code === ORIG_KEY) {
        const orig = translations[ORIG_KEY];
        if (!orig) {
          setToast({ type: "error", message: "该教程没有原文快照（旧版本翻译的），可用顶栏撤销恢复" });
          return;
        }
        setTranslateOpen(false);
        await syncStepsFromGuide(orig);
        setAppliedLang("orig");
        saveAppliedLang(recordingId, "orig");
        setToast({ type: "success", message: "已恢复原文" });
        return;
      }
      const cached = translations[code];
      if (cached) {
        setTranslateOpen(false);
        await syncStepsFromGuide(cached);
        setAppliedLang(code);
        saveAppliedLang(recordingId, code);
        setToast({ type: "success", message: `已切换到${LANG_LABELS[code] || code}译文` });
        return;
      }
      // 无缓存 → 首次翻译（完成后由 translate://done 收尾）
      void handleTranslate(code);
    },
    [translations, syncStepsFromGuide, handleTranslate, recordingId] // eslint-disable-line react-hooks/exhaustive-deps
  );

  /* ── translate://done（翻译完成 → 同步译文到编辑器并记下生效语言）── */
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<{ recordingId: string; lang: string; ok: boolean; summary: string }>(
      "translate://done",
      async (ev) => {
        if (ev.payload.recordingId !== recordingId) return;
        setTranslating(false);
        if (ev.payload.ok) {
          const t = await loadTranslations();
          const trans = t[ev.payload.lang];
          if (!trans) return;
          await syncStepsFromGuide(trans);
          setAppliedLang(ev.payload.lang);
          saveAppliedLang(recordingId, ev.payload.lang);
          setToast({ type: "success", message: `已翻译为${LANG_LABELS[ev.payload.lang] || ev.payload.lang}，可通过「翻译 → 原文」随时恢复` });
        }
      }
    ).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [recordingId, loadTranslations, syncStepsFromGuide]); // eslint-disable-line react-hooks/exhaustive-deps

  /* Delete target for overlay */
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  /* Inline title editing */
  const [recTitleEditing, setRecTitleEditing] = useState(false);
  const [recTitleDraft, setRecTitleDraft] = useState("");
  const recTitleInputRef = useRef<HTMLInputElement>(null);

  /* Lightbox */
  const [lightboxOpen, setLightboxOpen] = useState(false);
  const [lightboxIndex, setLightboxIndex] = useState(0);
  const [lightboxMarked, setLightboxMarked] = useState(false);
  /* ── 重点图：以操作落点为原点的放大 ─────────────────────────
     只改显示倍率（transform: scale），不改布局尺寸 —— 缩放不回流，
     通篇流的滚动位置不会跳；超出卡片的部分由容器 overflow:hidden 裁掉。
     transform-origin 指向操作落点（点击坐标），放大后画面仍然盯着用户
     当时操作的那个部位 —— 这就是「重点图」。
     口径（2026-10-06 用户定稿）：−/+ 改变的是「框显示图片的比例」——
     相对完整整图的比例：100% = 整图全貌填满框（图片视觉最小，− 锁此下限），
     + 放大到 175%（整图放大聚焦，+ 锁此上限），每档 10%，框始终被图片填满。
     真实比例区间（disp<100，如 25/50/75%）不归 −/+ 管，只由右边按钮
     从 100% 切入「顶栏全局比例态」（显示主图裁剪图原样填满框）。 */
  const shotZoomOf = (stepId: string) => shotZoom[stepId] ?? SHOT_ZOOM_DEFAULT;
  const isFocusShot = (stepId: string) => shotZoomOf(stepId) > 1;
  /** 某步「框显示图片的比例」（相对完整整图）：
      −/+ 口径（2026-10-06 用户定稿）：
      100% = 整图全貌填满框（视觉最小，− 锁此下限）；+ 放大显示直到 175%
      （整图放大聚焦，+ 锁此上限），每档 10%。
      100 = 查看全图态（显示 before 整图 scale 1，居中）；
      disp<100 = 顶栏全局比例态（显示主图裁剪图原样填满框，如 50%，
      scale 100/disp，锚点原点）—— 仅由右边按钮从 100% 切入，−/+ 不进入；
      disp>100 = 整图放大（倍率 × 100，如 1.5 → 150%）。 */
  const shotDispOf = (stepId: string): number => {
    if (isFullShot(stepId)) return 100;
    if (shotAtGlobalPct[stepId]) return perShotPcts[stepId] ?? 100;
    const z = shotZoomOf(stepId);
    return Math.min(175, Math.max(100, Math.round(z * 100)));
  };
  /** 设置某步显示：参数 dispPct 是期望的「框显示图片的比例」（相对完整整图）。
      clamp 到 [100, 175]：100 = 整图全貌填满框（视觉最小，− 锁此下限），
      175 = 整图放大 175%（+ 锁此上限）；倍率 = disp / 100。 */
  /** 视觉整图态判定：显示为整图 100%（before 整图 scale 1，填满框）即视为
      「整图 100%」—— 含显式 full 态（isFullShot）与初始默认态（shotZoom 1）。
      右边按钮「100% ↔ 顶栏全局比例」切换以此为准，避免初始默认态
      （isFullShot=false 但显示整图）第一次点击被吞。 */
  const isShotVisFull = (stepId: string) =>
    Math.abs(shotDispOf(stepId) - 100) < 0.001;

  const setShotZoomFor = (stepId: string, dispPct: number) => {
    const disp = Math.min(175, Math.max(100, Math.round(dispPct * 100) / 100));
    const v = Math.round((disp / 100) * 100) / 100;
    setShotZoom((prev) => ({ ...prev, [stepId]: v }));
  };
  /** −/+ 微调：围绕操作落点缩放（transform-origin 取点击坐标 x/y）。
       口径 = 「框显示图片的比例」（相对整图，每档 10%，区间 [100%, 175%]）：
       − 缩小显示直到 100%（整图全貌填满框，视觉最小，锁 100）；
       + 放大显示直到 175%（整图放大聚焦，最大，锁 175）。
       顶栏全局比例态（disp<100）起步时：−/+ 不进入该区间，
       − 直接缩到 100% 整图全貌，+ 从 100% 放大一档（110%）。 */
  const nudgeShotZoom = (stepId: string, dir: -1 | 1) => {
    if (shotAtGlobalPct[stepId]) {
      setShotAtGlobalPct((prev) =>
        prev[stepId] ? { ...prev, [stepId]: false } : prev,
      );
      if (dir === -1) {
        // 顶栏全局比例态点 −：缩到 100% 整图全貌（锁 100）
        setShowFullShot((prev) => ({ ...prev, [stepId]: true }));
        setShotZoom((prev) => ({ ...prev, [stepId]: 1 }));
        setShotZoomCenter((prev) => ({ ...prev, [stepId]: false }));
      } else {
        // 顶栏全局比例态点 +：从 100% 起步放大一档（110%）
        exitFullShot(stepId);
        setShotZoomCenter((prev) => ({ ...prev, [stepId]: false }));
        setShotZoomFor(stepId, 110);
      }
      return;
    }
    const cur = isFullShot(stepId)
      ? 100
      : Math.min(175, Math.max(100, Math.round(shotZoomOf(stepId) * 100)));
    const next = Math.min(175, Math.max(100, cur + dir * 10));
    if (Math.abs(next - cur) < 0.001) return; // 已在边界，−/+ 无可进
    if (next <= 100.001) {
      // 到 100% = 整图全貌：走「查看全图」态（before 整图 scale 1，居中）
      setShowFullShot((prev) => ({ ...prev, [stepId]: true }));
      setShotZoom((prev) => ({ ...prev, [stepId]: 1 }));
      setShotZoomCenter((prev) => ({ ...prev, [stepId]: false }));
      return;
    }
    exitFullShot(stepId);
    setShotZoomCenter((prev) => ({ ...prev, [stepId]: false }));
    setShotZoomFor(stepId, next);
  };

  /* ── 视频倍速与播放控制（自绘播放器用）────────────────── */
  const VIDEO_RATE_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2] as const;
  const videoRateOf = (stepId: string) => videoRate[stepId] ?? 1;
  const setVideoRateFor = (stepId: string, r: number) => {
    const v = Math.min(2, Math.max(0.5, Math.round(r * 100) / 100));
    setVideoRate((prev) => ({ ...prev, [stepId]: v }));
    const el = videoEls.current[stepId];
    if (el) el.playbackRate = v;
  };
  /** 倍速 −/+：按 VIDEO_RATE_STEPS 档位步进（与图片 −/+ 档位口径一致，每档 25%）。 */
  const nudgeVideoRate = (stepId: string, dir: -1 | 1) => {
    const cur = videoRateOf(stepId);
    let next = cur;
    if (dir === -1) {
      for (let i = VIDEO_RATE_STEPS.length - 1; i >= 0; i--) {
        if (VIDEO_RATE_STEPS[i] < cur - 0.001) { next = VIDEO_RATE_STEPS[i]; break; }
      }
    } else {
      for (let i = 0; i < VIDEO_RATE_STEPS.length; i++) {
        if (VIDEO_RATE_STEPS[i] > cur + 0.001) { next = VIDEO_RATE_STEPS[i]; break; }
      }
    }
    if (Math.abs(next - cur) < 0.001) return; // 已在边界档
    setVideoRateFor(stepId, next);
  };
  const toggleVideoPlay = (stepId: string) => {
    const el = videoEls.current[stepId];
    if (!el) return;
    if (el.paused) { void el.play().catch(() => { /* 用户未交互前 autoplay 限制 */ }); }
    else el.pause();
  };
  const seekVideoTo = (stepId: string, t: number) => {
    const el = videoEls.current[stepId];
    if (!el) return;
    el.currentTime = t;
    setVideoTime((prev) => ({ ...prev, [stepId]: t }));
  };
  const fmtTime = (t: number) => {
    if (!Number.isFinite(t) || t < 0) return "00:00";
    const m = Math.floor(t / 60);
    const s = Math.floor(t % 60);
    return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  };

  /* 每步截图的原始像素尺寸：算 transform-origin 的百分比要用它
     （origin 取的是元素自身盒子的比例，而图是 width:100% 缩放显示的）。 */
  const [shotNat, setShotNat] = useState<Record<string, { w: number; h: number }>>({});
  const rememberShotNat = (stepId: string, el: HTMLImageElement | null) => {
    if (!el || !el.naturalWidth || !el.naturalHeight) return;
    const w = el.naturalWidth;
    const h = el.naturalHeight;
    setShotNat((prev) => {
      const cur = prev[stepId];
      return cur && cur.w === w && cur.h === h ? prev : { ...prev, [stepId]: { w, h } };
    });
  };
  /** 缩放原点：中心模式下固定 50% 50%（放大镜 −/+ 微调）；落点模式下取操作落点
      （点击坐标）在当前显示图里的位置；坐标缺失或越界退回居中。
      主图是裁剪图（真实比例 <100%）时：落点坐标在 before 整图坐标系里，
      当前图 = before 按 frac 以落点为中心裁出（cropBoxFromAnchorFrac 贴边夹回），
      必须用同一几何反算落点在图内的相对位置，缩放中心才能咬住操作点 ——
      旧实现直接拿 before 坐标 ÷ 裁剪后尺寸，百分比全偏（还经常因 x>nat.w 退回居中）。 */
  const shotOriginOf = (step: Step): string => {
    if (shotZoomCenter[step.id]) return "50% 50%";
    const nat = shotNat[step.id];
    if (!nat || step.x == null || step.y == null) return "50% 50%";
    if (step.x < 0 || step.y < 0) return "50% 50%";
    const fracPct = perShotPcts[step.id];
    if (fracPct != null && fracPct > 0 && fracPct < 100) {
      const frac = fracPct / 100;
      const beforeW = nat.w / frac;
      const beforeH = nat.h / frac;
      const rect = cropBoxFromAnchorFrac({ x: step.x, y: step.y }, frac, beforeW, beforeH);
      const ox = Math.min(100, Math.max(0, ((step.x - rect.x) / rect.width) * 100));
      const oy = Math.min(100, Math.max(0, ((step.y - rect.y) / rect.height) * 100));
      return `${ox.toFixed(2)}% ${oy.toFixed(2)}%`;
    }
    if (step.x > nat.w || step.y > nat.h) return "50% 50%";
    return `${((step.x / nat.w) * 100).toFixed(2)}% ${((step.y / nat.h) * 100).toFixed(2)}%`;
  };

  /** 中间态（显示 before 整图放大到 disp 区域）的缩放原点：落点在整图内的相对位置。
      中间态 <img> 的 naturalWidth/Height 已是 before 尺寸（onLoad 后）；
      首次切换的一帧 nat 仍是裁剪图尺寸，落点越界会退回居中，onLoad 后即修正。 */
  const shotBeforeOriginOf = (step: Step): string => {
    if (shotZoomCenter[step.id]) return "50% 50%";
    const nat = shotNat[step.id];
    if (!nat || step.x == null || step.y == null) return "50% 50%";
    if (step.x < 0 || step.y < 0 || step.x > nat.w || step.y > nat.h) return "50% 50%";
    return `${((step.x / nat.w) * 100).toFixed(2)}% ${((step.y / nat.h) * 100).toFixed(2)}%`;
  };

  /* Tip expand */
  const [tipExpanded, setTipExpanded] = useState(false);

  const detailImgRef = useRef<HTMLImageElement>(null);

  /* 主图容器几何：state 驱动，禁止直接写 parent.style。
     旧实现 onLoad 里直接改 parent 的 inline 比例/maxWidth —— 这些值会残留：
     切步骤 / 全图↔裁图切换后，若新图的 load 事件没触发处理（React 对内存缓存图
     存在丢 onLoad 的已知问题），容器比例还是上一张图的 → object-fit: contain
     在底部/两侧留下灰色占位区（用户实测截图：容器是旧裁图的 1.66:1，全图 2.2:1）。
     现在：src 一变立即重置（回落 CSS 默认比例），新图 onLoad 或 effect 兜底写入
     新比例，几何永远与当前显示的图同步。memo 缓存已算过的比例，来回切换不闪灰。 */
  const [detailImgBox, setDetailImgBox] = useState<{ src: string; box: PreviewBox | null }>({
    src: "",
    box: null,
  });
  const detailBoxMemo = useRef<Map<string, PreviewBox>>(new Map());

  /* 兜底：图片已加载完成（complete）但 box 还没写入 → 直接按 naturalWidth 补一次。
     覆盖「内存缓存图丢 onLoad」的路径，保证容器比例必然收敛到当前显示的图。 */
  useEffect(() => {
    if (detailImgBox.box) return;
    const img = detailImgRef.current;
    if (!img || !img.complete || !img.naturalWidth) return;
    const box = previewBox(img.naturalWidth, img.naturalHeight);
    if (!box) return;
    detailBoxMemo.current.set(detailImgBox.src, box);
    setDetailImgBox((prev) => (prev.box ? prev : { ...prev, box }));
  }, [detailImgBox]);

  // 首屏视图恢复完成后才允许持久化，避免挂载瞬间用默认值 {} / 'full' 覆盖已保存的偏好
  const viewLoadedRef = useRef(false);

  /* Annotation (手动标注编辑器) */
  const [annotateOpen, setAnnotateOpen] = useState(false);
  const [annotateStepId, setAnnotateStepId] = useState<string>("");

  // ⚠️ 这里**不要**再给主预览容器写死高度。
  // v24 之前的遗留代码曾写 `height = clamp(w*9/16, 200, 520)`，把盒子锁成 16:9，
  // inline height 会盖掉 CSS 的 aspect-ratio —— v24 修好的宽度收窄因此白做，
  // 容器高度仍按 16:9 算，非 16:9 的图（16:10 / 4:3 / 带鱼屏 / 裁图后的极端比例）
  // 照样出 letterbox。实测 40 种组合里 31 例有留白，上下最大 312px。
  // 现在高度完全交给 CSS：width:100% + max-width + aspect-ratio + max-height。

  /* ── Load / reload recording ────────────────────────────── */

  const reloadRecording = useCallback(async () => {
    const data = await invoke<RecordingWithSteps>("load_recording", {
      id: recordingId,
    });
    setRecording(data.recording);
    setSteps(data.steps);
    if (data.steps.length > 0 && !data.steps.find((s) => s.id === selectedStepId)) {
      setSelectedStepId(data.steps[0].id);
    }
    return data.steps;
  }, [recordingId, selectedStepId]);

  /** 将某张步骤截图设为封面（预览用原图，上传用压缩图） */
  const selectStepCover = async (s: { id: string; screenshot_base64?: string | null }) => {
    if (!s.screenshot_base64) return;
    // ⚠️ downscaleBase64Image 只认 dataURL：raw base64 传入会 onerror 原样返回（压缩静默失效），必须先补前缀
    const small = await downscaleBase64Image(toPreviewSrc(s.screenshot_base64), 720, 0.85);
    setPubCoverBase64(small);
    setPubCoverPreview(small);
  };

  /** 智能选封面：排除空白/纯色截图，优先中段步骤，挑信息量最丰富的一张 */
  const handleSmartCover = async () => {
    const cand = steps.filter((s) => s.screenshot_base64);
    if (cand.length === 0) return;
    setSmartLoading(true);
    try {
      let best: typeof cand[number] | null = null;
      let bestScore = -1;
      await Promise.all(
        cand.map(async (s, i) => {
          const richness = await scoreScreenshotRichness(s.screenshot_base64 as string);
          const midness = 1 - Math.abs(i - (cand.length - 1) / 2) / (cand.length / 2 + 1);
          const score = richness * 0.7 + midness * 0.3;
          if (score > bestScore) { bestScore = score; best = s; }
        }),
      );
      if (best) await selectStepCover(best);
    } finally {
      setSmartLoading(false);
    }
  };

  /* ── voice://done（语音转写完成 → 刷新步骤显示【语音】内容，PRD V8）── */
  // 语音转写进行中提示条：外部状态变为 false（例如全局 voice://done 已置位）时收起
  useEffect(() => {
    if (!voiceTranscribing) setVoiceBanner(false);
  }, [voiceTranscribing]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<{
      recordingId: string;
      transcribed: number;
      failed: number;
      ok: boolean;
      summary: string;
    }>("voice://done", (ev) => {
      if (ev.payload.recordingId !== recordingId) return;
      setVoiceBanner(false); // 无论成功失败都收起「生成中」提示条
      if (ev.payload.ok && ev.payload.transcribed > 0) {
        reloadRecording().catch(() => {}); // 刷新步骤显示【语音】内容
        setToast({ type: "success", message: `语音转写完成，已为 ${ev.payload.transcribed} 个步骤补全语音说明` });
      } else if (ev.payload.ok) {
        setToast({ type: "success", message: "语音转写完成（未识别到有效语音）" });
      } else {
        setToast({ type: "error", message: "语音转写失败：" + (ev.payload.summary || "未知错误") });
      }
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [recordingId, reloadRecording]);

  /* ── ★ 步骤自动精简（规则层 / AI 层 / 撤销），定义在 reloadRecording 之后避免 deps TDZ ── */
  const handleAutoMerge = useCallback(async () => {
    if (!recordingId) return;
    setSimplifying("rules");
    try {
      const r = await invoke<{ before: number; after: number; reduced: number }>(
        "auto_merge_steps",
        { recordingId }
      );
      await reloadRecording();
      refreshHist();
      setToast({
        type: "success",
        message:
          r.reduced > 0
            ? `已自动合并：${r.before} 步 → ${r.after} 步（省 ${r.reduced} 步）`
            : "没有检测到可合并的冗余步骤",
      });
    } catch (err) {
      setToast({ type: "error", message: "自动合并失败: " + String(err) });
    } finally {
      setSimplifying(null);
    }
  }, [recordingId, reloadRecording, refreshHist]);

  const handleAiSimplify = useCallback(async () => {
    if (!recordingId) return;
    setSimplifying("ai");
    // 跨页面保留「进行中」标记：离开教程页也不丢，回来仍显示「AI 精简中…」
    useAppStore.getState().setSimplifyingId(recordingId);
    // 后台执行：只发起，不阻塞。完成后由全局 flowio://job-done 事件收尾
    //（刷新步骤 + 右上角弹窗提醒），软件不退出就一直跑到完。
    invoke("simplify_steps_ai", { recordingId })
      .then(() => {})
      .catch((err) => {
        setSimplifying(null);
        useAppStore.getState().setSimplifyingId(null);
        setToast({ type: "error", message: "AI 精简启动失败: " + String(err) });
      });
  }, [recordingId, setToast]);

  /* ── flowio://job-done（AI 精简完成 → 刷新步骤；离开教程页也照常收尾）── */
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<{ recording_id: string; op: string; ok: boolean; summary: string }>(
      "flowio://job-done",
      async (ev) => {
        if (ev.payload.recording_id !== recordingId) return;
        /* 流程摘要（AI 润色完成）→ 重载一次把新文案显示出来；失败就静默留着规则版 */
        if (ev.payload.op === "ai_summary") {
          if (ev.payload.ok) {
            await reloadRecording();
            refreshHist();
          }
          return;
        }
        if (ev.payload.op !== "ai_simplify") return;
        setSimplifying(null);
        useAppStore.getState().setSimplifyingId(null);
        if (!ev.payload.ok) {
          // 精简失败（多在免费通道限流时）：编辑页内直接弹红条，避免只靠右上角 9s 卡片被忽略
          setToast({ type: "error", message: "AI 精简失败：" + ev.payload.summary });
          return;
        }
        if (ev.payload.ok) {
          await reloadRecording();
          refreshHist();
        }
      }
    ).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; unlisten?.(); };
  }, [recordingId, reloadRecording, refreshHist]);

  /* ── ★ 全局撤销/重做 ───────────────────────────────────── */
  const applyHist = useCallback(async (cmd: "edit_undo" | "edit_redo") => {
    if (histBusy) return;
    setHistBusy(true);
    // 撤销/重做会改库里的 title/description：先掐掉待触发的自动保存，
    // 否则 800ms 后它会把刚撤销的内容原样写回去
    if (autoSaveRef.current) { clearTimeout(autoSaveRef.current); autoSaveRef.current = null; }
    try {
      const n = await invoke<number>(cmd, { recordingId });
      await reloadRecording();
      setToast({
        type: "success",
        message: cmd === "edit_undo" ? `已撤销，恢复到 ${n} 个步骤` : `已恢复，${n} 个步骤`,
      });
    } catch (err) {
      setToast({ type: "error", message: String(err) });
    } finally {
      setHistBusy(false);
      refreshHist();
    }
  }, [histBusy, recordingId, reloadRecording, refreshHist]);

  /* Ctrl+Z / Ctrl+Y（Ctrl+Shift+Z = 重做）；输入框内让位给原生文本撤销 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      const t = e.target as HTMLElement | null;
      const tag = t?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || t?.isContentEditable) return;
      const k = e.key.toLowerCase();
      if (k === "z" && !e.shiftKey) { e.preventDefault(); void applyHist("edit_undo"); }
      else if (k === "y" || (k === "z" && e.shiftKey)) { e.preventDefault(); void applyHist("edit_redo"); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [applyHist]);

  /* ── 查找 / 替换（Ctrl+F / Ctrl+H）────────────────────────────
   * 无按钮纯快捷键入口；浮条支持 Enter/Shift+Enter 导航、Esc 关闭。
   * 全部替换走后端 replace_in_steps：整段只压一次撤销快照，Ctrl+Z 一次还原。 */
  const [findOpen, setFindOpen] = useState(false);
  const [replaceMode, setReplaceMode] = useState(false); // Ctrl+H 打开时显示替换行
  const [findText, setFindText] = useState("");
  const [committedQuery, setCommittedQuery] = useState(""); // 回车提交后才真正搜索（打字过程不触发）
  const [replaceText, setReplaceText] = useState("");
  const [matchIdx, setMatchIdx] = useState(0);
  const [replacing, setReplacing] = useState(false);
  const findInputRef = useRef<HTMLInputElement>(null);
  const titleInputRef = useRef<HTMLInputElement>(null);
  const descInputRef = useRef<HTMLTextAreaElement>(null);

  /** 查找命中高亮：把 text 中所有大小写不敏感命中片段包上 <mark>（侧栏标题等纯文本处用） */
  const renderFindHighlight = (text: string, q: string): React.ReactNode => {
    if (!q) return text;
    const lc = text.toLowerCase();
    const qlc = q.toLowerCase();
    if (!lc.includes(qlc)) return text;
    const parts: React.ReactNode[] = [];
    let i = 0;
    let k = 0;
    for (;;) {
      const idx = lc.indexOf(qlc, i);
      if (idx === -1) { parts.push(text.slice(i)); break; }
      if (idx > i) parts.push(text.slice(i, idx));
      parts.push(<mark key={k++} className="find-hit">{text.slice(idx, idx + q.length)}</mark>);
      i = idx + q.length;
    }
    return parts;
  };

  /** 大小写不敏感整串替换（与 Rust replace_ci 同语义；回调进 handleReplaceAll 的本地同步） */
  const replaceAllCI = (hay: string, search: string, rep: string): string => {
    if (!search) return hay;
    const hayLc = hay.toLowerCase();
    const qLc = search.toLowerCase();
    if (hayLc.length !== hay.length || qLc.length !== search.length) return hay.split(search).join(rep);
    let out = "";
    let i = 0;
    for (;;) {
      const idx = hayLc.indexOf(qLc, i);
      if (idx === -1) { out += hay.slice(i); break; }
      out += hay.slice(i, idx) + rep;
      i = idx + search.length;
    }
    return out;
  };

  // 命中清单：每个（步骤 × 字段）一条；导航即选中该步骤并滚到侧栏可见
  const findMatches = useMemo(() => {
    const q = committedQuery.trim().toLowerCase();
    if (!findOpen || !q) return [];
    const out: { stepId: string; field: "title" | "description" }[] = [];
    steps.forEach((s) => {
      if ((s.title || "").toLowerCase().includes(q)) out.push({ stepId: s.id, field: "title" });
      if ((s.description || "").toLowerCase().includes(q)) out.push({ stepId: s.id, field: "description" });
    });
    return out;
  }, [findOpen, committedQuery, steps]);

  // 提交的查询词变化 → 回到第一个命中
  useEffect(() => { setMatchIdx(0); }, [committedQuery]);

  // 命中导航 → 选中步骤 + 侧栏滚动可见 + 标题/描述输入框内选中命中片段（原生选中高亮，一眼定位）
  useEffect(() => {
    if (!findOpen || findMatches.length === 0) return;
    const m = findMatches[Math.min(matchIdx, findMatches.length - 1)];
    if (!m) return;
    setSelectedStepId(m.stepId);
    const t = setTimeout(() => {
      document.getElementById(`step-item-${m.stepId}`)?.scrollIntoView({ behavior: "smooth", block: "nearest" });
      const q = committedQuery.trim();
      if (!q) return;
      const st = steps.find((s) => s.id === m.stepId);
      const val = (m.field === "title" ? st?.title : st?.description) || "";
      const idx = val.toLowerCase().indexOf(q.toLowerCase());
      const target = m.field === "title" ? titleInputRef.current : descInputRef.current;
      if (target && idx !== -1) {
        // 只设置选中片段、不抢焦点：焦点留在查找框，Enter/Shift+Enter 才能继续上下切换
        target.setSelectionRange(idx, idx + q.length);
      }
    }, 60);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [matchIdx, findOpen, findMatches]);

  const openFindBar = useCallback((withReplace: boolean) => {
    // 预填当前选中文本（若有）并立即按它搜索，像 VSCode 一样省一次输入
    const sel = (window.getSelection()?.toString() || "").trim();
    if (sel) {
      const t = sel.slice(0, 80);
      setFindText(t);
      setCommittedQuery(t);
    }
    setReplaceMode(withReplace);
    setFindOpen(true);
    setMatchIdx(0);
    setTimeout(() => findInputRef.current?.select(), 30);
  }, []);

  // Ctrl+F / Ctrl+H：全局接管（含焦点在输入框时；原生编辑器里这两键无语义）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
      const k = e.key.toLowerCase();
      if (k === "f") { e.preventDefault(); openFindBar(false); }
      else if (k === "h") { e.preventDefault(); openFindBar(true); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openFindBar]);

  const handleReplaceAll = useCallback(async () => {
    const q = findText;
    if (!q.trim()) { setToast({ type: "error", message: "请先输入查找内容" }); return; }
    if (replacing) return;
    setReplacing(true);
    try {
      const r = await invoke<{ changedSteps: number; replaced: number }>("replace_in_steps", {
        recordingId,
        search: q,
        replace: replaceText,
      });
      if (r.replaced === 0) {
        setToast({ type: "error", message: "没有找到可替换的内容" });
      } else {
        // 本地同步文本（不整页重载，避免截图 base64 重新拉取）
        setSteps((prev) => prev.map((s) => ({
          ...s,
          title: replaceAllCI(s.title || "", q, replaceText),
          description: replaceAllCI(s.description || "", q, replaceText),
        })));
        setToast({ type: "success", message: `已替换 ${r.replaced} 处（${r.changedSteps} 个步骤），Ctrl+Z 可整体撤销` });
        refreshHist();
      }
    } catch (err) {
      setToast({ type: "error", message: String(err) });
    } finally {
      setReplacing(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recordingId, findText, replaceText, replacing, refreshHist]);

  const findNavBtnStyle: CSSProperties = {
    width: 26, height: 26, display: "flex", alignItems: "center", justifyContent: "center",
    background: "transparent", border: "1px solid transparent", borderRadius: 6,
    color: "#57606a", fontSize: 14, cursor: "pointer", padding: 0, flexShrink: 0,
  };

  useEffect(() => {
    let cancelled = false;
    // 换教程（含 另存为 → /editor/newId 的编辑器内直跳）时先锁持久化，
    // 防止上一份教程的视图状态被写入新 key（viewLoadedRef 跨 loading 保持 true）。
    viewLoadedRef.current = false;
    (async () => {
      try {
        // 视图偏好与录制数据并行加载：SQLite 是权威存储（editor_view_get），
        // localStorage 只作回退（老教程首次打开时迁移入库）。
        const [data, dbView] = await Promise.all([
          invoke<RecordingWithSteps>("load_recording", { id: recordingId }),
          invoke<string | null>("editor_view_get", { recordingId }).catch(() => null as string | null),
        ]);
        if (cancelled) return;

        // ── 视图偏好恢复（在 setSteps 之前，首屏即最终视图，无闪跳）──
        // SQLite 与 localStorage 各存一份带 ts 的 JSON；恢复取 ts 较新者。
        // 背景：曾出现 WebView 存储丢失（→ SQLite 权威），又出现 DB 写入偶发失败
        // （fire-and-forget 静默）→ 若无条件 DB 优先，用户刚调好的倍率会被旧值覆盖
        // （"退出再进变回去"）。带 ts 对比后，无论哪端滞后/失败都能取到最新。
        let dbParsed: { shotZoom?: unknown; ts?: number } | null = null;
        if (dbView) {
          try { dbParsed = JSON.parse(dbView); } catch { dbParsed = null; }
        }
        let lsParsed: { shotZoom?: unknown; ts?: number } | null = null;
        let lsRaw: string | null = null;
        try { lsRaw = localStorage.getItem(`flowio_view_${recordingId}`); } catch { /* ignore */ }
        if (lsRaw) {
          try { lsParsed = JSON.parse(lsRaw); } catch { lsParsed = null; }
        }
        const useDb = !lsParsed || (dbParsed != null && (dbParsed.ts ?? 0) >= (lsParsed.ts ?? 0));
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const chosen: any = useDb ? dbParsed : lsParsed;
        if (chosen) {
          const z = chosen.shotZoom;
          setShotZoom(z && typeof z === "object" && !Array.isArray(z) ? z : {});
          // 选了 localStorage（DB 缺失或更旧）→ 把较新的值迁回 DB，保证下次 DB 权威
          if (!useDb && lsRaw) {
            void invoke("editor_view_set", { recordingId, value: lsRaw }).catch(() => { /* best-effort */ });
          }
        } else {
          setShotZoom({});
        }
        viewLoadedRef.current = true;

        if (cancelled) return;

        setRecording(data.recording);
        setSteps(data.steps);
        if (data.steps.length > 0) {
          setSelectedStepId(data.steps[0].id);
        }

        // 视图状态已在上方从 SQLite/localStorage 恢复并标记 viewLoadedRef，
        // 此处无需再处理。

        // 译文副本（翻译功能 T1）：并行加载，失败不阻塞编辑器
        // ★ v0.65.108: 加载后自动恢复上次生效的翻译语言
        loadTranslations().then((tmap) => {
          if (cancelled) return;
          const savedLang = loadAppliedLang(recordingId);
          if (!savedLang || !tmap) return;
          if (savedLang === ORIG_KEY && tmap[ORIG_KEY]) {
            setAppliedLang("orig");
          } else if (tmap[savedLang]) {
            const trans = tmap[savedLang];
            // 异步自动应用译文（不阻塞 UI 渲染）
            syncStepsFromGuide(trans).then(() => {
              if (!cancelled) setAppliedLang(savedLang);
            }).catch(() => {});
          }
        });

        // 撤销/重做栈深度（顶栏 ↶↷ 按钮显隐）
        invoke<{ undo: number; redo: number }>("edit_history_sizes", { recordingId })
          .then((v) => { if (!cancelled) setHist(v); })
          .catch(() => { if (!cancelled) setHist({ undo: 0, redo: 0 }); });
      } catch (err) {
        if (!cancelled) setError(String(err));
      } finally {
        if (!cancelled) {
          setLoading(false);
          // ★ v0.65.244：带 ?preview=1 进来 = 录制刚停，直接落在「成片预览」只读态
          if (searchParams.get("preview") === "1") setPreviewMode(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [recordingId]);

  /* ── Account ownership: 登录后把当前教程精确归属到登录账号 ── */

  useEffect(() => {
    // 仅在已登录时执行：先同步隔离标记，再精确归属当前教程（仅当记录仍属 __local__）
    if (!siteUser?.id || !recordingId) return;
    let cancelled = false;
    (async () => {
      try {
        await invoke("set_current_user", { userId: siteUser.id });
        if (cancelled) return;
        await invoke("claim_recording", { recordingId, userId: siteUser.id });
      } catch (err) {
        console.warn("[flowio] 归属当前教程到账号失败:", err);
      }
    })();
    return () => { cancelled = true; };
  }, [siteUser?.id, recordingId]);

  /* ── Derived ─────────────────────────────────────────────── */

  const selectedStep = steps.find((s) => s.id === selectedStepId);

  /* 持久化重点图倍率：localStorage（首屏同步缓存）+ SQLite（权威存储，
     WebView 存储曾出现丢失 → 退出重进又变回默认值；DB 写入 fire-and-forget，失败不阻塞 UI） */
  useEffect(() => {
    if (!recordingId) return;
    // 首屏恢复前不写，防止挂载瞬间用默认值覆盖已保存偏好
    if (!viewLoadedRef.current) return;
    // 带 ts 写入：恢复端用 ts 对比 DB/localStorage 取最新，DB 偶发写入失败
    // 时也不会被旧值覆盖（问题1：退出编辑再进变回去）。
    const json = JSON.stringify({ shotZoom, ts: Date.now() });
    try {
      localStorage.setItem(`flowio_view_${recordingId}`, json);
    } catch { /* storage full / unavailable */ }
    // DB 写入 debounce 300ms：连点 −/+ 只落最后一次，减少 SQLite 并发争用；
    // 卸载（退出编辑）时清理定时器 —— localStorage 已同步写入，恢复端仍能取到。
    const t = window.setTimeout(() => {
      void invoke("editor_view_set", { recordingId, value: json }).catch((e) => {
        console.warn("[flowio] editor_view_set 写入失败:", e);
      });
    }, 300);
    return () => window.clearTimeout(t);
  }, [recordingId, shotZoom]);

  const selectedIndex = steps.findIndex((s) => s.id === selectedStepId);

  /* 主截图：就是步骤截图本体。「重点图」是显示层的倍率（CSS transform），
     不改动数据 —— 预览/导出在渲染时才按倍率裁出可见区域。 */
  const getStepMainSrc = (step: Step): string | null => step.screenshot_base64 || null;
  /** Lightbox「查看图片」统一显示全图（before 原图），对**所有步骤**生效；
      无 before 图时退回主截图兜底。 */
  const lightboxSrcOf = (step: Step): string | null =>
    step.before_screenshot_base64 || step.screenshot_base64 || null;

  /* 兜底：img 的 onLoad 在内存缓存图上偶尔不触发（React 已知问题），
     这里直接 new Image() 预载一次，保证每步都能拿到原图像素尺寸，
     重点图的缩放原点才不会悄悄退回居中。 */
  useEffect(() => {
    steps.forEach((s) => {
      if (shotNat[s.id]) return;
      const b64 = getStepMainSrc(s);
      if (!b64) return;
      const im = new Image();
      im.onload = () => {
        if (!im.naturalWidth || !im.naturalHeight) return;
        const w = im.naturalWidth;
        const h = im.naturalHeight;
        setShotNat((prev) => (prev[s.id] ? prev : { ...prev, [s.id]: { w, h } }));
      };
      im.src = `data:image/jpeg;base64,${b64}`;
    });
  }, [steps.map((s) => `${s.id}|${s.screenshot_base64 ? s.screenshot_base64.length : 0}`).join("||")]);

  /* ── 通篇滚动编辑：草稿 · 落库 · 就地操作 ────────────────── */

  /* 防抖回调里取最新 steps，避免闭包拿到旧数组 */
  const stepsRef = useRef<Step[]>([]);
  stepsRef.current = steps;

  /* 每步草稿：没编辑过就回落到步骤当前值 */
  const draftOf = (s: Step) =>
    drafts[s.id] ?? { title: s.title ?? "", description: s.description ?? "" };

  /* 大标题的 contenteditable 是不受控的：按 step.id 记一份 DOM 引用，
     敲完/点工具条时从 DOM 现读 HTML（读 DOM 而不是读 state，光标才不会被顶回去） */
  const headingEls = useRef<Record<string, HTMLDivElement | null>>({});

  /* textarea 随内容长高（标题/描述都是一行起步、随输入撑开） */
  const autoGrow = (el: HTMLTextAreaElement | null) => {
    if (!el) return;
    el.style.height = "auto";
    el.style.height = el.scrollHeight + "px";
  };

  /* ── 第二十六轮：正文最前面那条「文档头」────────────────────────
     就是设计稿的 .doc-head：一行大标题（20px）+ 一行元信息
     （N 个步骤 · M 张截图 · 自动生成 · 可编辑）。
     标题与顶栏那颗是**同一个字段**（recordings.title），走同一条命令，
     就地改、失焦或回车落库，两处随时同步。 */
  const [docTitleDraft, setDocTitleDraft] = useState<string | null>(null);
  const docTitle = docTitleDraft ?? recording?.title ?? "";
  const commitDocTitle = async () => {
    const raw = (docTitleDraft ?? "").trim();
    setDocTitleDraft(null);
    if (!raw || raw === recording?.title) return;
    try {
      await invoke("update_recording_title", { id: recordingId, title: raw });
      setRecording((prev) => (prev ? { ...prev, title: raw } : prev));
      refreshHist();
    } catch (err) {
      setToast({ type: "error", message: String(err) });
    }
  };

  const scheduleDraftSave = (id: string, d: StepDraft) => {
    if (draftTimers.current[id]) clearTimeout(draftTimers.current[id]);
    draftTimers.current[id] = setTimeout(async () => {
      delete draftTimers.current[id];
      const s0 = stepsRef.current.find((x) => x.id === id);
      if (!s0) return;
      try {
        await invoke("update_step", {
          stepId: id,
          title: d.title,
          description: d.description,
          actionType: s0.action_type,
          tip: s0.tip || "",
          // 只有真的改过富文本才带上：undefined 会被 JSON.stringify 丢掉，
          // 后端走 COALESCE(?8, title_html) 保留原值（老调用方不用改）
          titleHtml: d.titleHtml,
        });
        setSteps((prev) =>
          prev.map((x) =>
            x.id === id
              ? {
                  ...x,
                  title: d.title,
                  description: d.description,
                  ...(d.titleHtml === undefined ? {} : { title_html: d.titleHtml }),
                }
              : x
          )
        );
        setDrafts((prev) => {
          if (!(id in prev)) return prev;
          const next = { ...prev };
          delete next[id];
          return next;
        });
        setAutoSaved(true);
        setTimeout(() => setAutoSaved(false), 1500);
        refreshHist();
      } catch (err) {
        console.error("Auto-save failed:", err);
      }
    }, 800);
  };

  const setDraftField = (id: string, field: "title" | "description", value: string) => {
    const s0 = stepsRef.current.find((x) => x.id === id);
    const base: StepDraft =
      drafts[id] ?? { title: s0?.title ?? "", description: s0?.description ?? "" };
    const next = { ...base, [field]: value };
    setDrafts((prev) => ({ ...prev, [id]: next }));
    scheduleDraftSave(id, next);
  };

  /** 大标题富文本：contenteditable 里敲完，把归一化 HTML + 纯文本一起落草稿 */
  const setDraftHeadingHtml = (id: string, html: string, plain: string) => {
    const s0 = stepsRef.current.find((x) => x.id === id);
    const base: StepDraft =
      drafts[id] ?? { title: s0?.title ?? "", description: s0?.description ?? "" };
    const next: StepDraft = { ...base, title: plain, titleHtml: html };
    setDrafts((prev) => ({ ...prev, [id]: next }));
    scheduleDraftSave(id, next);
  };

  const [autoSaved, setAutoSaved] = useState(false);

  /* ── 文档块 / 插入菜单（2026-09-19 第十三轮）──────────────────
     1) 左栏颗粒 ＋ 不再直接插一个空步骤，而是弹菜单：步骤 / 大标题 /
        导入已有教程 / 续录 —— 对标参考产品那条「+ 插入」工具条。
     2) 「大标题」是文档块（block_kind='heading'）：不占步骤编号、
        不计入步骤总数，编号只在真步骤之间连续。
     3) 「续录」由 App 负责（拉起录制 → 停录后把新步骤插到这个位置之后）。 */

  /** 真步骤（不含大标题 / 流程摘要等文档块）：用来算「N 个步骤」、大图预览翻页、编号回退 */
  const realSteps = useMemo(
    () => steps.filter((s) => !isDocBlock(s)),
    [steps]
  );

  /** 文档流块序列：真步骤按 1..N 连续编号（文档块不占号），大标题带上自己的层级；
   *  indent = 这一条应该缩进几级（步骤跟着最近的那个大标题走）。 */
  const previewBlocks = useMemo(() => {
    let num = 0;
    let level = 0;
    return steps.map((s) => {
      if (isSummaryBlock(s)) {
        return { step: s, isHeading: false, isSummary: true, isIntro: false, level: 0, num: 0, indent: 0 };
      }
      if (isHeadingBlock(s)) {
        level = Math.min(4, Math.max(1, s.block_level || 1));
        return { step: s, isHeading: true, isSummary: false, isIntro: false, level, num: 0, indent: level - 1 };
      }
      if (isIntroBlock(s)) {
        // 简介块跟着最近的大标题缩进（和步骤一样），但自己不占编号
        return { step: s, isHeading: false, isSummary: false, isIntro: true, level: 0, num: 0, indent: Math.min(3, level) };
      }
      num += 1;
      return { step: s, isHeading: false, isSummary: false, isIntro: false, level: 0, num, indent: Math.min(3, level) };
    });
  }, [steps]);

  /** 左栏「目录」的树：大标题是分组头，它下面的步骤缩进一级；折起来的分支不下发 */
  const dirEntries = useMemo(() => {
    let hiddenBelow: number | null = null; // 非 null 时，层级比它深的条目全藏起来
    return previewBlocks.map((b) => {
      if (b.isHeading) {
        if (hiddenBelow !== null && b.level > hiddenBelow) {
          return { ...b, hidden: true, collapsed: false };
        }
        hiddenBelow = null;
        const collapsed = collapsedHeadings.has(b.step.id);
        if (collapsed) hiddenBelow = b.level;
        return { ...b, hidden: false, collapsed };
      }
      return { ...b, hidden: hiddenBelow !== null, collapsed: false };
    });
  }, [previewBlocks, collapsedHeadings]);

  /** 改这一步「补充说明」的类型：描述 / 标注 / 警告 */
  const setNoteKindFor = useCallback(
    async (step: Step, kind: "" | "mark" | "warn") => {
      if (noteKindOf(step) === kind) return;
      try {
        await invoke("set_step_note_kind", { stepId: step.id, noteKind: kind });
        setSteps((prev) => prev.map((s) => (s.id === step.id ? { ...s, note_kind: kind } : s)));
        refreshHist();
      } catch (err) {
        console.error("Set note kind failed:", err);
      }
    },
    [refreshHist]
  );

  /** 在 insertPos 处插入一个「大标题」块（insertPos = 数组下标，即插在被点那行之后） */
  const handleAddHeading = useCallback(
    async (insertPos: number) => {
      try {
        await invoke("add_block", {
          recordingId,
          blockKind: "heading",
          orderIndex: insertPos,
          blockLevel: 2,
          title: "",
        });
        await reloadRecording();
        refreshHist();
      } catch (err) {
        console.error("Add heading failed:", err);
      }
    },
    [recordingId, reloadRecording, refreshHist]
  );

  /** 切换标题块层级 H1/H2/H3 */
  const handleSetBlockLevel = useCallback(async (stepId: string, level: number) => {
    try {
      await invoke("update_block_meta", { stepId, blockKind: "heading", blockLevel: level });
      setSteps((prev) => prev.map((s) => (s.id === stepId ? { ...s, block_level: level } : s)));
    } catch (err) {
      console.error("Set block level failed:", err);
    }
  }, []);

  /* ── 简介块（2026-09-19 第十九轮）─────────────────────────────
     用户在正文里自己写的引子（"这份教程适合谁、读完能学会什么"）。
     入口在左栏颗粒 ＋ 的插入菜单里（与「添加大标题」同一排）；
     正文存 description，走和其它就地可编辑控件同一套 800ms 防抖落库。
     图标键（截图浮层那颗「图片描述」）第十九轮按用户要求去掉 ——
     已有图注仍照常渲染在图片下方，不影响老数据。 */

  /** 在 insertPos 处插入一个「简介」块 */
  const handleAddIntro = useCallback(
    async (insertPos: number) => {
      try {
        await invoke("add_block", {
          recordingId,
          blockKind: "intro",
          orderIndex: insertPos,
          blockLevel: 1,
          title: "",
        });
        await reloadRecording();
        refreshHist();
      } catch (err) {
        console.error("Add intro failed:", err);
      }
    },
    [recordingId, reloadRecording, refreshHist]
  );

  /* ── 流程摘要（2026-09-19 第十六轮）────────────────────────────
     录制结束时后端已经同步写了「规则版」摘要（不联网、秒出），这里负责
     手动「重新生成」：让 AI 按规则版那份要点润色覆盖（不可用就保留规则版）。 */
  const regenerateSummary = useCallback(
    async (stepId: string) => {
      if (!recordingId || summaryBusy) return;
      setSummaryBusy(stepId);
      try {
        await invoke("summarize_recording_ai", { recordingId });
        await reloadRecording();
        refreshHist();
      } catch (err) {
        console.error("Summarize failed:", err);
        setToast({
          type: "error",
          message: "AI 摘要没生成成功，已保留自动生成的版本：" + String(err),
        });
      } finally {
        setSummaryBusy(null);
      }
    },
    [recordingId, summaryBusy, reloadRecording, refreshHist, setToast]
  );

  /** 大标题工具条上的富文本命令（WebView2 即 Chromium，execCommand 可用） */
  const execRich = (cmd: string, value?: string) => {
    const id = activeHeadingId;
    if (!id) return;
    try {
      // styleWithCSS：让 foreColor / hiliteColor 产出 <span style>，而不是废弃的 <font color>
      document.execCommand("styleWithCSS", false, "true");
      document.execCommand(cmd, false, value);
    } catch {
      /* 命令不被支持时静默忽略，别把编辑流程打断 */
    }
    const el = headingEls.current[id];
    if (el) {
      const html = normalizeInlineHtml(el);
      setDraftHeadingHtml(id, html, htmlToPlainText(html));
    }
    setColorMenu(null);
  };

  /** 目录栏：折起 / 展开某个大标题下面的这一节 */
  const toggleHeadingCollapse = (id: string) => {
    setCollapsedHeadings((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  /** 目录栏拖拽落库：整份 id 顺序交给 reorder_steps（它按文档块口径重排编号） */
  const applyReorder = async () => {
    const from = dragStepId;
    const target = dropAt;
    setDragStepId(null);
    setDropAt(null);
    if (!from || !target || from === target.id) return;
    const ids = steps.map((s) => s.id);
    const at = ids.indexOf(from);
    if (at < 0) return;
    ids.splice(at, 1);
    const to = ids.indexOf(target.id);
    if (to < 0) return;
    ids.splice(target.after ? to + 1 : to, 0, from);
    // 乐观更新：本地先动，失败就整体重载兜回来
    const byId = new Map(steps.map((s) => [s.id, s]));
    setSteps(ids.map((i) => byId.get(i)!).filter(Boolean));
    try {
      await invoke("reorder_steps", { recordingId, stepIds: ids });
      await reloadRecording();
      refreshHist();
    } catch (err) {
      console.error("Reorder failed:", err);
      await reloadRecording();
    }
  };

  /** 打开「+」插入菜单，菜单贴着加号右下方浮出 */
  const openInsertMenu = (e: React.MouseEvent, index: number) => {
    e.stopPropagation();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setInsertMenu({
      index,
      x: r.right + 8,
      y: Math.min(r.top - 8, Math.max(12, window.innerHeight - 260)),
    });
  };

  /** 打开「导入已有教程」选择器 */
  const openImportPicker = useCallback(
    async (insertAfter: number) => {
      setImportFor({ index: insertAfter });
      setImportList([]);
      try {
        const list = await invoke<
          Array<{ id: string; title: string; total_steps: number; created_at: string }>
        >("list_recordings");
        setImportList(list.filter((r) => r.id !== recordingId));
      } catch (err) {
        console.error("List recordings failed:", err);
      }
    },
    [recordingId]
  );

  /** 真导入：把源教程的真步骤搬到当前位置之后 */
  const doImport = useCallback(
    async (sourceId: string) => {
      if (!importFor || importBusy) return;
      setImportBusy(true);
      try {
        const n = await invoke<number>("import_steps_from", {
          recordingId,
          sourceRecordingId: sourceId,
          insertAfter: importFor.index,
        });
        await reloadRecording();
        refreshHist();
        setImportFor(null);
        setToast({ type: "success", message: `已导入 ${n} 个步骤，Ctrl+Z 可整体撤销` });
      } catch (err: any) {
        setToast({ type: "error", message: "导入失败：" + String(err?.message || err) });
      } finally {
        setImportBusy(false);
      }
    },
    [importFor, importBusy, recordingId, reloadRecording, refreshHist]
  );

  /* ── 截图就地操作（按步）─────────────────────────────────── */

  /* ── 这一步的媒体（图片 / GIF 动图 / 视频）─────────────────────
     老链路（upload_step_screenshot）把一切都当 JPEG 存成 `.jpg`：GIF 传上去就成
     一张不会动的图，视频存了也放不出来。第二十轮改走 upload_step_media ——
     按扩展名**原样落盘**，视频额外抽一张首帧当封面（缩略图 / 导出 / 发布都用它）。 */

  /** 视频抽首帧：`<video>` 读对象 URL → seek 到 0.1s → canvas → JPEG base64。
      抽不到（编码特别、0 时长）就返回空串，**不阻断上传** —— 视频照样能播，
      只是没有封面。 */
  const grabVideoPoster = (file: File): Promise<string> =>
    new Promise((resolve) => {
      const url = URL.createObjectURL(file);
      const v = document.createElement("video");
      let done = false;
      const finish = (out: string) => {
        if (done) return;
        done = true;
        window.clearTimeout(timer);
        URL.revokeObjectURL(url);
        resolve(out);
      };
      const timer = window.setTimeout(() => finish(""), 6000);
      v.muted = true;
      v.playsInline = true;
      v.preload = "auto";
      v.onloadeddata = () => {
        // 往后挪一点点：某些编码的首帧是全黑
        try {
          v.currentTime = Math.min(0.1, (v.duration || 0.2) / 2);
        } catch { /* 有些容器不给 seek，直接用当前帧 */ }
      };
      v.onseeked = () => {
        try {
          const c = document.createElement("canvas");
          c.width = v.videoWidth || 1280;
          c.height = v.videoHeight || 720;
          const ctx = c.getContext("2d");
          if (!ctx) return finish("");
          ctx.drawImage(v, 0, 0, c.width, c.height);
          finish(c.toDataURL("image/jpeg", 0.85).split(",")[1] || "");
        } catch {
          finish("");
        }
      };
      v.onerror = () => finish("");
      v.src = url;
    });

  /** 读成 data URL（Rust 侧会剥掉前缀，两边都容错）。 */
  const mediaToBase64 = (file: File): Promise<string> =>
    new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => resolve(String(r.result || ""));
      r.onerror = () => reject(new Error("读取文件失败"));
      r.readAsDataURL(file);
    });

  /** 上传一个媒体文件到指定步骤。类型与大小在这里先挡一道，Rust 侧还有一道白名单。 */
  const uploadMediaForStep = useCallback(
    async (stepId: string, file: File) => {
      if (!MEDIA_EXT_RE.test(file.name)) {
        setToast({ type: "error", message: "只支持 图片 / GIF / MP4：png、jpg、webp、gif、mp4、webm" });
        return;
      }
      const mb = file.size / 1024 / 1024;
      if (mb > MEDIA_MAX_MB) {
        setToast({ type: "error", message: `文件太大（${mb.toFixed(1)}MB），单个上限 ${MEDIA_MAX_MB}MB` });
        return;
      }
      const ext = extOf(file.name);
      const isVideo = ext === "mp4" || ext === "webm";
      try {
        const data = await mediaToBase64(file);
        const poster = isVideo ? await grabVideoPoster(file) : "";
        await invoke("upload_step_media", {
          stepId,
          base64Data: data,
          ext,
          posterBase64: poster || null,
        });
        setSelectedStepId(stepId);
        await reloadRecording();
        setToast({ type: "success", message: isVideo ? "视频已插入 · 按播放键预览" : "已插入" });
      } catch (err) {
        setToast({ type: "error", message: "上传失败：" + String(err) });
      }
    },
    [reloadRecording]
  );

  const pickMediaForStep = (stepId: string) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = "image/*,video/mp4,video/webm";
    input.onchange = (e) => {
      const file = (e.target as HTMLInputElement).files?.[0];
      if (file) void uploadMediaForStep(stepId, file);
    };
    input.click();
  };

  /** 拖拽进来 —— 兼容「文件」和「从网页拖来的图片」。 */
  const dropMediaForStep = (stepId: string, dt: DataTransfer) => {
    const fromList = dt.files && dt.files.length ? dt.files[0] : null;
    const fromItems = Array.from(dt.items || [])
      .map((i) => (i.kind === "file" ? i.getAsFile() : null))
      .find(Boolean);
    const file = fromList || fromItems;
    if (file) void uploadMediaForStep(stepId, file);
  };

  const pasteForStep = (e: React.MouseEvent, stepId: string) => {
    e.preventDefault();
    navigator.clipboard
      .read()
      .then((items) => {
        for (const item of items) {
          const hit = item.types.find((t) => t.startsWith("image/") || t.startsWith("video/"));
          if (!hit) continue;
          item.getType(hit).then((blob) => {
            const sub = (hit.split("/")[1] || "png").replace("jpeg", "jpg");
            // ⚠️ 这里不能用裸 `File`：本文件从 lucide-react 引入过同名的图标组件，会被遮蔽
            void uploadMediaForStep(stepId, new window.File([blob], `粘贴.${sub}`, { type: hit }));
          });
          return;
        }
      })
      .catch(() => console.log("Clipboard read failed or no media"));
  };

  const deleteScreenshotFor = (step: Step) => {
    if (!confirm("确定删除截图？")) return;
    invoke("delete_step_screenshot", { stepId: step.id, isAfter: true });
    setSteps((prev) => prev.map((s) => (s.id === step.id ? { ...s, screenshot_base64: null } : s)));
  };

  /* ── 滚动联动：滑到哪，左目录高亮跟到哪 ────────────────────
     编辑态与通篇预览态共用（预览态此前漏了两处：块上没有 data-step-id、
     下面 onScroll 又被 previewMode 关掉 ⇒ 右边怎么滑左边都不动）。 */

  const sopBodyRef = useRef<HTMLDivElement>(null);
  const sopScrollLock = useRef(0);

  const onSopScroll = useCallback(() => {
    if (Date.now() < sopScrollLock.current) return;
    const box = sopBodyRef.current;
    if (!box) return;
    // 参考线：容器顶部往下 28px。判据 = 「完全落在参考线之上的最后一块」，
    // 而不是「离参考线最近的那一块」—— 一块截图占满整屏时，屏幕外刚露头的
    // 下一块反而离参考线更近，会把高亮提前抢走（预览态块特别高，这个坑必踩）。
    const ref = box.getBoundingClientRect().top + 28;
    let bestId = "";
    box.querySelectorAll<HTMLElement>("[data-step-id]").forEach((el) => {
      if (el.getBoundingClientRect().top <= ref) bestId = el.dataset.stepId || "";
    });
    // 还没滚过第一块（或首块是文档行）时，兜底取第一个候选
    if (!bestId) bestId = box.querySelector<HTMLElement>("[data-step-id]")?.dataset.stepId || "";
    if (bestId && bestId !== selectedStepId) setSelectedStepId(bestId);
  }, [selectedStepId]);

  /* 目录跟随：高亮换了行，就把那一行滚进左栏可视区。
     用 block:"nearest" —— 行本来就在视野里时是空操作，所以「点目录跳转 / 查找
     定位 / 新增步骤」这些路径共用这一条也不会打架。 */
  useEffect(() => {
    if (!selectedStepId) return;
    document
      .getElementById(`step-item-${selectedStepId}`)
      ?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, [selectedStepId]);

  /* 标题/描述都是「自动长高」的 textarea：rows 只保证一行的高度，长标题会被
     overflow:hidden 裁掉。步骤数据一变就统一量一次 scrollHeight 顶开。 */
  useEffect(() => {
    const box = sopBodyRef.current;
    if (!box) return;
    box.querySelectorAll<HTMLTextAreaElement>(".sop-edit-title, .sop-edit-desc")
      .forEach((el) => autoGrow(el));
  }, [steps]);


  /* ── Upload screenshot ─────────────────────────────────── */

  const handleUploadScreenshot = useCallback(
    (base64: string, isAfter: boolean, isMarked: boolean) => {
      if (!selectedStep) return;
      invoke("upload_step_screenshot", {
        stepId: selectedStep.id,
        base64Data: base64,
        isAfter,
        isMarked,
      })
        .then(() => {
          setSteps((prev) =>
            prev.map((s) => {
              if (s.id !== selectedStep.id) return s;
              if (isMarked) return { ...s, marked_screenshot_base64: base64 };
              if (isAfter) return { ...s, screenshot_base64: base64 };
              return { ...s, screenshot_base64: base64 };
            })
          );
          refreshHist();
        })
        .catch((err) => console.error("Upload screenshot failed:", err));
    },
    [selectedStep, refreshHist]
  );

  /* ── Picking a file via hidden input ──────────────────── */

  const pickFileForUpload = useCallback(() => {
    if (!selectedStep) return;
    const input = document.createElement("input");
    input.type = "file";
    input.accept = "image/*";
    input.onchange = (e) => {
      const file = (e.target as HTMLInputElement).files?.[0];
      if (!file) return;
      const reader = new FileReader();
      reader.onload = (ev) => {
        const dataUrl = ev.target?.result as string;
        const base64 = dataUrl.split(",")[1];
        // ⚠️ isAfter 必须 false：upload 语义里 true 写 after_screenshot_path（前端不做主图），
        // false 才写主截图 screenshot_path（=主图源，退出重进不丢图）
        invoke("upload_step_screenshot", { stepId: selectedStep.id, base64Data: base64, isAfter: false, isMarked: false })
          .then(() => {
            setSteps(prev => prev.map(s =>
              s.id !== selectedStep.id ? s : {
                ...s,
                screenshot_base64: base64
              }
            ));
            refreshHist();
          });
      };
      reader.readAsDataURL(file);
    };
    input.click();
  }, [selectedStep, refreshHist]);

  /* ── Paste screenshot from clipboard ──────────────────── */

  const pasteScreenshot = useCallback(
    (e: React.MouseEvent) => {
      if (!selectedStep) return;
      e.preventDefault();
      navigator.clipboard
        .read()
        .then((items) => {
          for (const item of items) {
            for (const type of item.types) {
              if (type.startsWith("image/")) {
                item.getType(type).then((blob) => {
                  const reader = new FileReader();
                  reader.onload = (ev) => {
                    const dataUrl = ev.target?.result as string;
                    const base64 = dataUrl.split(",")[1];
                    // ⚠️ isAfter 必须 false：upload 语义里 true 写 after_screenshot_path（前端不做主图），
                    // false 才写主截图 screenshot_path（=主图源，退出重进不丢图）
                    invoke("upload_step_screenshot", { stepId: selectedStep.id, base64Data: base64, isAfter: false, isMarked: false })
                      .then(() => {
                        setSteps(prev => prev.map(s =>
                          s.id !== selectedStep.id ? s : {
                            ...s,
                            screenshot_base64: base64
                          }
                        ));
                        refreshHist();
                      });
                  };
                  reader.readAsDataURL(blob);
                });
                return;
              }
            }
          }
        })
        .catch(() => console.log("Clipboard read failed or no image"));
    },
    [selectedStep]
  );

  /* ── Delete step ─────────────────────────────────────────── */

  const handleDeleteStep = useCallback(
    (stepId: string) => {
      invoke("delete_step", { stepId }).catch((err) =>
        console.error("Delete step failed:", err)
      );
      setSteps((prev) => {
        const idx = prev.findIndex((s) => s.id === stepId);
        if (idx === -1) return prev;
        const nextSteps = prev.filter((s) => s.id !== stepId);
        return nextSteps.map((s, i) => ({ ...s, step_number: i + 1, order_index: i }));
      });
      setDeleteTarget(null);
      refreshHist();
      if (selectedStepId === stepId) {
        setSelectedStepId((prevId) => {
          const idx = steps.findIndex((s) => s.id === prevId);
          if (steps.length <= 1) return "";
          if (idx < steps.length - 1) return steps[idx + 1].id;
          return steps[idx - 1].id;
        });
      }
    },
    [selectedStepId, steps, refreshHist]
  );

  /* ── 批量删除（2026-09-20 第二十三轮：入口在目录）──────────────
     逐条 await delete_step（后端每删一条自己重排编号），本地一次性过滤
     勾选步骤再整表重排；全空 / 忙碌时按钮禁用，删完退出批量态。 */
  const handleBatchDelete = useCallback(
    async () => {
      const ids = Array.from(batchChecked);
      if (ids.length === 0 || batchBusy) return;
      setBatchBusy(true);
      try {
        for (const id of ids) {
          await invoke("delete_step", { stepId: id });
        }
        const killed = new Set(ids);
        setSteps((prev) =>
          prev
            .filter((s) => !killed.has(s.id))
            .map((s, i) => ({ ...s, step_number: i + 1, order_index: i }))
        );
        if (selectedStepId && killed.has(selectedStepId)) {
          const remaining = steps.filter((s) => !killed.has(s.id));
          setSelectedStepId(remaining.length > 0 ? remaining[0].id : "");
        }
        setBatchChecked(new Set());
        setBatchMode(false);
        refreshHist();
        setToast({ type: "success", message: `已删除 ${ids.length} 个步骤` });
      } catch (err) {
        setToast({ type: "error", message: "批量删除失败: " + String(err) });
      } finally {
        setBatchBusy(false);
      }
    },
    [batchChecked, batchBusy, steps, selectedStepId, refreshHist]
  );

  /* ── Insert step ────────────────────────────────────────── */

  const handleInsertStep = useCallback(
    async (afterIndex: number) => {
      const insertPos = afterIndex + 1;
      const stepData = {
        order_index: insertPos,
        step_number: insertPos + 1,
        action_type: "click",
        title: "新步骤",
        description: "",
        tip: "",
        screenshot_path: "",
        window_title: "",
      };

      try {
        const newStep: Step = await invoke("add_step", {
          recordingId,
          orderIndex: insertPos,
          stepData,
        });

        setSteps((prev) => {
          const next = [...prev];
          next.splice(insertPos, 0, newStep);
          return next.map((s, i) => ({ ...s, step_number: i + 1, order_index: i }));
        });
        setSelectedStepId(newStep.id);
        refreshHist();
        setTimeout(() => {
          // 滚动到新添加的步骤本身（可见即可），避免滚到列表末尾把新步骤滑出页面
          const el = document.getElementById(`step-item-${newStep.id}`);
          if (el) {
            el.scrollIntoView({ behavior: "smooth", block: "nearest" });
          }
        }, 50);
      } catch (err) {
        console.error("Add step failed:", err);
      }
    },
    [recordingId, refreshHist]
  );

  /* ── Move step ──────────────────────────────────────────── */

  const handleMoveStep = useCallback(
    (index: number, direction: "up" | "down") => {
      setSteps((prev) => {
        const targetIndex = direction === "up" ? index - 1 : index + 1;
        if (targetIndex < 0 || targetIndex >= prev.length) return prev;
        const next = [...prev];
        [next[index], next[targetIndex]] = [next[targetIndex], next[index]];
        return next.map((s, i) => ({ ...s, step_number: i + 1, order_index: i }));
      });
    },
    []
  );

  /* ── Update recording title ──────────────────────────────── */

  /* ── Click outside to close dropdowns ───────────────────── */

  useEffect(() => {
    const closeDropdowns = () => {
      // 不关导出弹窗：ExportDialog 的遮罩 onClick 负责（仅点击遮罩空白关闭），
      // 全局 mousedown 若一并 setExportOpen(false)，弹窗内任意点击都会先被关掉。
      setToolsOpen(false);
      setSimplifyOpen(false);
      setTranslateOpen(false);
    };
    window.addEventListener("mousedown", closeDropdowns);
    return () => window.removeEventListener("mousedown", closeDropdowns);
  }, []);

  /* ── Export ───────────────────────────────────────────────── */

  const handleExport = useCallback(
    async (format: string, options: ExportOptions) => {
      if (!recordingId) return;
      setExportOpen(false);
      // 图片格式按「图片方式」映射到后端格式：长截图 = long，逐张文件 = images
      let backendFormat = format;
      if (format === 'images') {
        backendFormat = options.imageMode === 'long' ? 'long' : 'images';
      } else if (format === 'word') {
        backendFormat = 'docx';
      }
      // ★ 第四十八轮：不再弹保存框，直接导出到固定目录「文档\录步导出」
      setExporting(true);
      setExportProgress(null);
      // 监听 Rust 侧 OCR 脱敏进度（导出前自动脱敏时上报 export://progress）
      const unlisten = await listen<{ done: number; total: number }>(
        "export://progress",
        (ev) => {
          const { done, total } = ev.payload;
          setExportProgress(`脱敏 ${done}/${total}`);
        }
      );
      try {
        await invoke<string>("export_recording", {
          id: recordingId,
          format: backendFormat,
          options,
        });
        // 完成提醒统一走全局 flowio://job-done 卡片（任意页面弹窗，带教程名）
      } catch (err) {
        // 失败提醒同样走全局卡片
      } finally {
        setExporting(false);
        setExportProgress(null);
        unlisten();
      }
    },
    [recordingId]
  );

/* ── Stage 2a：精准执行器（坐标确定性重放，非 LLM/视觉）── */
  // 真实执行：执行前先把本应用窗口彻底隐藏让出屏幕（hide 比 minimize 更彻底，
  // 避免某些系统上最小化失效导致窗口仍挡在前面、点击落到 Flowio 自身），
  // 否则全屏盖在最上层，重放的坐标点击会全部落在 Flowio 自己窗口上、目标应用收不到。
  const realExecute = useCallback(
    async (approvalMode: string) => {
      if (!recordingId) return;
      const win = getCurrentWindow();
      setExecuting(true);
      setToast({ type: "success", message: "开始精准执行，请勿操作鼠标键盘…" });
      try {
        try { await win.hide(); } catch { /* 忽略未能隐藏 */ }
        // 给 OS 一点时间把目标应用顶到前台，再下发动作
        await new Promise((r) => setTimeout(r, 300));
        const report = await invoke<any>("execute_recording", {
          recordingId, stepDelayMs: 300, approvalMode, approved: true,
        });
        const fails = (report.outcomes || []).filter((o: any) => o.status === "verify-fail").length;
        const panics = (report.outcomes || []).filter((o: any) => o.status === "step-panic").length;
        setToast({
          type: fails > 0 || panics > 0 ? "error" : "success",
          message: `执行完成：${report.total} 步，校验异常 ${fails} 步${panics > 0 ? `，异常步 ${panics} 步` : ""}`,
        });
      } catch (err) {
        setToast({ type: "error", message: `执行失败：${String(err)}` });
      } finally {
        setExecuting(false);
        try { await win.show(); } catch { /* 忽略 */ }
        try { await win.setFocus(); } catch { /* 忽略 */ }
      }
    },
    [recordingId]
  );

  const handleExecute = useCallback(
    async () => {
      if (!recordingId) { setToast({ type: "error", message: "未找到当前录制" }); return; }
      // 先预览计划；含敏感步骤（快捷键/复制/粘贴/拖拽）则弹审批闸门，
      // 此时**不下发任何桌面动作**，等用户点「批准并执行」后再用 approved=true 重放
      setExecuting(true);
      try {
        const preview = await invoke<any>("preview_replay", { recordingId });
        if (!preview.requiresApproval) {
          await realExecute("off");
        } else {
          setExecuting(false); // 闸门未过，不放行；弹窗等待用户确认
          setApprovalPlan(preview);
        }
      } catch (err) {
        setToast({ type: "error", message: `预览执行计划失败：${String(err)}` });
        setExecuting(false);
      }
    },
    [recordingId, realExecute]
  );

  // Stage 3：用户在审批闸门里点「批准并执行」后，才真实下发桌面动作
  const confirmApproval = useCallback(async () => {
    if (!recordingId || !approvalPlan) return;
    setApprovalPlan(null);
    await realExecute("check");
  }, [recordingId, approvalPlan, realExecute]);

/* ── Publish to website ──────────────────────────────────── */

// SHA-256 hex（与网站后端 content_hash 同口径），用于发布前按内容兜底查已存在教程
async function sha256Hex(s: string): Promise<string> {
  const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return Array.from(new Uint8Array(buf)).map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** ★ 端云一体化 v247：发布 / 静默草稿共用的「上传步骤载荷」组装。
 *   发布=所见即所得：把重点图（按该步倍率裁出「放大后仍可见」的区域）传上去，
 *   与编辑器显示、导出用的是同一套公式。落点缺失或倍率=1 时原样返回整图；
 *   任何失败都回退全图，不阻塞上传。
 *   压缩每张截图，显著降低上传体积（修复"保存草稿/发布失败"：完整截图会导致请求体过大/超时）。 */
async function buildUploadSteps(
  steps: Step[],
  getZoom: (stepId: string) => number
): Promise<unknown[]> {
  return Promise.all(
    steps.map(async (s) => {
      let shotB64: string | undefined = s.screenshot_base64 || undefined;
      if (shotB64) {
        try {
          shotB64 = await focusShotBase64(shotB64, s.x, s.y, getZoom(s.id));
        } catch { /* 裁剪失败 → 回退全图 */ }
      }
      return {
        order_index: s.order_index,
        step_number: s.step_number,
        action_type: s.action_type,
        title: s.title,
        description: s.description,
        tip: s.tip,
        x: s.x,
        y: s.y,
        window_title: s.window_title,
        screenshot_base64: shotB64 ? await downscaleBase64Image(shotB64) : undefined,
        before_screenshot_base64: s.before_screenshot_base64 ? await downscaleBase64Image(s.before_screenshot_base64) : undefined,
        after_screenshot_base64: s.after_screenshot_base64 ? await downscaleBase64Image(s.after_screenshot_base64) : undefined,
      };
    })
  );
}

const handlePublish = useCallback(async (targetStatus: "draft" | "published" = "published", openWeb = false) => {
    if (!siteToken) { setToast({ type: "error", message: "请先登录后再发布" }); return; }
    if (!recordingId) { setToast({ type: "error", message: "未找到当前录制，请重试" }); return; }
    setPublishing(true);
    setPublishedGuide(null);
    try {
      // 重新加载录制数据以确保截图是最新的
      const data = await invoke<RecordingWithSteps>("load_recording", { id: recordingId });
      const category = pubCategory || categories[0]?.id || "other";

      const isDraft = targetStatus === "draft";
      const title = pubTitle.trim() || (isDraft ? (recording?.title || "未命名教程") : "");
      const applications = pubApplications.map((s) => s.trim()).filter(Boolean);
      // 应用名不自动生成（用户：自己填）——只用表单里手填的第一个应用，没有就空串
      const app_name = applications[0] || "";
      // 发布（表单路径）需校验必填；草稿放宽，用录制信息兜底
      if (!isDraft && !title) { setToast({ type: "error", message: "请填写标题" }); return; }
      if (!isDraft && applications.length === 0) { setToast({ type: "error", message: "请至少填写一个涉及应用" }); return; }
      if (!isDraft && !pubCategory) { setToast({ type: "error", message: "请选择分类" }); return; }

      // 标签直接发送（不再合并场景名称——场景下拉已移除，改为自由文本「场景描述」）
      const tagsToSend = pubTags;

      // 发布=所见即所得：重点图/压缩逻辑统一走 buildUploadSteps（与静默草稿同一份口径）
      const steps = await buildUploadSteps(data.steps, (id) => shotZoomOf(id));

      // 覆盖发布：同一录制曾发布过（本地映射记录 guide id）→ 带 guide_id 让网站覆盖更新而非新建
      let prevGuideId: string | null = null;
      try { prevGuideId = localStorage.getItem(`flowio_pub_${recordingId}`); } catch { /* ignore */ }

      // 双保险：本地映射丢失（清缓存 / 换机 / 跨端先由扩展发布过）时，按 content_hash 兜底查
      // 同主题目录已有教程，命中则覆盖更新，承接旧教程的点赞 / 评论 / 收藏，避免重复上传产生孤儿教程。
      if (!prevGuideId) {
        try {
          const hash = await sha256Hex(JSON.stringify({
            title: String(title).trim(),
            app_name,
            steps: steps.map((s: any) => s.title),
          }));
          const q = `https://flowio.cn/api/collections/guides/records?filter=${encodeURIComponent(`content_hash = "${hash}" && owner = "${siteUser?.id || ""}"`)}&fields=id,slug&perPage=1`;
          const r = await fetch(q, { headers: { Authorization: `Bearer ${siteToken}` } });
          if (r.ok) {
            const d = await r.json();
            const it = d.items?.[0];
            if (it?.id) prevGuideId = String(it.id);
          }
        } catch { /* 兜底查询失败不阻塞发布 */ }
      }
      const isUpdateMode = !!prevGuideId;
      const uploadData: Record<string, unknown> = {
        title,
        summary: pubSummary.trim(),
        app_name,
        category,
        tags: tagsToSend,
        software: applications,
        description: "",
        visibility: "public",
        status: targetStatus,
        downloadable: pubDownloadable,
        guide_id: prevGuideId || undefined,
        steps,
        // ★ T7 多语言上传：已有译文副本时随教程一起上传到网站
        translations: (() => {
          // __orig 是本地原文快照，不上传网站
          const entries = Object.entries(translations).filter(([k]) => k !== ORIG_KEY);
          return entries.length > 0 ? Object.fromEntries(entries) : undefined;
        })(),
      };
      if (pubCoverBase64) {
        uploadData.cover_base64 = pubCoverBase64;
      } else {
        // 封面=所见即所得（与步骤截图同一套重点图公式）：未手动选封面时，
        // 取首个有主图的步骤按其倍率裁重点图；该步无主图但有 after 图时回退 after（不裁）。
        // 任何失败回退全图，不阻塞发布。
        const firstImg = data.steps.find((s) => s.screenshot_base64);
        let coverSrc: string | undefined = firstImg?.screenshot_base64 || undefined;
        if (coverSrc && firstImg) {
          try {
            coverSrc = await focusShotBase64(coverSrc, firstImg.x, firstImg.y, shotZoomOf(firstImg.id));
          } catch { /* 裁剪失败 → 回退全图 */ }
        }
        if (!coverSrc && firstImg?.after_screenshot_base64) coverSrc = firstImg.after_screenshot_base64;
        if (coverSrc) uploadData.cover_base64 = await downscaleBase64Image(toPreviewSrc(coverSrc), 720, 0.85);
      }

      // POST 到网站 API（上传大量截图时可能较慢，设置 90s 超时）
      const PB_URL = "https://flowio.cn";
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), 90000);
      let res: Response;
      try {
        res = await fetch(`${PB_URL}/api/guides/upload`, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${siteToken}`,
          },
          body: JSON.stringify(uploadData),
          signal: controller.signal,
        });
      } finally {
        clearTimeout(timer);
      }

      const result = await res.json().catch(() => ({}) as Record<string, unknown>);
      if (!res.ok || !result.ok) {
        const detail = result.error ? `: ${String(result.error)}` : ` (HTTP ${res.status})`;
        if (res.status === 401) {
          setToast({ type: "error", message: "登录已过期，请重新登录后再发布" });
        } else if (res.status === 400) {
          setToast({ type: "error", message: "参数错误" + detail });
        } else if (res.status === 500) {
          setToast({ type: "error", message: "服务器错误" + detail });
        } else {
          setToast({ type: "error", message: "发布失败" + detail });
        }
        return;
      }

      setPublishedGuide(result.guide || null);
      // ★ T9：附挂团队库。🔴 **永不阻断发布** —— 教程已经在网站上了（用户手上有链接），
      //   这一步失败只是「没进团队库」。措辞由 lib/teamPublish 统一 —— 绝不许说成
      //   「发布失败」，那会让人重发一遍、制造重复教程。
      const attach = pubWorkspaceId && result.guide?.id
        ? await attachTutorialToWorkspace(siteToken, pubWorkspaceId, String(result.guide.id))
        : null;
      // 记录「录制 → 已发布教程」映射：下次发布同一录制时覆盖更新而非新建
      if (result.guide?.id) {
        try { localStorage.setItem(`flowio_pub_${recordingId}`, String(result.guide.id)); } catch { /* ignore */ }
      }
      // ★ r65：记下云端 slug —— 再点「在网页编辑」时秒跳，不必重传整篇截图
      const gRes = result.guide as { slug?: string; edit_url?: string } | undefined;
      const gResUrl = gRes?.edit_url ? `${SITE_ORIGIN}${gRes.edit_url}` : webEditUrl(gRes?.slug || "");
      if (gRes?.slug) {
        try { localStorage.setItem(webSlugKey(recordingId), gRes.slug); } catch { /* 配额忽略 */ }
      }
      // ★ 保存发布表单快照：下次重发同一录制时自动恢复填写内容，免重复输入
      try {
        const formSnap = {
          guideId: result.guide?.id || "",
          // ★ T9：记住这次选的团队空间，重发同一录制时自动恢复
          workspaceId: pubWorkspaceId || "",
          // ★ 端云一体化 v247：把公开链接与 slug 一起存下来（对齐插件二维码快照），
          //   重传是覆盖式 PATCH 同一篇（slug 不变），存下的地址长期有效。
          url: gResUrl || "",
          slug: gRes?.slug || "",
          title: pubTitle,
          applications: pubApplications,
          category: pubCategory,
          tags: pubTags,
          downloadable: pubDownloadable,
          scene: pubSummary,
          coverBase64: pubCoverBase64 && pubCoverBase64.length <= 1_500_000 ? pubCoverBase64 : null,
        };
        localStorage.setItem(`flowio_pubform_${recordingId}`, JSON.stringify(formSnap));
      } catch { /* 配额不足等忽略 */ }
      setShowPublishDialog(false);
      setPubStep("pick");
      // ★ r65「在网页编辑」（对齐 Scribe）：存完草稿直接把人送到网页编辑器。
      //   🔴 打开失败**绝不回滚已完成的保存** —— 草稿已经在云端了，只是这次没跳过去；
      //      措辞更不许说「发布失败」（那会让人重发一遍、制造重复教程）。
      if (openWeb) {
        const url = gRes?.edit_url ? `${SITE_ORIGIN}${gRes.edit_url}` : webEditUrl(gRes?.slug || "");
        if (url) {
          try { await invoke("open_external_url", { url }); }
          catch (e) { console.warn("[flowio] [r65] 打开网页编辑失败:", e); }
        } else {
          setToast({ type: "error", message: WEB_EDIT_OPEN_FAILED });
        }
      }
      const okMsg = isDraft
        ? (isUpdateMode ? "草稿已更新！" : "草稿已保存！")
        : (isUpdateMode ? "教程已更新，点赞/评论/收藏已保留" : "发布成功！教程已上传到网站");
      // ★ T9：把附挂团队库的结果并进同一条提示（没选空间 ⇒ 原样返回 okMsg）
      setToast({
        type: "success",
        message: composePublishToast({ base: okMsg, target: pubWorkspaceId, list: pubWorkspaces, attach }),
      });
    } catch (err) {
      const isAbort = (err as { name?: string })?.name === "AbortError";
      setToast({ type: "error", message: isAbort ? "发布超时（90秒），请检查网络后重试" : "发布失败: " + String(err) });
    } finally {
      setPublishing(false);
    }
  }, [siteToken, siteUser, recording, recordingId, categories, pubTitle, pubApplications, pubCategory, pubTags, pubDownloadable, pubCoverBase64, pubSummary, translations, shotZoom, pubWorkspaceId, pubWorkspaces]);

  /** ★ 端云一体化 v247 对齐（对标插件 maybeAutoSaveDraft）：录制停止且已登录时，
   *   静默把教程存为网站草稿（status=draft，fire-and-forget）。
   *   去重：本地已记 `flowio_pub_<rid>`（发过/存过）或 `flowio_webslug_<rid>`（已有云端实体）⇒ 不动，
   *   避免把已发布的教程降级成草稿、或重复建草稿。
   *   失败（未登录 / 网络 / 校验）一律静默吞掉，绝不影响用户编辑。 */
  const autoSaveDraftQuiet = useCallback(async () => {
    if (!siteToken || !recordingId) return;
    try {
      if (localStorage.getItem(`flowio_pub_${recordingId}`)) return;
      if (localStorage.getItem(webSlugKey(recordingId))) return;
      const data = await invoke<RecordingWithSteps>("load_recording", { id: recordingId });
      if (!data || !data.steps?.length) return;
      const title = (recording?.title || data.recording.title || "未命名教程").trim();
      const app_name = (recording?.app_name || data.recording.app_name || "").trim();
      const steps = await buildUploadSteps(data.steps, (id) => shotZoomOf(id));
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), 90000);
      let res: Response;
      try {
        res = await fetch("https://flowio.cn/api/guides/upload", {
          method: "POST",
          headers: { "Content-Type": "application/json", Authorization: `Bearer ${siteToken}` },
          body: JSON.stringify({
            title,
            summary: "",
            app_name,
            category: "other",
            tags: [],
            software: [],
            description: "",
            visibility: "public",
            status: "draft",
            downloadable: "no",
            steps,
          }),
          signal: controller.signal,
        });
      } finally {
        clearTimeout(timer);
      }
      const result = await res.json().catch(() => ({}) as Record<string, unknown>);
      if (!res.ok || !result.ok || !result.guide) return; // 静默
      const g = result.guide as { id?: string; slug?: string };
      if (g.id) {
        try { localStorage.setItem(`flowio_pub_${recordingId}`, String(g.id)); } catch { /* 配额忽略 */ }
        if (g.slug) {
          try { localStorage.setItem(webSlugKey(recordingId), g.slug); } catch { /* 配额忽略 */ }
        }
      }
    } catch { /* 自动存草稿失败不阻塞用户 */ }
  }, [siteToken, recordingId, recording, shotZoom]);

  /* ★ 端云一体化 v247 对齐：录制停止进「成片预览」着陆且已登录 ⇒ 自动静默存草稿。
     fire-and-forget —— 上传在后台上，不阻塞预览/编辑；同一录制只触发一次。 */
  const autoDraftFiredRef = useRef(false);
  useEffect(() => {
    if (!previewLanding || !siteToken) return;
    if (autoDraftFiredRef.current) return;
    autoDraftFiredRef.current = true;
    void autoSaveDraftQuiet();
  }, [previewLanding, siteToken, autoSaveDraftQuiet]);

  /** ★ r65「在网页编辑」（对齐 Scribe）：把这篇录制交给 flowio.cn 的网页编辑器。
   *
   *  路线：本地记过云端 slug ⇒ 直接开（秒跳，不重传）；
   *        没记过 ⇒ 走 `handlePublish("draft", true)` 存一份草稿再开。
   *
   *  🔴 与扩展端 `src/shared/webEdit.ts` 同口径；三条纪律也同源：
   *    ① 绝不丢数据（端内编辑照旧）② 不说「发布失败」③ 未登录给下一步而不是报错。
   */
  const openInWebEditor = useCallback(async () => {
    if (!recordingId || publishing) return;
    if (!siteToken) { setToast({ type: "error", message: WEB_EDIT_NEED_LOGIN }); return; }
    let slug = "";
    try { slug = localStorage.getItem(webSlugKey(recordingId)) || ""; } catch { /* ignore */ }
    if (!slug) {
      // 还没往云端存过 ⇒ 存一份草稿，upload 成功后由 openWeb 分支接管跳转
      await handlePublish("draft", true);
      return;
    }
    const url = webEditUrl(slug);
    if (!url) { setToast({ type: "error", message: WEB_EDIT_OPEN_FAILED }); return; }
    try { await invoke("open_external_url", { url }); }
    catch (e) {
      console.warn("[flowio] [r65] 打开网页编辑失败:", e);
      setToast({ type: "error", message: WEB_EDIT_OPEN_FAILED });
    }
  }, [recordingId, siteToken, publishing, handlePublish]);

  /* ── 另存为副本 ──────────────────────────────────────────── */
  // 复制整篇教程为一份独立的新副本（标题加「 (副本)」后缀），
  // 副本有全新 recording id → 发布映射/编辑历史/精简快照全部隔离，改副本不伤原教程。
  const handleSaveAs = useCallback(async () => {
    if (!recordingId) return;
    try {
      const newId = await invoke<string>("duplicate_recording", { id: recordingId });
      setToast({ type: "success", message: "已另存为副本，正在打开副本..." });
      navigate(`/editor/${newId}`);
    } catch (err) {
      setToast({ type: "error", message: "另存为失败: " + String(err) });
    }
  }, [recordingId, navigate]);

  /* ── 隐私脱敏 ────────────────────────────────────────────── */

  // 文本脱敏：对步骤描述中的敏感信息执行替换
  const handleRedact = useCallback(async () => {
    // 🔴 脱敏功能总开关：关闭时提示先开启（后端同样强制拒绝，这里是友好提示）
    try {
      const cfg = await invoke<{ redact_before_export?: boolean }>("get_redaction_config");
      if (!cfg.redact_before_export) {
        setToast({ type: "error", message: "脱敏功能未开启：请到 设置 → 脱敏 打开「脱敏功能」" });
        return;
      }
    } catch { /* 配置读取失败 → 交给后端守卫兜底 */ }
    setRedacting(true);
    try {
      const res = await invoke<{ changed: number; droppedTranslations: number; changedIndices?: number[] }>(
        "redact_recording_text",
        { recordingId },
      );
      const list = await reloadRecording();
      refreshHist();
      if (res.changed > 0) {
        // ★ v310: 持久弹窗列出被处理的步骤（toast 几秒就没了，用户不知道改了哪些）
        const idxs = res.changedIndices ?? [];
        const lines = idxs.length
          ? idxs.map((oi) => {
              const i = list.findIndex((s) => (s as { order_index?: number }).order_index === oi);
              return i >= 0 ? `第 ${i + 1} 步 · ${list[i]?.title ?? ""}` : `步骤 ${oi + 1}`;
            })
          : [`共 ${res.changed} 个步骤的文本被脱敏`];
        if (res.droppedTranslations > 0) {
          lines.push(`另有 ${res.droppedTranslations} 条含敏感信息的旧译文已作废（导出时回退已脱敏原文）`);
        }
        setRedactResult({ title: "文本脱敏完成", lines });
      } else {
        setToast({ type: "success", message: "未检测到敏感信息" });
      }
    } catch (err) {
      setToast({ type: "error", message: "隐私脱敏失败: " + String(err) });
    } finally {
      setRedacting(false);
    }
  }, [recordingId, reloadRecording, refreshHist]);

  // 扫描全部截图中的敏感文字区域并自动脱敏（OCR）
  const showScanToast = useCallback((toast: { type: "success" | "error"; message: string }) => {
    setScanToast(toast);
    if (scanToastTimerRef.current) clearTimeout(scanToastTimerRef.current);
    scanToastTimerRef.current = window.setTimeout(
      () => setScanToast(null),
      toast.type === "error" ? 6000 : 4000
    );
  }, []);

  const handleScanRedact = useCallback(async () => {
    if (steps.length === 0) return;
    // 🔴 脱敏功能总开关：关闭时提示先开启（后端同样强制拒绝，这里是友好提示）
    try {
      const cfg = await invoke<{ redact_before_export?: boolean }>("get_redaction_config");
      if (!cfg.redact_before_export) {
        showScanToast({ type: "error", message: "脱敏功能未开启：请到 设置 → 脱敏 打开「脱敏功能」" });
        return;
      }
    } catch { /* 配置读取失败 → 交给后端守卫兜底 */ }
    if (scanToastTimerRef.current) clearTimeout(scanToastTimerRef.current);
    setScanRedacting(true);
    setScanProgress({ done: 0, total: 0, currentStepId: "" });
    setScanToast(null);
    // 进度监听（每次扫描重新注册，确保拿到完整进度）
    const unlisten = await listen<{ done: number; total: number; currentStepId: string }>(
      "scan_redact_progress",
      (ev) => {
        setScanProgress({ done: ev.payload.done, total: ev.payload.total, currentStepId: ev.payload.currentStepId });
      }
    );
    try {
      const cfg = await invoke<{ mode?: "blur" | "block" }>("get_redaction_config");
      const mode = cfg.mode === "block" ? "block" : "blur";
      // 后台执行：只发起，不阻塞。完成后由 scan_redact_done 事件收尾（刷新 + 弹窗）。
      await invoke("scan_redact_screenshots", { recordingId, mode });
    } catch (err) {
      setScanRedacting(false);
      setScanProgress({ done: 0, total: 0, currentStepId: "" });
      showScanToast({ type: "error", message: "扫描截图失败: " + String(err) });
    } finally {
      unlisten();
    }
  }, [recordingId, steps.length, showScanToast]);

  /* ── scan_redact_done（扫描脱敏完成 → 刷新+弹窗；后台执行，离开教程也照常收尾）── */
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<{ total: number; processed: number; redactedCount: number; failedCount: number; cancelled: boolean; redactedSteps?: number[] }>(
      "scan_redact_done",
      async (ev) => {
        const r = ev.payload;
        setScanRedacting(false);
        setScanProgress({ done: 0, total: 0, currentStepId: "" });
        const list2 = await reloadRecording().catch(() => null);
        if (r.cancelled) {
          showScanToast({ type: "success", message: "扫描已取消" });
        } else if (r.total === 0) {
          showScanToast({ type: "success", message: "没有可扫描的截图" });
        } else if (r.failedCount > 0 && r.redactedCount === 0) {
          showScanToast({ type: "error", message: "扫描完成，未检测到敏感信息" });
        } else if (r.redactedCount > 0) {
          // ★ v310: 持久弹窗列出被遮盖的步骤（不点 × 不消失）
          const idxs = r.redactedSteps ?? [];
          const lines = idxs.length
            ? idxs.map((oi) => {
                const i = list2?.findIndex((s) => (s as { order_index?: number }).order_index === oi) ?? -1;
                return i >= 0 ? `第 ${i + 1} 步 · ${list2?.[i]?.title ?? ""}` : `步骤 ${oi + 1}`;
              })
            : [`共 ${r.redactedCount} 张截图中的敏感区域被遮盖`];
          setRedactResult({ title: "截图扫描脱敏完成", lines });
          if (r.failedCount > 0) {
            showScanToast({ type: "error", message: `部分截图识别失败（${r.failedCount} 张已跳过）` });
          }
        } else if (r.failedCount > 0) {
          showScanToast({ type: "success", message: "扫描脱敏完成（部分失败）" });
        } else {
          showScanToast({ type: "success", message: "未检测到敏感信息" });
        }
      }
    ).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; unlisten?.(); };
  }, [recordingId, reloadRecording, showScanToast]);

  const handleCancelScanRedact = useCallback(async () => {
    try {
      await invoke("cancel_scan_redact");
    } catch (err) {
      console.error("cancel scan failed:", err);
    }
  }, []);

  // 组件卸载时清理扫描结果 toast 定时器
  useEffect(() => {
    return () => {
      if (scanToastTimerRef.current) clearTimeout(scanToastTimerRef.current);
    };
  }, []);

  /* ── AI Generate ─────────────────────────────────────────── */

  const handleAiGenerate = useCallback(async () => {
    setAiGenerating(true);
    try {
      const result: string = await invoke("generate_ai_steps", {
        recordingId,
        domainMode: domainMode === "auto" ? null : domainMode,
      });
      alert(result);
      await reloadRecording();
      refreshHist();
    } catch (err) {
      const msg = String(err);
      if (msg.includes("AI 配置") || msg.includes("配置 AI 服务商")) {
        const { confirm } = await import("@tauri-apps/plugin-dialog");
        const goSettings = await confirm("尚未配置 AI 服务商，是否前往设置页配置？", {
          title: "AI 生成",
          okLabel: "前往设置",
          cancelLabel: "取消",
        });
        if (goSettings) {
          navigate("/settings");
        }
      } else {
        alert(`AI 生成失败：${msg}`);
      }
    } finally {
      setAiGenerating(false);
    }
  }, [recordingId, reloadRecording, refreshHist]);

  /* ── Lightbox helpers ────────────────────────────────────── */

  const openLightbox = (index: number) => {
    setLightboxIndex(index);
    setLightboxMarked(false);
    setLightboxOpen(true);
  };

  const lightboxPrev = () =>
    setLightboxIndex((i) => Math.max(0, i - 1));
  const lightboxNext = () =>
    setLightboxIndex((i) => Math.min(realSteps.length - 1, i + 1));

  /* ── Annotation (手动标注) ───────────────────────────────── */

  /** 打开标注编辑器，固定编辑 after 全图（screenshot_base64） */
  const openAnnotate = (stepId: string) => {
    const s = steps.find((st) => st.id === stepId);
    if (!s) return;
    const base64 = s.screenshot_base64;
    if (!base64) {
      setToast({ type: "error", message: "该步骤没有可标注的截图" });
      return;
    }
    setAnnotateStepId(stepId);
    setAnnotateOpen(true);
  };

  /**
   * 标注完成：
   * - 有圈选裁切（hasCrop）：「注释 + 裁切」的结果顶替全图（screenshot_path），
   *   并清空裁图锚点 —— 已经没有独立的「裁图」槽位了，用户圈的就是新的主图。
   *   原全图不保留（应用内可撤销）。
   * - 仅标注无裁切：注释烧回全图。
   */
  const handleAnnotateSave = async (
    _annos: Annotation[],
    result?: { full?: string; anchor?: CropAnchor | null; cropped?: string | undefined },
  ) => {
    const full = result?.full;
    const anchor = result?.anchor ?? null;
    const cropped = result?.cropped;
    const hasCrop = !!anchor && !!cropped;
    if (!full && !anchor && !cropped) {
      setAnnotateOpen(false);
      return;
    }
    const stepId = annotateStepId;
    try {
      if (hasCrop) {
        // 圈选裁切 → 裁切结果顶替全图，并清空裁图锚点。
        // 5b 对齐（插件「手调框落 cropRect 不改锚点」）：手调框的 cropRect 直接
        // 按像素烧录为最终裁切结果（cropped 固化进 full 槽位），锚点被清空而
        // 不是被手调框反算覆盖 —— 「重置为自动」仅存在于编辑器内
        // （AnnotationsOverlay 持 initialAnchor 时回到 cropBoxFromAnchor 50% 窗口）。
        const base64 = cropped!.split(",")[1] || "";
        if (base64) {
          await invoke("save_step_screenshot_full", { stepId, base64Data: base64 });
        }
        await invoke("update_step_crop", { stepId, x: null, y: null, cropW: null, cropH: null });
      } else if (full) {
        // 仅标注（无裁切）→ 注释烧回全图
        const base64 = full.split(",")[1] || "";
        if (base64) {
          await invoke("save_step_screenshot_full", { stepId, base64Data: base64 });
        }
      }
      // 重新从库加载步骤（screenshot_base64 与锚点可能都变了）
      await reloadRecording();
      setToast({ type: "success", message: "标注已保存" });
      refreshHist();
    } catch (err) {
      console.error("保存标注失败:", err);
      setToast({ type: "error", message: "保存标注失败" });
    }
    setAnnotateOpen(false);
  };

  /* ── 复刻插件「统一改变截图大小」：顶栏「图片 X%」按钮 ──────────
      插件口径（editor.tsx r489）：50% = 重点部位占整张图的一半（cropRect 归一化边长），
      顶栏按钮循环档位 100/75/50/25，落点是每步截图本身（桌面端烧录模型，
      批量按锚点中心裁切后顶替主图；save_step_screenshot_full 自带 hist 快照可撤销）。 */

  const SHOT_SCALE_STEPS = [100, 75, 50, 25] as const;

  const nextShotScalePct = (cur: number | null): number => {
    if (cur == null) return 75;
    const idx = SHOT_SCALE_STEPS.indexOf(cur as (typeof SHOT_SCALE_STEPS)[number]);
    return SHOT_SCALE_STEPS[(idx < 0 ? 0 : idx + 1) % SHOT_SCALE_STEPS.length];
  };

  const [shotScalePctNow, setShotScalePctNow] = useState<number | null>(null);
  const [shotScaleBusy, setShotScaleBusy] = useState(false);
  // ★ 防并发重入：state 更新是异步的，快速连点会在同一帧穿过 busy 检查，
  //   并发执行多轮「8 步 invoke + reloadRecording」→ SQLite 争用 / 内存暴涨 → 界面假死。
  //   用 ref 同步加锁，点击瞬间即可挡住后续重入。
  const shotScaleBusyRef = useRef(false);

  /** 读取 base64 图片真实像素尺寸（screenshot_base64 为无前缀裸 base64）。 */
  const readImageSize = (b64: string): Promise<{ width: number; height: number } | null> =>
    new Promise((resolve) => {
      const img = new Image();
      img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
      img.onerror = () => resolve(null);
      img.src = b64.startsWith("data:") ? b64 : `data:image/jpeg;base64,${b64}`;
    });

  /**
   * 实时统计整篇共同比例：以真实主图尺寸 / before 整图尺寸为准。
   * 统一比例烧录 / 手动标注裁剪后 x/y 锚点不代表真实比例（x/y 恒在、尺寸已变），
   * 只有实际像素能反映 —— 全部一致返回该百分比，不一致返回 null（顶栏显示「混合」）。
   */
  const [liveShotScalePct, setLiveShotScalePct] = useState<number | null>(null);
  // 每步「真实比例」（主图像素 / before 整图像素，无 before 时回退锚点口径）。
  // 与显示倍率解耦：真实比例只依赖数据（steps），读图尺寸较慢，单独缓存。
  const [perShotPcts, setPerShotPcts] = useState<Record<string, number | null>>({});
  useEffect(() => {
    let cancelled = false;
    const targets = steps.filter(
      (s) => !isDocBlock(s) && !!s.screenshot_base64,
    );
    if (targets.length === 0) {
      setPerShotPcts({});
      return;
    }
    (async () => {
      const map: Record<string, number | null> = {};
      for (const s of targets) {
        const main = await readImageSize(s.screenshot_base64 as string);
        if (!main || main.width <= 0) { map[s.id] = null; continue; }
        if (s.before_screenshot_base64) {
          const base = await readImageSize(s.before_screenshot_base64);
          if (base && base.width > 0) {
            // 等比裁剪宽高同比例，取宽度比四舍五入到整数百分比
            map[s.id] = Math.round((main.width / base.width) * 100);
            continue;
          }
        }
        // 无 before 整图（老数据 / 上传图）：回退锚点口径
        map[s.id] = s.x != null && s.y != null ? 50 : 100;
      }
      if (cancelled) return;
      setPerShotPcts(map);
    })();
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [steps]);

  // 显示口径 = 真实比例 × 显示倍率（−/+ 微调也算「图片大小」，
  // 某步倍率被改后各步显示大小不一致 → 顶栏同样显示「混合」）。
  // 全部一致时优先展示真实比例（与顶栏统一比例档位 100/75/50/25 语义一致）。
  useEffect(() => {
    const targets = steps.filter(
      (s) => !isDocBlock(s) && !!s.screenshot_base64,
    );
    if (targets.length === 0) {
      setLiveShotScalePct(null);
      return;
    }
    const shown: (number | null)[] = targets.map((s) => shotDispOf(s.id));
    const validShown = shown.filter((p): p is number => p != null);
    if (validShown.length === 0) { setLiveShotScalePct(null); return; }
    const firstShown = validShown[0];
    if (!validShown.every((p) => p === firstShown)) {
      setLiveShotScalePct(null);
      return;
    }
    // 显示一致：优先展示真实比例（未动 −/+ 时按钮显示的就是真实比例）
    const realVals = targets.map((s) => perShotPcts[s.id]).filter((p): p is number => p != null);
    if (realVals.length > 0 && realVals.every((p) => p === realVals[0])) {
      setLiveShotScalePct(realVals[0]);
    } else {
      setLiveShotScalePct(firstShown);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [perShotPcts, shotZoom, showFullShot, shotAtGlobalPct]);

  // 只在未初始化时用统计值兜底；用户点过档位后保持用户选择，刷新页面回统计值
  useEffect(() => {
    if (shotScalePctNow == null) setShotScalePctNow(liveShotScalePct);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liveShotScalePct]);

  const onShotScaleAll = async (pct: number) => {
    if (shotScaleBusyRef.current) return;
    shotScaleBusyRef.current = true;
    setShotScaleBusy(true);
    // 统一大小：100% 档还原整图并清空锚点（导出端据此判定整图），只处理有锚点步；
    // 75/50/25 档处理所有有截图步 —— 有锚点按落点裁剪，无锚点（整窗截图 / 刚还原整图）
    // 由 cropBase64ByAnchorFrac 以截图中心兜底，保证 100→75→50→25→100 循环可重入，
    // 不被「100% 清锚点」破坏（修复：连点多下后提示"已经是 X%"不再推进档位）。
    const fullPct = pct === 100;
    // ★ 100% 档 targets 与 75/50/25 档一致：所有有截图步（不再要求有锚点）。
    //   之前 100% 只筛「有锚点步」：100% 清锚点后（或 100→75→50→25 循环后
    //   锚点已空）再点 100% ⇒ targets 为空，主图文件不被还原，仅按钮推进到
    //   "100%" —— 用户看到「图片 100% 但主图仍是 25% 尺寸」。
    //   现在 100% 对所有有截图步写回 before 整图并清锚点，25→100 循环必然还原。
    const targets = steps.filter(
      (s) => !isDocBlock(s) && !!s.id && !!s.screenshot_base64,
    );
    const centerCount = targets.filter((s) => s.x == null || s.y == null).length;
    let done = 0;
    let skipped = 0;
    try {
      const frac = pct / 100;
      for (const s of targets) {
        // 以录制时刻 before 全屏原图为基准（与主图同尺寸、坐标兼容），
        // 不再用降质内存 base64（downscale 到 1280 宽），避免越裁越小。
        const raw = await invoke<string>("read_step_before_raw_base64", {
          stepId: s.id,
        });
        if (!raw) { skipped += 1; continue; }
        if (frac >= 1) {
          // 100% = 全图：把 before 原图写回主图（走 hist 快照，路径不变），
          // 并清空 x/y/crop 锚点 —— 插件端等价 viewMode=full；
          // 桌面端导出/AI 消费端 crop_region_50 只要见到 x/y 锚点就恒裁 50%
          // 重点图特写，只写回整图不清锚点 ⇒ 导出的还是"非100%的重点图"。
          await invoke("save_step_screenshot_full", {
            stepId: s.id,
            base64Data: raw,
          });
          await invoke("update_step_crop", {
            stepId: s.id,
            x: null,
            y: null,
            cropW: null,
            cropH: null,
          });
        } else {
          // 有锚点按落点裁剪；无锚点（整图）传 null → cropBase64ByAnchorFrac 以截图中心兜底
          const out = await cropBase64ByAnchorFrac(
            raw,
            s.x != null && s.y != null ? { x: s.x, y: s.y } : null,
            frac,
          );
          if (!out) { skipped += 1; continue; }
          await invoke("save_step_screenshot_full", {
            stepId: s.id,
            base64Data: out,
          });
        }
        done += 1;
      }
      await reloadRecording();
      refreshHist();
      // ★ 统一比例后把参与步的显示倍率一并复位：数据层已写回统一尺寸的截图，
      //   但显示倍率 shotZoom（localStorage flowio_view_<recordingId>）仍是旧放大值，
      //   主图 <img> 按 transform: scale(zoom) 渲染 ⇒ 各步画面大小仍不一致，
      //   顶栏统计（真实比例 × 显示倍率）仍显示「混合」，用户困惑"统一了怎么还混合"。
      //   所有档位统一后显示倍率复位为 1（图片完整填满截图框 = 框显示 100%），
      //   让数据与显示两层都一致；100% 档同时保证「图片 100% 且主图是整图」。
      setShotZoom((prev) => {
        const next: Record<string, number> = {};
        const z = 1;
        for (const s of targets) next[s.id] = z;
        return { ...prev, ...next };
      });
      // ★ 一并退出参与步的「查看全图」态并清掉中心缩放模式：
      //   showFullShot 的步显示 before 整图（scale 1）、shotZoomCenter 的步以
      //   截图中心为缩放原点，二者都会让该步显示大小/取景与其他步不一致，
      //   只复位 shotZoom 时用户仍会看到「某步还放大着/对不上」。统一后
      //   所有参与步回落到统一的当前比例 × 统一倍率，取景原点也归一。
      setShowFullShot((prev) => {
        let changed = false;
        const next = { ...prev };
        for (const s of targets) if (next[s.id]) { next[s.id] = false; changed = true; }
        return changed ? next : prev;
      });
      setShotZoomCenter((prev) => {
        let changed = false;
        const next = { ...prev };
        for (const s of targets) if (next[s.id]) { next[s.id] = false; changed = true; }
        return changed ? next : prev;
      });
      // ★ 一并退出参与步的「顶栏全局比例态」：顶栏态显示主图裁剪图原样填满框，
      //   统一后数据层已写回新的统一尺寸，若仍停留在顶栏态会让各步显示大小不一致，
      //   统一后所有参与步回落到统一的当前比例 × 统一倍率，取景原点也归一。
      setShotAtGlobalPct((prev) => {
        let changed = false;
        const next = { ...prev };
        for (const s of targets) if (next[s.id]) { next[s.id] = false; changed = true; }
        return changed ? next : prev;
      });
      setShotScalePctNow(pct);
      const suffix = centerCount > 0 ? `（其中 ${centerCount} 张无锚点整图按中心取景）` : "";
      if (pct === 100) {
        setToast({
          type: "success",
          message:
            done > 0
              ? `已把 ${done} 张图还原为整图（100%）${suffix}`
              : skipped > 0
                ? `没有可还原的截图（${skipped} 张读取失败）`
                : "所有截图已经是整图了",
        });
      } else if (done > 0) {
        setToast({
          type: "success",
          message: `已把 ${done} 张图统一成 ${pct}%${suffix}`,
        });
      } else {
        setToast({
          type: "success",
          message:
            skipped > 0
              ? `没有可统一的截图（${skipped} 张读取失败或保持原样）`
              : "没有可统一的截图",
        });
      }
    } catch (err) {
      console.error("统一截图大小失败:", err);
      setToast({ type: "error", message: "统一截图大小失败，请重试" });
    } finally {
      shotScaleBusyRef.current = false;
      setShotScaleBusy(false);
    }
  };

  /* ── Portal toolbar into titlebar ─────────────────────── */

  const titlebarLeft = (
    <div style={{ display: "flex", alignItems: "center", gap: 8, height: "100%", overflow: "hidden" }}>
      {/* 退回录制列表：一枚 ← 箭头，不再占文字宽度（顶栏左侧留给撤销/重做与标题） */}
      <button
        className="titlebar-back-btn"
        onClick={() => navigate("/")}
        data-tip={"返回录制列表\n改动已自动保存"}
      >
        <ArrowLeft size={16} />
      </button>
      {/* 全局撤销/重做（WPS 式）：可撤销标题/描述编辑、删步、精简、合并、改图等一切编辑 */}
      <div style={{ display: "flex", alignItems: "center", gap: 2 }} onMouseDown={(e) => e.stopPropagation()}>
        <button
          onClick={() => void applyHist("edit_undo")}
          disabled={histBusy || hist.undo === 0}
          data-tip={"撤销 Ctrl+Z\n返回上一步编辑（可连续多次）\n编辑标题/描述、删步、精简、合并都可撤销"}
          style={{ background: "transparent", border: "none", color: hist.undo === 0 ? "#5b6470" : "#e6edf3", cursor: hist.undo === 0 ? "default" : "pointer", padding: "4px 6px", borderRadius: 4, display: "flex", alignItems: "center", opacity: hist.undo === 0 ? 0.55 : 1 }}
        >
          {histBusy ? <Loader2 size={14} className="spinner" /> : <Undo2 size={14} />}
        </button>
        <button
          onClick={() => void applyHist("edit_redo")}
          disabled={histBusy || hist.redo === 0}
          data-tip={"恢复 Ctrl+Y\n重做刚被撤销的操作"}
          style={{ background: "transparent", border: "none", color: hist.redo === 0 ? "#5b6470" : "#e6edf3", cursor: hist.redo === 0 ? "default" : "pointer", padding: "4px 6px", borderRadius: 4, display: "flex", alignItems: "center", opacity: hist.redo === 0 ? 0.55 : 1 }}
        >
          <Redo2 size={14} />
        </button>
      </div>
      {recTitleEditing ? (
        <input
          ref={recTitleInputRef}
          autoFocus
          maxLength={30}
          value={recTitleDraft}
          onChange={(e) => setRecTitleDraft(e.target.value)}
          onBlur={async () => {
            const trimmed = recTitleDraft.trim();
            if (!trimmed) { setRecTitleEditing(false); return; }
            if (trimmed === recording?.title) { setRecTitleEditing(false); return; }
            try {
              await invoke("update_recording_title", { id: recordingId, title: trimmed });
              setRecording((prev) => prev ? { ...prev, title: trimmed } : prev);
              refreshHist();
              setRecTitleEditing(false);
            } catch (err) {
              setToast({ type: "error", message: String(err) });
              setTimeout(() => recTitleInputRef.current?.focus(), 0);
            }
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") { setRecTitleDraft(recording?.title || ""); setRecTitleEditing(false); }
          }}
          style={{ color: "#111827", fontSize: 14, fontWeight: 600, padding: "2px 6px", border: "1px solid #ff6b35", borderRadius: 4, outline: "none", width: 200, background: "#fff", marginLeft: "auto" }}
        />
      ) : (
        <span
          data-tauri-drag-region
          onClick={() => { setRecTitleDraft(recording?.title || ""); setRecTitleEditing(true); }}
          style={{ color: "#e6edf3", fontSize: 14, fontWeight: 600, userSelect: "none", cursor: "pointer", padding: "2px 6px", borderRadius: 4, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0, flexShrink: 1, marginLeft: "auto" }}
          data-tip={recording?.title || "Untitled"}
          title={recording?.title || ""}
        >
          {recording?.title || "Untitled"}
        </span>
      )}
    </div>
  );

  const titlebarActions = (
    <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
      {/* 编辑器双模式切换：简洁（默认，不调模型）/ 复杂（开放 AI 功能） */}
      <div
        className="ed-mode-switch"
        style={{ display: "flex", alignItems: "center", borderRadius: 6, border: "1px solid #2b3340", overflow: "hidden", fontSize: 12 }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <button
          onClick={() => switchEditorMode("simple")}
          data-tip={"简洁模式\n纯本地编辑，不调模型、不耗积分（默认）"}
          style={{ border: "none", cursor: "pointer", padding: "3px 9px", fontSize: 12, background: !isComplex ? "#ff6b35" : "transparent", color: !isComplex ? "#fff" : "#9aa4b2" }}
        >
          简洁
        </button>
        <button
          onClick={() => switchEditorMode("complex")}
          data-tip={"复杂模式\n开放 AI 功能（AI 精简 / 生成 / 扫描脱敏 / 翻译 / 摘要重生成），按调用消耗积分"}
          style={{ border: "none", borderLeft: "1px solid #2b3340", cursor: "pointer", padding: "3px 9px", fontSize: 12, background: isComplex ? "#ff6b35" : "transparent", color: isComplex ? "#fff" : "#9aa4b2" }}
        >
          复杂
        </button>
      </div>
      {/* ★ 复刻插件「统一改变截图大小」：图片 X% —— 一条按钮循环档位 100/75/50/25，
          一键把所有步骤的重点图统一成同一比例（Ctrl+Z 可整篇撤销）。 */}
      {!previewMode && steps.length > 0 && (
        <button
          onClick={() => void onShotScaleAll(nextShotScalePct(shotScalePctNow))}
          disabled={shotScaleBusy}
          data-tip={"统一改变截图大小\n整篇所有步骤的重点图占整张图的比例（100% 整图 / 75% / 50% / 25%，点一下进下一档；重点部位会占满显示区域）；各步比例不一致时显示『混合』"}
          style={{
            background: "transparent", border: "none",
            color: liveShotScalePct != null ? "#ff6b35" : "#e6edf3",
            cursor: shotScaleBusy ? "default" : "pointer", padding: "3px 8px",
            borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4,
            opacity: shotScaleBusy ? 0.6 : 1,
          }}
        >
          {shotScaleBusy ? "处理中…" : `图片 ${liveShotScalePct != null ? `${liveShotScalePct}%` : "混合"}`}
        </button>
      )}
      {/* 精简 / 执行：第十五轮从左侧目录栏搬上来（顶栏本来就只有它俩是「对整篇动手」的
          重度操作，跟导出/发布同一层）。文字按钮，不带图标，与顶栏其余按钮一致。 */}
      {!previewMode && steps.length > 0 && (
        <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
          <button
            onClick={() => { setSimplifyOpen(!simplifyOpen); setExportOpen(false); setToolsOpen(false); setTranslateOpen(false); }}
            disabled={simplifying !== null || aiBusy}
            data-tip={"把啰嗦的步骤合并掉\n录完会自动跑一遍；这里也可以手动重跑或换用 AI"}
            style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: simplifying !== null || aiBusy ? "default" : "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4, opacity: simplifying !== null || aiBusy ? 0.6 : 1 }}
          >
            {aiBusy ? "AI 精简中…" : simplifying === "rules" ? "规则合并中…" : "精简"} <ChevronDown size={12} />
          </button>
          {simplifyOpen && (
            <div className="export-dropdown step-simplify-menu">
              {isComplex && (
              <button
                className="export-dropdown-item"
                onClick={() => { setSimplifyOpen(false); void handleAiSimplify(); }}
                disabled={simplifying !== null || aiBusy}
                data-tip={"AI 精简（推荐）\n先跑本地规则，再让 AI 按语义把重复步骤压成一步\n能处理规则看不懂的情况（比如弹框里坐标各异的一串点击），需要几秒到十几秒"}
              >
                {aiBusy ? <Loader2 size={14} className="spinner" /> : <Sparkles size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">AI 精简（推荐）</span>
              </button>
              )}
              <button
                className="export-dropdown-item"
                data-tip={"仅规则合并\n只跑本地规则：重复点击→双击、连续输入合并、删掉误点到自己窗口的末步\n不联网、瞬间完成，但看不懂语义上的重复"}
                onClick={() => { setSimplifyOpen(false); void handleAutoMerge(); }}
                disabled={simplifying !== null}
              >
                {simplifying === "rules" ? <Loader2 size={14} className="spinner" /> : <Wand2 size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">仅规则合并</span>
              </button>
            </div>
          )}
        </div>
      )}
      {!previewMode && AUTOMATION_ENABLED && steps.length > 0 && (
        <button
          onClick={() => handleExecute()}
          disabled={executing}
          data-tip={"精准执行\n按录制时的屏幕坐标确定性重放（非 AI 猜点），在目标窗口自动点选；每步后截图校验"}
          style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: executing ? "default" : "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          执行{executing ? "中…" : ""}
        </button>
      )}
      {!previewMode && (
        <button
          onClick={() => void handleSaveAs()}
          data-tip="另存为副本：复制一份完全相同的教程（含截图），改副本不会破坏原教程"
          style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          另存为
        </button>
      )}
      <div style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
        <button
          onClick={() => { setExportOpen(true); setToolsOpen(false); setTranslateOpen(false); setSimplifyOpen(false); }}
          disabled={exporting}
          data-tip={"导出教程\n导出为 Word / PDF / PPT / 视频 / 图片等；要用译文导出，先在「翻译」里选好导出语言"}
          style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          {exportProgress ? exportProgress : exporting ? "导出中..." : "导出"}
        </button>
      </div>
      {/* 执行/演练/执行历史 已移至精简下拉后面 */}
      {!previewMode && (
        <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
          <button
            onClick={() => { setToolsOpen(!toolsOpen); setExportOpen(false); setTranslateOpen(false); setSimplifyOpen(false); }}
            data-tip={"更多工具\n上传网站、AI 生成步骤说明、敏感信息脱敏"}
            style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
          >
            工具 <ChevronDown size={12} />
          </button>
          {toolsOpen && (
            <div className="export-dropdown" style={{ right: 0, zIndex: 9999 }}>
              <button
                className="export-dropdown-item"
                onClick={async () => {
                  setToolsOpen(false);
                  if (!siteToken) {
                    try {
                      await invoke("open_external_url", { url: "https://flowio.cn/author/login?redirect=flowio://auth" });
                    } catch (err) {
                      setToast({ type: "error", message: "打开登录页失败: " + String(err) });
                    }
                    return;
                  }
                  setShowPublishDialog(true);
                  setPubStep("pick");
                  // ★ T9：拉「可发布的团队空间」。取不到**不算错** —— 发布到网站是主线，团队库是附加。
                  listMyWorkspaces(siteToken)
                    .then((list) => {
                      const arr = Array.isArray(list) ? list : [];
                      setPubWorkspaces(arr);
                      // 🔴 快照里选过的空间若已不在可发布列表里（被移出空间 / 降成 viewer）⇒
                      //    回落到「不发布」。否则这次发布会挂到一个已经不存在的目标上，白失败一次。
                      setPubWorkspaceId((cur) => (cur && !arr.some((w) => w.id === cur) ? "" : cur));
                    })
                    .catch(() => setPubWorkspaces([]));
                  // ★ 恢复上次发布填写的表单信息（重发免重填）；无快照时用默认值
                  let pubFormRestored = false;
                  /** ★ T9：快照里选过的团队空间 */
                  let pubWsSavedId = "";
                  try {
                    const raw = localStorage.getItem(`flowio_pubform_${recordingId}`);
                    if (raw) {
                      const f = JSON.parse(raw);
                      setPubTitle(f.title || recording?.title || "");
                      setPubApplications(Array.isArray(f.applications) ? f.applications : []);
                      setPubCategory(f.category || "");
                      setPubTags(Array.isArray(f.tags) ? f.tags : []);
                      setPubDownloadable(f.downloadable === "no" ? "no" : "yes");
                      setPubSummary(f.scene || "");
                      setPubCoverBase64(f.coverBase64 || null);
                      setPubCoverPreview(f.coverBase64 || null);
                      pubWsSavedId = typeof f.workspaceId === "string" ? f.workspaceId : "";
                      pubFormRestored = true;
                    }
                  } catch { /* 快照损坏则忽略 */ }
                  if (!pubFormRestored) {
                    setPubTitle(recording?.title || "");
                    setPubApplications([]);
                    setPubCategory("");
                    setPubTags([]);
                    setPubDownloadable("yes");
                    setPubSummary("");
                    setPubCoverBase64(null);
                    setPubCoverPreview(null);
                  }
                  setPubAppInput("");
                  setPubTagInput("");
                  setPubWorkspaceId(pubWsSavedId);
                }}
                data-tip={siteToken
                  ? "把这篇教程上传到 flowio.cn\n可先存草稿，也可填写标题/应用/分类后直接发布"
                  : "登录后才能上传到 flowio.cn（点击去登录）"}
              >
                <Upload size={14} className="export-dropdown-item-icon" />
                <span className="export-dropdown-item-label">{siteToken ? "上传网站" : "登录后上传"}</span>
              </button>
              {/* 另存为已移至顶栏按钮，此处不重复 */}
              {isComplex && (
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); handleAiGenerate(); }}
                disabled={aiGenerating}
                data-tip={"AI 生成步骤说明\n让 AI 看每一步的截图，自动写出标题和操作描述；已有的描述会被覆盖"}
              >
                {aiGenerating ? <Loader2 size={14} className="spinner" /> : <Sparkles size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">{aiGenerating ? "生成中..." : "AI 生成"}</span>
                <select
                  value={domainMode}
                  onChange={(e) => setDomainMode(e.target.value as typeof domainMode)}
                  onClick={(e) => e.stopPropagation()}
                  style={{ cursor: "pointer", fontSize: 11, padding: "1px 4px", borderRadius: 4, border: "1px solid #555", background: "#333", color: "#ccc", marginLeft: 4 }}
                >
                  <option value="auto">自动</option>
                  <option value="spreadsheet">表格</option>
                  <option value="browser">浏览器</option>
                  <option value="code">代码</option>
                  <option value="terminal">终端</option>
                </select>
              </button>
              )}
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); handleRedact(); }}
                disabled={redacting}
                data-tip={"文本脱敏\n把步骤文字里的手机号、身份证、邮箱、银行卡等换成占位符（截图不变）"}
              >
                {redacting ? <Loader2 size={14} className="spinner" /> : <ShieldCheck size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">{redacting ? "脱敏中..." : "文本脱敏"}</span>
              </button>
              {isComplex && (
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); handleScanRedact(); }}
                disabled={scanRedacting || steps.length === 0}
                data-tip={"扫描截图脱敏\n用 OCR 找出截图画面里的敏感信息并打码，比「文本脱敏」更彻底，也更慢"}
                style={{ opacity: steps.length === 0 ? 0.5 : 1 }}
              >
                {scanRedacting ? <Loader2 size={14} className="spinner" /> : <ScanSearch size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">{scanRedacting ? "扫描中..." : "扫描脱敏"}</span>
              </button>
              )}
              {/* ★ v310: 脱敏设置入口（与插件端对齐）——跳设置页脱敏 tab */}
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); navigate("/settings#redaction"); }}
                data-tip={"脱敏设置\n选择检测哪些类别的敏感信息（身份证/手机号/邮箱…），以及自定义正则规则"}
              >
                <SettingsIcon size={14} className="export-dropdown-item-icon" />
                <span className="export-dropdown-item-label">脱敏设置…</span>
              </button>
            </div>
          )}
        </div>
      )}

      {/* 翻译：选择语言直接翻译，完成后同步到编辑器步骤 */}
      {!previewMode && isComplex && (
        <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
          <button
            onClick={() => { setTranslateOpen(!translateOpen); setExportOpen(false); setToolsOpen(false); setSimplifyOpen(false); }}
            disabled={translating}
            data-tip={previewMode
              ? "切换预览语言\n在这个下拉里选「原文」或某种译文，预览会立刻跟着换"
              : "翻译\n点语言把标题和每步描述翻译过去；已翻译过的语言点一下立即切换，不用重新翻译\n选「原文」随时切回翻译前的文字\n译文存成副本，也决定导出语言"}
            style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4, opacity: translating ? 0.6 : 1 }}
          >
            {translating && <Loader2 size={14} className="spinner" />} 翻译 <ChevronDown size={12} />
          </button>
          {translateOpen && (
            <div className="export-dropdown" style={{ right: 0, zIndex: 9999 }}>
              <div className="export-dropdown-section">
                {/* 原文：恢复翻译前的快照（首次翻译时由后端自动保存） */}
                <button
                  className={`export-dropdown-item${appliedLang === "orig" ? " active" : ""}`}
                  onClick={() => { void handleLangClick(ORIG_KEY); }}
                  data-tip={translations[ORIG_KEY]
                    ? "立即切回翻译前的原文（不重新翻译）"
                    : "该教程还没有原文快照\n下次翻译时会自动保存一份"}
                >
                  <span className="export-dropdown-item-icon">📄</span>
                  <span className="export-dropdown-item-label">原文</span>
                </button>
                {/* 原生即中文，zh 不作为可翻译目标出现在列表（存量 zh 译文仍可经后端切换） */}
                {Object.entries(LANG_LABELS).filter(([code]) => code !== "zh").map(([code, label]) => (
                  <button
                    key={code}
                    className={`export-dropdown-item${appliedLang === code ? " active" : ""}`}
                    onClick={() => { void handleLangClick(code); }}
                    data-tip={translations[code]
                      ? `立即切换到${label}译文（已有缓存，不用重新翻译）`
                      : `翻译成${label}（将替换编辑器中的文字，原文自动快照）`}
                  >
                    <span className="export-dropdown-item-icon">{translations[code] ? "✅" : "🌐"}</span>
                    <span className="export-dropdown-item-label">{label}</span>
                  </button>
                ))}
              </div>
              {appliedLang && appliedLang !== "orig" && translations[appliedLang] && (
                <div className="export-dropdown-section">
                  <button
                    className="export-dropdown-item"
                    onClick={() => { setTranslateOpen(false); void handleTranslate(appliedLang); }}
                    data-tip={`丢弃${LANG_LABELS[appliedLang] || appliedLang}缓存译文，按当前步骤文字重新翻译（较慢）`}
                  >
                    <span className="export-dropdown-item-icon">↻</span>
                    <span className="export-dropdown-item-label">重新翻译</span>
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
      )}

      <button
        onClick={() => {
          if (previewMode) {
            // 出预览 = 进编辑；若来自录制着陆（?preview=1）顺带清掉 URL 参数
            if (previewLanding) exitPreviewLanding();
            else setPreviewMode(false);
          } else {
            setPreviewMode(true);
          }
        }}
        data-tip={previewMode ? "进入编辑\n退出成片预览，回到编辑器" : "预览成品效果\n按读者看到的样子通篇预览（导出/上传前先看一眼）"}
        style={{ background: "#ff6b35", border: "none", color: "#fff", cursor: "pointer", padding: "4px 12px", borderRadius: 4, fontSize: 13, fontWeight: 600, display: "flex", alignItems: "center", gap: 4 }}
      >
        {previewMode ? "编辑" : "预览"}
      </button>

    </div>
  );

  /* ── Render ──────────────────────────────────────────────── */

  /* 预览模式取文案：翻译已直接同步到步骤，预览直接用步骤当前文本 */
  const previewStepText = (step: Step): { title: string; description: string } => {
    return { title: step.title, description: step.description || "" };
  };
  /* ★ v0.65.244 对齐：成片体检汇总（只对真步骤体检；内容块/摘要/简介天然豁免） */
  const health = useMemo(() => healthSummaryOf(realSteps), [realSteps]);
  const healthStepIdOf = (n: number): string | null => realSteps.find((s) => s.step_number === n)?.id ?? null;
  /** 体检条点问题步骤 chip → 滚到该步骤（右侧成片正文） */
  const jumpToHealthStep = (n: number) => {
    const id = healthStepIdOf(n);
    if (!id) return;
    const el = document.getElementById(`sop-step-${id}`);
    if (el) el.scrollIntoView({ behavior: "smooth", block: "start" });
    const left = document.getElementById(`step-item-${id}`);
    if (left) left.scrollIntoView({ behavior: "smooth", block: "nearest" });
  };
  const healthChips = useMemo(() => {
    const chips: { step: number; level: "error" | "warn"; label: string }[] = [];
    for (const i of health.issues) {
      if (chips.length > 0 && chips[chips.length - 1].step === i.step) continue;
      chips.push({ step: i.step, level: i.level, label: healthLabel(i.code) });
    }
    return chips;
  }, [health]);

  /* ── Loading / Error（★ 必须位于所有 hook 之后：提前 return 会跳过下方
     useMemo，导致渲染轮次间 hook 数量不一致，触发 React #310 白屏） ── */

  if (loading) {
    return (
      <div className="editor-loading">
        <Loader2 size={32} className="spinner" />
        <p>加载录制数据...</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="editor-loading">
        <p className="text-red">加载失败：{error}</p>
        <button className="btn mt-3" onClick={() => navigate("/")} data-tip="返回录制列表">
          <ArrowLeft size={14} /> 返回 Dashboard
        </button>
      </div>
    );
  }

  return (
    <div className="editor-layout">
      {/* Portal toolbar into titlebar */}
      {createPortal(titlebarLeft, document.getElementById("titlebar-editor-left")!)}
      {createPortal(titlebarActions, document.getElementById("titlebar-editor-actions")!)}

      {/* 截图扫描脱敏进度条 + 取消 */}
      {scanRedacting && (
        <div
          style={{
            position: "fixed",
            top: 70,
            left: "50%",
            transform: "translateX(-50%)",
            zIndex: 9998,
            background: "#fff",
            border: "1px solid #ff6b35",
            borderRadius: 8,
            padding: "10px 14px",
            boxShadow: "0 4px 16px rgba(0,0,0,0.15)",
            display: "flex",
            alignItems: "center",
            gap: 12,
            minWidth: 380,
            maxWidth: "70vw",
          }}
        >
          <Loader2 size={16} className="spinner" style={{ color: "#ff6b35", flexShrink: 0 }} />
          <div style={{ flex: 1 }}>
            <div style={{ fontSize: 12, color: "#333", marginBottom: 4 }}>
              {scanProgress && scanProgress.total > 0
                ? `正在扫描脱敏 ${Math.min(100, Math.round((scanProgress.done / scanProgress.total) * 100))}%`
                : "正在扫描脱敏..."}
            </div>
            <div style={{ height: 8, background: "#eee", borderRadius: 4, overflow: "hidden" }}>
              <div
                style={{
                  height: "100%",
                  width: `${
                    scanProgress && scanProgress.total > 0
                      ? Math.min(100, Math.round((scanProgress.done / scanProgress.total) * 100))
                      : 0
                  }%`,
                  background: "#ff6b35",
                  transition: "width 0.2s",
                }}
              />
            </div>
          </div>
          <button
            onClick={handleCancelScanRedact}
            data-tip="停止扫描，已处理的部分会保留"
            style={{ flexShrink: 0, background: "transparent", border: "1px solid #ddd", borderRadius: 4, padding: "4px 10px", fontSize: 12, cursor: "pointer", color: "#666" }}
          >
            取消
          </button>
        </div>
      )}

      {/* 扫描结果提示 */}
      {scanToast && (
        <div
          style={{
            position: "fixed",
            top: 130,
            left: "50%",
            transform: "translateX(-50%)",
            zIndex: 9999,
            background: scanToast.type === "success" ? "#f0f9f0" : "#fdf0f0",
            border: `1px solid ${scanToast.type === "success" ? "#4caf50" : "#e57373"}`,
            borderRadius: 8,
            padding: "8px 16px",
            fontSize: 13,
            color: scanToast.type === "success" ? "#2e7d32" : "#c62828",
            boxShadow: "0 4px 16px rgba(0,0,0,0.1)",
            maxWidth: "70vw",
          }}
        >
          {scanToast.message}
        </div>
      )}

      {/* 翻译确认弹窗（PRD T3）：选语言 + 提示译文副本不覆盖原文 */}
      {/* ★ v310: 脱敏结果弹窗——带 ×，不点不消失（toast 太快，用户不知道哪些步骤被处理了） */}
      {redactResult && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 10001, background: "rgba(0,0,0,0.45)", display: "flex", alignItems: "center", justifyContent: "center" }}
        >
          <div
            style={{ position: "relative", background: "#fff", borderRadius: 10, padding: "20px 22px", width: 420, boxShadow: "0 8px 32px rgba(0,0,0,0.2)" }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <button
              onClick={() => setRedactResult(null)}
              data-tip="关闭"
              aria-label="关闭"
              style={{ position: "absolute", top: 10, right: 10, width: 26, height: 26, border: "none", borderRadius: 6, background: "transparent", color: "#57606a", fontSize: 14, cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center" }}
            >✕</button>
            <div style={{ fontSize: 15, fontWeight: 600, color: "#24292e", display: "flex", alignItems: "center", gap: 6 }}>
              <ShieldCheck size={16} /> {redactResult.title}
            </div>
            <div style={{ fontSize: 12.5, color: "#57606a", marginTop: 8 }}>
              以下步骤已被脱敏处理（改错了可点 ↶ 撤销）：
            </div>
            <ul style={{ margin: "8px 0 4px", padding: "8px 12px", maxHeight: 220, overflowY: "auto", listStyle: "none", background: "#f6f8fa", border: "1px solid #e5e7eb", borderRadius: 8, fontSize: 13, lineHeight: 1.9, color: "#24292e" }}>
              {redactResult.lines.map((l, i) => (
                <li key={i} style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{l}</li>
              ))}
            </ul>
            <div style={{ display: "flex", justifyContent: "flex-end", marginTop: 14 }}>
              <button
                onClick={() => setRedactResult(null)}
                style={{ padding: "6px 16px", borderRadius: 6, border: "none", background: "#3366CC", color: "#fff", cursor: "pointer", fontSize: 13 }}
              >知道了</button>
            </div>
          </div>
        </div>
      )}


      {/* Stage 3：执行审批闸门——计划含敏感步骤（快捷键/复制/粘贴/拖拽）时弹出计划确认 */}
      {approvalPlan && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 10002, background: "rgba(0,0,0,0.45)", display: "flex", alignItems: "center", justifyContent: "center" }}
        >
          <div
            style={{ position: "relative", background: "#fff", borderRadius: 10, padding: "20px 22px", width: 460, boxShadow: "0 8px 32px rgba(0,0,0,0.2)" }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <button
              onClick={() => setApprovalPlan(null)}
              data-tip="关闭"
              aria-label="关闭"
              style={{ position: "absolute", top: 10, right: 10, width: 26, height: 26, border: "none", borderRadius: 6, background: "transparent", color: "#57606a", fontSize: 14, cursor: "pointer", display: "flex", alignItems: "center", justifyContent: "center" }}
            >✕</button>
            <div style={{ fontSize: 15, fontWeight: 600, color: "#24292e", display: "flex", alignItems: "center", gap: 6 }}>
              <ShieldCheck size={16} /> 执行前确认
            </div>
            <div style={{ fontSize: 12.5, color: "#57606a", marginTop: 8 }}>
              本教程包含 <b style={{ color: "#d97706" }}>敏感操作</b>（快捷键 / 复制 / 粘贴 / 拖拽），重放将真实操作你的桌面。请确认下列步骤无误后再放行：
            </div>
            <ul style={{ margin: "8px 0 4px", padding: "8px 12px", maxHeight: 240, overflowY: "auto", listStyle: "none", background: "#f6f8fa", border: "1px solid #e5e7eb", borderRadius: 8, fontSize: 13, lineHeight: 1.9, color: "#24292e" }}>
              {approvalPlan.steps.map((s) => (
                <li key={s.stepNumber} style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", display: "flex", alignItems: "center", gap: 6 }}>
                  <span style={{ color: "#8c959f", minWidth: 24 }}>#{s.stepNumber}</span>
                  <span style={{ color: s.sensitive ? "#d97706" : "#24292e", fontWeight: s.sensitive ? 600 : 400 }}>{s.sensitive ? "⚠ " : ""}{ACTION_LABELS[s.actionType] || s.actionType}</span>
                  <span style={{ color: "#57606a", overflow: "hidden", textOverflow: "ellipsis" }}>{s.target}</span>
                </li>
              ))}
            </ul>
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 14 }}>
              <button
                onClick={() => setApprovalPlan(null)}
                style={{ padding: "6px 16px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", cursor: "pointer", fontSize: 13 }}
              >取消</button>
              <button
                onClick={confirmApproval}
                disabled={executing}
                style={{ padding: "6px 16px", borderRadius: 6, border: "none", background: "#3fb950", color: "#fff", cursor: executing ? "default" : "pointer", fontSize: 13 }}
              >批准并执行</button>
            </div>
          </div>
        </div>
      )}

      {/* ★ T8 译文可编辑弹窗 */}

      {/* 第十九轮：两栏包进 .editor-zoom —— 整页缩放只作用于「页面」本身（目录 + 正文）。
          弹窗/进度条那些 position:fixed 浮层留在外面，缩放不会把它们的定位算歪。 */}
      <div className="editor-zoom" style={{ zoom: pageZoom }}>
      {/* Left: Step List（宽度由分隔条拖拽决定，第十八轮） */}
      <div className="editor-sidebar" style={{ width: sideWidth, minWidth: 0 }}>
        {/* 第十九轮：步骤数从右端挪到「目录」后面；右端腾出来的位置放整页缩放
            （－ 缩小 / ＋ 放大 / ⟲ 比例恢复原样）。 */}
        {/* 第二十八轮：头部自己带下内边距 + 一条分隔线（见 styles.css
            「② 目录层次」）—— 原来靠 inline style 给左右内边距，会挡住样式表
            算不了下边距，所以内边距整块交给 CSS。 */}
        <div className="text-sm text-secondary dir-head">
          <span className="dir-title">目录</span>
          <span className="dir-count">{realSteps.length} 个步骤</span>
          <div className="dir-zoom">
            {/* 第二十四轮：目录只有一种形态（大纲列表），缩略图网格整块移除；
                这里只留「批量删除」入口。放在缩放三键前面，同一排小图标语言。 */}
            <button
              type="button"
              className={`dir-tool-btn${batchMode ? " is-on" : ""}`}
              onClick={toggleBatchMode}
              data-tip={batchMode ? "退出批量删除" : "批量删除步骤\n勾选多个步骤一次删掉（标题/摘要不参与）"}
              aria-label={batchMode ? "退出批量删除" : "批量删除步骤"}
            >
              <ListChecks size={13} />
            </button>
            <span className="dir-zoom-sep" />
            <button
              type="button"
              className="dir-zoom-btn"
              disabled={pageZoom <= ZOOM_MIN}
              onClick={() => nudgePageZoom(-1)}
              data-tip={`缩小整页比例（当前 ${Math.round(pageZoom * 100)}%）`}
              aria-label="缩小整页比例"
            >
              <ZoomOut size={13} />
            </button>
            <button
              type="button"
              className="dir-zoom-btn"
              disabled={pageZoom >= ZOOM_MAX}
              onClick={() => nudgePageZoom(1)}
              data-tip={`放大整页比例（当前 ${Math.round(pageZoom * 100)}%）`}
              aria-label="放大整页比例"
            >
              <ZoomIn size={13} />
            </button>
            <span className="dir-zoom-sep" />
            <button
              type="button"
              className="dir-zoom-btn is-reset"
              disabled={pageZoom === ZOOM_DEFAULT && sideWidth === SIDE_W_DEFAULT}
              onClick={() => { applyPageZoom(ZOOM_DEFAULT); resetSideWidth(); }}
              data-tip={"比例恢复原样（100%）\n栏宽也一并回到默认"}
              aria-label="比例与栏宽恢复原样"
            >
              <RotateCcw size={13} />
            </button>
          </div>
        </div>

        {/* Step list —— 第二十四轮：目录只有大纲列表一种形态（缩略图网格已整块移除） */}
        <div className={`step-list${batchMode ? " is-batching" : ""}`}>
          {/* Insert zone before first step（批量态不显示插入槽） */}
          {!batchMode && (
            <div className="step-insert-point">
              <div className="step-insert-line" />
              <div className="step-insert-btn" onClick={(e) => openInsertMenu(e, -1)} data-tip="在这里插入：步骤 / 大标题 / 续录">
                <Plus size={12} />
              </div>
            </div>
          )}

          {dirEntries.map((entry, index) => {
            const step = entry.step;
            if (entry.hidden) return null;
            return (
            <Fragment key={step.id}>
              <div
                id={`step-item-${step.id}`}
                /* 流程摘要固定在正文最前，不给拖（拖走了它就不再是"开头那段总览"） */
                draggable={!isSummaryBlock(step) && !batchMode}
                onDragStart={(e) => {
                  setDragStepId(step.id);
                  // dataTransfer 在合成事件里可能为 null，别让它把拖拽打断
                  if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                }}
                onDragOver={(e) => {
                  if (!dragStepId || dragStepId === step.id) return;
                  e.preventDefault();
                  const r = e.currentTarget.getBoundingClientRect();
                  const after = e.clientY > r.top + r.height / 2;
                  setDropAt((prev) =>
                    prev && prev.id === step.id && prev.after === after
                      ? prev
                      : { id: step.id, after }
                  );
                }}
                onDrop={(e) => {
                  e.preventDefault();
                  void applyReorder();
                }}
                onDragEnd={() => {
                  setDragStepId(null);
                  setDropAt(null);
                }}
                /* 缩进级别交给 CSS 变量（第二十四轮）：整行左缘由样式表统一算 ——
                   把手 / 折叠箭头 / 序号位三段都锚在这条基准线上，
                   原来直接在行上写 paddingLeft，样式表没法参与对齐计算。 */
                style={{ "--nav-indent": entry.indent } as React.CSSProperties}
                className={`step-item group relative${selectedStepId === step.id ? " active" : ""}${previewMode ? " preview-active" : ""}${dropAt && dropAt.id === step.id ? (dropAt.after ? " is-drop-after" : " is-drop-before") : ""}`}
                onClick={() => {
                  // 批量态：点行 = 勾选/取消（文档块不参与）
                  if (batchMode && !isDocBlock(step)) { toggleBatchCheck(step.id); return; }
                  // ★ 预览模式也要更新选中态：否则橙色高亮停在原步骤（如 23），
                  //   点击 18 只有预览滚动、高亮纹丝不动（用户报：高亮不跟随）。
                  setSelectedStepId(step.id);
                  setLightboxMarked(false);
                  // 预览态 / 通篇编辑态都是"点目录 → 滚到该步"。
                  // 平滑滚动期间先锁住滚动联动，免得中途把高亮抢到别的步骤上。
                  if (previewMode || sopBodyRef.current) {
                    sopScrollLock.current = Date.now() + 900;
                    const el = document.getElementById(`sop-step-${step.id}`);
                    if (el) el.scrollIntoView({ behavior: "smooth", block: "start" });
                  }
                }}
              >
                {/* 第二十轮：拖拽把手放在**步骤最前面**（编号左侧那一列），鼠标移到这一行才浮现。
                    刻意放在这里 —— 它是 .step-item 的第一个子元素，连下面那个 absolute 的
                    删除按钮组都排在它后面，「移动按钮在步骤前」在 DOM 顺序上也成立。
                    摘要块不可拖（draggable=false），所以不给它把手。 */}
                {/* 批量态：把手换成勾选框（第二十三轮 · 设计稿口径）；
                    非批量维持把手。摘要块两者都没有。 */}
                {batchMode && !isDocBlock(step) ? (
                  <span
                    className={`selbox${batchChecked.has(step.id) ? " on" : ""}`}
                    onClick={(e) => { e.stopPropagation(); toggleBatchCheck(step.id); }}
                    aria-label="选中这一步"
                  >
                    {batchChecked.has(step.id) && <Check size={11} />}
                  </span>
                ) : !isSummaryBlock(step) ? (
                  <span className="step-item-grip" aria-hidden="true">
                    <GripVertical size={13} />
                  </span>
                ) : null}

                {/* Action group – hover reveal (delete only)
                    批量态收起单步删除（删步统一走目录底部操作条，免得两套删除打架） */}
                {!batchMode && (
                <div className="step-action-group">
                  <button
                    onClick={(e) => { e.stopPropagation(); setDeleteTarget(step.id); }}
                    className="step-action-btn step-action-delete"
                    data-tip="删除步骤"
                  >
                    <Trash2 className="step-action-icon" />
                  </button>
                </div>
                )}

                {/* Delete overlay */}
                {deleteTarget === step.id && (
                  <div className="step-delete-overlay" onClick={(e) => e.stopPropagation()}>
                    <div className="step-delete-card" onClick={(e) => e.stopPropagation()}>
                      <div className="step-delete-icon-wrap">
                        <Trash2 />
                      </div>
                      <p className="step-delete-title">
                        {isSummaryBlock(step)
                          ? "删除流程摘要"
                          : isIntroBlock(step)
                            ? "删除简介"
                            : isHeadingBlock(step)
                              ? "删除标题"
                              : `删除步骤 ${entry.num}`}
                      </p>
                      <p className="step-delete-subtitle">
                        {isSummaryBlock(step)
                          ? "（录制结束时自动生成的那段）"
                          : isIntroBlock(step)
                            ? "（你自己写的这段引子）"
                            : step.title || "(无标题)"}
                      </p>
                      <div className="step-delete-actions">
                        <button
                          className="step-delete-cancel"
                          onClick={(e) => { e.stopPropagation(); setDeleteTarget(null); }}
                          data-tip="不删了"
                        >
                          取消
                        </button>
                        <button
                          className="step-delete-confirm"
                          onClick={(e) => { e.stopPropagation(); handleDeleteStep(step.id); }}
                          data-tip={isSummaryBlock(step) ? "删掉这段摘要，正文最前面不再显示" : "删除这一步，后面的步骤序号自动前移"}
                        >
                          删除
                        </button>
                      </div>
                    </div>
                  </div>
                )}

                {isHeadingBlock(step) ? (
                  /* 大标题行前面挂一个折叠箭头：折起来只在左栏生效，不动右侧文档流 */
                  <button
                    type="button"
                    className={`step-item-caret${entry.collapsed ? " is-collapsed" : ""}`}
                    onClick={(e) => { e.stopPropagation(); toggleHeadingCollapse(step.id); }}
                    data-tip={entry.collapsed ? "展开这一节" : "折起这一节"}
                    aria-label={entry.collapsed ? "展开这一节" : "折起这一节"}
                  >
                    <ChevronDown size={12} />
                  </button>
                ) : entry.indent > 0 ? (
                  <span className="step-item-dot" />
                ) : null}
                {isSummaryBlock(step) ? (
                  /* 文档块不占编号：序号位放一枚标签 —— 大标题是 H1~H4 的**边框小标**，
                     摘要 / 简介是「摘 / 介」的裸小字，与步骤数字同一列右对齐。
                     （设计稿 .nrow.is-h .nn / .nrow.is-doc 的口径） */
                  <span className="step-item-slot is-doc">摘</span>
                ) : isHeadingBlock(step) ? (
                  <span className="step-item-slot is-h">H{entry.level}</span>
                ) : isIntroBlock(step) ? (
                  <span className="step-item-slot is-doc">介</span>
                ) : (
                  <span className="step-item-number">{String(entry.num).padStart(2, "0")}</span>
                )}
                <div className="step-item-body">
                  <div className={`step-item-title${isHeadingBlock(step) ? " is-heading" : isSummaryBlock(step) ? " is-summary" : isIntroBlock(step) ? " is-intro" : ""}`}>
                    {/* 预览模式跟随所选语言显示译文标题（缺步骤回退原文），与右侧预览一致；查找时命中片段高亮 */}
                    {(() => {
                      if (isSummaryBlock(step)) return "流程摘要";
                      if (isIntroBlock(step)) return "简介";
                      const empty = isHeadingBlock(step) ? "(空标题)" : "(无标题)";
                      const t = previewMode ? (previewStepText(step).title || empty) : (step.title || empty);
                      return findOpen ? renderFindHighlight(t, committedQuery.trim()) : t;
                    })()}
                  </div>
                  {/* 第十五轮：条目压成一行 —— 步骤不再显示 CLICK / TYPE 小字。
                      第二十四轮：大标题的 H{n} 徽标上移到序号位（.step-item-slot），
                      这里不再挂第二枚标记。 */}
                </div>
                {(() => {
                  // 缩略图跟随主截图（裁剪图优先，无则全图）
                  const src = getStepMainSrc(step);
                  // 大标题 / 流程摘要块没有截图，别渲染空缩略图占位
                  if (isDocBlock(step)) return null;
                  return src ? (
                    <div className="step-item-thumb">
                      <img
                        src={`data:image/jpeg;base64,${src}`}
                        alt={`步骤 ${entry.num}`}
                      />
                    </div>
                  ) : (
                    <div className="step-item-thumb step-item-thumb-empty">
                      <span className="thumb-empty-type">{step.action_type || ''}</span>
                    </div>
                  );
                })()}
                {/* 第十七轮：目录行里那组 H1~H4 切级键去掉了 —— 正文里大标题自己的
                    工具条（.sop-bar-btn.is-lv）和它一模一样，同一个操作在两处出现容易被
                    误当成两个功能；切级统一走正文工具条。层级仍由上面的 H{n} 小徽标标示。 */}
              </div>
              {/* Insert zone after this step —— 与首插槽一致：批量态不显示 */}
              {!batchMode && (
              <div className="step-insert-point">
                <div className="step-insert-line" />
                <div className="step-insert-btn" onClick={(e) => openInsertMenu(e, index)} data-tip="在这里插入：步骤 / 大标题 / 续录">
                  <Plus size={12} />
                </div>
              </div>
              )}
            </Fragment>
            );
          })}
          <div />
        </div>

        {/* 第二十三轮：目录底部批量操作条（仅批量态出现；0 选中禁用删除） */}
        {batchMode && (
          <div className="sb-batchbar">
            <span className="sb-batchbar-n">已选 {batchChecked.size} 步</span>
            <button
              type="button"
              className="sb-batchbar-del"
              disabled={batchChecked.size === 0 || batchBusy}
              onClick={(e) => { e.stopPropagation(); void handleBatchDelete(); }}
              data-tip={batchBusy ? "删除中…" : "删除勾选的步骤，序号自动前移"}
            >
              <Trash2 size={12} /> {batchBusy ? "删除中…" : "删除"}
            </button>
            <button
              type="button"
              className="sb-batchbar-cancel"
              onClick={(e) => { e.stopPropagation(); toggleBatchMode(); }}
            >
              取消
            </button>
          </div>
        )}
      </div>

      {/* 第十八轮：两栏之间的可拖拽分隔条 —— 拖它 = 整体调节左右宽度 */}
      <div
        className={`editor-split${splitDragging ? " is-dragging" : ""}`}
        role="separator"
        aria-orientation="vertical"
        aria-label="拖动调节左右两栏宽度"
        data-tip="拖动调节左右宽度 · 双击恢复默认"
        onMouseDown={(e) => {
          e.preventDefault();
          splitDragRef.current = { startX: e.clientX, startW: sideWidth, w: sideWidth, zoom: pageZoom };
          setSplitDragging(true);
        }}
        /* 第十九轮：双击 = 栏宽恢复原样（拖动是隐式的，得有颗复位键） */
        onDoubleClick={resetSideWidth}
      />

      {/* Right: Step Detail / SOP Preview */}
      <div className={`editor-preview${previewMode ? " preview-active" : ""}`}>
        <div
          className="editor-preview-body"
          ref={sopBodyRef}
          onScroll={onSopScroll}
        >
          {previewMode ? (
            <div className="sop-preview">
              {/* ★ v0.65.244 对齐：成片体检条（sticky 悬浮在成片正文顶部；无问题 → 通过态） */}
              {health.ok ? (
                <div className="health-bar is-ok">
                  <span className="health-bar-badge">
                    <CheckCircle2 size={14} />
                  </span>
                  <span className="health-bar-text">成片没问题，可以直接导出 / 发布</span>
                </div>
              ) : (
                <div className={`health-bar${health.errorCount > 0 ? " has-errors" : " has-warns"}`}>
                  <span className="health-bar-badge">
                    {health.errorCount > 0 ? <XCircle size={14} /> : <AlertTriangle size={14} />}
                  </span>
                  <span className="health-bar-text">
                    {health.errorCount > 0
                      ? `${health.errorCount} 处问题会让成片断掉，先修再导出吧`
                      : `${health.warnCount} 处建议优化，不影响复现`}
                  </span>
                  <span className="health-bar-chips">
                    {healthChips.map((c) => (
                      <button
                        key={`${c.step}-${c.label}`}
                        type="button"
                        className={`health-chip is-${c.level}`}
                        onClick={() => jumpToHealthStep(c.step)}
                        data-tip={`跳到第 ${c.step} 步：${c.label}`}
                      >
                        第{c.step}步 · {c.label}
                      </button>
                    ))}
                  </span>
                </div>
              )}
              {/* Full manual */}
              <div className="sop-preview-body">
                {previewBlocks.map((b, idx) => {
                  const step = b.step;
                  /* 大标题块：按 H1~H4 成行渲染（字号与导出 HTML 的 .doc-heading 一致） */
                  if (b.isHeading) {
                    const pt = previewStepText(step);
                    // 切了译文就按译文走（译文没有富文本），否则用标题自己的富文本
                    const html =
                      pt.title === (step.title ?? "")
                        ? headingInitialHtml(step)
                        : escapeText(pt.title);
                    return (
                      <div
                        key={step.id}
                        id={`sop-step-${step.id}`}
                        data-step-id={step.id}
                        className="sop-preview-step sop-preview-heading-wrap"
                      >
                        <div
                          className={`sop-preview-heading h${b.level}`}
                          dangerouslySetInnerHTML={{ __html: html || escapeText("(空标题)") }}
                        />
                        {idx < previewBlocks.length - 1 && <hr className="sop-step-divider" />}
                      </div>
                    );
                  }
                  /* 流程摘要块：录制结束自动生成，固定在正文最前。
                     版式与导出 HTML 的 .doc-summary 逐值一致（浅灰圆角卡 + 要点列表），
                     不占编号、不出「N 步骤」那一行。 */
                  if (b.isSummary) {
                    const sum = parseSummary(step.description ?? "");
                    return (
                      <div
                        key={step.id}
                        id={`sop-step-${step.id}`}
                        data-step-id={step.id}
                        className="sop-preview-step sop-preview-summary-wrap"
                      >
                        <div className="sop-preview-summary">
                          {sum.intro && <p>{sum.intro}</p>}
                          {sum.bullets.length > 0 && (
                            <ul>
                              {sum.bullets.map((bl, i) => (
                                <li key={i}>
                                  {bl.name ? (
                                    <><b>{bl.name}</b>：{bl.detail}</>
                                  ) : (
                                    bl.detail
                                  )}
                                </li>
                              ))}
                            </ul>
                          )}
                        </div>
                        {idx < previewBlocks.length - 1 && <hr className="sop-step-divider" />}
                      </div>
                    );
                  }
                  /* 简介块（第十九轮）：用户自己写的一段引子，排在它被插入的位置上。
                     版式与导出 HTML 的 .doc-intro 一致（不占编号、不出「N 步骤」行）。 */
                  if (b.isIntro) {
                    const text = (previewMode ? previewStepText(step).description : step.description) ?? "";
                    if (!text.trim()) return null;
                    return (
                      <div
                        key={step.id}
                        id={`sop-step-${step.id}`}
                        data-step-id={step.id}
                        className="sop-preview-step sop-preview-intro-wrap"
                      >
                        <div className="sop-preview-intro">{text.trim()}</div>
                        {idx < previewBlocks.length - 1 && <hr className="sop-step-divider" />}
                      </div>
                    );
                  }
                  return (
                  <div key={step.id} id={`sop-step-${step.id}`} data-step-id={step.id} className="sop-preview-step">
                    {/* Step header */}
                    <div className="sop-preview-step-header">
                      <span className="sop-preview-step-num"><span className="step-num-circle">{b.num}</span>步骤</span>
                    </div>
                    <h3 className="sop-preview-step-title">{previewStepText(step).title || "(无标题)"}</h3>

                    {/* Screenshot */}
                    {(() => {
                      // 预览的截图与编辑态同一张（重点图的倍率在下面按步套用）
                      const src = getStepMainSrc(step);
                      const disp = shotDispOf(step.id);
                      const fracPct = perShotPcts[step.id] ?? 100;
                      const showBefore = disp <= 100.001 || disp > fracPct + 0.5;
                      const shownSrc = showBefore ? step.before_screenshot_base64 || src : src;
                      return shownSrc ? (
                        <div className="sop-preview-image sop-edit-shot">
                          <img
                            src={`data:image/jpeg;base64,${shownSrc}`}
                            alt={`步骤 ${b.num} 截图`}
                            onLoad={(e) => rememberShotNat(step.id, e.currentTarget)}
                            style={{
                              transform: `scale(${(() => {
                                if (Math.abs(disp - 100) < 0.001) return 1;
                                // 顶栏全局比例态：主图裁剪图按真实比例放大填满框（与编辑态一致）
                                if (disp < 100) return Math.round((100 / disp) * 100) / 100;
                                return shotZoomOf(step.id);
                              })()})`,
                              transformOrigin: Math.abs(disp - 100) < 0.001 ? "50% 50%" : shotOriginOf(step),
                            }}
                          />
                        </div>
                      ) : null;
                    })()}

                    {/* 图片描述：紧贴截图下方一行小灰字（与导出 .step-caption 同值） */}
                    {step.image_caption?.trim() && (
                      <div className="sop-preview-caption">{step.image_caption.trim()}</div>
                    )}

                    {/* Description */}
                    {previewStepText(step).description && (
                      /* 标注 / 警告按类型上色（与导出 HTML 的 .step-desc.is-mark/.is-warn 同值） */
                      <div className={`sop-preview-desc is-${noteKindOf(step) || "desc"}`}>
                        {previewStepText(step).description}
                      </div>
                    )}

                    {/* Tip */}
                    {step.tip && (
                      <div className="sop-preview-tip">
                        <strong>提示：</strong>{step.tip}
                      </div>
                    )}

                    {/* Divider */}
                    {idx < previewBlocks.length - 1 && <hr className="sop-step-divider" />}
                  </div>
                  );
                })}
              </div>
            </div>
          ) : (
            /* ── 通篇滚动编辑：所有步骤排成一条可滚动的文档流，滑到哪改到哪 ──
               结构沿用预览（.sop-preview-* / .step-num-circle），标题与描述换成
               就地可编辑的输入控件；删除这一步的 ✕ 挂在「步骤」这一行，
               图片上的功能键（编辑图片 / 删除截图 / 倍率）鼠标移到图片上才浮出。 */
            <div className="sop-preview sop-edit">
              {/* ── 文档头（第二十六轮 · 设计稿 .doc-head）────────────────
                  正文最前面那一块：大标题 + 元信息行。它是「文档」的开头，
                  不是某个步骤，所以不进「52px 序号列 + 内容列」那套栅格。
                  标题就地可编辑 —— 与顶栏那颗是同一个 recordings.title。 */}
              <div className="sop-doc-head">
                <textarea
                  className="sop-doc-h1"
                  rows={1}
                  value={docTitle}
                  placeholder="未命名教程"
                  onChange={(e) => {
                    autoGrow(e.currentTarget);
                    setDocTitleDraft(e.target.value);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") { e.preventDefault(); e.currentTarget.blur(); }
                    if (e.key === "Escape") setDocTitleDraft(null);
                  }}
                  onBlur={() => void commitDocTitle()}
                  data-tip="教程标题（与顶栏那个名字同一个）"
                />
                <div className="sop-doc-meta">
                  <span>{realSteps.length} 个步骤</span>
                  <span className="sop-doc-dot" />
                  <span>{realSteps.filter((s) => !!getStepMainSrc(s)).length} 张截图</span>
                  <span className="sop-doc-dot" />
                  <span>自动生成 · 可编辑</span>
                  <span className="sop-doc-pill">未发布</span>
                </div>
              </div>
              <div className="sop-preview-body">
                {steps.map((step, idx) => {
                  const d = draftOf(step);
                  const src = getStepMainSrc(step);
                  const isSel = selectedStepId === step.id;
                  const focusShot = isFocusShot(step.id);
                  const kind = noteKindOf(step);
                  if (isHeadingBlock(step)) {
                    /* ── 大标题块：一行可编辑大标题（富文本）──
                       正文是 contenteditable —— 不受控，React 不去改它的 innerHTML，
                       否则每敲一个字都会被"受控值"把光标顶回开头。
                       聚焦时标题上方浮出工具条：I U S ｜ A 字色 ｜ 高亮 ｜ H1~H4。 */
                    const lv = Math.min(4, Math.max(1, step.block_level || 1));
                    return (
                      <div
                        key={step.id}
                        id={`sop-step-${step.id}`}
                        data-step-id={step.id}
                        className="sop-edit-step sop-edit-heading"
                        /* 层级交给 CSS：序号位那枚 H1~H4 小标签的内容由它拼出来 */
                        data-lv={lv}
                        onMouseDown={() => {
                          if (selectedStepId !== step.id) setSelectedStepId(step.id);
                        }}
                        /* 工具条淡出后，悬着的取色面板要一起收掉（否则再悬浮会莫名弹出来） */
                        onMouseLeave={() => setColorMenu(null)}
                      >
                        {/* 工具条常驻 DOM，靠 .sop-edit-heading:hover / :focus-within 淡入。
                            以前是聚焦才插入节点、并 absolute 到 top:-42px —— 会压住上一条
                            步骤的说明文字；现在大标题块自己留出一条 38px 的槽位，工具条落在槽里。 */}
                        <div
                          className="sop-heading-bar"
                          onMouseDown={(e) => e.preventDefault()}
                        >
                            <button
                              type="button"
                              className="sop-bar-btn is-italic"
                              onClick={() => execRich("italic")}
                              data-tip="斜体"
                              aria-label="斜体"
                            >I</button>
                            <button
                              type="button"
                              className="sop-bar-btn is-underline"
                              onClick={() => execRich("underline")}
                              data-tip="下划线"
                              aria-label="下划线"
                            >U</button>
                            <button
                              type="button"
                              className="sop-bar-btn is-strike"
                              onClick={() => execRich("strikeThrough")}
                              data-tip="删除线"
                              aria-label="删除线"
                            >S</button>
                            <span className="sop-bar-sep" />
                            <div className="sop-bar-pop-wrap">
                              <button
                                type="button"
                                className="sop-bar-btn is-more"
                                onClick={() => setColorMenu(colorMenu === "text" ? null : "text")}
                                data-tip="文字颜色"
                                aria-label="文字颜色"
                              >A<ChevronDown size={9} /></button>
                              {colorMenu === "text" && (
                                <div className="sop-bar-pop">
                                  <button
                                    type="button"
                                    className="sop-bar-swatch is-none"
                                    onClick={() => execRich("removeFormat")}
                                    data-tip="默认"
                                    aria-label="默认颜色"
                                  />
                                  {RICH_TEXT_COLORS.map((c) => (
                                    <button
                                      key={c.value}
                                      type="button"
                                      className="sop-bar-swatch"
                                      style={{ background: c.value }}
                                      onClick={() => execRich("foreColor", c.value)}
                                      data-tip={c.label}
                                      aria-label={c.label}
                                    />
                                  ))}
                                </div>
                              )}
                            </div>
                            <div className="sop-bar-pop-wrap">
                              <button
                                type="button"
                                className="sop-bar-btn is-more"
                                onClick={() => setColorMenu(colorMenu === "hl" ? null : "hl")}
                                data-tip="高亮"
                                aria-label="高亮"
                              ><Highlighter size={12} /><ChevronDown size={9} /></button>
                              {colorMenu === "hl" && (
                                <div className="sop-bar-pop">
                                  <button
                                    type="button"
                                    className="sop-bar-swatch is-none"
                                    onClick={() => execRich("hiliteColor", "#ffffff")}
                                    data-tip="无高亮"
                                    aria-label="无高亮"
                                  />
                                  {RICH_HIGHLIGHTS.map((c) => (
                                    <button
                                      key={c.value}
                                      type="button"
                                      className="sop-bar-swatch"
                                      style={{ background: c.value }}
                                      onClick={() => execRich("hiliteColor", c.value)}
                                      data-tip={c.label}
                                      aria-label={c.label}
                                    />
                                  ))}
                                </div>
                              )}
                            </div>
                            <span className="sop-bar-sep" />
                            {HEADING_LEVELS.map((l) => (
                              <button
                                key={l}
                                type="button"
                                className={`sop-bar-btn is-lv${lv === l ? " is-on" : ""}`}
                                onClick={() => void handleSetBlockLevel(step.id, l)}
                                data-tip={`设为 ${l} 级标题`}
                                aria-label={`设为 ${l} 级标题`}
                              >H{l}</button>
                            ))}
                        </div>
                        <div className="sop-heading-row">
                          <span className="sop-heading-hash"><Hash size={15} /></span>
                          <div
                            ref={(el) => {
                              headingEls.current[step.id] = el;
                              // 不受控：只在挂载时灌一次初始 HTML，之后完全交给浏览器
                              if (el && el.dataset.hydrated !== "1") {
                                el.innerHTML = headingInitialHtml(step);
                                el.dataset.hydrated = "1";
                              }
                            }}
                            className={`sop-edit-rich sop-heading-t${lv}`}
                            contentEditable
                            suppressContentEditableWarning
                            role="textbox"
                            aria-label="大标题"
                            data-placeholder="大标题"
                            spellCheck={false}
                            onFocus={(e) => {
                              if (selectedStepId !== step.id) setSelectedStepId(step.id);
                              setActiveHeadingId(step.id);
                              setColorMenu(null);
                            }}
                            onBlur={() => {
                              if (activeHeadingId === step.id) setActiveHeadingId(null);
                              setColorMenu(null);
                            }}
                            onInput={(e) => {
                              const el = e.currentTarget;
                              const html = normalizeInlineHtml(el);
                              setDraftHeadingHtml(step.id, html, htmlToPlainText(html));
                            }}
                            onPaste={(e) => {
                              // 只吃纯文本：粘进来的样式一律不进白名单
                              e.preventDefault();
                              const text = e.clipboardData
                                .getData("text/plain")
                                .replace(/\s*\n\s*/g, " ");
                              document.execCommand("insertText", false, text);
                            }}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") e.preventDefault(); // 大标题只有一行
                            }}
                          />
                          <div className="sop-heading-ops">
                            <button
                              type="button"
                              className="sop-step-del"
                              onClick={(e) => { e.stopPropagation(); setDeleteTarget(step.id); }}
                              data-tip="删除这个标题"
                              aria-label="删除这个标题"
                            >
                              <X size={13} />
                            </button>
                          </div>
                        </div>
                        {idx < steps.length - 1 && <hr className="sop-step-divider" />}
                      </div>
                    );
                  }
                  if (isSummaryBlock(step)) {
                    /* ── 流程摘要块（录制结束时自动生成，固定在正文最前）──
                       默认渲染**排版好的卡片**（与预览/导出同一版式）；
                       点「编辑」切到原文输入框（首行概述、下面每行 `- **要点**：说明`），
                       改完 800ms 防抖落库，和编辑器里其它就地可编辑的地方同一套。 */
                    const sum = parseSummary(d.description ?? "");
                    const editing = summaryEditId === step.id;
                    return (
                      <div
                        key={step.id}
                        id={`sop-step-${step.id}`}
                        data-step-id={step.id}
                        className="sop-edit-step sop-edit-summary"
                        onMouseDown={() => {
                          if (selectedStepId !== step.id) setSelectedStepId(step.id);
                        }}
                      >
                        <div className="sop-summary-card">
                          <div className="sop-summary-head">
                            <Sparkles size={13} className="sop-summary-spark" />
                            <span className="sop-summary-title">流程摘要</span>
                            <span className="sop-summary-badge">自动生成</span>
                            <div className="sop-summary-ops">
                              <button
                                type="button"
                                className="sop-summary-op"
                                onClick={(e) => { e.stopPropagation(); setSummaryEditId(editing ? null : step.id); }}
                                data-tip={editing ? "看排版后的样子" : "直接改摘要文字"}
                              >
                                {editing ? "预览" : "编辑"}
                              </button>
              {isComplex && (
              <button
                type="button"
                className="sop-summary-op"
                disabled={summaryBusy === step.id}
                onClick={(e) => { e.stopPropagation(); void regenerateSummary(step.id); }}
                data-tip="按当前步骤重新生成一遍（AI 不可用时保留现在这份）"
              >
                {summaryBusy === step.id
                  ? <><Loader2 size={12} className="spinner" /> 生成中</>
                  : "重新生成"}
              </button>
              )}
                              <button
                                type="button"
                                className="sop-summary-del"
                                onClick={(e) => { e.stopPropagation(); setDeleteTarget(step.id); }}
                                data-tip="删除这段摘要"
                                aria-label="删除这段摘要"
                              >
                                <X size={13} />
                              </button>
                            </div>
                          </div>
                          {editing ? (
                            <textarea
                              className="sop-edit-desc sop-summary-input"
                              rows={4}
                              value={d.description}
                              placeholder={"第一行写整篇的概述；下面每行一条要点，格式：- **要点名**：说明"}
                              onChange={(e) => {
                                autoGrow(e.currentTarget);
                                setDraftField(step.id, "description", e.target.value);
                              }}
                              onFocus={(e) => {
                                autoGrow(e.currentTarget);
                                if (selectedStepId !== step.id) setSelectedStepId(step.id);
                              }}
                            />
                          ) : (
                            <div
                              className="sop-summary-body"
                              onClick={(e) => { e.stopPropagation(); setSummaryEditId(step.id); }}
                            >
                              {sum.intro && <p>{sum.intro}</p>}
                              {sum.bullets.length > 0 && (
                                <ul>
                                  {sum.bullets.map((bl, i) => (
                                    <li key={i}>
                                      {bl.name ? (<><b>{bl.name}</b>：{bl.detail}</>) : (bl.detail)}
                                    </li>
                                  ))}
                                </ul>
                              )}
                              {!sum.intro && sum.bullets.length === 0 && (
                                <p className="sop-summary-empty">（还没有摘要，点「重新生成」再试一次）</p>
                              )}
                            </div>
                          )}
                        </div>
                        {idx < steps.length - 1 && <hr className="sop-step-divider" />}
                      </div>
                    );
                  }
                  if (isIntroBlock(step)) {
                    /* ── 简介块（第十九轮）──
                       自己在正文里写的引子。一个多行输入框，随内容长高，
                       改完 800ms 防抖落库（与「补充说明」同一套）。 */
                    return (
                      <div
                        key={step.id}
                        id={`sop-step-${step.id}`}
                        data-step-id={step.id}
                        className="sop-edit-step sop-edit-intro"
                        onMouseDown={() => {
                          if (selectedStepId !== step.id) setSelectedStepId(step.id);
                        }}
                      >
                        <div className="sop-intro-card">
                          <div className="sop-intro-head">
                            <AlignLeft size={13} className="sop-intro-mark" />
                            <span className="sop-intro-title">简介</span>
                            <span className="sop-intro-badge">自定</span>
                            <button
                              type="button"
                              className="sop-intro-del"
                              onClick={(e) => { e.stopPropagation(); setDeleteTarget(step.id); }}
                              data-tip="删除这段简介"
                              aria-label="删除这段简介"
                            >
                              <X size={13} />
                            </button>
                          </div>
                          <textarea
                            className="sop-edit-desc sop-intro-input"
                            rows={3}
                            value={d.description}
                            placeholder="写一句整篇的简介：这份教程适合谁看、跟着做完能学会什么"
                            onChange={(e) => {
                              autoGrow(e.currentTarget);
                              setDraftField(step.id, "description", e.target.value);
                            }}
                            onFocus={(e) => {
                              autoGrow(e.currentTarget);
                              if (selectedStepId !== step.id) setSelectedStepId(step.id);
                            }}
                          />
                        </div>
                        {idx < steps.length - 1 && <hr className="sop-step-divider" />}
                      </div>
                    );
                  }
                  return (
                    <div
                      key={step.id}
                      id={`sop-step-${step.id}`}
                      data-step-id={step.id}
                      /* 第二十三轮：is-sel = 选中卡（徽章橙 + 微暖边 + 阴影升档）；
                          hasmedia = 有媒体的步骤卡，宽档切「媒体左 + 描述侧栏右」双栏 */
                      className={
                        "sop-edit-step" +
                        (isSel ? " is-sel" : "") +
                        (isVideoStep(step) || src ? " hasmedia" : "")
                      }
                      onMouseDown={() => {
                        if (selectedStepId !== step.id) {
                          setSelectedStepId(step.id);
                          setLightboxMarked(false);
                        }
                      }}
                    >
                      {/* 头部一行：圆徽 + 就地可编辑的标题 + 最右的删除键。
                          标题自 2026-09-19 起不再单独占一行（每步省一整行高度）。 */}
                      <div className="sop-preview-step-header sop-edit-head">
                        <span className={"step-num-circle" + (isSel ? " is-sel" : "")}>{String(step.step_number || idx + 1).padStart(2, "0")}</span>
                        {/* 标题这一行右侧还挂着两枚元信息（第二十六轮 · 设计稿 .btitle 行）：
                            动作类型来自 step.action_type；「易错」= 这一步的 note_kind 是 warn。
                            删除键排在它们右边，整串都在这条 wrap 里靠右。 */}
                        <div className="sop-edit-titlewrap">
                        <textarea
                          className="sop-edit-title"
                          rows={1}
                          value={d.title}
                          placeholder="这一步做什么"
                          onChange={(e) => {
                            autoGrow(e.currentTarget);
                            setDraftField(step.id, "title", e.target.value);
                          }}
                          onFocus={(e) => {
                            autoGrow(e.currentTarget);
                            if (selectedStepId !== step.id) setSelectedStepId(step.id);
                          }}
                        />
                        {/* 弹性空档：标题左边、标签与工具键右边（第二十九轮）——
                            动作类型标不再紧跟标题文字，和「插入 / ✕」聚成行尾一组。 */}
                        <span className="sop-edit-spacer" aria-hidden="true" />
                        {step.action_type ? <span className="sop-act-tag">{step.action_type}</span> : null}
                        {kind === "warn" ? <span className="sop-warn-chip">易错</span> : null}
                        {/* 块的「插入」（设计稿 .btools）：鼠标进块或它是当前步时才浮出，
                            点开的就是左栏那套插入菜单（步骤 / 大标题 / 简介 / 导入 / 续录）。 */}
                        <button
                          type="button"
                          className="sop-block-insert"
                          onClick={(e) => { e.stopPropagation(); openInsertMenu(e, idx); }}
                          data-tip="在这一步后面插入：步骤 / 大标题 / 简介"
                        >
                          插入
                        </button>
                        <button
                          type="button"
                          className="sop-step-del"
                          onClick={(e) => { e.stopPropagation(); setDeleteTarget(step.id); }}
                          data-tip="删除这一步，后面的步骤序号自动前移"
                          aria-label="删除这一步"
                        >
                          <X size={13} />
                        </button>
                        </div>
                      </div>

                      {/* 媒体：视频 → 原生播放器；图片（含 GIF 动图）→ 可缩放/裁切的截图 */}
                      {isVideoStep(step) ? (
                        <div className="sop-preview-image sop-edit-shot sop-shot-video">
                          <video
                            src={stepVideoSrc(step)}
                            poster={src ? `data:image/jpeg;base64,${src}` : undefined}
                            playsInline
                            preload="metadata"
                            ref={(el) => { videoEls.current[step.id] = el; }}
                            onLoadedMetadata={(e) => {
                              const v = e.currentTarget;
                              setVideoDur((prev) => ({ ...prev, [step.id]: v.duration || 0 }));
                              v.playbackRate = videoRateOf(step.id);
                            }}
                            onTimeUpdate={(e) => {
                              const v = e.currentTarget;
                              setVideoTime((prev) => ({ ...prev, [step.id]: v.currentTime }));
                            }}
                            onPlay={() => setVideoPlaying((prev) => ({ ...prev, [step.id]: true }))}
                            onPause={() => setVideoPlaying((prev) => ({ ...prev, [step.id]: false }))}
                            onEnded={() => setVideoPlaying((prev) => ({ ...prev, [step.id]: false }))}
                          />
                          {/* 自绘控制条：播放/暂停 + 倍速 −/+（档位 0.5~2）+ 进度 + 时间 + 全屏。
                              替代原生 controls —— 原生 −/+ 倍速步进不可控（会变成 0.21x 之类）。 */}
                          <div className="sop-video-controls">
                            <button
                              type="button"
                              className="sop-video-btn"
                              onClick={(e) => { e.stopPropagation(); toggleVideoPlay(step.id); }}
                              aria-label={videoPlaying[step.id] ? "暂停" : "播放"}
                            >
                              {videoPlaying[step.id] ? <Pause size={13} /> : <Play size={13} />}
                            </button>
                            <button
                              type="button"
                              className="sop-video-btn"
                              onClick={(e) => { e.stopPropagation(); nudgeVideoRate(step.id, -1); }}
                              aria-label="放慢"
                            >
                              <Minus size={13} />
                            </button>
                            <span className="sop-video-rate">{videoRateOf(step.id).toFixed(2)}x</span>
                            <button
                              type="button"
                              className="sop-video-btn"
                              onClick={(e) => { e.stopPropagation(); nudgeVideoRate(step.id, 1); }}
                              aria-label="加快"
                            >
                              <Plus size={13} />
                            </button>
                            <input
                              type="range"
                              className="sop-video-seek"
                              min={0}
                              max={videoDur[step.id] || 0}
                              step={0.01}
                              value={Math.min(videoTime[step.id] || 0, videoDur[step.id] || 0)}
                              onChange={(e) => seekVideoTo(step.id, Number(e.target.value))}
                              aria-label="播放进度"
                            />
                            <span className="sop-video-time">
                              {fmtTime(videoTime[step.id] ?? 0)} / {fmtTime(videoDur[step.id] ?? 0)}
                            </span>
                            <button
                              type="button"
                              className="sop-video-btn"
                              onClick={(e) => {
                                e.stopPropagation();
                                const el = videoEls.current[step.id];
                                if (el) void el.requestFullscreen?.().catch(() => { /* 不支持则忽略 */ });
                              }}
                              aria-label="全屏"
                            >
                              <Maximize2 size={13} />
                            </button>
                          </div>
                          {/* 视频只有「删除」—— 裁图 / 标注那套是给静态截图用的，
                              对视频没有意义，不渲染那颗铅笔键。 */}
                          <button
                            type="button"
                            className="sop-shot-del"
                            onClick={(e) => { e.stopPropagation(); deleteScreenshotFor(step); }}
                            aria-label="删除这段视频"
                          >
                            <X size={13} />
                          </button>
                        </div>
                      ) : src ? (
                        <div className="sop-preview-image sop-edit-shot">
                          <img
                            src={`data:image/jpeg;base64,${(() => {
                              const disp = shotDispOf(step.id);
                              const fracPct = perShotPcts[step.id] ?? 100;
                              // 100% 全图态 / 放大倍率态：整图全貌或倍率放大都以 before 整图为底；
                              // 顶栏全局比例态（disp<100）：显示主图（裁剪图）原样放大填满框
                              return shotAtGlobalPct[step.id]
                                ? src
                                : disp <= 100.001 || disp > fracPct + 0.5
                                  ? step.before_screenshot_base64 || src
                                  : src;
                            })()}`}
                            alt={`步骤 ${step.step_number || idx + 1} 截图`}
                            onClick={() => openLightbox(realSteps.findIndex((s) => s.id === step.id))}
                            onLoad={(e) => rememberShotNat(step.id, e.currentTarget)}
                            style={{
                              cursor: "zoom-in",
                              transform: `scale(${(() => {
                                const disp = shotDispOf(step.id);
                                if (Math.abs(disp - 100) < 0.001) return 1;
                                // 顶栏全局比例态：主图裁剪图按真实比例放大填满框（如 50% → scale 2）
                                if (disp < 100) return Math.round((100 / disp) * 100) / 100;
                                return shotZoomOf(step.id);
                              })()})`,
                              transformOrigin: (() => {
                                const disp = shotDispOf(step.id);
                                if (Math.abs(disp - 100) < 0.001) return "50% 50%";
                                return shotOriginOf(step);
                              })(),
                            }}
                          />
                          {/* 悬浮在图片上的功能键（鼠标移到图片上才浮现）：
                              「编辑图片」在左、删除截图的 ✕ 在最右。
                              第十九轮按用户要求去掉「图片描述」那颗键 —— 两个键同为 24px
                              高，圆心落在同一条水平线上（之前铅笔是 19px 的胶囊，中心偏高）。 */}
                          <button
                            type="button"
                            className="sop-shot-annotate"
                            onClick={(e) => { e.stopPropagation(); openAnnotate(step.id); }}
                            aria-label="编辑图片"
                          >
                            <PencilLine size={13} />
                          </button>
                          <button
                            type="button"
                            className="sop-shot-del"
                            onClick={(e) => { e.stopPropagation(); deleteScreenshotFor(step); }}
                            aria-label="删除这张截图"
                          >
                            <X size={13} />
                          </button>
                          {/* 图片右下角：−/+ 微调倍率 + 定位圆钮
                              （准星=放大至点击位置，四角向内=还原原图）
                              ⚠️ 第十五轮：图片上的浮层一律不再挂 data-tip —— 鼠标扫过
                              截图时一直被气泡追着跑，视觉很吵（用户直接点名去掉）。 */}
                          <div className="sop-shot-tools">
                            <div className="sop-shot-zoom">
                              <button
                                type="button"
                                className="step-detail-view-segmented-btn"
                                onClick={(e) => { e.stopPropagation(); nudgeShotZoom(step.id, -1); }}
                                aria-label="缩小"
                              >
                                <Minus size={13} />
                              </button>
                              <button
                                type="button"
                                className="step-detail-view-segmented-btn"
                                onClick={(e) => { e.stopPropagation(); nudgeShotZoom(step.id, 1); }}
                                aria-label="放大"
                              >
                                <Plus size={13} />
                              </button>
                            </div>
                            <button
                              type="button"
                              className="sop-shot-zoomtoggle"
                              onClick={(e) => {
                                e.stopPropagation();
                                if (isShotVisFull(step.id)) {
                                  // 100% 整图 → 顶栏全局比例态：显示当前主图
                                  // （真实比例裁剪图）原样填满框，缩放原点取操作落点
                                  setShowFullShot((prev) => ({ ...prev, [step.id]: false }));
                                  setShotAtGlobalPct((prev) => ({ ...prev, [step.id]: true }));
                                  setShotZoom((prev) => ({ ...prev, [step.id]: 1 }));
                                  setShotZoomCenter((prev) => ({ ...prev, [step.id]: false }));
                                } else {
                                  // 顶栏态/微调态 → 100% 整图：复位倍率与缩放原点，显示 before 原图
                                  setShotAtGlobalPct((prev) => ({ ...prev, [step.id]: false }));
                                  setShowFullShot((prev) => ({ ...prev, [step.id]: true }));
                                  setShotZoom((prev) => ({ ...prev, [step.id]: 1 }));
                                  setShotZoomCenter((prev) => ({ ...prev, [step.id]: false }));
                                }
                              }}
                              aria-label={isShotVisFull(step.id) ? "还原当前图片比例" : "查看全图（100%）"}
                            >
                              {isShotVisFull(step.id) ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
                            </button>
                          </div>
                        </div>
                      ) : (
                        <div
                          className="sop-edit-shot sop-edit-shot--empty"
                          onClick={() => {
                            if (selectedStepId !== step.id) setSelectedStepId(step.id);
                            pickMediaForStep(step.id);
                          }}
                          onContextMenu={(e) => pasteForStep(e, step.id)}
                          onDragOver={(e) => { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; }}
                          onDrop={(e) => { e.preventDefault(); dropMediaForStep(step.id, e.dataTransfer); }}
                        >
                          <span className="sop-edit-shot-type">{step.action_type || ""}</span>
                          <span className="sop-edit-shot-hint">点击或拖拽上传 · 截图 / 视频 / 动图 · 右键粘贴 Ctrl+V</span>
                        </div>
                      )}

                      {/* 图注行（第二十六轮 · 设计稿的 .shot figcaption）——————
                          截图正下方那一行 11px 小灰字：左边是图注（老数据照常显示，
                          没图注就退回窗口标题），右边是原图尺寸 + 「点开看大图」。
                          图注不再是可点入口（编辑入口第十九轮就撤了）。 */}
                      {(src || isVideoStep(step)) && (
                        <div className="sop-edit-caption">
                          <span className="sop-cap-text">
                            {step.image_caption?.trim() || step.window_title?.trim() || ""}
                          </span>
                          <span className="sop-cap-dim">
                            {(() => {
                              const nat = shotNat[step.id];
                              const dim = nat ? `${nat.w} × ${nat.h}` : "";
                              const tail = isVideoStep(step) ? "" : "点开看大图";
                              return dim && tail ? `${dim} · ${tail}` : (dim || tail);
                            })()}
                          </span>
                        </div>
                      )}

                      {/* 补充说明：就地可编辑。第十六轮（对标参考产品）：
                          类型 chip「描述 / 标注 / 警告」挪到**输入框同一行的右侧**（原来压在输入框上面占一整行），
                          正文是一圈浅边框的圆角输入框，选 标注 / 警告 时整框换成对应色。 */}
                      <div className={`sop-edit-note is-${kind || "desc"}`}>
                        <textarea
                          className="sop-edit-desc"
                          rows={1}
                          value={d.description}
                          placeholder={NOTE_PLACEHOLDER[kind]}
                          onChange={(e) => {
                            autoGrow(e.currentTarget);
                            setDraftField(step.id, "description", e.target.value);
                          }}
                          onFocus={(e) => {
                            autoGrow(e.currentTarget);
                            if (selectedStepId !== step.id) setSelectedStepId(step.id);
                          }}
                        />
                        {/* 类型 chip 排在行尾（DOM 放在输入框之后，Tab 顺序与视觉一致） */}
                        <div className="sop-note-kinds">
                          {NOTE_KINDS.map(({ id: k, label, tip }) => (
                            <button
                              key={k || "desc"}
                              type="button"
                              className={`sop-note-kind${kind === k ? " is-on" : ""}`}
                              onClick={(e) => { e.stopPropagation(); void setNoteKindFor(step, k); }}
                              data-tip={tip}
                              aria-label={tip}
                            >
                              {label}
                            </button>
                          ))}
                        </div>
                      </div>

                      {/* 提示（AI 给的补充说明，默认收起）。
                          ⚠️「变量」控件已于第十三轮按要求**整体移除**：所有步骤都不再显示
                          那颗 ✓变量 + 参数名输入框。数据层 is_variable / param_name 保留不动，
                          导出 SKILL.md 的既有行为不变，只是编辑器里不再暴露这个入口。 */}
                      {step.tip && (
                        <div className="sop-edit-meta">
                          <button
                            type="button"
                            className="sop-edit-tip-toggle"
                            onClick={(e) => {
                              e.stopPropagation();
                              setOpenTips((p) => ({ ...p, [step.id]: !p[step.id] }));
                            }}
                            data-tip={openTips[step.id] ? "收起 AI 给的补充说明" : "展开 AI 给的补充说明"}
                          >
                            {openTips[step.id] ? <ChevronDown size={13} /> : <ChevronRight size={13} />} 提示
                          </button>
                        </div>
                      )}
                      {step.tip && openTips[step.id] && (
                        <div className="sop-preview-tip"><strong>提示：</strong>{step.tip}</div>
                      )}

                      <div className="sop-edit-foot">
                        {isSel && autoSaved && (
                          <span className="sop-edit-saved"><Check size={12} /> 已自动保存</span>
                        )}
                      </div>

                      {idx < steps.length - 1 && <hr className="sop-step-divider" />}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>
      </div>
      {/* ↑ 收 .editor-zoom（两栏缩放区） */}

      {/* ── 左栏颗粒 ＋ 的插入菜单（第十三轮）────────────────────
          步骤 / 大标题 / 简介 / 导入已有教程 / 续录；点空白处关掉。 */}
      {insertMenu &&
        createPortal(
          <>
            <div className="insert-menu-backdrop" onMouseDown={() => setInsertMenu(null)} />
            <div
              className="insert-menu"
              style={{ left: insertMenu.x, top: insertMenu.y }}
              onMouseDown={(e) => e.stopPropagation()}
            >
              <button
                type="button"
                className="insert-menu-item"
                onClick={() => {
                  const i = insertMenu.index;
                  setInsertMenu(null);
                  void handleInsertStep(i);
                }}
              >
                <Plus size={14} /> 添加步骤
              </button>
              <button
                type="button"
                className="insert-menu-item"
                onClick={() => {
                  const i = insertMenu.index;
                  setInsertMenu(null);
                  void handleAddHeading(i + 1);
                }}
              >
                <Hash size={14} /> 添加大标题
              </button>
              <button
                type="button"
                className="insert-menu-item"
                onClick={() => {
                  const i = insertMenu.index;
                  setInsertMenu(null);
                  void handleAddIntro(i + 1);
                }}
              >
                <AlignLeft size={14} /> 添加简介
              </button>
              <button
                type="button"
                className="insert-menu-item"
                onClick={() => {
                  const i = insertMenu.index;
                  setInsertMenu(null);
                  void openImportPicker(i);
                }}
              >
                <FolderInput size={14} /> 导入已有教程
              </button>
              <div className="insert-menu-sep" />
              <button
                type="button"
                className="insert-menu-item"
                onClick={() => {
                  const i = insertMenu.index;
                  setInsertMenu(null);
                  if (onResumeInto) onResumeInto(recordingId, i);
                }}
              >
                <Camera size={14} />
                <span className="insert-menu-label">
                  续录
                  <span className="insert-menu-hint">从这里开始接着录</span>
                </span>
              </button>
            </div>
          </>,
          document.body
        )}

      {/* ── 导入已有教程：选一份别的教程，把它的步骤搬到当前位置之后 ── */}
      {importFor && (
        <div className="import-backdrop" onMouseDown={() => setImportFor(null)}>
          <div className="import-modal" onMouseDown={(e) => e.stopPropagation()}>
            <div className="import-head">
              <strong>导入已有教程</strong>
              <button
                type="button"
                className="import-close"
                onClick={() => setImportFor(null)}
                aria-label="关闭"
              >
                <X size={15} />
              </button>
            </div>
            <p className="import-sub">把它的步骤搬到当前位置之后（大标题不带过来）</p>
            <div className="import-list">
              {importList.length === 0 && <div className="import-empty">没有其它教程可导入</div>}
              {importList.map((r) => (
                <button
                  key={r.id}
                  type="button"
                  className="import-row"
                  disabled={importBusy}
                  onClick={() => void doImport(r.id)}
                >
                  <span className="import-row-title">{r.title || "未命名教程"}</span>
                  <span className="import-row-meta">{r.total_steps} 步</span>
                </button>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* ── Lightbox ──────────────────────────────────────── */}
      {lightboxOpen && (
        <Lightbox
          steps={realSteps}
          currentIndex={lightboxIndex}
          onClose={() => setLightboxOpen(false)}
          onPrev={lightboxPrev}
          onNext={lightboxNext}
          getMainSrc={lightboxSrcOf}
          showMarked={lightboxMarked}
          onToggleMarked={() => setLightboxMarked(!lightboxMarked)}
        />
      )}

      {/* ── Annotation overlay ────────────────────────────── */}
      {annotateOpen && (() => {
        const targetStep = steps.find((s) => s.id === annotateStepId);
        if (!targetStep) return null;
        // 标注固定使用 after 全图（screenshot_base64）
        const base64 = targetStep.screenshot_base64;
        if (!base64) return null;
        // 若当前处于裁图视图，把现有裁图锚点作为初始选区传入，方便在现有裁图上继续微调。
        // 5b 修复：x/y 是锚点**中心**，不能当矩形左上角直接传给 initialCropRect
        // （旧代码偏移半个框）；改传 initialAnchor，由 AnnotationsOverlay 用
        // cropBoxFromAnchor 换算成恒 50% 窗口（左上角 + 贴边夹回）。
        const hasAnchor = (targetStep.crop_w ?? 0) > 0 && (targetStep.crop_h ?? 0) > 0;
        const initialAnchor = hasAnchor
          ? { x: targetStep.x ?? 0, y: targetStep.y ?? 0, cropW: targetStep.crop_w ?? 0, cropH: targetStep.crop_h ?? 0 }
          : null;
        return (
          <AnnotationsOverlay
            imageUrl={`data:image/jpeg;base64,${base64}`}
            annotations={[]}
            actionType={targetStep.action_type}
            initialAnchor={initialAnchor}
            onSave={handleAnnotateSave}
            onClose={() => setAnnotateOpen(false)}
          />
        );
      })()}

      {/* Export format picker dialog —— 选完格式/选项直接导出到固定目录「文档\录步导出」 */}
      {exportOpen && (
        <ExportDialog
          isOpen={exportOpen}
          onClose={() => setExportOpen(false)}
          onSelect={(format, options) => handleExport(format, options)}
          steps={steps}
        />
      )}

      {/* Toast */}
      {toast && createPortal(
        <div
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            zIndex: 10001,
            display: "flex",
            alignItems: "flex-start",
            gap: "12px",
            maxWidth: "560px",
            padding: "12px 16px",
            borderRadius: "8px",
            fontSize: "16px",
            fontWeight: 500,
            boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
            background: toast.type === "success" ? "#fff7ed" : "#fef2f2",
            color: toast.type === "success" ? "#c2410c" : "#991b1b",
            border: toast.type === "success" ? "1px solid #fed7aa" : "1px solid #fecaca",
          }}
        >
          <span style={{ wordBreak: "break-word", lineHeight: 1.5 }}>{toast.message}</span>
          <button
            onClick={() => setToast(null)}
            title="关闭"
            style={{
              flexShrink: 0,
              width: 22,
              height: 22,
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              padding: 0,
              border: "none",
              borderRadius: "50%",
              background: "rgba(0,0,0,0.08)",
              color: "inherit",
              fontSize: 12,
              lineHeight: 1,
              cursor: "pointer",
            }}
          >
            ✕
          </button>
        </div>,
        document.body
      )}

      {/* 语音转写生成中提示条：录制跳编辑界面时若语音尚未生成则常驻顶部，生成好自动收起 */}
      {voiceBanner && createPortal(
        <div
          style={{
            position: "fixed",
            top: 52,
            left: "50%",
            transform: "translateX(-50%)",
            zIndex: 10000,
            display: "flex",
            alignItems: "center",
            gap: "10px",
            padding: "8px 14px",
            borderRadius: "8px",
            fontSize: "13px",
            fontWeight: 500,
            boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
            background: "#eef4ff",
            color: "#1d4ed8",
            border: "1px solid #bfdbfe",
          }}
        >
          <Loader2 size={15} className="spinner" />
          <span>语音转写生成中…步骤描述将自动补全，请稍候</span>
          <button
            onClick={() => setVoiceBanner(false)}
            style={{ background: "transparent", border: "none", cursor: "pointer", color: "#1d4ed8", fontSize: 14, lineHeight: 1, padding: 0, marginLeft: 2 }}
          >×</button>
        </div>,
        document.body
      )}

      {/* 查找 / 替换浮条（Ctrl+F / Ctrl+H，无按钮纯快捷键入口） */}
      {findOpen && createPortal(
        <div
          style={{
            position: "fixed", top: 52, right: 16, zIndex: 10000,
            display: "flex", flexDirection: "column", gap: 8,
            padding: "10px 12px", borderRadius: 10, width: 360,
            background: "#fff", border: "1px solid #e2e6ea",
            boxShadow: "0 8px 24px rgba(0,0,0,0.14)",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <input
              ref={findInputRef}
              value={findText}
              onChange={(e) => { setFindText(e.target.value); }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  findInputRef.current?.focus(); // 确保焦点始终在查找框（防外部抢走后 Enter 失灵）
                  const q = findText.trim();
                  if (q !== committedQuery.trim()) {
                    // 查询词有改动 → 先按回车提交搜索
                    setCommittedQuery(findText);
                    setMatchIdx(0);
                    return;
                  }
                  if (findMatches.length) {
                    setMatchIdx((i) => (e.shiftKey ? (i - 1 + findMatches.length) % findMatches.length : (i + 1) % findMatches.length));
                  }
                } else if (e.key === "Escape") {
                  e.preventDefault(); setFindOpen(false);
                }
              }}
              placeholder="查找步骤标题 / 描述…"
              autoFocus
              style={{ flex: 1, minWidth: 0, height: 30, padding: "0 10px", fontSize: 13, border: "1px solid #d0d5dd", borderRadius: 7, outline: "none" }}
            />
            <span style={{ fontSize: 12, color: findMatches.length ? "#57606a" : "#c0492c", minWidth: 44, textAlign: "center", flexShrink: 0 }}>
              {committedQuery.trim() ? (findMatches.length ? `${Math.min(matchIdx + 1, findMatches.length)}/${findMatches.length}` : "无结果") : "回车搜索"}
            </span>
            <button onClick={() => setMatchIdx((i) => (findMatches.length ? (i - 1 + findMatches.length) % findMatches.length : 0))} title="上一个 (Shift+Enter)" style={findNavBtnStyle}>↑</button>
            <button onClick={() => setMatchIdx((i) => (findMatches.length ? (i + 1) % findMatches.length : 0))} title="下一个 (Enter)" style={findNavBtnStyle}>↓</button>
            <button onClick={() => setFindOpen(false)} title="关闭 (Esc)" style={findNavBtnStyle}>×</button>
          </div>
          {replaceMode && (
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <input
                value={replaceText}
                onChange={(e) => setReplaceText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") { e.preventDefault(); void handleReplaceAll(); }
                  else if (e.key === "Escape") { e.preventDefault(); setFindOpen(false); }
                }}
                placeholder="替换为（留空 = 删除该内容）"
                style={{ flex: 1, minWidth: 0, height: 30, padding: "0 10px", fontSize: 13, border: "1px solid #d0d5dd", borderRadius: 7, outline: "none" }}
              />
              <button
                onClick={() => void handleReplaceAll()}
                disabled={replacing}
                style={{ height: 30, padding: "0 12px", fontSize: 12.5, fontWeight: 600, color: "#fff", background: "#f0521a", border: "none", borderRadius: 7, cursor: replacing ? "default" : "pointer", opacity: replacing ? 0.6 : 1, flexShrink: 0 }}
              >
                全部替换
              </button>
            </div>
          )}
          <div style={{ fontSize: 11, color: "#8a929c", lineHeight: 1.5 }}>
            输入后按 Enter 搜索 · 再按 Enter/Shift+Enter 上下切换 · Esc 关闭；替换作用于所有步骤的标题与描述（不分大小写），Ctrl+Z 可整体撤销
          </div>
        </div>,
        document.body
      )}

      {/* 上传网站弹窗：先选 发布/存草稿，再进表单（与浏览器插件一致） */}
      {showPublishDialog && createPortal(
        (() => {
          const closePub = () => { setShowPublishDialog(false); setPubStep("pick"); };
          // ★ 端云一体化 v247 回流：本地已记云端 slug ⇒ 弹窗里直接亮出链接（刷新后依然在）
          let cloudUrl = "";
          try { const slug = localStorage.getItem(webSlugKey(recordingId)) || ""; cloudUrl = webEditUrl(slug); } catch { /* 配额忽略 */ }
          const addApp = () => {
            const v = pubAppInput.trim();
            if (v && !pubApplications.includes(v)) setPubApplications([...pubApplications, v]);
            setPubAppInput("");
          };
          const removeApp = (a: string) => setPubApplications(pubApplications.filter((x) => x !== a));
          const addTag = () => {
            const v = pubTagInput.trim();
            if (v && !pubTags.includes(v)) setPubTags([...pubTags, v]);
            setPubTagInput("");
          };
          const removeTag = (t: string) => setPubTags(pubTags.filter((x) => x !== t));
          return (
            <div
              onClick={closePub}
              style={{
                position: "fixed", inset: 0, zIndex: 10000,
                background: "rgba(0,0,0,0.6)",
                display: "flex", alignItems: "center", justifyContent: "center",
                padding: 16,
              }}
            >
              <div
                onClick={(e) => e.stopPropagation()}
                style={{
                  width: 440, maxWidth: "100%", maxHeight: "88vh", overflowY: "auto",
                  background: "#0d1117", border: "1px solid #30363d", borderRadius: 10,
                  padding: 22, color: "#e6edf3", boxShadow: "0 12px 40px rgba(0,0,0,0.5)",
                }}
              >
                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 16 }}>
                  <div style={{ fontSize: 17, fontWeight: 600 }}>上传到 flowio.cn</div>
                  <button onClick={closePub} data-tip="关闭" style={{ background: "transparent", border: "none", color: "#8b949e", cursor: "pointer", padding: 4, borderRadius: 4 }}>
                    <X size={18} />
                  </button>
                </div>

                {pubStep === "pick" ? (
                  <div>
                    <div style={{ fontSize: 13, color: "#8b949e", marginBottom: 16, lineHeight: 1.6 }}>
                      将当前录屏教程上传到 flowio.cn。可直接发布，或先存为草稿稍后再发布。
                      {cloudUrl && (
                        <span style={{ display: "block", marginTop: 8, fontSize: 12.5 }}>
                          <CheckCircle2 size={12} style={{ verticalAlign: "-2px", marginRight: 4, color: "#3fb950" }} />
                          云端已保存：
                          <a
                            href={cloudUrl}
                            onClick={(e) => { e.preventDefault(); void invoke("open_external_url", { url: cloudUrl }); }}
                            style={{ color: "#58a6ff", textDecoration: "underline", cursor: "pointer" }}
                          >
                            {cloudUrl}
                          </a>
                        </span>
                      )}
                    </div>
                    <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                      <button
                        onClick={() => setPubStep("form")}
                        disabled={publishing}
                        data-tip="填写标题、涉及应用和分类后对外发布"
                        style={{
                          padding: "12px 16px", borderRadius: 8, border: "none",
                          background: "#3366CC", color: "#fff", cursor: publishing ? "default" : "pointer",
                          fontSize: 15, fontWeight: 600, textAlign: "left",
                        }}
                      >
                        发布
                        <div style={{ fontSize: 12, fontWeight: 400, opacity: 0.85, marginTop: 2 }}>填写标题、应用与分类后对外发布</div>
                      </button>
                      <button
                        onClick={() => handlePublish("draft")}
                        disabled={publishing}
                        data-tip="用录制信息兜底，先存到网站草稿箱，之后再去网站补充发布"
                        style={{
                          padding: "12px 16px", borderRadius: 8,
                          border: "1px solid #30363d", background: "transparent", color: "#e6edf3",
                          cursor: publishing ? "default" : "pointer", fontSize: 15, fontWeight: 600, textAlign: "left",
                        }}
                      >
                        {publishing ? "保存中..." : "存草稿"}
                        <div style={{ fontSize: 12, fontWeight: 400, opacity: 0.7, marginTop: 2 }}>用录制信息兜底，先保存到网站草稿箱</div>
                      </button>
                      {/* ★ r65「在网页编辑」（对齐 Scribe）：端内只负责录，排版 / 团队库 / 分享去网页 */}
                      <button
                        onClick={openInWebEditor}
                        disabled={publishing}
                        data-tip="到 flowio.cn 网页里排版、发布到团队库、生成分享链接"
                        style={{
                          padding: "12px 16px", borderRadius: 8,
                          border: "1px solid #30363d", background: "transparent", color: "#e6edf3",
                          cursor: publishing ? "default" : "pointer", fontSize: 15, fontWeight: 600, textAlign: "left",
                        }}
                      >
                        {WEB_EDIT_BUTTON}
                        <div style={{ fontSize: 12, fontWeight: 400, opacity: 0.7, marginTop: 2 }}>{WEB_EDIT_HINT}</div>
                      </button>
                    </div>
                  </div>
                ) : (
                  <div>
                    {/* ── 封面已移至底部（与插件统一顺序） ── */}

                    <label style={pubDialogStyles.label}>标题</label>
                    <input
                      style={pubDialogStyles.input}
                      value={pubTitle}
                      placeholder={recording?.title || "未命名教程"}
                      onChange={(e) => setPubTitle(e.target.value)}
                    />

                    <label style={pubDialogStyles.label}>场景描述</label>
                    <textarea
                      style={{ ...pubDialogStyles.input, height: 56, resize: "vertical" }}
                      value={pubSummary}
                      placeholder="一句话说明这篇教程适用的业务场景"
                      rows={2}
                      onChange={(e) => setPubSummary(e.target.value)}
                    />

                    <label style={pubDialogStyles.label}>涉及应用</label>
                    <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 6 }}>
                      {pubApplications.map((a) => (
                        <span key={a} style={{ display: "inline-flex", alignItems: "center", gap: 4, padding: "3px 8px", borderRadius: 14, background: "rgba(51,102,204,0.15)", color: "#93b4e8", fontSize: 12 }}>
                          {a}
                          <span onClick={() => removeApp(a)} style={{ cursor: "pointer", color: "#c9d4ea" }}>×</span>
                        </span>
                      ))}
                    </div>
                    <div style={{ display: "flex", gap: 6 }}>
                      <input
                        style={pubDialogStyles.input}
                        value={pubAppInput}
                        placeholder="如：Excel、Chrome"
                        onChange={(e) => setPubAppInput(e.target.value)}
                        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addApp(); } }}
                      />
                      <button style={pubDialogStyles.addBtn} onClick={addApp} data-tip="把输入框里的应用名加进去（也可直接回车）">添加</button>
                    </div>

                    <label style={pubDialogStyles.label}>标签（可选）</label>
                    <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginBottom: 6 }}>
                      {pubTags.map((t) => (
                        <span key={t} style={{ display: "inline-flex", alignItems: "center", gap: 4, padding: "3px 8px", borderRadius: 14, background: "rgba(51,102,204,0.15)", color: "#93b4e8", fontSize: 12 }}>
                          {t}
                          <span onClick={() => removeTag(t)} style={{ cursor: "pointer", color: "#c9d4ea" }}>×</span>
                        </span>
                      ))}
                    </div>
                    <div style={{ display: "flex", gap: 6 }}>
                      <input
                        style={pubDialogStyles.input}
                        value={pubTagInput}
                        placeholder="可选，回车添加"
                        onChange={(e) => setPubTagInput(e.target.value)}
                        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addTag(); } }}
                      />
                      <button style={pubDialogStyles.addBtn} onClick={addTag} data-tip="把输入框里的标签加进去（也可直接回车）">添加</button>
                    </div>

                    {/* 场景下拉已移除（v0.xx 与插件统一：改为自由文本「场景描述」） */}

                    <label style={pubDialogStyles.label}>分类</label>
                    <select
                      style={pubDialogStyles.input}
                      value={pubCategory}
                      onChange={(e) => setPubCategory(e.target.value)}
                    >
                      <option value="">请选择分类</option>
                      {categories.map((c) => (
                        <option key={c.id} value={c.id}>{c.name}</option>
                      ))}
                    </select>

                    {/* T9：发布到团队库（可选）。列表与「可发布」判定都来自服务端。 */}
                    <label style={pubDialogStyles.label}>
                      {TEAM_PUBLISH_LABEL}
                      <span style={{ marginLeft: 6, fontSize: 12, color: "#6e7681" }}>
                        {pubWorkspaces.length === 0
                          ? TEAM_PUBLISH_EMPTY_HINT
                          : "团队成员可在团队库里检索到这篇教程"}
                      </span>
                    </label>
                    <select
                      style={pubDialogStyles.input}
                      value={pubWorkspaceId}
                      onChange={(e) => setPubWorkspaceId(e.target.value)}
                      disabled={pubWorkspaces.length === 0}
                    >
                      <option value={TEAM_PUBLISH_NONE}>{TEAM_PUBLISH_NONE_LABEL}</option>
                      {pubWorkspaces.map((w) => (
                        <option key={w.id} value={w.id}>{teamWorkspaceLabel(w)}</option>
                      ))}
                    </select>

                    <label style={{ display: "flex", alignItems: "center", gap: 8, margin: "14px 0 4px", fontSize: 13, color: "#8b949e", cursor: "pointer" }}>
                      <input type="checkbox" checked={pubDownloadable === "yes"} onChange={(e) => setPubDownloadable(e.target.checked ? "yes" : "no")} />
                      允许他人下载源文件
                    </label>

                    {/* 封面（与插件统一：放在最后） */}
                    <label style={pubDialogStyles.label}>封面（可选）</label>
                    <div style={{ marginBottom: 8 }}>
                      {pubCoverPreview ? (
                        <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 8 }}>
                          <img src={toPreviewSrc(pubCoverPreview)} alt="封面预览" style={{ width: 120, height: 72, objectFit: "cover", borderRadius: 6, border: "2px solid #58a6ff" }} />
                          <span style={{ fontSize: 12, color: "#8b949e" }}>当前封面</span>
                        </div>
                      ) : (
                        <div style={{ fontSize: 12, color: "#8b949e", marginBottom: 8 }}>未选封面，将自动使用首张步骤截图</div>
                      )}
                      <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <label style={pubDialogStyles.addBtn} data-tip="上传自定义图片作为封面">
                          上传图片
                          <input
                            type="file"
                            accept="image/*"
                            style={{ display: "none" }}
                            onChange={(e) => {
                              const file = e.target.files?.[0];
                              if (!file) return;
                              const reader = new FileReader();
                              reader.onload = async () => {
                                const b64 = String(reader.result);
                                const small = await downscaleBase64Image(b64, 720, 0.85);
                                setPubCoverPreview(small);
                                setPubCoverBase64(small);
                              };
                              reader.readAsDataURL(file);
                            }}
                          />
                        </label>
                        <button
                          type="button"
                          onClick={handleSmartCover}
                          disabled={smartLoading || steps.filter((s) => s.screenshot_base64).length === 0}
                          style={pubDialogStyles.addBtn}
                          data-tip="自动从步骤截图里挑一张最有代表性的"
                        >
                          {smartLoading ? "分析中…" : "智能选封面"}
                        </button>
                        {pubCoverPreview && (
                          <button
                            type="button"
                            onClick={() => { setPubCoverBase64(null); setPubCoverPreview(null); }}
                            style={pubDialogStyles.addBtn}
                            data-tip="移除封面，改用默认首张截图"
                          >
                            清除
                          </button>
                        )}
                      </div>
                    </div>
                    {steps.filter((s) => s.screenshot_base64).length > 0 && (
                      <div style={{ marginTop: 8 }}>
                        <div style={{ fontSize: 12, color: "#8b949e", marginBottom: 6 }}>从步骤截图选择</div>
                        <select
                          style={{ ...pubDialogStyles.input, height: 34, cursor: "pointer" }}
                          value=""
                          onChange={(e) => {
                            const s = steps.find((x) => x.id === e.target.value);
                            if (s) void selectStepCover(s);
                          }}
                          data-tip="选中即设为封面，上方预览可见效果"
                        >
                          <option value="">选择步骤截图作为封面…</option>
                          {steps
                            .filter((s) => s.screenshot_base64)
                            .map((s) => (
                              <option key={s.id} value={s.id}>
                                第{s.step_number}步：{s.title || s.action_type}
                              </option>
                            ))}
                        </select>
                      </div>
                    )}

                    <div style={{ display: "flex", justifyContent: "space-between", marginTop: 18, gap: 10 }}>
                      <button
                        onClick={() => setPubStep("pick")}
                        disabled={publishing}
                        data-tip="回到上一步（已填的内容会保留）"
                        style={{ flex: 1, padding: "10px 0", borderRadius: 8, border: "1px solid #30363d", background: "transparent", color: "#e6edf3", cursor: publishing ? "default" : "pointer", fontSize: 14 }}
                      >
                        返回
                      </button>
                      <button
                        onClick={() => handlePublish("published")}
                        disabled={publishing}
                        data-tip="按上面填的信息发布到 flowio.cn，别人立刻能搜到"
                        style={{ flex: 2, padding: "10px 0", borderRadius: 8, border: "none", background: "#3366CC", color: "#fff", cursor: publishing ? "default" : "pointer", fontSize: 14, fontWeight: 600, display: "flex", alignItems: "center", justifyContent: "center", gap: 6 }}
                      >
                        {publishing && <Loader2 size={14} className="spinner" />}
                        {publishing ? "发布中..." : "直接发布"}
                      </button>
                    </div>
                  </div>
                )}
              </div>
            </div>
          );
        })(),
        document.body
      )}

      {/* 未登录 + 录制完成后的「保存到云端」登录引导弹窗 */}
      <LoginSaveModal
        open={showLoginSaveModal}
        onClose={handleLoginSaveModalClose}
        onLogin={handleLoginSaveModalLogin}
      />
    </div>
  );
}
