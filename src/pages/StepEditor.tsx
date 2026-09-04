import { useState, useEffect, useCallback, useRef, Fragment } from "react";
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
  ShieldCheck,
  ScanSearch,
  Wrench,
  Upload,
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
import { CROP_PAD } from "../lib/cropAnchor";
import { previewBox } from "../lib/previewBox";
import type { Recording, Step } from "../types";

/* ── Export format options ─────────────────────────────────── */

const EXPORT_FORMATS = [
  { value: "html", label: "HTML", ext: "", icon: FileCode },
  { value: "docx", label: "Word", ext: "", icon: FileText },
  { value: "pdf", label: "PDF", ext: "", icon: File },
] as const;

/** 各导出格式的悬浮说明（说清「导出来能干嘛」，而不是重复格式名） */
const EXPORT_TIPS: Record<string, string> = {
  html: "导出单个 HTML 文件，带目录；双击即可打开，也能直接放到网站上",
  docx: "导出 Word 文档，方便交给同事继续编辑排版",
  pdf: "导出 PDF，适合直接发给别人看或打印",
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
  en: "English",
  ja: "日本語",
  ko: "한국어",
  es: "Español",
};

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
          } else {
            left = Math.round(x - rw / 2);
          }
          top = Math.round(y - rh / 2);
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
}

// 把 base64 截图压缩为更小的 JPEG，降低上传体积，避免“保存草稿/发布”因请求体过大而失败
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

export default function StepEditor({ siteToken, siteUser }: StepEditorProps) {
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
  const [saving, setSaving] = useState(false);
  const [saveSuccess, setSaveSuccess] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportProgress, setExportProgress] = useState<string | null>(null);
  const [redacting, setRedacting] = useState(false);
  const [scanRedacting, setScanRedacting] = useState(false);
  const [scanProgress, setScanProgress] = useState<{ done: number; total: number; currentStepId: string }>({ done: 0, total: 0, currentStepId: "" });
  const [scanToast, setScanToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const scanToastTimerRef = useRef<number | null>(null);
  const [showSaveDialog, setShowSaveDialog] = useState<string | null>(null);
  const [toast, setToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const [aiGenerating, setAiGenerating] = useState(false);
  const [domainMode, setDomainMode] = useState<"auto" | "spreadsheet" | "browser" | "code" | "terminal">("auto");

  /* Translation state（译文副本：不覆盖原文，预览/导出可切语言，PRD T1-T9） */
  const [translations, setTranslations] = useState<TranslationsMap>({});
  const [translating, setTranslating] = useState(false);
  const [translateModalOpen, setTranslateModalOpen] = useState(false);
  // ★ T8 译文可编辑：编辑弹窗状态（语言 + 草稿）
  const [editTransOpen, setEditTransOpen] = useState(false);
  const [editTransLang, setEditTransLang] = useState<string>("en");
  const [editDraftTitle, setEditDraftTitle] = useState("");
  const [editDraftSteps, setEditDraftSteps] = useState<{ i: number; title: string; description: string }[]>([]);
  const [savingTranslation, setSavingTranslation] = useState(false);
  const [translateLang, setTranslateLang] = useState<string>("en");
  /* 导出语言：null = 原文 */
  const [exportLang, setExportLang] = useState<string | null>(null);

  const loadTranslations = useCallback(async () => {
    try {
      const t = await invoke<TranslationsMap | null>("get_recording_translations", { recordingId });
      setTranslations(t ?? {});
    } catch {
      /* 译文加载失败不阻塞编辑器，保持 {} 即可 */
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
    // 适用场景（与网站编辑器一致）
    const [pubSceneSelect, setPubSceneSelect] = useState("");
    const SCENE_ENTRIES: { emoji: string; name: string; slug: string }[] = [
      { emoji: "👥", name: "新员工入职", slug: "new-employee" },
      { emoji: "🎓", name: "客户培训", slug: "customer-training" },
      { emoji: "🔧", name: "问题排查", slug: "troubleshooting" },
      { emoji: "💰", name: "财务合规", slug: "finance-compliance" },
      { emoji: "📝", name: "日常办公", slug: "daily-office" },
      { emoji: "🚀", name: "软件上线", slug: "software-launch" },
      { emoji: "🎬", name: "录步", slug: "lu-bu" },
    ];

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

  // Auto-dismiss toast
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 2000);
    return () => clearTimeout(timer);
  }, [toast]);

  /* ── translate://done（翻译完成 → 刷新译文副本，PRD T4）── */
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<{ recordingId: string; lang: string; ok: boolean; summary: string }>(
      "translate://done",
      (ev) => {
        if (ev.payload.recordingId !== recordingId) return;
        setTranslating(false);
        if (ev.payload.ok) {
          loadTranslations();
          setToast({ type: "success", message: `已生成${LANG_LABELS[ev.payload.lang] || ev.payload.lang}译文，可在预览/导出中切换语言` });
        } else {
          setToast({ type: "error", message: "翻译失败: " + (ev.payload.summary || "未知错误") });
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
  }, [recordingId, loadTranslations]);

  /* ── 翻译（PRD T2/T3）：只翻标题+步骤描述，存译文副本不覆盖原文 ── */
  const handleTranslate = useCallback(
    async (lang: string) => {
      setTranslateModalOpen(false);
      if (steps.length === 0) {
        setToast({ type: "error", message: "没有可翻译的步骤" });
        return;
      }
      setTranslating(true);
      try {
        await invoke("translate_recording", { recordingId, lang });
        // 完成后由 translate://done 事件收尾（toast + 刷新译文）
      } catch (err) {
        setTranslating(false);
        setToast({ type: "error", message: "翻译失败: " + String(err) });
      }
    },
    [recordingId, steps.length]
  );

  /* ── ★ T8 译文可编辑：打开编辑弹窗（按当前步骤顺序构建草稿，缺失字段回退原文） ── */
  const transLangs = Object.keys(translations);
  const openEditTrans = useCallback((lang: string) => {
    const t = translations[lang];
    setEditTransLang(lang);
    setEditDraftTitle(t?.title ?? "");
    setEditDraftSteps(
      steps.map((s, idx) => {
        const st = t?.steps?.[String(s.order_index ?? idx)];
        return {
          i: s.order_index ?? idx,
          title: st?.title ?? s.title ?? "",
          description: st?.description ?? s.description ?? "",
        };
      })
    );
    setEditTransOpen(true);
  }, [translations, steps]);

  const handleSaveTranslation = useCallback(async () => {
    if (!recordingId || savingTranslation) return;
    setSavingTranslation(true);
    try {
      const stepsMap: Record<string, { title: string; description: string }> = {};
      for (const s of editDraftSteps) {
        stepsMap[String(s.i)] = { title: s.title, description: s.description };
      }
      const next: TranslationsMap = {
        ...translations,
        [editTransLang]: { title: editDraftTitle, steps: stepsMap },
      };
      await invoke("save_recording_translations", { recordingId, translations: next });
      setTranslations(next);
      setEditTransOpen(false);
      setToast({ type: "success", message: `已保存${LANG_LABELS[editTransLang] || editTransLang}译文` });
    } catch (err) {
      setToast({ type: "error", message: "保存译文失败: " + String(err) });
    } finally {
      setSavingTranslation(false);
    }
  }, [recordingId, translations, editTransLang, editDraftTitle, editDraftSteps, savingTranslation]);

  /* Preview mode */
  const [previewMode, setPreviewMode] = useState(false);
  /* ── 步骤精简：自动合并（规则）/ AI 精简（撤销上移为顶栏全局 ↶↷）── */
  const [simplifying, setSimplifying] = useState<null | "rules" | "ai">(null);
  const [simplifyOpen, setSimplifyOpen] = useState(false);
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

  /* Delete target for overlay */
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  /* Inline title editing */
  const [recTitleEditing, setRecTitleEditing] = useState(false);
  const [recTitleDraft, setRecTitleDraft] = useState("");

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

  /* 裁剪图（400×400 @点击坐标）缓存：key=step.id，为所有步骤异步预生成，避免预览/缩略图重复计算 */
  const [cropCache, setCropCache] = useState<Record<string, string>>({});
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

  /* ── voice://done（语音转写完成 → 刷新步骤显示【语音】内容，PRD V8）── */
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
      if (ev.payload.ok && ev.payload.transcribed > 0) {
        reloadRecording().catch(() => {});
        setToast({ type: "success", message: `语音转写完成：${ev.payload.summary}` });
      } else {
        setToast({ type: "error", message: `语音转写失败：${ev.payload.summary || "无可识别语音"}` });
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
    try {
      const r = await invoke<{
        before: number; after: number; reduced: number; kind: string;
      }>("simplify_steps_ai", { recordingId });
      const list = await reloadRecording();
      await rebuildCropCache(list);
      refreshHist();
      setToast({
        type: "success",
        // kind 为 "rules" 表示 AI 那一层没生效（剩余步骤太少，不值得再调一次），
        // 但规则层已经先跑过了，用户仍然拿到收益 —— 必须说清楚，别让人以为没反应。
        message:
          r.reduced > 0
            ? r.kind === "ai"
              ? `AI 精简完成：${r.before} 步 → ${r.after} 步（省 ${r.reduced} 步）`
              : `已自动合并：${r.before} 步 → ${r.after} 步（AI 判断无需再合并）`
            : "没有检测到可合并的冗余步骤",
      });
    } catch (err) {
      setToast({ type: "error", message: "AI 精简失败: " + String(err) });
    } finally {
      setSimplifying(null);
    }
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

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const data = await invoke<RecordingWithSteps>("load_recording", {
          id: recordingId,
        });
        if (cancelled) return;

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

        // 视图状态（stepViewCropped / globalView）已由 useState 惰性初始化从 localStorage 同步恢复，
        // 此处不再重复 setState（否则新对象引用触发重渲染会破坏首屏一致性）。
        // 只需标记恢复完成以允许后续持久化写入。
        viewLoadedRef.current = true;

        // 译文副本（翻译功能 T1）：并行加载，失败不阻塞编辑器
        loadTranslations();

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

  /* 持久化裁图/全图视图偏好到 localStorage */
  useEffect(() => {
    if (!recordingId) return;
    // 首屏视图恢复前不写，防止挂载瞬间用默认值覆盖已保存偏好
    if (!viewLoadedRef.current) return;
    try {
      localStorage.setItem(`flowio_view_${recordingId}`, JSON.stringify({
        stepViewCropped,
        globalView,
      }));
    } catch { /* storage full / unavailable */ }
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
        invoke("upload_step_screenshot", { stepId: selectedStep.id, base64Data: base64, isAfter: true, isMarked: false })
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
                    invoke("upload_step_screenshot", { stepId: selectedStep.id, base64Data: base64, isAfter: true, isMarked: false })
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
          // 导出语言（PRD T9）：null = 原文，否则用对应语言的译文副本覆盖标题/描述
          lang: exportLang ?? undefined,
        });
        setToast({ type: "success", message: `导出成功: ${path}` });
      } catch (err) {
        setToast({ type: "error", message: "导出失败: " + String(err) });
      } finally {
        setExporting(false);
        setExportProgress(null);
        unlisten();
      }
    },
    [recordingId, showSaveDialog, exportLang, stepViewCropped]
  );

  /* ── Publish to website ──────────────────────────────────── */

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

      // 适用场景的名称并入标签（与网站编辑器保持一致）
      const sceneName = SCENE_ENTRIES.find((s) => s.slug === pubSceneSelect)?.name || "";
      const tagsToSend = sceneName ? [...new Set([...pubTags, sceneName])] : pubTags;

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

      // 构建上传数据（截图用 base64）
      // 覆盖发布：同一录制曾发布过（本地映射记录 guide id）→ 带 guide_id 让网站覆盖更新而非新建
      let prevGuideId: string | null = null;
      try { prevGuideId = localStorage.getItem(`flowio_pub_${recordingId}`); } catch { /* ignore */ }
      const uploadData: Record<string, unknown> = {
        title,
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
        translations: Object.keys(translations).length > 0 ? translations : undefined,
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
        if (coverSrc) uploadData.cover_base64 = await downscaleBase64Image(coverSrc, 720, 0.85);
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
      setShowPublishDialog(false);
      setPubStep("pick");
      setToast({ type: "success", message: isDraft ? "草稿已保存！" : "发布成功！教程已上传到网站" });
    } catch (err) {
      const isAbort = (err as { name?: string })?.name === "AbortError";
      setToast({ type: "error", message: isAbort ? "发布超时（90秒），请检查网络后重试" : "发布失败: " + String(err) });
    } finally {
      setPublishing(false);
    }
  }, [siteToken, recording, recordingId, categories, pubTitle, pubApplications, pubCategory, pubTags, pubDownloadable, pubCoverBase64, pubSceneSelect, translations, stepViewCropped, deletedCrop]);

  /* ── 隐私脱敏 ────────────────────────────────────────────── */

  // 文本脱敏：对步骤描述中的敏感信息执行替换
  const handleRedact = useCallback(async () => {
    setRedacting(true);
    try {
      const changed = await invoke<number>("redact_recording_text", { recordingId });
      setToast({ type: "success", message: changed > 0 ? "文本脱敏完成" : "未检测到敏感信息" });
      const list = await reloadRecording();
      await rebuildCropCache(list);
      refreshHist();
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
    if (scanToastTimerRef.current) clearTimeout(scanToastTimerRef.current);
    setScanRedacting(true);
    setScanProgress({ done: 0, total: 0, currentStepId: "" });
    setScanToast(null);
    try {
      const unlisten = await listen<{ done: number; total: number; currentStepId: string }>(
        "scan_redact_progress",
        (ev) => {
          setScanProgress({ done: ev.payload.done, total: ev.payload.total, currentStepId: ev.payload.currentStepId });
        }
      );
      // 兜底：后端扫描结束事件，确保按钮/进度一定复位
      const unlistenDone = await listen(
        "scan_redact_done",
        () => {
          setScanRedacting(false);
          setScanProgress({ done: 0, total: 0, currentStepId: "" });
        }
      );
      try {
        const cfg = await invoke<{ mode?: "blur" | "block" }>("get_redaction_config");
        const result = await invoke<{ processed: number; total: number; redactedCount: number; failedCount: number; cancelled: boolean }>(
          "scan_redact_screenshots",
          { recordingId, mode: cfg.mode === "block" ? "block" : "blur" }
        );
        if (result.cancelled) {
          showScanToast({ type: "success", message: "扫描已取消" });
        } else if (result.total === 0) {
          showScanToast({ type: "success", message: "没有可扫描的截图" });
        } else if (result.failedCount > 0 && result.redactedCount === 0) {
          showScanToast({ type: "error", message: "扫描完成，未检测到敏感信息" });
        } else if (result.failedCount > 0) {
          showScanToast({ type: "success", message: "扫描脱敏完成（部分失败）" });
        } else if (result.redactedCount === 0) {
          showScanToast({ type: "success", message: "未检测到敏感信息" });
        } else {
          showScanToast({ type: "success", message: "扫描脱敏完成" });
        }
        const list = await reloadRecording();
        await rebuildCropCache(list);
      } finally {
        unlisten();
        unlistenDone();
      }
    } catch (err) {
      showScanToast({ type: "error", message: "扫描截图失败: " + String(err) });
    } finally {
      setScanRedacting(false);
      setScanProgress({ done: 0, total: 0, currentStepId: "" });
    }
  }, [recordingId, reloadRecording, rebuildCropCache, steps.length, showScanToast]);

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
    setAnnotateStepId(stepId);
    setAnnotateOpen(true);
  };

  /** 标注完成：把烧录后的 PNG base64 通过 crop_step_screenshot 落盘并更新前端状态 */
  const handleAnnotateSave = async (_annos: Annotation[], baked?: string) => {
    if (!baked) {
      setAnnotateOpen(false);
      return;
    }
    const base64 = baked.split(",")[1] || "";
    if (!base64) {
      setAnnotateOpen(false);
      return;
    }
    const stepId = annotateStepId;
    try {
      await invoke("crop_step_screenshot", {
        stepId,
        croppedBase64: base64,
        isAfter: true,
      });
      setSteps((prev) =>
        prev.map((s) => {
          if (s.id !== stepId) return s;
          return { ...s, screenshot_base64: base64 };
        })
      );
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
    <div style={{ display: "flex", alignItems: "center", gap: 8, height: "100%" }}>
      <button
        onClick={() => navigate("/")}
        data-tip={"返回录制列表\n改动已自动保存"}
        style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
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
          autoFocus
          value={recTitleDraft}
          onChange={(e) => setRecTitleDraft(e.target.value)}
          onBlur={async () => {
            const trimmed = recTitleDraft.trim();
            if (trimmed && trimmed !== recording?.title) {
              await invoke("update_recording_title", { id: recordingId, title: trimmed });
              setRecording((prev) => prev ? { ...prev, title: trimmed } : prev);
              refreshHist();
            }
            setRecTitleEditing(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") { setRecTitleDraft(recording?.title || ""); setRecTitleEditing(false); }
          }}
          style={{ color: "#111827", fontSize: 14, fontWeight: 600, padding: "2px 6px", border: "1px solid #ff6b35", borderRadius: 4, outline: "none", width: 200, background: "#fff" }}
        />
      ) : (
        <span
          data-tauri-drag-region
          onClick={() => { setRecTitleDraft(recording?.title || ""); setRecTitleEditing(true); }}
          style={{ color: "#e6edf3", fontSize: 14, fontWeight: 600, userSelect: "none", cursor: "pointer", padding: "2px 6px", borderRadius: 4, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: 420 }}
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
      <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
        <button
          onClick={() => setExportOpen(!exportOpen)}
          disabled={exporting}
          data-tip={"导出教程\n导出为 Word / PDF / HTML；如果有译文，可以先在上面选语言"}
          style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          <Download size={14} /> {exportProgress ? exportProgress : exporting ? "导出中..." : "导出"} <ChevronDown size={12} />
        </button>
        {exportOpen && (
          <div className="export-dropdown" style={{ right: 0 }}>
            {/* 导出语言（PRD T9）：原文 + 已生成译文的语言 */}
            {Object.keys(translations).length > 0 && (
              <div style={{ padding: "6px 10px", borderBottom: "1px solid #30363d" }}>
                <div style={{ fontSize: 11, color: "#8b949e", marginBottom: 4 }}>导出语言</div>
                <div style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
                  <button
                    className={`export-dropdown-item${exportLang === null ? " active" : ""}`}
                    onClick={() => setExportLang(null)}
                    style={{ padding: "3px 8px", fontSize: 12 }}
                    data-tip="用录制时的原始语言导出"
                  >
                    原文
                  </button>
                  {Object.keys(translations).map((lang) => (
                    <button
                      key={lang}
                      className={`export-dropdown-item${exportLang === lang ? " active" : ""}`}
                      onClick={() => setExportLang(lang)}
                      style={{ padding: "3px 8px", fontSize: 12 }}
                      data-tip={`用${LANG_LABELS[lang] || lang}译文导出`}
                    >
                      {LANG_LABELS[lang] || lang}
                    </button>
                  ))}
                </div>
              </div>
            )}
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
            onClick={() => setToolsOpen(!toolsOpen)}
            data-tip={"更多工具\n上传网站、AI 生成步骤说明、敏感信息脱敏、整篇翻译"}
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
                  setPubTitle(recording?.title || "");
                }}
                data-tip={siteToken
                  ? "把这篇教程上传到 flowio.cn\n可先存草稿，也可填写标题/应用/分类后直接发布"
                  : "登录后才能上传到 flowio.cn（点击去登录）"}
              >
                <Upload size={14} className="export-dropdown-item-icon" />
                <span className="export-dropdown-item-label">{siteToken ? "上传网站" : "登录后上传"}</span>
              </button>
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
                onClick={() => { setToolsOpen(false); setTranslateModalOpen(true); }}
                disabled={translating}
                data-tip={"翻译教程\n把标题和每一步描述翻译成别的语言；译文存成副本，原文一直保留"}
              >
                {translating ? <Loader2 size={14} className="spinner" /> : <Languages size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">{translating ? "翻译中..." : "翻译教程"}</span>
              </button>
              {/* ★ T8 译文可编辑：编辑已有译文副本 */}
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); openEditTrans(transLangs[0] || "en"); }}
                disabled={transLangs.length === 0 || steps.length === 0}
                style={{ opacity: transLangs.length === 0 ? 0.5 : 1 }}
                data-tip={transLangs.length === 0
                  ? "还没有译文，请先点「翻译教程」"
                  : "编辑已有译文\n逐句修改翻译结果，不影响原文"}
              >
                <PencilLine size={14} className="export-dropdown-item-icon" />
                <span className="export-dropdown-item-label">编辑译文</span>
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
      <div className="titlebar-view-segmented" data-tip="选择教程主要截图风格（作用于所有步骤）">
        <button
          className={`titlebar-view-segmented-btn${globalView === 'full' ? ' active' : ''}`}
          onClick={() => applyGlobalView('full')}
          data-tip={"全部步骤都用完整截图\n画面完整，适合读者需要先看清整个界面的教程"}
        >全图</button>
        <button
          className={`titlebar-view-segmented-btn${globalView === 'crop' ? ' active' : ''}`}
          onClick={() => applyGlobalView('crop')}
          data-tip={"全部步骤只截取操作区域\n重点突出、体积小，适合步骤密集的教程"}
        >裁图</button>
      </div>
    </div>
  );

  /* ── Render ──────────────────────────────────────────────── */

  /* 预览模式取文案（PRD T6/T8）：exportLang 有译文时用译文，缺步骤回退原文 */
  const previewStepText = (step: Step): { title: string; description: string } => {
    if (exportLang && translations[exportLang]) {
      const t = translations[exportLang].steps[String(step.order_index)];
      return {
        title: t?.title || step.title,
        description: t?.description || step.description || "",
      };
    }
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
      {translateModalOpen && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 10000, background: "rgba(0,0,0,0.45)", display: "flex", alignItems: "center", justifyContent: "center" }}
          onMouseDown={() => setTranslateModalOpen(false)}
        >
          <div
            style={{ background: "#fff", borderRadius: 10, padding: "20px 22px", width: 340, boxShadow: "0 8px 32px rgba(0,0,0,0.2)" }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div style={{ fontSize: 15, fontWeight: 600, color: "#24292e", display: "flex", alignItems: "center", gap: 6 }}>
              <Languages size={16} /> 翻译教程
            </div>
            <div style={{ fontSize: 12.5, color: "#57606a", marginTop: 8, lineHeight: 1.6 }}>
              将翻译教程标题和各步骤的文字描述（共 {steps.length} 个步骤）。截图与标注不会被修改，译文以副本保存，原文始终保留。
            </div>
            <label style={{ display: "block", margin: "14px 0 4px", fontSize: 13, color: "#57606a" }}>目标语言</label>
            <select
              value={translateLang}
              onChange={(e) => setTranslateLang(e.target.value)}
              style={{ width: "100%", boxSizing: "border-box", padding: "7px 9px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", fontSize: 13 }}
            >
              {Object.entries(LANG_LABELS).map(([code, label]) => (
                <option key={code} value={code}>
                  {label}{translations[code] ? "（已有译文，将覆盖）" : ""}
                </option>
              ))}
            </select>
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 18 }}>
              <button
                onClick={() => setTranslateModalOpen(false)}
                data-tip="不翻译，关闭"
                style={{ padding: "6px 14px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", cursor: "pointer", fontSize: 13 }}
              >
                取消
              </button>
              <button
                onClick={() => handleTranslate(translateLang)}
                data-tip="开始翻译，会调用 AI，需要几十秒"
                style={{ padding: "6px 14px", borderRadius: 6, border: "none", background: "#3366CC", color: "#fff", cursor: "pointer", fontSize: 13 }}
              >
                开始翻译
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ★ T8 译文可编辑弹窗 */}
      {editTransOpen && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 10000, background: "rgba(0,0,0,0.45)", display: "flex", alignItems: "center", justifyContent: "center" }}
          onMouseDown={() => setEditTransOpen(false)}
        >
          <div
            style={{ background: "#fff", borderRadius: 10, padding: "20px 22px", width: 560, maxHeight: "80vh", overflowY: "auto", boxShadow: "0 8px 32px rgba(0,0,0,0.2)" }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div style={{ fontSize: 15, fontWeight: 600, color: "#24292e", display: "flex", alignItems: "center", gap: 6 }}>
              <PencilLine size={16} /> 编辑译文（{LANG_LABELS[editTransLang] || editTransLang}）
            </div>
            <div style={{ fontSize: 12.5, color: "#57606a", marginTop: 8, lineHeight: 1.6 }}>
              直接修改译文文本，保存后覆盖该语言副本。截图与原文不受影响。
            </div>
            <label style={{ display: "block", margin: "14px 0 4px", fontSize: 13, color: "#57606a" }}>语言</label>
            <select
              value={editTransLang}
              onChange={(e) => openEditTrans(e.target.value)}
              style={{ width: "100%", boxSizing: "border-box", padding: "7px 9px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", fontSize: 13 }}
            >
              {transLangs.map((code) => (
                <option key={code} value={code}>
                  {LANG_LABELS[code] || code}
                </option>
              ))}
            </select>

            <label style={{ display: "block", margin: "14px 0 4px", fontSize: 13, color: "#57606a" }}>教程标题</label>
            <input
              value={editDraftTitle}
              onChange={(e) => setEditDraftTitle(e.target.value)}
              style={{ width: "100%", boxSizing: "border-box", padding: "7px 9px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", fontSize: 13 }}
            />

            {editDraftSteps.map((s, idx) => (
              <div key={idx} style={{ marginTop: 14, paddingTop: 12, borderTop: "1px solid #eaeef2" }}>
                <div style={{ fontSize: 12, color: "#8b949e", marginBottom: 4 }}>步骤 {idx + 1}</div>
                <input
                  value={s.title}
                  onChange={(e) =>
                    setEditDraftSteps((prev) => prev.map((p, i) => (i === idx ? { ...p, title: e.target.value } : p)))
                  }
                  placeholder="步骤标题"
                  style={{ width: "100%", boxSizing: "border-box", padding: "6px 9px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", fontSize: 13, marginBottom: 6 }}
                />
                <textarea
                  value={s.description}
                  onChange={(e) =>
                    setEditDraftSteps((prev) => prev.map((p, i) => (i === idx ? { ...p, description: e.target.value } : p)))
                  }
                  placeholder="步骤描述"
                  rows={2}
                  style={{ width: "100%", boxSizing: "border-box", padding: "6px 9px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", fontSize: 13, resize: "vertical" }}
                />
              </div>
            ))}

            <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 18 }}>
              <button
                onClick={() => setEditTransOpen(false)}
                data-tip="放弃这次修改并关闭"
                style={{ padding: "6px 14px", borderRadius: 6, border: "1px solid #d0d7de", background: "#fff", color: "#24292e", cursor: "pointer", fontSize: 13 }}
              >
                取消
              </button>
              <button
                onClick={() => void handleSaveTranslation()}
                disabled={savingTranslation}
                data-tip="保存译文修改（原文不受影响）"
                style={{ padding: "6px 14px", borderRadius: 6, border: "none", background: "#3366CC", color: "#fff", cursor: savingTranslation ? "default" : "pointer", fontSize: 13, display: "flex", alignItems: "center", gap: 6 }}
              >
                {savingTranslation && <Loader2 size={13} className="spinner" />}
                {savingTranslation ? "保存中..." : "保存译文"}
              </button>
            </div>
          </div>
        </div>
      )}

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
                disabled={simplifying !== null}
                onClick={() => setSimplifyOpen(!simplifyOpen)}
              >
                {simplifying ? <Loader2 size={12} className="spinner" /> : <Sparkles size={12} />}
                {simplifying === "ai" ? "AI 精简中…"
                  : simplifying === "rules" ? "规则合并中…"
                  : "精简"}
                <ChevronDown size={11} style={{ opacity: 0.8 }} />
              </button>
              {simplifyOpen && (
                <div className="export-dropdown step-simplify-menu">
                  <button
                    className="export-dropdown-item"
                    onClick={() => { setSimplifyOpen(false); void handleAiSimplify(); }}
                    disabled={simplifying !== null}
                    data-tip={"AI 精简（推荐）\n先跑本地规则，再让 AI 按语义把重复步骤压成一步\n能处理规则看不懂的情况（比如弹框里坐标各异的一串点击），需要几秒到十几秒"}
                  >
                    {simplifying === "ai" ? <Loader2 size={14} className="spinner" /> : <Sparkles size={14} className="export-dropdown-item-icon" />}
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
                  if (previewMode) {
                    const el = document.getElementById(`sop-step-${step.id}`);
                    if (el) el.scrollIntoView({ behavior: "smooth" });
                  } else {
                    setSelectedStepId(step.id);
                    setLightboxMarked(false);
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
                    {step.title || "(无标题)"}
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
                  <div className="step-detail-image">
                    {/* 全图 / 裁图 切换（仅当前步骤，悬浮截图时显示在右下角） */}
                    <div className="step-detail-view-segmented" data-tip="切换当前步骤的截图：全图 / 裁图（只影响这一步）">
                      <button
                        className={`step-detail-view-segmented-btn${!getViewCropped(selectedStep?.id ?? '') ? ' active' : ''}`}
                        onClick={() => setViewCropped(selectedStep?.id ?? '', false)}
                        data-tip="这一步用完整截图"
                      >全图</button>
                      <button
                        className={`step-detail-view-segmented-btn${getViewCropped(selectedStep?.id ?? '') ? ' active' : ''}`}
                        onClick={() => setViewCropped(selectedStep?.id ?? '', true)}
                        data-tip="这一步只截取操作区域（只影响当前步骤）"
                      >裁图</button>
                    </div>
                    <img
                      ref={detailImgRef}
                      src={`data:image/jpeg;base64,${_src}`}
                      alt={`步骤 ${selectedIndex + 1} 截图`}
                      onClick={() => openLightbox(selectedIndex)}
                      onLoad={(e) => {
                        const img = e.currentTarget;
                        const parent = img.parentElement;
                        const box = previewBox(img.naturalWidth, img.naturalHeight);
                        if (parent && box) {
                          parent.style.setProperty("--step-img-aspect", `${box.aspect}`);
                          parent.style.maxWidth = `${box.maxWidth}px`;
                          parent.style.minHeight = `${box.minHeight}px`;
                        }
                      }}
                      style={{ cursor: "pointer" }}
                    />
                    <button
                      className="screenshot-annotate-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        openAnnotate(selectedStep.id);
                      }}
                      data-tip={"标注这张截图\n裁剪、加文字、高亮、箭头、模糊；标完覆盖当前步骤的图"}
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
                      data-tip={"删除当前这张图\n只在「裁图」模式下是删裁图，全图模式下删整张截图"}
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
                  value={editTitle}
                  onChange={(e) => setEditTitle(e.target.value)}
                />
              </div>

              {/* Description */}
              <div className="form-group">
                <label>描述</label>
                <textarea
                  value={editDescription}
                  onChange={(e) => setEditDescription(e.target.value)}
                  rows={4}
                />
              </div>

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
        return (
          <AnnotationsOverlay
            imageUrl={`data:image/jpeg;base64,${base64}`}
            annotations={[]}
            onSave={handleAnnotateSave}
            onClose={() => setAnnotateOpen(false)}
          />
        );
      })()}

      {/* Custom Save Dialog */}
      {showSaveDialog && (() => {
        const fmt = showSaveDialog;
        const formatExt: Record<string, string> = { html: "html", markdown: "md", pdf: "pdf" };
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
            alignItems: "center",
            gap: "12px",
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
          <span>{toast.message}</span>
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
                    <label style={pubDialogStyles.label}>封面（可选）</label>
                    {pubCoverPreview ? (
                      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
                        <img src={pubCoverPreview} alt="封面预览" style={{ width: 120, height: 72, objectFit: "cover", borderRadius: 6, border: "1px solid #30363d" }} />
                        <button type="button" onClick={() => { setPubCoverBase64(null); setPubCoverPreview(null); }} style={pubDialogStyles.addBtn} data-tip="移除封面，改用默认截图">移除</button>
                      </div>
                    ) : (
                      <input
                        type="file"
                        accept="image/*"
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
                        style={{ ...pubDialogStyles.input, color: "#8b949e" }}
                      />
                    )}

                    <label style={pubDialogStyles.label}>标题</label>
                    <input
                      style={pubDialogStyles.input}
                      value={pubTitle}
                      placeholder={recording?.title || "未命名教程"}
                      onChange={(e) => setPubTitle(e.target.value)}
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

                    <label style={pubDialogStyles.label}>适用场景</label>
                    <select
                      style={pubDialogStyles.input}
                      value={pubSceneSelect}
                      onChange={(e) => setPubSceneSelect(e.target.value)}
                    >
                      <option value="">不指定</option>
                      {SCENE_ENTRIES.map((s) => (
                        <option key={s.slug} value={s.slug}>{s.emoji} {s.name}</option>
                      ))}
                    </select>

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
