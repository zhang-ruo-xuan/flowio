import { useState, useEffect, useLayoutEffect, useCallback, useRef, Fragment } from "react";
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
} from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FileSaveDialog from "../components/FileSaveDialog";
import AnnotationsOverlay from "../components/AnnotationsOverlay";
import type { Annotation } from "../lib/annotate";
import type { Recording, Step } from "../types";

/* ── Export format options ─────────────────────────────────── */

const EXPORT_FORMATS = [
  { value: "html", label: "HTML", ext: "", icon: FileCode },
  { value: "docx", label: "Word", ext: "", icon: FileText },
  { value: "pdf", label: "PDF", ext: "", icon: File },
] as const;

/* ── Backend response types ────────────────────────────────── */

interface RecordingWithSteps {
  recording: Recording;
  steps: Step[];
}

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

/** 元素矩形外扩 padding（px） */
const CROP_PAD = 24;
/** 裁剪区域最小边长（px），不足时以矩形中心外扩 */
const CROP_MIN_SIZE = 280;
/** 裁剪区域最大不超过屏幕宽/高的比例 */
const CROP_MAX_SCREEN_RATIO = 0.9;
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
          // 元素矩形 + padding，不足最小尺寸以中心外扩
          let rw = cropW + CROP_PAD * 2;
          let rh = cropH + CROP_PAD * 2;
          rw = Math.max(rw, CROP_MIN_SIZE);
          rh = Math.max(rh, CROP_MIN_SIZE);
          // 超过屏幕 90% 等比缩放（只缩不放），保持宽高比
          const scale = Math.min(
            1,
            (W * CROP_MAX_SCREEN_RATIO) / rw,
            (H * CROP_MAX_SCREEN_RATIO) / rh,
          );
          rw = Math.max(1, Math.round(rw * scale));
          rh = Math.max(1, Math.round(rh * scale));
          // Stage C: type 步骤左扩原始矩形宽的 1/3，把左侧 label 带进裁剪图
          if (actionType === "type") {
            const extra = Math.round(cropW / 3);
            rw += extra;
            left = Math.round(x - rw / 2 - extra);
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

        // clamp 到图像边界
        left = Math.max(0, left);
        top = Math.max(0, top);
        cw = Math.min(cw, W - left);
        ch = Math.min(ch, H - top);
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
          <button className="lightbox-close" onClick={onClose}>
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
            >
              <ChevronLeft size={28} />
            </button>
            <button
              className="lightbox-nav lightbox-next"
              onClick={onNext}
              disabled={currentIndex === steps.length - 1}
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
  siteUser?: { id: string; email: string; display_name?: string; bio?: string; avatar?: string } | null;
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
        const res = await fetch("http://47.102.154.233/api/categories");
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

  /* Preview mode */
  const [previewMode, setPreviewMode] = useState(false);

  /* Delete target for overlay */
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

  /* Inline title editing */
  const [recTitleEditing, setRecTitleEditing] = useState(false);
  const [recTitleDraft, setRecTitleDraft] = useState("");

  /* Lightbox */
  const [lightboxOpen, setLightboxOpen] = useState(false);
  const [lightboxIndex, setLightboxIndex] = useState(0);
  const [lightboxMarked, setLightboxMarked] = useState(false);
  /* 主图视图：false=全图（默认，screenshot_base64），true=裁剪图（点击坐标 400×400 裁剪） */
  const [stepViewCropped, setStepViewCropped] = useState<Record<string, boolean>>({});
  const getViewCropped = (stepId: string) => stepViewCropped[stepId] ?? false;
  const setViewCropped = (stepId: string, value: boolean) => setStepViewCropped(prev => ({ ...prev, [stepId]: value }));
  /* 裁剪图是否已被用户删除（true=该步骤裁剪图视为不存在，仅影响裁图视图，全图不受影响） */
  const [deletedCrop, setDeletedCrop] = useState<Record<string, boolean>>({});
  const isCropDeleted = (stepId: string) => !!deletedCrop[stepId];
  /* 全局视图风格：'full'=全图，'crop'=裁剪图（顶部统一切换按钮高亮当前教程主要风格） */
  const [globalView, setGlobalView] = useState<'full' | 'crop'>('full');

  /* Tip expand */
  const [tipExpanded, setTipExpanded] = useState(false);

  const detailImgRef = useRef<HTMLImageElement>(null);

  /* 裁剪图（400×400 @点击坐标）缓存：key=step.id，为所有步骤异步预生成，避免预览/缩略图重复计算 */
  const [cropCache, setCropCache] = useState<Record<string, string>>({});

  /* Annotation (手动标注编辑器) */
  const [annotateOpen, setAnnotateOpen] = useState(false);
  const [annotateStepId, setAnnotateStepId] = useState<string>("");

  const stepDetailImageRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = stepDetailImageRef.current;
    if (!el) return;
    const setHeight = () => {
      const w = el.clientWidth;
      if (w > 0) {
        el.style.height = `${Math.min(Math.max(w * 9 / 16, 200), 520)}px`;
      }
    };
    setHeight();
    const ro = new ResizeObserver(setHeight);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

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
  }, [recordingId, selectedStepId]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const data = await invoke<RecordingWithSteps>("load_recording", {
          id: recordingId,
        });
        if (cancelled) return;
        setRecording(data.recording);
        setSteps(data.steps);
        if (data.steps.length > 0) {
          setSelectedStepId(data.steps[0].id);
        }

        // 默认视图：全图（screenshot_base64）；裁剪图通过分段按钮手动切换
        setStepViewCropped({});
        setGlobalView('full');
        setCropCache({});
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
  const selectedIndex = steps.findIndex((s) => s.id === selectedStepId);

  /* 主截图解析：全图（screenshot_base64）或裁剪图（缓存）；裁剪图已删或全图缺失时返回 null */
  const getStepMainSrc = (step: Step): string | null => {
    const base = step.screenshot_base64;
    if (!base) return null;
    const useCrop = getViewCropped(step.id);
    if (useCrop && isCropDeleted(step.id)) return null;
    if (useCrop) {
      const cropped = cropCache[step.id];
      if (cropped) return cropped;
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
    } catch (err) {
      console.error("Save step failed:", err);
    } finally {
      setSaving(false);
    }
  }, [selectedStep, editTitle, editDescription]);

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
        })
        .catch((err) => console.error("Upload screenshot failed:", err));
    },
    [selectedStep]
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
          });
      };
      reader.readAsDataURL(file);
    };
    input.click();
  }, [selectedStep]);

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
      if (selectedStepId === stepId) {
        setSelectedStepId((prevId) => {
          const idx = steps.findIndex((s) => s.id === prevId);
          if (steps.length <= 1) return "";
          if (idx < steps.length - 1) return steps[idx + 1].id;
          return steps[idx - 1].id;
        });
      }
    },
    [selectedStepId, steps]
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
    [recordingId]
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
    [recordingId, showSaveDialog]
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

      // 压缩每张截图，显著降低上传体积（修复“保存草稿/发布失败”：完整截图会导致请求体过大/超时）
      const steps = await Promise.all(
        data.steps.map(async (s) => ({
          order_index: s.order_index,
          step_number: s.step_number,
          action_type: s.action_type,
          title: s.title,
          description: s.description,
          tip: s.tip,
          x: s.x,
          y: s.y,
          window_title: s.window_title,
          screenshot_base64: s.screenshot_base64 ? await downscaleBase64Image(s.screenshot_base64) : undefined,
          before_screenshot_base64: s.before_screenshot_base64 ? await downscaleBase64Image(s.before_screenshot_base64) : undefined,
          after_screenshot_base64: s.after_screenshot_base64 ? await downscaleBase64Image(s.after_screenshot_base64) : undefined,
        }))
      );

      // 构建上传数据（截图用 base64）
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
        steps,
      };
      if (pubCoverBase64) uploadData.cover_base64 = pubCoverBase64;

      // POST 到网站 API（上传大量截图时可能较慢，设置 90s 超时）
      const PB_URL = "http://47.102.154.233";
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
      setShowPublishDialog(false);
      setPubStep("pick");
      setToast({ type: "success", message: isDraft ? "草稿已保存！" : "发布成功！教程已上传到网站" });
    } catch (err) {
      const isAbort = (err as { name?: string })?.name === "AbortError";
      setToast({ type: "error", message: isAbort ? "发布超时（90秒），请检查网络后重试" : "发布失败: " + String(err) });
    } finally {
      setPublishing(false);
    }
  }, [siteToken, recording, recordingId, categories, pubTitle, pubApplications, pubCategory, pubTags, pubDownloadable, pubCoverBase64, pubSceneSelect]);

  /* ── 隐私脱敏 ────────────────────────────────────────────── */

  // 文本脱敏：对步骤描述中的敏感信息执行替换
  const handleRedact = useCallback(async () => {
    setRedacting(true);
    try {
      const changed = await invoke<number>("redact_recording_text", { recordingId });
      setToast({ type: "success", message: changed > 0 ? "文本脱敏完成" : "未检测到敏感信息" });
      await reloadRecording();
    } catch (err) {
      setToast({ type: "error", message: "隐私脱敏失败: " + String(err) });
    } finally {
      setRedacting(false);
    }
  }, [recordingId, reloadRecording]);

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
        await reloadRecording();
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
  }, [recordingId, reloadRecording, steps.length, showScanToast]);

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
      const result: string = await invoke("generate_ai_steps", { recordingId });
      alert(result);
      await reloadRecording();
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
  }, [recordingId, reloadRecording]);

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
        <button className="btn mt-3" onClick={() => navigate("/")}>
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
        title="返回"
        style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
      >
        <ArrowLeft size={14} /> 返回
      </button>
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
          style={{ color: "#e6edf3", fontSize: 14, fontWeight: 600, userSelect: "none", cursor: "pointer", padding: "2px 6px", borderRadius: 4 }}
          title="点击编辑名称"
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
          title="导出"
          style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          <Download size={14} /> {exportProgress ? exportProgress : exporting ? "导出中..." : "导出"} <ChevronDown size={12} />
        </button>
        {exportOpen && (
          <div className="export-dropdown" style={{ right: 0 }}>
            {EXPORT_FORMATS.map((fmt) => {
              const Icon = fmt.icon;
              return (
                <button
                  key={fmt.value}
                  className="export-dropdown-item"
                  onClick={() => handleExport(fmt.value)}
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
        <button
          onClick={async () => {
            if (!siteToken) {
              try {
                await invoke("open_external_url", { url: "http://47.102.154.233/login?redirect=flowio://auth" });
              } catch (err) {
                setToast({ type: "error", message: "打开登录页失败: " + String(err) });
              }
              return;
            }
            setShowPublishDialog(true);
            setPubStep("pick");
            setPubTitle(recording?.title || "");
          }}
          title={siteToken ? "上传到 flowio.cn" : "登录后可上传到网站"}
          style={{
            background: siteToken ? "#3366CC" : "rgba(51,102,204,0.15)",
            border: "none",
            color: siteToken ? "#fff" : "#93b4e8",
            cursor: "pointer",
            padding: "3px 10px",
            borderRadius: 4,
            fontSize: 13,
            display: "flex",
            alignItems: "center",
            gap: 4,
          }}
        >
          <Upload size={14} />
          {siteToken ? "上传网站" : "登录后上传"}
        </button>
      )}

      {!previewMode && (
        <div className="export-dropdown-wrap" style={{ position: "relative" }} onMouseDown={(e) => e.stopPropagation()}>
          <button
            onClick={() => setToolsOpen(!toolsOpen)}
            title="更多工具"
            style={{ background: "transparent", border: "1px solid #ff6b35", color: "#ff8a5c", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
          >
            <Wrench size={14} /> 工具 <ChevronDown size={12} />
          </button>
          {toolsOpen && (
            <div className="export-dropdown" style={{ right: 0, zIndex: 9999 }}>
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); handleAiGenerate(); }}
                disabled={aiGenerating}
              >
                {aiGenerating ? <Loader2 size={14} className="spinner" /> : <Sparkles size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">{aiGenerating ? "生成中..." : "AI 生成"}</span>
              </button>
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); handleRedact(); }}
                disabled={redacting}
              >
                {redacting ? <Loader2 size={14} className="spinner" /> : <ShieldCheck size={14} className="export-dropdown-item-icon" />}
                <span className="export-dropdown-item-label">{redacting ? "脱敏中..." : "文本脱敏"}</span>
              </button>
              <button
                className="export-dropdown-item"
                onClick={() => { setToolsOpen(false); handleScanRedact(); }}
                disabled={scanRedacting || steps.length === 0}
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
        title={previewMode ? "退出预览" : "预览"}
        style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
      >
        {previewMode ? <><X size={14} /> 退出预览</> : <><Eye size={14} /> 预览</>}
      </button>
      {/* 全图 / 裁剪图 统一切换：一键设置所有步骤主截图，选择教程主要风格 */}
      <div className="titlebar-view-segmented" title="选择教程主要截图风格">
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

      {/* Left: Step List */}
      <div className="editor-sidebar">
        <div className="text-sm text-secondary mb-2" style={{ padding: "0 16px" }}>
          {steps.length} 个步骤
        </div>

        {/* Step list */}
        <div className="step-list">
          {/* Insert zone before first step */}
          <div className="step-insert-point">
            <div className="step-insert-line" />
            <div className="step-insert-btn" onClick={() => handleInsertStep(-1)}>
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
                    title="删除步骤"
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
                        >
                          取消
                        </button>
                        <button
                          className="step-delete-confirm"
                          onClick={(e) => { e.stopPropagation(); handleDeleteStep(step.id); }}
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
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ opacity: 0.4 }}><path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z"/><circle cx="12" cy="13" r="4"/></svg>
                    </div>
                  );
                })()}
              </div>
              {/* Insert zone after this step */}
              <div className="step-insert-point">
                <div className="step-insert-line" />
                <div className="step-insert-btn" onClick={() => handleInsertStep(index)}>
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
                    <h3 className="sop-preview-step-title">{step.title || "(无标题)"}</h3>

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
                    {step.description && (
                      <div className="sop-preview-desc">{step.description}</div>
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
              <div
                className="step-detail-image"
                ref={stepDetailImageRef}
              >
                {(() => {
                  // 主截图：该步骤 stepViewCropped=true 且有裁剪图缓存时用裁剪图，否则用全图
                  const src = getStepMainSrc(selectedStep) || "";
                  return (
                    <>
                      {src ? (
                        <>
                          <img
                            ref={detailImgRef}
                            src={`data:image/jpeg;base64,${src}`}
                            alt={`步骤 ${selectedIndex + 1} 截图`}
                            onClick={() => openLightbox(selectedIndex)}
                            style={{ cursor: "pointer" }}
                          />
                          {/* 主截图模式悬浮胶囊按钮：右下角悬浮，点击切换当前步骤全图/裁图 */}
                          <button
                            className="step-detail-view-pill"
                            onClick={(e) => {
                              e.stopPropagation();
                              const cur = getViewCropped(selectedStep?.id ?? '');
                              setViewCropped(selectedStep?.id ?? '', !cur);
                              setGlobalView(!cur ? 'crop' : 'full');
                            }}
                            title="切换主截图：全图 / 裁图"
                          >
                            {getViewCropped(selectedStep?.id ?? '') ? '裁图' : '全图'}
                          </button>
                          <button
                            className="screenshot-annotate-btn"
                            onClick={(e) => {
                              e.stopPropagation();
                              // 标注固定使用 after 全图（screenshot_base64）
                              openAnnotate(selectedStep.id);
                            }}
                            title="裁剪/添加文字/高亮/箭头/模糊标注"
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
                              // 删除只针对当前视图：裁图模式删裁图（全图保留），全图模式删全图
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
                                  // 全图删除后裁图也不复存在，清理裁图删除标记并切回全图视图
                                  setDeletedCrop(prev => ({ ...prev, [selectedStep.id]: false }));
                                  setViewCropped(selectedStep.id, false);
                                }
                              }
                            }}
                            title="删除当前截图"
                          >
                            ✕
                          </button>
                        </>
                      ) : (
                        <>
                          <svg viewBox="0 0 16 9" style={{ width: '100%', pointerEvents: 'none', visibility: 'hidden' }} />
                          <div
                            className="step-detail-no-image"
                            onClick={pickFileForUpload}
                            onContextMenu={pasteScreenshot}
                        >
                          <div className="upload-hint">
                            <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
                              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
                              <circle cx="8.5" cy="8.5" r="1.5"/>
                              <polyline points="21 15 16 10 5 21"/>
                            </svg>
                            <span>上传截图</span>
                            <span>点击或拖拽上传步骤截图</span>
                          </div>
                        </div>
                          {/* 另一张图仍存在时保留切换入口：裁图被删但全图还在 → 可切回全图 */}
                          {selectedStep.screenshot_base64 && (
                            <button
                              className="step-detail-view-pill"
                              onClick={(e) => {
                                e.stopPropagation();
                                setViewCropped(selectedStep.id, false);
                              }}
                              title="切换到全图"
                            >
                              查看全图
                            </button>
                          )}
                        </>
                      )}
                    </>
                  );
                })()}
              </div>

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

              {/* Save button */}
              <div className="step-detail-actions">
                <button
                  className="btn btn-primary"
                  onClick={handleSaveStep}
                  disabled={saving}
                >
                  {saving ? (
                    <>
                      <Loader2 size={14} className="spinner" /> 保存中...
                    </>
                  ) : saveSuccess ? (
                    <>
                      <Check size={14} /> 已保存
                    </>
                  ) : (
                    "保存修改"
                  )}
                </button>
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
                  <button onClick={closePub} title="关闭" style={{ background: "transparent", border: "none", color: "#8b949e", cursor: "pointer", padding: 4, borderRadius: 4 }}>
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
                        <button type="button" onClick={() => { setPubCoverBase64(null); setPubCoverPreview(null); }} style={pubDialogStyles.addBtn}>移除</button>
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
                      <button style={pubDialogStyles.addBtn} onClick={addApp}>添加</button>
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
                      <button style={pubDialogStyles.addBtn} onClick={addTag}>添加</button>
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
                        style={{ flex: 1, padding: "10px 0", borderRadius: 8, border: "1px solid #30363d", background: "transparent", color: "#e6edf3", cursor: publishing ? "default" : "pointer", fontSize: 14 }}
                      >
                        返回
                      </button>
                      <button
                        onClick={() => handlePublish("published")}
                        disabled={publishing}
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
    </div>
  );
}
