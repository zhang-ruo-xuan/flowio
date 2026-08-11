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
} from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import FileSaveDialog from "../components/FileSaveDialog";
import type { Recording, Step } from "../types";

/* ── Export format options ─────────────────────────────────── */

const EXPORT_FORMATS = [
  { value: "html", label: "HTML 网页", ext: ".html", icon: FileCode },
  { value: "docx", label: "Word 文档", ext: ".docx", icon: FileText },
  { value: "pdf", label: "PDF", ext: ".pdf", icon: File },
] as const;

/* ── Backend response types ────────────────────────────────── */

interface RecordingWithSteps {
  recording: Recording;
  steps: Step[];
}

/* ── Lightbox ──────────────────────────────────────────────── */

interface LightboxProps {
  steps: Step[];
  currentIndex: number;
  onClose: () => void;
  onPrev: () => void;
  onNext: () => void;
  /** When true the lightbox shows the "marked" (annotated) variant */
  showMarked: boolean;
  onToggleMarked: () => void;
  /** When true the lightbox shows after screenshot; false shows before */
  showAfter: boolean;
}

function Lightbox({
  steps,
  currentIndex,
  onClose,
  onPrev,
  onNext,
  showMarked,
  onToggleMarked,
  showAfter,
}: LightboxProps) {
  const step = steps[currentIndex];
  if (!step) return null;

  const srcBase64 = showAfter
    ? step.screenshot_base64
    : step.before_screenshot_base64;
  if (!srcBase64) return null;

  return (
    <div className="lightbox-overlay" onClick={onClose}>
      <div className="lightbox-content" onClick={(e) => e.stopPropagation()}>
        {/* Close button */}
        <button className="lightbox-close" onClick={onClose}>
          <X size={24} />
        </button>

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

export default function StepEditor() {
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
  const [exporting, setExporting] = useState(false);
  const [showSaveDialog, setShowSaveDialog] = useState<string | null>(null);
  const [toast, setToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const [aiGenerating, setAiGenerating] = useState(false);

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
  const [stepViewAfter, setStepViewAfter] = useState<Record<string, boolean>>({});
  const getViewAfter = (stepId: string) => stepViewAfter[stepId] ?? false;
  const setViewAfter = (stepId: string, value: boolean) => setStepViewAfter(prev => ({ ...prev, [stepId]: value }));

  /* Tip expand */
  const [tipExpanded, setTipExpanded] = useState(false);

  /* Crop */
  const [cropMode, setCropMode] = useState(false);
  const [cropTarget, setCropTarget] = useState<"before" | "after" | "marked">("before");
  const [cropBox, setCropBox] = useState<{ x: number; y: number; w: number; h: number } | null>(null);
  const [cropImgNatural, setCropImgNatural] = useState<{ nw: number; nh: number }>({ nw: 0, nh: 0 });
  const cropImgRef = useRef<HTMLImageElement>(null);
  const detailImgRef = useRef<HTMLImageElement>(null);

  const stepDetailImageRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = stepDetailImageRef.current;
    if (!el) return;
    const setHeight = () => {
      const w = el.clientWidth;
      if (w > 0) {
        el.style.height = `${Math.min(Math.max(w * 9 / 16, 200), 420)}px`;
      }
    };
    setHeight();
    const ro = new ResizeObserver(setHeight);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  /* Crop drag state: track which handle / move is active */
  type HandleId = "top" | "bottom" | "left" | "right";
  interface DragState {
    type: "move" | "resize";
    handle?: HandleId;
    startMouseX: number; // clientX
    startMouseY: number; // clientY
    startBox: { x: number; y: number; w: number; h: number };
  }
  const [dragState, setDragState] = useState<DragState | null>(null);

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

        // Default: show "after" screenshot for click/type/keyboard steps (has action marks)
        const actionTypes = new Set(["click", "type", "keyboard"]);
        const initial: Record<string, boolean> = {};
        for (const s of data.steps) {
          initial[s.id] = actionTypes.has(s.action_type);
        }
        setStepViewAfter(initial);
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
  const selectedIndex = steps.findIndex((s) => s.id === selectedStepId);

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
        invoke("upload_step_screenshot", { stepId: selectedStep.id, base64Data: base64, isAfter: false, isMarked: false })
          .then(() => {
            setSteps(prev => prev.map(s =>
              s.id !== selectedStep.id ? s : {
                ...s,
                screenshot_base64: base64,
                before_screenshot_base64: base64
              }
            ));
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
                    invoke("upload_step_screenshot", { stepId: selectedStep.id, base64Data: base64, isAfter: false, isMarked: false })
                      .then(() => {
                        setSteps(prev => prev.map(s =>
                          s.id !== selectedStep.id ? s : {
                            ...s,
                            screenshot_base64: base64,
                            before_screenshot_base64: base64
                          }
                        ));
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

  const stepListEndRef = useRef<HTMLDivElement>(null);

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
          stepListEndRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
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
      }
    },
    [recordingId, showSaveDialog]
  );

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

  /* ── Crop ────────────────────────────────────────────────── */

  /** Convert clientX/clientY to natural-image coordinates */
  const clientToNatural = (clientX: number, clientY: number) => {
    if (!cropImgRef.current) return { x: 0, y: 0 };
    const img = cropImgRef.current;
    const rect = img.getBoundingClientRect();
    return {
      x: (clientX - rect.left) * (img.naturalWidth / rect.width),
      y: (clientY - rect.top) * (img.naturalHeight / rect.height),
    };
  };

  const enterCropMode = (target: "before" | "after" | "marked") => {
    setCropTarget(target);
    setCropMode(true);
    setDragState(null);
    // Initialize crop box to full image size using the detail image (always mounted)
    const detailImg = detailImgRef.current;
    if (detailImg && detailImg.naturalWidth > 0) {
      setCropBox({ x: 0, y: 0, w: detailImg.naturalWidth, h: detailImg.naturalHeight });
    }
    // else: onLoad of overlay img will set cropImgNatural and we'll init there
  };

  const cancelCrop = () => {
    setCropMode(false);
    setCropBox(null);
    setDragState(null);
  };

  const handleCropMouseDown = (e: React.MouseEvent<HTMLImageElement>) => {
    if (!cropMode || !cropImgRef.current) return;
    e.preventDefault();
    const img = cropImgRef.current;
    const rect = img.getBoundingClientRect();
    const scaleX = img.naturalWidth / rect.width;
    const scaleY = img.naturalHeight / rect.height;
    const nx = (e.clientX - rect.left) * scaleX;
    const ny = (e.clientY - rect.top) * scaleY;

    const EDGE_HIT = 8; // px display-space tolerance for edge hit

    if (cropBox) {
      const bx = cropBox.x / scaleX;
      const by = cropBox.y / scaleY;
      const bw = cropBox.w / scaleX;
      const bh = cropBox.h / scaleY;
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;

      // Top edge (includes top-left / top-right corners)
      if (Math.abs(my - by) <= EDGE_HIT && mx >= bx - EDGE_HIT && mx <= bx + bw + EDGE_HIT) {
        setDragState({ type: "resize", handle: "top", startMouseX: e.clientX, startMouseY: e.clientY, startBox: { ...cropBox } });
        return;
      }
      // Bottom edge
      if (Math.abs(my - (by + bh)) <= EDGE_HIT && mx >= bx - EDGE_HIT && mx <= bx + bw + EDGE_HIT) {
        setDragState({ type: "resize", handle: "bottom", startMouseX: e.clientX, startMouseY: e.clientY, startBox: { ...cropBox } });
        return;
      }
      // Left edge
      if (Math.abs(mx - bx) <= EDGE_HIT && my >= by - EDGE_HIT && my <= by + bh + EDGE_HIT) {
        setDragState({ type: "resize", handle: "left", startMouseX: e.clientX, startMouseY: e.clientY, startBox: { ...cropBox } });
        return;
      }
      // Right edge
      if (Math.abs(mx - (bx + bw)) <= EDGE_HIT && my >= by - EDGE_HIT && my <= by + bh + EDGE_HIT) {
        setDragState({ type: "resize", handle: "right", startMouseX: e.clientX, startMouseY: e.clientY, startBox: { ...cropBox } });
        return;
      }

      // Check if click is inside the crop box → move
      if (nx >= cropBox.x && nx <= cropBox.x + cropBox.w && ny >= cropBox.y && ny <= cropBox.y + cropBox.h) {
        setDragState({
          type: "move",
          startMouseX: e.clientX,
          startMouseY: e.clientY,
          startBox: { ...cropBox },
        });
        return;
      }
    }

    // Click outside → do nothing
  };

  /** Clamp crop box within image boundaries */
  const clampBox = (box: { x: number; y: number; w: number; h: number }, imgW: number, imgH: number, minW = 10, minH = 10) => {
    const clamped = { ...box };
    if (clamped.x < 0) { clamped.w += clamped.x; clamped.x = 0; }
    if (clamped.y < 0) { clamped.h += clamped.y; clamped.y = 0; }
    if (clamped.x + clamped.w > imgW) clamped.w = imgW - clamped.x;
    if (clamped.y + clamped.h > imgH) clamped.h = imgH - clamped.y;
    if (clamped.w < minW) clamped.w = minW;
    if (clamped.h < minH) clamped.h = minH;
    return clamped;
  };

  const handleCropMouseMove = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!dragState || !cropImgRef.current) return;
    e.preventDefault();
    const img = cropImgRef.current;
    const rect = img.getBoundingClientRect();
    const scaleX = img.naturalWidth / rect.width;
    const scaleY = img.naturalHeight / rect.height;
    const dx = (e.clientX - dragState.startMouseX) * scaleX;
    const dy = (e.clientY - dragState.startMouseY) * scaleY;
    const { startBox } = dragState;
    const imgW = img.naturalWidth;
    const imgH = img.naturalHeight;

    let newBox = { ...startBox };

    if (dragState.type === "move") {
      newBox.x = startBox.x + dx;
      newBox.y = startBox.y + dy;
      // Clamp so the box stays within image
      if (newBox.x < 0) newBox.x = 0;
      if (newBox.y < 0) newBox.y = 0;
      if (newBox.x + newBox.w > imgW) newBox.x = imgW - newBox.w;
      if (newBox.y + newBox.h > imgH) newBox.y = imgH - newBox.h;
    } else {
      // Resize
      switch (dragState.handle) {
        case "top":
          newBox = { x: startBox.x, y: startBox.y + dy, w: startBox.w, h: startBox.h - dy };
          break;
        case "bottom":
          newBox = { x: startBox.x, y: startBox.y, w: startBox.w, h: startBox.h + dy };
          break;
        case "left":
          newBox = { x: startBox.x + dx, y: startBox.y, w: startBox.w - dx, h: startBox.h };
          break;
        case "right":
          newBox = { x: startBox.x, y: startBox.y, w: startBox.w + dx, h: startBox.h };
          break;
      }
      newBox = clampBox(newBox, imgW, imgH);
    }

    setCropBox(newBox);
  };

  const handleCropMouseUp = () => {
    if (dragState) {
      setDragState(null);
    }
  };

  const confirmCrop = async () => {
    if (!cropBox || !selectedStep || !cropImgRef.current) return;
    const img = cropImgRef.current;

    // Crop using canvas
    const canvas = document.createElement("canvas");
    canvas.width = cropBox.w;
    canvas.height = cropBox.h;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.drawImage(img, cropBox.x, cropBox.y, cropBox.w, cropBox.h, 0, 0, cropBox.w, cropBox.h);

    const croppedBase64 = canvas.toDataURL("image/jpeg", 0.9).split(",")[1];

    try {
      await invoke("crop_step_screenshot", {
        stepId: selectedStep.id,
        croppedBase64,
        isAfter: cropTarget === "after",
      });

      setSteps((prev) =>
        prev.map((s) => {
          if (s.id !== selectedStep.id) return s;
          if (cropTarget === "after") {
            return { ...s, screenshot_base64: croppedBase64 };
          }
          if (cropTarget === "marked") {
            return { ...s, marked_screenshot_base64: croppedBase64 };
          }
          return { ...s, before_screenshot_base64: croppedBase64 };
        })
      );

      cancelCrop();
    } catch (err) {
      console.error("Crop failed:", err);
    }
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
          style={{ color: "#1f2328", fontSize: 14, fontWeight: 600, padding: "2px 6px", border: "1px solid #0969da", borderRadius: 4, outline: "none", width: 200, background: "#fff" }}
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
      <div className="export-dropdown-wrap" style={{ position: "relative" }}>
        <button
          onClick={() => setExportOpen(!exportOpen)}
          disabled={exporting}
          title="导出"
          style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          <Download size={14} /> {exporting ? "导出中..." : "导出"} <ChevronDown size={12} />
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
                  <span className="export-dropdown-item-ext">{fmt.ext}</span>
                </button>
              );
            })}
          </div>
        )}
      </div>
      {!previewMode && (
        <button
          onClick={handleAiGenerate}
          disabled={aiGenerating}
          title="AI 生成"
          style={{ background: "transparent", border: "1px solid #7c3aed", color: "#a78bfa", cursor: "pointer", padding: "3px 8px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
        >
          {aiGenerating ? (
            <><Loader2 size={14} className="spinner" /> 生成中...</>
          ) : (
            <><Sparkles size={14} /> AI 生成</>
          )}
        </button>
      )}
      <button
        onClick={() => setPreviewMode(!previewMode)}
        title={previewMode ? "退出预览" : "预览"}
        style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "4px 6px", borderRadius: 4, fontSize: 13, display: "flex", alignItems: "center", gap: 4 }}
      >
        {previewMode ? <><X size={14} /> 退出预览</> : <><Eye size={14} /> 预览</>}
      </button>
    </div>
  );

  /* ── Render ──────────────────────────────────────────────── */

  return (
    <div className="editor-layout">
      {/* Portal toolbar into titlebar */}
      {createPortal(titlebarLeft, document.getElementById("titlebar-editor-left")!)}
      {createPortal(titlebarActions, document.getElementById("titlebar-editor-actions")!)}

      {/* Left: Step List */}
      <div className="editor-sidebar">
        <div className="text-sm text-secondary mb-2" style={{ padding: "0 16px" }}>
          {steps.length} 个步骤
        </div>

        {/* Step list */}
        <div className="step-list">
          {/* Insert zone before first step */}
          <div className="step-insert-point" onClick={() => handleInsertStep(-1)}>
            <div className="step-insert-line" />
            <div className="step-insert-btn">
              <Plus size={14} />
            </div>
          </div>

          {steps.map((step, index) => (
            <Fragment key={step.id}>
              <div
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
                  const after = getViewAfter(step.id);
                  const src = after
                    ? (step.screenshot_base64 || step.before_screenshot_base64)
                    : (step.before_screenshot_base64 || step.screenshot_base64);
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
              <div className="step-insert-point" onClick={() => handleInsertStep(index)}>
                <div className="step-insert-line" />
                <div className="step-insert-btn">
                  <Plus size={14} />
                </div>
              </div>
            </Fragment>
          ))}
          <div ref={stepListEndRef} />
        </div>
      </div>

      {/* Right: Step Detail / SOP Preview */}
      <div className={`editor-preview${cropMode ? " crop-active" : ""}${previewMode ? " preview-active" : ""}`}>
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
                      const after = getViewAfter(step.id);
                      const src = after
                        ? (step.screenshot_base64 || step.before_screenshot_base64)
                        : (step.before_screenshot_base64 || step.screenshot_base64);
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
                  const src = getViewAfter(selectedStep?.id ?? '')
                    ? selectedStep.screenshot_base64
                    : selectedStep.before_screenshot_base64;
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
                          <button
                            className="screenshot-delete-btn"
                            onClick={(e) => {
                              e.stopPropagation();
                              const label = getViewAfter(selectedStep?.id ?? '') ? '操作后' : '操作前';
                              if (confirm(`确定删除${label}截图？`)) {
                                invoke('delete_step_screenshot', {
                                  stepId: selectedStep.id,
                                  isAfter: getViewAfter(selectedStep?.id ?? ''),
                                });
                                const updated = [...steps];
                                if (getViewAfter(selectedStep?.id ?? '')) {
                                  updated[selectedIndex] = { ...updated[selectedIndex], screenshot_base64: null };
                                } else {
                                  updated[selectedIndex] = { ...updated[selectedIndex], before_screenshot_base64: null };
                                }
                                setSteps(updated);
                                const s = updated[selectedIndex];
                                if (getViewAfter(selectedStep?.id ?? '')) {
                                  if (!s.screenshot_base64 && s.before_screenshot_base64) {
                                    setViewAfter(selectedStep?.id ?? '', false);
                                  }
                                } else {
                                  if (!s.before_screenshot_base64 && s.screenshot_base64) {
                                    setViewAfter(selectedStep?.id ?? '', true);
                                  }
                                }
                              }
                            }}
                            title="删除截图"
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
                        </>
                      )}
                    </>
                  );
                })()}
              </div>

              {/* ── Screenshot toolbar ── */}
              {(selectedStep.screenshot_base64 || selectedStep.before_screenshot_base64) && (
                <div className="image-overlay-toolbar">
                  <div className="step-screenshot-segmented">
                    <button
                      className={`step-screenshot-segmented-btn ${getViewAfter(selectedStep?.id ?? '') ? '' : 'active'}`}
                      onClick={() => setViewAfter(selectedStep?.id ?? '', false)}
                    >操作前</button>
                    <button
                      className={`step-screenshot-segmented-btn ${getViewAfter(selectedStep?.id ?? '') ? 'active' : ''}`}
                      onClick={() => setViewAfter(selectedStep?.id ?? '', true)}
                    >操作后</button>
                  </div>
                  <button
                    className="step-screenshot-crop-btn"
                    onClick={() => enterCropMode(getViewAfter(selectedStep?.id ?? '') ? 'after' : 'before')}
                  >
                    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
                      <rect x="3" y="3" width="10" height="10" rx="1" />
                      <path d="M3 7h2l2 4 3-6 2 3h1" />
                    </svg>
                    裁剪
                  </button>
                </div>
              )}

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
          showMarked={lightboxMarked}
          onToggleMarked={() => setLightboxMarked(!lightboxMarked)}
          showAfter={getViewAfter(selectedStep?.id ?? '')}
        />
      )}

      {/* ── Crop overlay ──────────────────────────────────── */}
      {cropMode && selectedStep && (
        <div className="crop-overlay" onClick={cancelCrop} onMouseMove={handleCropMouseMove} onMouseUp={handleCropMouseUp}>
          <div
            className="crop-overlay-container"
            onClick={e => e.stopPropagation()}
          >
            <img
              ref={cropImgRef}
              src={`data:image/jpeg;base64,${getViewAfter(selectedStep?.id ?? '') ? selectedStep.screenshot_base64 : selectedStep.before_screenshot_base64}`}
              alt="裁剪"
              onMouseDown={handleCropMouseDown}
              onLoad={(e) => {
                const img = e.currentTarget;
                const nw = img.naturalWidth;
                const nh = img.naturalHeight;
                setCropImgNatural({ nw, nh });
                // If cropBox wasn't initialized by enterCropMode, set it now
                if (!cropBox && nw > 0) {
                  setCropBox({ x: 0, y: 0, w: nw, h: nh });
                }
              }}
              style={{ cursor: dragState ? "none" : "crosshair", userSelect: "none", display: "block", maxWidth: "90vw", maxHeight: "75vh", objectFit: "contain" }}
            />
            {/* Crop box overlay + edge strips */}
            {cropBox && cropImgNatural.nw > 0 && (() => {
              const edgeThickness = 10; // 10px edge strip, 5px on each side of the line
              const leftPct = `${(cropBox.x / cropImgNatural.nw) * 100}%`;
              const topPct = `${(cropBox.y / cropImgNatural.nh) * 100}%`;
              const widthPct = `${(cropBox.w / cropImgNatural.nw) * 100}%`;
              const heightPct = `${(cropBox.h / cropImgNatural.nh) * 100}%`;
              const edgeBaseStyle = (cursor: string): React.CSSProperties => ({
                position: "absolute",
                zIndex: 5,
                boxSizing: "border-box",
                cursor,
                background: "rgba(74,144,217,0.3)",
                transition: "background 0.1s",
              });
              const onEdgeMouseDown = (handle: HandleId) => (e: React.MouseEvent) => {
                e.preventDefault();
                e.stopPropagation();
                setDragState({ type: "resize", handle, startMouseX: e.clientX, startMouseY: e.clientY, startBox: { ...cropBox } });
              };
              return (
                <>
                  {/* Darkening mask */}
                  <div
                    className="crop-box-overlay"
                    style={{
                      position: "absolute",
                      left: leftPct, top: topPct, width: widthPct, height: heightPct,
                      boxShadow: "0 0 0 9999px rgba(0, 0, 0, 0.55)",
                      border: "2px dashed rgba(255,255,255,0.85)",
                      pointerEvents: "none",
                      boxSizing: "border-box",
                      zIndex: 3,
                    }}
                  />
                  {/* Top edge */}
                  <div onMouseDown={onEdgeMouseDown("top")}
                    onMouseEnter={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.6)"}
                    onMouseLeave={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.3)"}
                    style={{ ...edgeBaseStyle("ns-resize"), left: leftPct, top: `calc(${topPct} - ${edgeThickness / 2}px)`, width: widthPct, height: edgeThickness }} />
                  {/* Bottom edge */}
                  <div onMouseDown={onEdgeMouseDown("bottom")}
                    onMouseEnter={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.6)"}
                    onMouseLeave={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.3)"}
                    style={{ ...edgeBaseStyle("ns-resize"), left: leftPct, top: `calc(${topPct} + ${heightPct} - ${edgeThickness / 2}px)`, width: widthPct, height: edgeThickness }} />
                  {/* Left edge */}
                  <div onMouseDown={onEdgeMouseDown("left")}
                    onMouseEnter={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.6)"}
                    onMouseLeave={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.3)"}
                    style={{ ...edgeBaseStyle("ew-resize"), left: `calc(${leftPct} - ${edgeThickness / 2}px)`, top: topPct, width: edgeThickness, height: heightPct }} />
                  {/* Right edge */}
                  <div onMouseDown={onEdgeMouseDown("right")}
                    onMouseEnter={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.6)"}
                    onMouseLeave={e => (e.currentTarget as HTMLDivElement).style.background = "rgba(74,144,217,0.3)"}
                    style={{ ...edgeBaseStyle("ew-resize"), left: `calc(${leftPct} + ${widthPct} - ${edgeThickness / 2}px)`, top: topPct, width: edgeThickness, height: heightPct }} />
                  {/* Size label */}
                  <div
                    style={{
                      position: "absolute",
                      left: `calc(${leftPct} + 4px)`,
                      top: `calc(${topPct} + ${heightPct} + 6px)`,
                      fontSize: 11,
                      color: "#fff",
                      background: "rgba(0,0,0,0.6)",
                      padding: "1px 6px",
                      borderRadius: 3,
                      zIndex: 5,
                      pointerEvents: "none",
                    }}
                  >
                    {Math.round(cropBox.w)} x {Math.round(cropBox.h)}
                  </div>
                </>
              );
            })()}
          </div>
          {/* Toolbar */}
          <div className="crop-overlay-toolbar">
            <span>拖拽边框调整裁剪框</span>
            <button className="btn btn-sm btn-primary" onClick={confirmCrop} disabled={!cropBox}>确认</button>
            <button className="btn btn-sm" onClick={cancelCrop}>取消</button>
          </div>
        </div>
      )}

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
            zIndex: 9999,
            display: "flex",
            alignItems: "center",
            gap: "12px",
            padding: "12px 16px",
            borderRadius: "8px",
            fontSize: "16px",
            fontWeight: 500,
            boxShadow: "0 4px 12px rgba(0,0,0,0.15)",
            background: toast.type === "success" ? "#f0fdf4" : "#fef2f2",
            color: toast.type === "success" ? "#166534" : "#991b1b",
            border: toast.type === "success" ? "1px solid #bbf7d0" : "1px solid #fecaca",
          }}
        >
          <span>{toast.message}</span>
        </div>,
        document.body
      )}
    </div>
  );
}
