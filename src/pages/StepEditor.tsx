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
  ChevronUp,
  Trash2,
  FileCode,
  FileText,
  File,
  ImageIcon,
  ShieldCheck,
  ScanSearch,
  Settings as SettingsIcon,
  Wrench,
  Upload,
  Copy,
  Languages,
  PencilLine,
  Wand2,
  Undo2,
  Redo2,
} from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FileSaveDialog from "../components/FileSaveDialog";
import AnnotationsOverlay from "../components/AnnotationsOverlay";
import LoginSaveModal from "../components/LoginSaveModal";
import type { Annotation } from "../lib/annotate";
import { CROP_PAD, type CropAnchor } from "../lib/cropAnchor";
import { isUniformCropPixels } from "../lib/cropBlank";
import { previewBox, type PreviewBox } from "../lib/previewBox";
import type { Recording, Step } from "../types";
import { useAppStore } from "../store/appStore";

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
  skill: "导出 Agent Skills 标准的 SKILL.md，交给 CodeBuddy / Claude 等 AI 助手按步骤自动执行；标记了变量的步骤会变成可填参数",
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

/* ── Crop helper（Scribe-style 自适应裁剪）────────────── */

// 元素矩形外扩 padding（px）—— 8px 让文本贴边但留呼吸。
// 从 lib/cropAnchor 引入，与「手动框选裁图」的反算共用同一个取值，避免两处分叉。
/** 无元素矩形时，以点击点为中心裁屏幕的此比例 */
const CROP_FALLBACK_RATIO = 0.4;

/**
 * 将全图（screenshot_base64）按 Scribe 风格自适应裁剪：
 * - 整窗类型（scroll/nav/screenshot）：原样返回不裁剪；
 * - type：元素矩形左扩 1/3 宽，带出左侧 label（如"用户名/密码"）；
 * - 有元素矩形 (crop_w/crop_h，中心 x/y)：以矩形外扩 CROP_PAD 为基准，
 *   不足 CROP_MIN_SIZE 以中心外扩补足，超过屏幕 90% 等比缩放，保留宽高比；
 * - 无矩形但有点击坐标：以点击点为中心裁屏幕约 CROP_FALLBACK_RATIO 兜底；
 * - 其余情况回退原图。边界 clamp，输出 JPEG 质量 85 的 base64。
 */
function cropAdaptiveBase64(
  base64: string,
  x: number | null,
  y: number | null,
  cropW: number | null,
  cropH: number | null,
  actionType?: string,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      try {
        const W = img.naturalWidth;
        const H = img.naturalHeight;
        if (W <= 0 || H <= 0) {
          resolve(base64);
          return;
        }

        // Stage C: 整窗类型不裁剪，原样返回
        if (actionType === "scroll" || actionType === "nav" || actionType === "screenshot") {
          resolve(base64);
          return;
        }

        let left: number;
        let top: number;
        let cw: number;
        let ch: number;

        if (cropW != null && cropH != null && cropW > 0 && cropH > 0 && x != null && y != null) {
          // 严格按元素真实矩形裁切：尊重原宽高比，仅加 CROP_PAD 让文本贴边但不贴死。
          // 不强制最小/最大尺寸，避免把 URL 地址栏这种长条拉成大方块把文字糊掉。
          let rw = cropW + CROP_PAD * 2;
          let rh = cropH + CROP_PAD * 2;
          // Stage C: type 步骤左侧多留原始矩形宽的 1/3，把左侧 label 带进裁剪图。
          // ⚠️ 是 - extra / 2 不是 - extra：rw 已含 extra，居中项 x - rw/2 已在左右
          // 各分到 extra/2；再左移 extra 会过冲，使右侧留白变成 CROP_PAD - extra/2
          // （元素宽 > 48px 即为负，右边缘被切掉）。与 lib/cropAnchor 保持一致。
          if (actionType === "type") {
            const extra = Math.round(cropW / 3);
            rw += extra;
            left = Math.round(x - rw / 2 - extra / 2);
            top = Math.round(y - rh / 2);
          } else if (actionType === "drag") {
            // DRAG 步骤高度方向多留上下文：拖拽选区多为扁长（表格行 ~25px），
            // 仅 +16px（CROP_PAD*2）看不出在拖哪里。额外至少 100px（约 4 行）、
            // 并按选区高 0.6× 增长，让裁图带出周围行/列，用户能辨认拖拽位置与内容。
            const extraH = Math.max(Math.round(cropH * 0.6), 100);
            rh += extraH;
            top = Math.round(y - rh / 2);
            left = Math.round(x - rw / 2);
          } else {
            left = Math.round(x - rw / 2);
            top = Math.round(y - rh / 2);
          }
          cw = rw;
          ch = rh;
        } else if (x != null && y != null && x >= 0 && y >= 0) {
          // 无矩形兜底：以点击点为中心裁屏幕约 40%
          cw = Math.max(1, Math.round(W * CROP_FALLBACK_RATIO));
          ch = Math.max(1, Math.round(H * CROP_FALLBACK_RATIO));
          left = Math.round(x - cw / 2);
          top = Math.round(y - ch / 2);
        } else {
          resolve(base64);
          return;
        }

        // 贴边时整体平移进图内，而不是缩小裁剪框 —— 缩小会把内容切掉一半，
        // 且锚点被挤到一边（用户报：复制的地方不在裁图中间）。
        cw = Math.min(cw, W);
        ch = Math.min(ch, H);
        left = Math.min(Math.max(0, left), Math.max(0, W - cw));
        top = Math.min(Math.max(0, top), Math.max(0, H - ch));
        if (cw <= 0 || ch <= 0) {
          resolve(base64);
          return;
        }

        const canvas = document.createElement("canvas");
        canvas.width = cw;
        canvas.height = ch;
        const ctx = canvas.getContext("2d");
        if (!ctx) {
          resolve(base64);
          return;
        }
        ctx.drawImage(img, left, top, cw, ch, 0, 0, cw, ch);
        // 空白裁剪守卫：录制时 UIA 不可用会记到「Program Manager」宿主伪元素矩形，
        // 裁出纯灰空白、毫无信息量 —— 回退整图（见 lib/cropBlank.ts 同步清单）。
        const guard = ctx.getImageData(0, 0, cw, ch);
        if (isUniformCropPixels(guard.data, cw, ch)) {
          resolve(base64);
          return;
        }
        resolve(canvas.toDataURL("image/jpeg", 0.85).split(",")[1] || base64);
      } catch (err) {
        reject(err);
      }
    };
    img.onerror = () => reject(new Error("截图解码失败"));
    img.src = `data:image/jpeg;base64,${base64}`;
  });
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

export default function StepEditor({ siteToken, siteUser, voiceTranscribing }: StepEditorProps) {
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
  const [editTitle, setEditTitle] = useState("");
  const [editDescription, setEditDescription] = useState("");
  // 变量标记（SKILL.md 自动化导出用）：仅输入类步骤（type/paste）可标
  const [editIsVar, setEditIsVar] = useState(false);
  const [editParamName, setEditParamName] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [translateOpen, setTranslateOpen] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportProgress, setExportProgress] = useState<string | null>(null);
  const [redacting, setRedacting] = useState(false);
  // ★ v310: 脱敏结果弹窗（带 ×，不点不消失；与插件端对齐）
  const [redactResult, setRedactResult] = useState<{ title: string; lines: string[] } | null>(null);
  const [scanRedacting, setScanRedacting] = useState(false);
  const [scanProgress, setScanProgress] = useState<{ done: number; total: number; currentStepId: string }>({ done: 0, total: 0, currentStepId: "" });
  const [scanToast, setScanToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const scanToastTimerRef = useRef<number | null>(null);
  const [showSaveDialog, setShowSaveDialog] = useState<string | null>(null);
  const [toast, setToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const [voiceBanner, setVoiceBanner] = useState(voiceTranscribing ?? false); // 录制跳编辑界面时，若语音未好则顶部常驻提示
  const [aiGenerating, setAiGenerating] = useState(false);
  const [domainMode, setDomainMode] = useState<"auto" | "spreadsheet" | "browser" | "code" | "terminal">("auto");

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
  /* 首屏同步读取已保存的裁图/全图偏好，避免先全图再闪跳到裁图 */
  const readInitialView = (): { stepViewCropped: Record<string, boolean>; globalView: 'full' | 'crop' } => {
    let stepViewCropped: Record<string, boolean> = {};
    let globalView: 'full' | 'crop' = 'full';
    try {
      const savedView = localStorage.getItem(`flowio_view_${recordingId}`);
      if (savedView) {
        const parsed = JSON.parse(savedView);
        stepViewCropped = parsed.stepViewCropped || {};
        globalView = parsed.globalView || 'full';
      }
    } catch { /* ignore corrupt data */ }
    return { stepViewCropped, globalView };
  };
  /* 主图视图：false=全图（默认，screenshot_base64），true=裁剪图（点击坐标 400×400 裁剪） */
  const [stepViewCropped, setStepViewCropped] = useState<Record<string, boolean>>(
    () => (recordingId ? readInitialView().stepViewCropped : {})
  );
  const getViewCropped = (stepId: string) => stepViewCropped[stepId] ?? false;
  const setViewCropped = (stepId: string, value: boolean) => setStepViewCropped(prev => {
    const next = { ...prev, [stepId]: value };
    // 同步标题栏全局开关：若改动后所有步骤已一致，则让顶部高亮跟随，避免两处控件不同步
    const allSame = steps.every((s) => (next[s.id] ?? false) === value);
    if (allSame) setGlobalView(value ? 'crop' : 'full');
    return next;
  });
  /* 裁剪图是否已被用户删除（true=该步骤裁剪图视为不存在，仅影响裁图视图，全图不受影响） */
  const [deletedCrop, setDeletedCrop] = useState<Record<string, boolean>>({});
  const isCropDeleted = (stepId: string) => !!deletedCrop[stepId];
  /* 全局视图风格：'full'=全图，'crop'=裁剪图（顶部统一切换按钮高亮当前教程主要风格） */
  const [globalView, setGlobalView] = useState<'full' | 'crop'>(
    () => (recordingId ? readInitialView().globalView : 'full')
  );

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

  /* 裁剪图（400×400 @点击坐标）缓存：key=step.id，为所有步骤异步预生成，避免预览/缩略图重复计算 */
  const [cropCache, setCropCache] = useState<Record<string, string>>({});
  // 首屏视图恢复完成后才允许持久化，避免挂载瞬间用默认值 {} / 'full' 覆盖已保存的偏好
  const viewLoadedRef = useRef(false);

  /* Annotation (手动标注编辑器) */
  const [annotateOpen, setAnnotateOpen] = useState(false);
  const [annotateStepId, setAnnotateStepId] = useState<string>("");
  /** 打开标注时的当前视图：'full'=全图，'crop'=裁图。决定裁切结果落到哪个槽位 */
  const [annotateMode, setAnnotateMode] = useState<'full' | 'crop'>('full');

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

  /**
   * 重建裁图缓存。
   *
   * 🔴 精简/撤销后**必须**调用：合并会把末步的主图写到承载步上，但 step.id 不变，
   * 缓存里的还是合并前那张旧图 → 界面会显示「张冠李戴」的截图。
   * 与首次加载共用同一套预生成逻辑（先算完再一次提交，避免逐帧跳变）。
   */
  const rebuildCropCache = useCallback(async (list: Step[]) => {
    const cache: Record<string, string> = {};
    await Promise.all(
      list
        .filter((s) => s.screenshot_base64)
        .map(async (s) => {
          try {
            cache[s.id] = await cropAdaptiveBase64(
              s.screenshot_base64 as string,
              s.x, s.y, s.crop_w, s.crop_h, s.action_type,
            );
          } catch {
            cache[s.id] = "";
          }
        }),
    );
    setCropCache(cache);
  }, []);

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
  }, [recordingId, reloadRecording, rebuildCropCache]);

  /* ── ★ 步骤自动精简（规则层 / AI 层 / 撤销），定义在 reloadRecording 之后避免 deps TDZ ── */
  const handleAutoMerge = useCallback(async () => {
    if (!recordingId) return;
    setSimplifying("rules");
    try {
      const r = await invoke<{ before: number; after: number; reduced: number }>(
        "auto_merge_steps",
        { recordingId }
      );
      const list = await reloadRecording();
      await rebuildCropCache(list);
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
  }, [recordingId, reloadRecording, rebuildCropCache, refreshHist]);

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
        if (ev.payload.op !== "ai_simplify") return;
        setSimplifying(null);
        useAppStore.getState().setSimplifyingId(null);
        if (!ev.payload.ok) {
          // 精简失败（多在免费通道限流时）：编辑页内直接弹红条，避免只靠右上角 9s 卡片被忽略
          setToast({ type: "error", message: "AI 精简失败：" + ev.payload.summary });
          return;
        }
        if (ev.payload.ok) {
          const list = await reloadRecording();
          await rebuildCropCache(list);
          refreshHist();
        }
      }
    ).then((fn) => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; unlisten?.(); };
  }, [recordingId, reloadRecording, rebuildCropCache, refreshHist]);

  /* ── ★ 全局撤销/重做：还原后必须 rebuildCropCache（理由同精简）── */
  const applyHist = useCallback(async (cmd: "edit_undo" | "edit_redo") => {
    if (histBusy) return;
    setHistBusy(true);
    // 撤销/重做会改库里的 title/description：先掐掉待触发的自动保存，
    // 否则 800ms 后它会把刚撤销的内容原样写回去
    if (autoSaveRef.current) { clearTimeout(autoSaveRef.current); autoSaveRef.current = null; }
    try {
      const n = await invoke<number>(cmd, { recordingId });
      const list = await reloadRecording();
      await rebuildCropCache(list);
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
  }, [histBusy, recordingId, reloadRecording, rebuildCropCache, refreshHist]);

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
        let viewJson: string | null = dbView;
        let needMigrate = false;
        if (!viewJson) {
          try { viewJson = localStorage.getItem(`flowio_view_${recordingId}`); } catch { /* ignore */ }
          needMigrate = !!viewJson;
        }
        if (viewJson) {
          try {
            const parsed = JSON.parse(viewJson);
            setStepViewCropped(parsed.stepViewCropped || {});
            setGlobalView(parsed.globalView === "crop" ? "crop" : "full");
            if (needMigrate) {
              void invoke("editor_view_set", { recordingId, value: viewJson }).catch(() => { /* best-effort */ });
            }
          } catch { /* 坏数据忽略，用默认全图 */ }
        } else {
          setStepViewCropped({});
          setGlobalView("full");
        }
        viewLoadedRef.current = true;

        // 关键：在 setSteps 之前预生成**所有步骤**的裁剪图缓存，然后单次提交。
        // 这样首次渲染就是最终状态（spinner → 裁图/全图），不存在
        // "灰框→裁图"或"全图→裁图"的任何中间帧。
        // （此前是 setSteps 先渲染、cropCache 异步逐个填充 → 每步都跳一次。）
        const cache: Record<string, string> = {};
        await Promise.all(
          data.steps
            .filter((s) => s.screenshot_base64)
            .map(async (s) => {
              try {
                cache[s.id] = await cropAdaptiveBase64(
                  s.screenshot_base64 as string,
                  s.x, s.y, s.crop_w, s.crop_h, s.action_type,
                );
              } catch {
                cache[s.id] = "";
              }
            }),
        );
        if (cancelled) return;

        setCropCache(cache);
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
        if (!cancelled) setLoading(false);
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

  /* 裁剪图缓存：为所有有截图的步骤异步预生成（key=step.id），steps/坐标变化时重算 */
  useEffect(() => {
    let cancelled = false;
    const stepsWithImg = steps.filter((s) => s.screenshot_base64);
    // 清理已无截图步骤的残留缓存
    setCropCache((prev) => {
      const next: Record<string, string> = {};
      Object.keys(prev).forEach((k) => {
        const st = steps.find((s) => s.id === k);
        if (st && st.screenshot_base64) next[k] = prev[k];
      });
      return next;
    });
    stepsWithImg.forEach((step) => {
      cropAdaptiveBase64(step.screenshot_base64 as string, step.x, step.y, step.crop_w, step.crop_h, step.action_type)
        .then((b64) => { if (!cancelled) setCropCache((prev) => ({ ...prev, [step.id]: b64 })); })
        .catch(() => { if (!cancelled) setCropCache((prev) => ({ ...prev, [step.id]: "" })); });
    });
    return () => { cancelled = true; };
  }, [steps.map((s) => `${s.id}|${s.screenshot_base64}|${s.x}|${s.y}|${s.crop_w}|${s.crop_h}|${s.action_type}`).join("||")]);

  /* 持久化裁图/全图视图偏好：localStorage（首屏同步缓存）+ SQLite（权威存储，
     WebView 存储曾出现丢失 → 退出重进回全图；DB 写入 fire-and-forget，失败不阻塞 UI） */
  useEffect(() => {
    if (!recordingId) return;
    // 首屏视图恢复前不写，防止挂载瞬间用默认值覆盖已保存偏好
    if (!viewLoadedRef.current) return;
    const json = JSON.stringify({ stepViewCropped, globalView });
    try {
      localStorage.setItem(`flowio_view_${recordingId}`, json);
    } catch { /* storage full / unavailable */ }
    void invoke("editor_view_set", { recordingId, value: json }).catch(() => { /* best-effort */ });
  }, [recordingId, stepViewCropped, globalView]);

  const selectedIndex = steps.findIndex((s) => s.id === selectedStepId);

  /* 主截图解析：全图（screenshot_base64）或裁剪图（缓存）；裁剪图已删或全图缺失时返回 null */
  const getStepMainSrc = (step: Step): string | null => {
    const base = step.screenshot_base64;
    if (!base) return null;
    const useCrop = getViewCropped(step.id);
    if (useCrop && isCropDeleted(step.id)) return null;
    if (useCrop) {
      const cropped = cropCache[step.id];
      // 裁图缓存已就绪：返回裁剪结果（空字符串=裁剪失败→返回null隐藏）
      if (cropped !== undefined) return cropped || null;
      // 裁图缓存尚未就绪（首次加载/cold start）：返回null而非全图，
      // 避免"全图→裁图"闪跳。用户会看到短暂空白后裁图出现。
      return null;
    }
    return base;
  };

  /* 顶部统一切换：一键将所有步骤的主截图设为全图或裁剪图（教程主要风格） */
  const applyGlobalView = (mode: 'full' | 'crop') => {
    setGlobalView(mode);
    setStepViewCropped((prev) => {
      const next: Record<string, boolean> = {};
      steps.forEach((s) => { next[s.id] = mode === 'crop'; });
      return next;
    });
  };

  /* ── Sync edits when selection changes ──────────────────── */

  useEffect(() => {
    if (selectedStep) {
      setEditTitle(selectedStep.title);
      setEditDescription(selectedStep.description);
      setEditIsVar(!!selectedStep.is_variable);
      setEditParamName(selectedStep.param_name || "");
      setTipExpanded(false);
    }
  }, [selectedStepId, steps]);

  /* ── Save step ───────────────────────────────────────────── */

  const handleSaveStep = useCallback(async () => {
    if (!selectedStep) return;
    setSaving(true);
    setSaveSuccess(false);
    try {
      await invoke("update_step", {
        stepId: selectedStep.id,
        title: editTitle,
        description: editDescription,
        actionType: selectedStep.action_type,
        tip: selectedStep.tip || "",
      });
      setSteps((prev) =>
        prev.map((s) =>
          s.id === selectedStep.id
            ? { ...s, title: editTitle, description: editDescription }
            : s
        )
      );
      setSaveSuccess(true);
      setTimeout(() => setSaveSuccess(false), 2000);
      refreshHist();
    } catch (err) {
      console.error("Save step failed:", err);
    } finally {
      setSaving(false);
    }
  }, [selectedStep, editTitle, editDescription, refreshHist]);

  /* ── 变量标记（SKILL.md 自动化导出用）──────────────────────
     独立于标题/描述自动保存：切换立即落库，参数名失焦时落库，
     避免与 800ms 自动保存互相覆盖。老调用方不传变量字段，
     Rust 端 COALESCE 保留现值。 */

  const persistVariable = useCallback(
    async (isVar: boolean, paramName: string) => {
      if (!selectedStep) return;
      try {
        await invoke("update_step", {
          stepId: selectedStep.id,
          title: editTitle,
          description: editDescription,
          actionType: selectedStep.action_type,
          tip: selectedStep.tip || "",
          isVariable: isVar,
          paramName,
        });
        setSteps((prev) =>
          prev.map((s) =>
            s.id === selectedStep.id
              ? { ...s, is_variable: isVar, param_name: paramName }
              : s
          )
        );
      } catch (err) {
        console.error("Save variable mark failed:", err);
      }
    },
    [selectedStep, editTitle, editDescription]
  );

  const toggleVariable = useCallback(() => {
    if (!selectedStep) return;
    const next = !editIsVar;
    setEditIsVar(next);
    let name = editParamName.trim();
    if (next && !name) {
      // 默认参数名：param_1 / param_2 …（避开本教程已有的变量名）
      const used = new Set(
        steps.filter((s) => s.is_variable && s.param_name).map((s) => s.param_name)
      );
      let i = 1;
      while (used.has(`param_${i}`)) i += 1;
      name = `param_${i}`;
      setEditParamName(name);
    }
    void persistVariable(next, name);
  }, [selectedStep, editIsVar, editParamName, steps, persistVariable]);

  /* ── Auto-save (debounce 800ms) ─────────────────────────── */
  const [autoSaved, setAutoSaved] = useState(false);

  useEffect(() => {
    if (!selectedStep) return;
    // 仅当值与原始步骤不同时才触发自动保存
    const titleChanged = editTitle !== selectedStep.title;
    const descChanged = editDescription !== selectedStep.description;
    if (!titleChanged && !descChanged) return;

    setAutoSaved(false);
    if (autoSaveRef.current) clearTimeout(autoSaveRef.current);
    autoSaveRef.current = setTimeout(async () => {
      try {
        await invoke("update_step", {
          stepId: selectedStep.id,
          title: editTitle,
          description: editDescription,
          actionType: selectedStep.action_type,
          tip: selectedStep.tip || "",
        });
        setSteps((prev) =>
          prev.map((s) =>
            s.id === selectedStep.id
              ? { ...s, title: editTitle, description: editDescription }
              : s
          )
        );
        setAutoSaved(true);
        setTimeout(() => setAutoSaved(false), 1500);
        refreshHist();
      } catch (err) {
        console.error("Auto-save failed:", err);
      }
    }, 800);
    return () => { if (autoSaveRef.current) clearTimeout(autoSaveRef.current); };
  }, [editTitle, editDescription, selectedStep, refreshHist]);

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
            // 上传新图后恢复裁图可用并切回全图视图，让用户看到上传的图
            setDeletedCrop(prev => ({ ...prev, [selectedStep.id]: false }));
            setViewCropped(selectedStep.id, false);
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
                        // 上传新图后恢复裁图可用并切回全图视图
                        setDeletedCrop(prev => ({ ...prev, [selectedStep.id]: false }));
                        setViewCropped(selectedStep.id, false);
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
      setExportOpen(false);
      setToolsOpen(false);
      setSimplifyOpen(false);
      setTranslateOpen(false);
    };
    window.addEventListener("mousedown", closeDropdowns);
    return () => window.removeEventListener("mousedown", closeDropdowns);
  }, []);

  /* ── Export ───────────────────────────────────────────────── */

  const handleExport = useCallback(
    (format: string) => {
      setExportOpen(false);
      setShowSaveDialog(format);
    },
    []
  );

  const handleSaveConfirm = useCallback(
    async (savedPath: string) => {
      const format = showSaveDialog!;
      setShowSaveDialog(null);
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
        const path = await invoke<string>("export_recording", {
          id: recordingId,
          format,
          outputPath: savedPath,
          cropStepIds: Object.keys(stepViewCropped).filter(id => stepViewCropped[id]),
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
    [recordingId, showSaveDialog, stepViewCropped]
  );

/* ── Publish to website ──────────────────────────────────── */

// SHA-256 hex（与网站后端 content_hash 同口径），用于发布前按内容兜底查已存在教程
async function sha256Hex(s: string): Promise<string> {
  const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return Array.from(new Uint8Array(buf)).map((b) => b.toString(16).padStart(2, "0")).join("");
}

const handlePublish = useCallback(async (targetStatus: "draft" | "published" = "published") => {
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
      const app_name = applications[0] || (isDraft ? (recording?.app_name || "网页应用") : "");
      // 发布（表单路径）需校验必填；草稿放宽，用录制信息兜底
      if (!isDraft && !title) { setToast({ type: "error", message: "请填写标题" }); return; }
      if (!isDraft && applications.length === 0) { setToast({ type: "error", message: "请至少填写一个涉及应用" }); return; }
      if (!isDraft && !pubCategory) { setToast({ type: "error", message: "请选择分类" }); return; }

      // 标签直接发送（不再合并场景名称——场景下拉已移除，改为自由文本「场景描述」）
      const tagsToSend = pubTags;

      // 发布=所见即所得：步骤视图为「裁图」时上传裁剪结果（与编辑器显示、导出 cropStepIds 同一视图偏好），
      // 整窗类型（scroll/nav/screenshot）cropAdaptiveBase64 内部自动原样返回；裁剪失败回退全图，不阻塞发布。
      // 压缩每张截图，显著降低上传体积（修复"保存草稿/发布失败"：完整截图会导致请求体过大/超时）
      const steps = await Promise.all(
        data.steps.map(async (s) => {
          const useCrop = getViewCropped(s.id) && !isCropDeleted(s.id) && !!s.screenshot_base64;
          let shotB64: string | undefined = s.screenshot_base64 || undefined;
          if (useCrop && shotB64) {
            try {
              shotB64 = await cropAdaptiveBase64(shotB64, s.x, s.y, s.crop_w, s.crop_h, s.action_type);
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
        // 封面=所见即所得（与步骤截图同一视图逻辑）：未手动选封面时，取首个有主图的步骤按其视图偏好解析
        // ——该步视图为「裁图」传裁剪图，「全图」传全图；整窗类型自动保持全图；
        // 该步无主图但有 after 图时回退 after（不裁）。任何失败回退全图，不阻塞发布。
        const firstImg = data.steps.find((s) => s.screenshot_base64);
        let coverSrc: string | undefined = firstImg?.screenshot_base64 || undefined;
        if (coverSrc && firstImg && getViewCropped(firstImg.id) && !isCropDeleted(firstImg.id)) {
          try {
            coverSrc = await cropAdaptiveBase64(coverSrc, firstImg.x, firstImg.y, firstImg.crop_w, firstImg.crop_h, firstImg.action_type);
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
      // 记录「录制 → 已发布教程」映射：下次发布同一录制时覆盖更新而非新建
      if (result.guide?.id) {
        try { localStorage.setItem(`flowio_pub_${recordingId}`, String(result.guide.id)); } catch { /* ignore */ }
      }
      // ★ 保存发布表单快照：下次重发同一录制时自动恢复填写内容，免重复输入
      try {
        const formSnap = {
          guideId: result.guide?.id || "",
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
      const okMsg = isDraft
        ? (isUpdateMode ? "草稿已更新！" : "草稿已保存！")
        : (isUpdateMode ? "教程已更新，点赞/评论/收藏已保留" : "发布成功！教程已上传到网站");
      setToast({ type: "success", message: okMsg });
    } catch (err) {
      const isAbort = (err as { name?: string })?.name === "AbortError";
      setToast({ type: "error", message: isAbort ? "发布超时（90秒），请检查网络后重试" : "发布失败: " + String(err) });
    } finally {
      setPublishing(false);
    }
  }, [siteToken, siteUser, recording, recordingId, categories, pubTitle, pubApplications, pubCategory, pubTags, pubDownloadable, pubCoverBase64, pubSummary, translations, stepViewCropped, deletedCrop]);

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
      await rebuildCropCache(list);
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
  }, [recordingId, reloadRecording, rebuildCropCache, refreshHist]);

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
        if (list2) await rebuildCropCache(list2).catch(() => {});
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
  }, [recordingId, reloadRecording, rebuildCropCache, showScanToast]);

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
      const list = await reloadRecording();
      await rebuildCropCache(list);
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
  }, [recordingId, reloadRecording, rebuildCropCache, refreshHist]);

  /* ── Lightbox helpers ────────────────────────────────────── */

  const openLightbox = (index: number) => {
    setLightboxIndex(index);
    setLightboxMarked(false);
    setLightboxOpen(true);
  };

  const lightboxPrev = () =>
    setLightboxIndex((i) => Math.max(0, i - 1));
  const lightboxNext = () =>
    setLightboxIndex((i) => Math.min(steps.length - 1, i + 1));

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
    // 记录当前视图：从「裁图」视图打开 → 此后裁切顶替裁图；从「全图」打开 → 顶替全图
    setAnnotateMode(getViewCropped(stepId) ? 'crop' : 'full');
    setAnnotateStepId(stepId);
    setAnnotateOpen(true);
  };

  /**
   * 标注完成——按打开标注时的视图（annotateMode）决定裁切结果落到哪个槽位：
   * - 全图视图裁剪（hasCrop && mode==='full'）：
   *     把「注释+裁切」的结果直接覆盖全图（screenshot_path），并清空裁图锚点。
   *     原全图不保留（应用内可撤销）。
   * - 裁图视图重裁（hasCrop && mode==='crop'）：
   *     注释烧回全图（save_step_screenshot_full，原全图不动）+ 更新裁图锚点（顶替之前的裁图）。
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
      if (hasCrop && annotateMode === 'full') {
        // 全图视图裁剪 → 裁切结果顶替全图，并清空裁图锚点
        const base64 = cropped!.split(",")[1] || "";
        if (base64) {
          await invoke("save_step_screenshot_full", { stepId, base64Data: base64 });
        }
        await invoke("update_step_crop", { stepId, x: null, y: null, cropW: null, cropH: null });
      } else if (hasCrop && annotateMode === 'crop') {
        // 裁图视图重裁 → 注释烧回全图（原全图不动）+ 更新裁图锚点（顶替之前的裁图）
        if (full) {
          const base64 = full.split(",")[1] || "";
          if (base64) {
            await invoke("save_step_screenshot_full", { stepId, base64Data: base64 });
          }
        }
        await invoke("update_step_crop", {
          stepId,
          x: anchor!.x,
          y: anchor!.y,
          cropW: anchor!.cropW,
          cropH: anchor!.cropH,
        });
      } else if (full) {
        // 仅标注（无裁切）→ 注释烧回全图
        const base64 = full.split(",")[1] || "";
        if (base64) {
          await invoke("save_step_screenshot_full", { stepId, base64Data: base64 });
        }
      }
      // 重新从库加载步骤 + 重建裁图缓存（screenshot_base64 与锚点可能都变了）
      const list = await reloadRecording();
      await rebuildCropCache(list);
      setToast({ type: "success", message: "标注已保存" });
      refreshHist();
    } catch (err) {
      console.error("保存标注失败:", err);
      setToast({ type: "error", message: "保存标注失败" });
    }
    setAnnotateOpen(false);
  };

  /* ── Loading / Error ─────────────────────────────────────── */

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

  /* ── Portal toolbar into titlebar ─────────────────────── */

  const titlebarLeft = (
    <div style={{ display: "flex", alignItems: "center", gap: 8, height: "100%", overflow: "hidden" }}>
      <button
        onClick={() => navigate("/")}
        data-tip={"返回录制列表\n改动已自动保存"}
        style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px 4px 2px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
      >
        <ArrowLeft size={14} /> 返回
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
      {!previewMode && (
        <button
          onClick={() => void handleSaveAs()}
          data-tip="另存为副本：复制一份完全相同的教程（含截图），改副本不会破坏原教程"
          style={{ background: "transparent", border: "1px solid #8b5cf6", color: "#a78bfa", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          <Copy size={14} /> 另存为
        </button>
      )}
      <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
        <button
          onClick={() => { setExportOpen(!exportOpen); setToolsOpen(false); setTranslateOpen(false); setSimplifyOpen(false); }}
          disabled={exporting}
          data-tip={"导出教程\n导出为 Word / PDF / HTML / Markdown 等；要用译文导出，先在「翻译」里选好导出语言"}
          style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          <Download size={14} /> {exportProgress ? exportProgress : exporting ? "导出中..." : "导出"} <ChevronDown size={12} />
        </button>
        {exportOpen && (
          <div className="export-dropdown" style={{ right: 0 }}>
            {/* 导出语言已迁到「翻译」下拉，这里只保留格式 */}
            {EXPORT_FORMATS.map((fmt) => {
              const Icon = fmt.icon;
              return (
                <button
                  key={fmt.value}
                  className="export-dropdown-item"
                  onClick={() => handleExport(fmt.value)}
                  data-tip={EXPORT_TIPS[fmt.value]}
                >
                  <Icon size={14} className="export-dropdown-item-icon" />
                  <span className="export-dropdown-item-label">{fmt.label}</span>
                </button>
              );
            })}
          </div>
        )}
      </div>
      {!previewMode && (
        <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
          <button
            onClick={() => { setToolsOpen(!toolsOpen); setExportOpen(false); setTranslateOpen(false); setSimplifyOpen(false); }}
            data-tip={"更多工具\n上传网站、AI 生成步骤说明、敏感信息脱敏"}
            style={{ background: "transparent", border: "1px solid #ff6b35", color: "#ff8a5c", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
          >
            <Wrench size={14} /> 工具 <ChevronDown size={12} />
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
                  // ★ 恢复上次发布填写的表单信息（重发免重填）；无快照时用默认值
                  let pubFormRestored = false;
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
                }}
                data-tip={siteToken
                  ? "把这篇教程上传到 flowio.cn\n可先存草稿，也可填写标题/应用/分类后直接发布"
                  : "登录后才能上传到 flowio.cn（点击去登录）"}
              >
                <Upload size={14} className="export-dropdown-item-icon" />
                <span className="export-dropdown-item-label">{siteToken ? "上传网站" : "登录后上传"}</span>
              </button>
              {/* 另存为已移至顶栏按钮，此处不重复 */}
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
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); handleRedact(); }}
                disabled={redacting}
                data-tip={"文本脱敏\n把步骤文字里的手机号、身份证、邮箱、银行卡等换成占位符（截图不变）"}
              >
                {redacting ? <Loader2 size={14} className="spinner" /> : <ShieldCheck size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">{redacting ? "脱敏中..." : "文本脱敏"}</span>
              </button>
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
      {!previewMode && (
        <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
          <button
            onClick={() => { setTranslateOpen(!translateOpen); setExportOpen(false); setToolsOpen(false); setSimplifyOpen(false); }}
            disabled={translating}
            data-tip={previewMode
              ? "切换预览语言\n在这个下拉里选「原文」或某种译文，预览会立刻跟着换"
              : "翻译\n点语言把标题和每步描述翻译过去；已翻译过的语言点一下立即切换，不用重新翻译\n选「原文」随时切回翻译前的文字\n译文存成副本，也决定导出语言"}
            style={{ background: "transparent", border: "1px solid #3366CC", color: "#7aa2f7", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4, opacity: translating ? 0.6 : 1 }}
          >
            {translating ? <Loader2 size={14} className="spinner" /> : <Languages size={14} />} 翻译 <ChevronDown size={12} />
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
        onClick={() => setPreviewMode(!previewMode)}
        data-tip={previewMode ? "退出预览，回到编辑" : "预览成品效果\n按读者看到的样子通篇预览（导出/上传前先看一眼）"}
        style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
      >
        {previewMode ? <><X size={14} /> 退出预览</> : <><Eye size={14} /> 预览</>}
      </button>

      {/* 全图 / 裁图 统一切换：作用于全部步骤（选择教程主要截图风格） */}
      <div className="titlebar-view-segmented">
        <button
          className={`titlebar-view-segmented-btn${globalView === 'full' ? ' active' : ''}`}
          onClick={() => applyGlobalView('full')}
        >全图</button>
        <button
          className={`titlebar-view-segmented-btn${globalView === 'crop' ? ' active' : ''}`}
          onClick={() => applyGlobalView('crop')}
        >裁图</button>
      </div>
    </div>
  );

  /* ── Render ──────────────────────────────────────────────── */

  /* 预览模式取文案：翻译已直接同步到步骤，预览直接用步骤当前文本 */
  const previewStepText = (step: Step): { title: string; description: string } => {
    return { title: step.title, description: step.description || "" };
  };

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


      {/* ★ T8 译文可编辑弹窗 */}

      {/* Left: Step List */}
      <div className="editor-sidebar">
        <div className="text-sm text-secondary mb-2" style={{ padding: "0 16px", display: "flex", alignItems: "center", gap: 8 }}>
          <span style={{ marginRight: "auto" }}>{steps.length} 个步骤</span>
        </div>

        {/* 步骤精简：一个下拉入口收纳「AI 精简 / 仅规则合并」。
            规则合并录完已自动跑过，这里的入口用于存量录制补跑与手动重跑。
            撤销已上移为顶栏全局 ↶↷（可撤销一切编辑，不只是精简）。 */}
        {!previewMode && steps.length > 0 && (
          <div className="step-simplify-bar">
            <div style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
              <button
                className="step-simplify-btn primary"
                data-tip={"把啰嗦的步骤合并掉\n录完会自动跑一遍；这里也可以手动重跑或换用 AI"}
                disabled={simplifying !== null || aiBusy}
                onClick={() => { setSimplifyOpen(!simplifyOpen); setExportOpen(false); setToolsOpen(false); setTranslateOpen(false); }}
              >
                {aiBusy ? <Loader2 size={12} className="spinner" /> : <Sparkles size={12} />}
                {aiBusy ? "AI 精简中…"
                  : simplifying === "rules" ? "规则合并中…"
                  : "精简"}
                <ChevronDown size={11} style={{ opacity: 0.8 }} />
              </button>
              {simplifyOpen && (
                <div className="export-dropdown step-simplify-menu">
                  <button
                    className="export-dropdown-item"
                    onClick={() => { setSimplifyOpen(false); void handleAiSimplify(); }}
                    disabled={simplifying !== null || aiBusy}
                    data-tip={"AI 精简（推荐）\n先跑本地规则，再让 AI 按语义把重复步骤压成一步\n能处理规则看不懂的情况（比如弹框里坐标各异的一串点击），需要几秒到十几秒"}
                  >
                    {aiBusy ? <Loader2 size={14} className="spinner" /> : <Sparkles size={14} className="export-dropdown-item-icon" />}
                    <span className="export-dropdown-item-label">AI 精简（推荐）</span>
                  </button>
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
          </div>
        )}

        {/* Step list */}
        <div className="step-list">
          {/* Insert zone before first step */}
          <div className="step-insert-point">
            <div className="step-insert-line" />
            <div className="step-insert-btn" onClick={() => handleInsertStep(-1)} data-tip="在最前面插入一个步骤">
              <Plus size={12} />
            </div>
          </div>

          {steps.map((step, index) => (
            <Fragment key={step.id}>
              <div
                id={`step-item-${step.id}`}
                className={`step-item group relative${selectedStepId === step.id ? " active" : ""}${previewMode ? " preview-active" : ""}`}
                onClick={() => {
                  // ★ 预览模式也要更新选中态：否则橙色高亮停在原步骤（如 23），
                  //   点击 18 只有预览滚动、高亮纹丝不动（用户报：高亮不跟随）。
                  setSelectedStepId(step.id);
                  setLightboxMarked(false);
                  if (previewMode) {
                    const el = document.getElementById(`sop-step-${step.id}`);
                    if (el) el.scrollIntoView({ behavior: "smooth" });
                  }
                }}
              >
                {/* Action group – hover reveal (delete only) */}
                <div className="step-action-group">
                  <button
                    onClick={(e) => { e.stopPropagation(); setDeleteTarget(step.id); }}
                    className="step-action-btn step-action-delete"
                    data-tip="删除步骤"
                  >
                    <Trash2 className="step-action-icon" />
                  </button>
                </div>

                {/* Delete overlay */}
                {deleteTarget === step.id && (
                  <div className="step-delete-overlay" onClick={(e) => e.stopPropagation()}>
                    <div className="step-delete-card" onClick={(e) => e.stopPropagation()}>
                      <div className="step-delete-icon-wrap">
                        <Trash2 />
                      </div>
                      <p className="step-delete-title">删除步骤 {index + 1}</p>
                      <p className="step-delete-subtitle">{step.title || "(无标题)"}</p>
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
                          data-tip="删除这一步，后面的步骤序号自动前移"
                        >
                          删除
                        </button>
                      </div>
                    </div>
                  </div>
                )}

                <span className="step-item-number">{index + 1}</span>
                <div className="step-item-body">
                  <div className="step-item-title">
                    {/* 预览模式跟随所选语言显示译文标题（缺步骤回退原文），与右侧预览一致；查找时命中片段高亮 */}
                    {(() => {
                      const t = previewMode ? (previewStepText(step).title || "(无标题)") : (step.title || "(无标题)");
                      return findOpen ? renderFindHighlight(t, committedQuery.trim()) : t;
                    })()}
                  </div>
                  <div className="step-item-type">{step.action_type}</div>
                </div>
                {(() => {
                  // 缩略图跟随主截图（裁剪图优先，无则全图）
                  const src = getStepMainSrc(step);
                  return src ? (
                    <div className="step-item-thumb">
                      <img
                        src={`data:image/jpeg;base64,${src}`}
                        alt={`步骤 ${index + 1}`}
                      />
                    </div>
                  ) : (
                    <div className="step-item-thumb step-item-thumb-empty">
                      <span className="thumb-empty-type">{step.action_type || ''}</span>
                    </div>
                  );
                })()}
              </div>
              {/* Insert zone after this step */}
              <div className="step-insert-point">
                <div className="step-insert-line" />
                <div className="step-insert-btn" onClick={() => handleInsertStep(index)} data-tip={`在步骤 ${index + 1} 后面插入一个步骤`}>
                  <Plus size={12} />
                </div>
              </div>
            </Fragment>
          ))}
          <div />
        </div>
      </div>

      {/* Right: Step Detail / SOP Preview */}
      <div className={`editor-preview${previewMode ? " preview-active" : ""}`}>
        <div className="editor-preview-body">
          {previewMode ? (
            <div className="sop-preview">
              {/* Full manual */}
              <div className="sop-preview-body">
                {steps.map((step, idx) => (
                  <div key={step.id} id={`sop-step-${step.id}`} className="sop-preview-step">
                    {/* Step header */}
                    <div className="sop-preview-step-header">
                      <span className="sop-preview-step-num"><span className="step-num-circle">{idx + 1}</span>步骤</span>
                    </div>
                    <h3 className="sop-preview-step-title">{previewStepText(step).title || "(无标题)"}</h3>

                    {/* Screenshot */}
                    {(() => {
                      // 预览跟随步骤选中截图：该步骤 stepViewCropped=true 且有裁剪图缓存时用裁剪图，否则用全图
                      const src = getStepMainSrc(step);
                      return src ? (
                        <div className="sop-preview-image">
                          <img
                            src={`data:image/jpeg;base64,${src}`}
                            alt={`步骤 ${idx + 1} 截图`}
                          />
                        </div>
                      ) : null;
                    })()}

                    {/* Description */}
                    {previewStepText(step).description && (
                      <div className="sop-preview-desc">{previewStepText(step).description}</div>
                    )}

                    {/* Tip */}
                    {step.tip && (
                      <div className="sop-preview-tip">
                        <strong>提示：</strong>{step.tip}
                      </div>
                    )}

                    {/* Divider */}
                    {idx < steps.length - 1 && <hr className="sop-step-divider" />}
                  </div>
                ))}
              </div>
            </div>
          ) : selectedStep ? (
            <>
              {/* ── Screenshot ──── */}
              {(() => {
                const _src = getStepMainSrc(selectedStep) || "";
                const _noImage = !_src || (selectedStep?.action_type === 'COPY') || (selectedStep?.action_type === 'HOTKEY');
                /* 渲染期同步：显示的图变了就立即重置容器几何（重置为 null → 回落 CSS
                   默认比例，新图 onLoad/effect 兜底马上写入新比例）。没有这一步，
                   上一张图的 inline 比例会残留，contain 在新图周围留下灰色占位区。 */
                if (detailImgBox.src !== _src) {
                  setDetailImgBox({ src: _src, box: detailBoxMemo.current.get(_src) ?? null });
                }
                if (_noImage) {
                  return (
                    <div className="step-detail-image step-detail-image--empty" style={{ backgroundColor: '#f5f6f7' }}>
                      <div className="step-detail-no-image step-detail-no-image-compact"
                        onClick={pickFileForUpload} onContextMenu={pasteScreenshot}
                        style={{ position: 'relative', display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center' }}>
                        <div className="upload-hint" style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '6px', color: '#9aa0a6' }}>
                          <span className="no-image-action-type" style={{ fontSize: '11px', fontWeight: 600, letterSpacing: '1px', color: '#c0c4cc', textTransform: 'uppercase' }}>
                            {selectedStep?.action_type?.toUpperCase() || ''}
                          </span>
                          <span className="no-image-title" style={{ fontSize: '18px', fontWeight: 600, color: '#24292e', marginTop: '2px' }}>
                            {selectedStep?.title || '(无标题)'}
                          </span>
                          <span className="no-image-hint" style={{ fontSize: '12px', color: '#9aa0a6' }}>点击或右键粘贴上传截图</span>
                        </div>
                      </div>
                    </div>
                  );
                }

                // ── 有截图：图片 + 切换 + 标注 + 删除 ──
                return (
                  <div
                    className="step-detail-image"
                    style={
                      detailImgBox.box
                        ? ({
                            "--step-img-aspect": `${detailImgBox.box.aspect}`,
                            maxWidth: `${detailImgBox.box.maxWidth}px`,
                            minHeight: `${detailImgBox.box.minHeight}px`,
                          } as React.CSSProperties)
                        : undefined
                    }
                  >
                    {/* 全图 / 裁图 切换（仅当前步骤，悬浮截图时显示在右下角） */}
                    <div className="step-detail-view-segmented">
                      <button
                        className={`step-detail-view-segmented-btn${!getViewCropped(selectedStep?.id ?? '') ? ' active' : ''}`}
                        onClick={() => setViewCropped(selectedStep?.id ?? '', false)}
                      >全图</button>
                      <button
                        className={`step-detail-view-segmented-btn${getViewCropped(selectedStep?.id ?? '') ? ' active' : ''}`}
                        onClick={() => setViewCropped(selectedStep?.id ?? '', true)}
                      >裁图</button>
                    </div>
                    <img
                      ref={detailImgRef}
                      src={`data:image/jpeg;base64,${_src}`}
                      alt={`步骤 ${selectedIndex + 1} 截图`}
                      onClick={() => openLightbox(selectedIndex)}
                      onLoad={(e) => {
                        const box = previewBox(e.currentTarget.naturalWidth, e.currentTarget.naturalHeight);
                        if (!box) return;
                        detailBoxMemo.current.set(_src, box);
                        setDetailImgBox((prev) => ({ ...prev, box }));
                      }}
                      style={{ cursor: "pointer" }}
                    />
                    <button
                      className="screenshot-annotate-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        openAnnotate(selectedStep.id);
                      }}
                    >
                      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
                        <path d="M8 2v12M2 8h12" />
                        <circle cx="8" cy="8" r="5.5" />
                      </svg>
                      标注
                    </button>
                    <button
                      className="screenshot-delete-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        const isCropView = getViewCropped(selectedStep?.id ?? '');
                        if (isCropView) {
                          if (confirm("确定删除裁剪图？")) {
                            setDeletedCrop(prev => ({ ...prev, [selectedStep.id]: true }));
                          }
                        } else {
                          if (confirm("确定删除截图？")) {
                            invoke('delete_step_screenshot', {
                              stepId: selectedStep.id,
                              isAfter: true,
                            });
                            const updated = [...steps];
                            updated[selectedIndex] = { ...updated[selectedIndex], screenshot_base64: null };
                            setSteps(updated);
                            setDeletedCrop(prev => ({ ...prev, [selectedStep.id]: false }));
                            setViewCropped(selectedStep.id, false);
                          }
                        }
                      }}
                    >
                      ✕
                    </button>
                  </div>
                );
              })()}

              {/* ── Scrollable form ──── */}
              <div className="step-detail-scroll">

              {/* Title */}
              <div className="form-group">
                <label>标题</label>
                <input
                  type="text"
                  ref={titleInputRef}
                  value={editTitle}
                  onChange={(e) => setEditTitle(e.target.value)}
                />
              </div>

              {/* Description */}
              <div className="form-group">
                <label>描述</label>
                <textarea
                  ref={descInputRef}
                  value={editDescription}
                  onChange={(e) => setEditDescription(e.target.value)}
                  rows={4}
                />
              </div>

              {/* 变量标记（仅输入类步骤）：SKILL.md 自动化导出用 */}
              {(selectedStep.action_type === "type" || selectedStep.action_type === "paste") && (
                <div className="form-group">
                  <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                    <button
                      type="button"
                      onClick={toggleVariable}
                      data-tip="标记为变量后，导出的 SKILL.md 会把该输入变成可填参数，AI 助手执行时按需填入"
                      style={{
                        display: "inline-flex",
                        alignItems: "center",
                        gap: 4,
                        padding: "3px 10px",
                        fontSize: 12,
                        borderRadius: 999,
                        border: "1px solid " + (editIsVar ? "#f59e0b" : "var(--border, #d4d4d8)"),
                        background: editIsVar ? "#fef3c7" : "transparent",
                        color: editIsVar ? "#92400e" : "inherit",
                        cursor: "pointer",
                        whiteSpace: "nowrap",
                      }}
                    >
                      {editIsVar ? <Check size={12} /> : null} 变量
                    </button>
                    <span style={{ fontSize: 12, color: "var(--text-secondary, #71717a)" }}>
                      {editIsVar ? "此输入在每次运行时会变化" : "把输入值标记为可变参数"}
                    </span>
                  </div>
                  {editIsVar && (
                    <input
                      type="text"
                      value={editParamName}
                      onChange={(e) => setEditParamName(e.target.value)}
                      onBlur={() => {
                        const name = editParamName.trim();
                        if (name !== editParamName || name !== selectedStep.param_name) {
                          setEditParamName(name);
                          void persistVariable(true, name);
                        }
                      }}
                      placeholder="参数名，如 order_id"
                      style={{ marginTop: 6 }}
                    />
                  )}
                </div>
              )}

              {/* Tip (expandable) */}
              {selectedStep.tip && (
                <div className="form-group">
                  <button
                    className="tip-toggle"
                    onClick={() => setTipExpanded(!tipExpanded)}
                    data-tip={tipExpanded ? "收起 AI 给出的补充说明" : "展开 AI 给出的补充说明"}
                  >
                    {tipExpanded ? (
                      <ChevronDown size={14} />
                    ) : (
                      <ChevronRight size={14} />
                    )}{" "}
                    提示
                  </button>
                  {tipExpanded && (
                    <div className="step-detail-tip">{selectedStep.tip}</div>
                  )}
                </div>
              )}

              {/* Auto-save indicator */}
              <div className="step-detail-actions" style={{ justifyContent: 'flex-start' }}>
                {autoSaved && (
                  <span style={{ color: '#22c55e', fontSize: 12, display: 'flex', alignItems: 'center', gap: 4 }}>
                    <Check size={12} /> 已自动保存
                  </span>
                )}
              </div>
              </div>
            </>
          ) : (
            <p className="text-secondary">选择左侧步骤以编辑</p>
          )}
        </div>
      </div>

      {/* ── Lightbox ──────────────────────────────────────── */}
      {lightboxOpen && (
        <Lightbox
          steps={steps}
          currentIndex={lightboxIndex}
          onClose={() => setLightboxOpen(false)}
          onPrev={lightboxPrev}
          onNext={lightboxNext}
          getMainSrc={getStepMainSrc}
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
        // 若当前处于裁图视图，把现有裁图框作为初始选区传入，方便在现有裁图上继续微调
        const hasAnchor = (targetStep.crop_w ?? 0) > 0 && (targetStep.crop_h ?? 0) > 0;
        const initialCropRect = hasAnchor
          ? { x: targetStep.x ?? 0, y: targetStep.y ?? 0, width: targetStep.crop_w ?? 0, height: targetStep.crop_h ?? 0 }
          : null;
        return (
          <AnnotationsOverlay
            imageUrl={`data:image/jpeg;base64,${base64}`}
            annotations={[]}
            actionType={targetStep.action_type}
            initialCropRect={initialCropRect}
            onSave={handleAnnotateSave}
            onClose={() => setAnnotateOpen(false)}
          />
        );
      })()}

      {/* Custom Save Dialog */}
      {showSaveDialog && (() => {
        const fmt = showSaveDialog;
        const formatExt: Record<string, string> = { html: "html", markdown: "md", txt: "txt", pdf: "pdf", long: "png", skill: "md" };
        const defaultName = (recording?.title || "recording")
          .replace(/[\\/:*?"<>|]/g, "_")
          .substring(0, 120) + "." + (formatExt[fmt] || fmt);
        return (
          <FileSaveDialog
            isOpen={true}
            title="导出录制"
            defaultName={defaultName}
            filters={[{ name: fmt.toUpperCase(), extensions: [formatExt[fmt] || fmt] }]}
            onClose={() => setShowSaveDialog(null)}
            onSave={handleSaveConfirm}
          />
        );
      })()}

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
