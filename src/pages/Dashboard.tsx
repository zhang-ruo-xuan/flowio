import { useEffect, useLayoutEffect, useState, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { Circle, Search, MoreVertical, Play, Pencil, CheckCircle2, XCircle, Trash2, Pause, Square, ListChecks, Download, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import ExportDialog, { type ExportFormat, type ExportOptions } from "../components/ExportDialog";
import type { Step } from "../types";
import { useAuthStore } from "../store/authStore";

interface Recording {
  id: string;
  title: string;
  app_name: string;
  status: string;
  total_steps: number;
  created_at: string;
  ai_generated: boolean;
  text_redacted: boolean;
  scan_redacted: boolean;
  /** 来源域名（对齐插件 history 的 source_domain），无来源为空串 */
  source_domain: string;
}

function formatTime(dateStr: string): string {
  const d = new Date(dateStr + "Z");
  const now = new Date();
  const diffMs = now.getTime() - d.getTime();
  const diffMin = Math.floor(diffMs / 60000);
  const diffHr = Math.floor(diffMs / 3600000);
  const diffDay = Math.floor(diffMs / 86400000);

  if (diffMin < 1) return "刚刚";
  if (diffMin < 60) return `${diffMin} 分钟前`;
  if (diffHr < 24) return `${diffHr} 小时前`;
  if (diffDay === 1) return "昨天";
  if (diffDay < 7) return `${diffDay} 天前`;

  const thisYear = now.getFullYear();
  const dYear = d.getFullYear();
  if (dYear === thisYear) {
    return `${d.getMonth() + 1}月${d.getDate()}日`;
  }
  return `${dYear}/${d.getMonth() + 1}/${d.getDate()}`;
}

interface DashboardProps {
  onEdit: (id: string) => void;
  onStartRecording: () => void;
  recording?: boolean;
  isPaused?: boolean;
  onPauseRecording?: () => void;
  onResumeRecording?: () => void;
  onStopRecording?: () => void;
  locationKey?: string;
}

const PAGE_SIZE = 10;
const PAGE_STORAGE_KEY = "flowio_dashboard_page";

export default function Dashboard({
  onEdit,
  onStartRecording,
  recording,
  isPaused,
  onPauseRecording,
  onResumeRecording,
  onStopRecording,
  locationKey,
}: DashboardProps) {
  const { siteUser } = useAuthStore();
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [search, setSearch] = useState("");
  // 翻页状态需要跨 mount 持久化：用户翻到第 2 页点进教程，编辑器里改完
  // 返回时希望仍在第 2 页，否则每次都被打回第 1 页体感很差。sessionStorage
  // 在程序退出/重开时自动失效（→ 回到第 1 页），不需要额外清理。
  const [page, setPageState] = useState<number>(() => {
    const raw = sessionStorage.getItem(PAGE_STORAGE_KEY);
    const n = raw ? parseInt(raw, 10) : 1;
    return Number.isFinite(n) && n >= 1 ? n : 1;
  });
  const setPage = (updater: number | ((prev: number) => number)) => {
    setPageState((prev) => {
      const next = typeof updater === "function" ? updater(prev) : updater;
      sessionStorage.setItem(PAGE_STORAGE_KEY, String(next));
      return next;
    });
  };
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  // ★ 批量删除：选中 id 集合（跨页保留，删除后清空）
  const [selected, setSelected] = useState<string[]>([]);
  // ★ 批量模式（第二十一轮）：平时**不渲染**多选框列，点「批量」才进入；退出即清空选中。
  //   用户原话：「加一个批量的按钮，批量删除，批量导出什么的，平时的时候多选框不显示」
  const [batchMode, setBatchMode] = useState(false);
  const [batchExportOpen, setBatchExportOpen] = useState(false);
  const [batchBusy, setBatchBusy] = useState(false);
  const [batchProgress, setBatchProgress] = useState<{ done: number; total: number } | null>(null);
  const [menuPos, setMenuPos] = useState({ top: 0, left: 0 });
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [toast, setToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  // ★ 单条导出：菜单里只留一个「导出」按钮，点击后弹出 ExportDialog 选格式/选项，
  //   确认后直接导出到固定导出目录（不再弹保存框）。
  const [exportDialog, setExportDialog] = useState<{ id: string } | null>(null);
  // ★ 单条导出：弹窗打开时异步拉 steps，供 ExportDialog 导出前跑成片体检
  const [exportSteps, setExportSteps] = useState<Step[] | null>(null);
  const [editingAppId, setEditingAppId] = useState<string | null>(null);
  const [appNameDraft, setAppNameDraft] = useState("");
  const appNameInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  // ★ 行操作菜单：底部空间不足时向上弹出（记录触发按钮位置 + 定位完成前隐藏防闪烁）
  const menuAnchorRef = useRef<DOMRect | null>(null);
  const [menuPlaced, setMenuPlaced] = useState(false);
  const renameInputRef = useRef<HTMLInputElement>(null);
  // 表头「全选」框：本页部分选中时要显示 indeterminate（原生属性得手动设）
  const headCheckRef = useRef<HTMLInputElement>(null);

  // ★ 单条导出体检数据：弹窗打开时拉一次 steps（失败静默，退回不拦截）
  useEffect(() => {
    if (!exportDialog) {
      setExportSteps(null);
      return;
    }
    invoke<{ recording: unknown; steps: Step[] }>("load_recording", { id: exportDialog.id })
      .then((d) => setExportSteps(d.steps))
      .catch(() => setExportSteps(null));
  }, [exportDialog]);

  // Auto-dismiss toast（★ 10s：错误文案长，2s 看不完会误以为功能坏了）
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 10000);
    return () => clearTimeout(timer);
  }, [toast]);

  // 依赖登录态：登录/退出后按账号隔离刷新列表（Rust 端按 settings.current_user 过滤）
  const [listLoaded, setListLoaded] = useState(false);
  useEffect(() => {
    setListLoaded(false);
    invoke<Recording[]>("list_recordings")
      .then((r) => {
        setRecordings(r);
        setListLoaded(true);
      })
      .catch((err) => console.error("Failed to list recordings:", err));
  }, [locationKey, siteUser?.id]);

  useEffect(() => {
    if (renamingId && renameInputRef.current) {
      renameInputRef.current.focus();
      renameInputRef.current.select();
    }
  }, [renamingId]);

  useEffect(() => {
    if (editingAppId && appNameInputRef.current) {
      appNameInputRef.current.focus();
      appNameInputRef.current.select();
    }
  }, [editingAppId]);

  // Click outside closes menu
  useEffect(() => {
    if (!openMenuId) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpenMenuId(null);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [openMenuId]);

  // ★ 菜单渲染后测量真实尺寸：宽度是 max-content（随内容变），所以靠右对齐的 left
  //   必须量出来再定，不能再用写死的 160；高度则决定要不要翻到按钮上方弹出。
  //   测量发生在 useLayoutEffect 里（绘制前），而菜单在此之前 visibility:hidden，故无跳动。
  useLayoutEffect(() => {
    const el = menuRef.current;
    const anchor = menuAnchorRef.current;
    if (!openMenuId || !el || !anchor) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    let top = anchor.bottom + 4;
    if (top + h > window.innerHeight - 8) top = Math.max(8, anchor.top - h - 4);
    // 右边缘对齐 ⋮ 按钮；万一菜单比可用空间还宽/贴左边，兜到 8px 安全边距
    const left = Math.max(8, Math.min(anchor.right - w, window.innerWidth - w - 8));
    setMenuPos({ top, left });
    setMenuPlaced(true);
  }, [openMenuId]);

  const handleExport = (id: string) => {
    setOpenMenuId(null);
    setExportDialog({ id });
  };

  // ★ 固定导出目录（第四十八轮）：选完格式/选项直接导出到「文档\录步导出」，
  //   不再弹保存框选位置（顶栏有「打开导出目录」按钮随时去看）。
  const handleSingleFormatSelect = async (format: ExportFormat, options: ExportOptions) => {
    if (!exportDialog) return;
    const id = exportDialog.id;
    setExportDialog(null);
    let backendFormat: string = format;
    if (backendFormat === "images") {
      backendFormat = options.imageMode === "long" ? "long" : "images";
    } else if (backendFormat === "word") {
      backendFormat = "docx";
    }
    setToast({ type: "success", message: "正在导出…" });
    try {
      const path = await invoke<string>("export_recording", {
        id,
        format: backendFormat,
        options,
      });
      setToast({ type: "success", message: `已导出到 ${path}` });
    } catch (err) {
      console.error("Export failed:", err);
      setToast({ type: "error", message: "导出失败: " + String(err) });
    }
  };

  const handleDelete = async (recordingId: string) => {
    setOpenMenuId(null);
    const confirmed = await confirm('确定要删除这条录制吗？此操作不可恢复。', {
      kind: 'warning',
      title: '确认删除',
      cancelLabel: '取消',
      okLabel: '删除',
    });
    if (confirmed) {
      try {
        await invoke('delete_recording', { id: recordingId });
        setRecordings((prev) => prev.filter((r) => r.id !== recordingId));
        setToast({ type: 'success', message: '已删除' });
      } catch (err) {
        console.error('Delete failed:', err);
        setToast({ type: 'error', message: '删除失败: ' + String(err) });
      }
    }
  };

  // ★ 批量删除：确认后一次性调用 delete_recordings（Rust 端逐条复用单删逻辑）
  const handleBatchDelete = async () => {
    const ids = [...selected];
    if (ids.length === 0) return;
    const confirmed = await confirm(`确定要删除选中的 ${ids.length} 条录制吗？此操作不可恢复。`, {
      kind: 'warning',
      title: '批量删除',
      cancelLabel: '取消',
      okLabel: `删除 ${ids.length} 条`,
    });
    if (!confirmed) return;
    try {
      const n = await invoke<number>('delete_recordings', { ids });
      setRecordings((prev) => prev.filter((r) => !ids.includes(r.id)));
      setSelected([]);
      setToast({ type: 'success', message: `已删除 ${n} 条录制` });
    } catch (err) {
      console.error('Batch delete failed:', err);
      setToast({ type: 'error', message: '批量删除失败: ' + String(err) });
    }
  };

  const toggleSelected = (id: string) => {
    setSelected((prev) => (prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]));
  };

  // ★ 退出批量模式：多选框整列消失 + 清空选中 + 清掉进度残留
  const exitBatchMode = () => {
    setBatchMode(false);
    setSelected([]);
    setBatchProgress(null);
  };

  // ★ 批量导出（第四十八轮）：选完格式/选项直接导出 ZIP 到「文档\录步导出」。
  //   后端为每个录制建一个文件夹，统一打包成 ZIP。
  const handleBatchExportStart = () => {
    if (selected.length === 0) return;
    setBatchExportOpen(true);
  };

  const handleBatchExportFormat = async (format: ExportFormat, options: ExportOptions) => {
    setBatchExportOpen(false);
    const ids = [...selected];
    if (ids.length === 0) return;

    let actualFormat: string = format;
    if (actualFormat === "images") {
      actualFormat = options.imageMode === "long" ? "long" : "images";
    } else if (actualFormat === "word") {
      actualFormat = "docx";
    }

    setBatchBusy(true);
    setBatchProgress({ done: 0, total: ids.length });
    try {
      const result = await invoke<string>("batch_export_recordings", {
        ids,
        format: actualFormat,
        options,
      });
      setBatchBusy(false);
      setBatchProgress(null);
      setSelected([]);
      setToast({ type: "success", message: `已批量导出：${result}` });
    } catch (err) {
      console.error("Batch export failed:", err);
      setBatchBusy(false);
      setBatchProgress(null);
      setToast({ type: "error", message: "批量导出失败: " + String(err) });
    }
  };

  const handleRename = async () => {
    if (!renamingId || !renameDraft.trim()) {
      setRenamingId(null);
      return;
    }
    try {
      await invoke("update_recording_title", { id: renamingId, title: renameDraft.trim() });
      setRecordings((prev) =>
        prev.map((r) =>
          r.id === renamingId ? { ...r, title: renameDraft.trim() } : r
        )
      );
    } catch (err) {
      const msg = String(err);
      console.error("Rename failed:", msg);
      setToast({ type: "error", message: msg });
      // 保持重命名态，不退出，让用户修改后重试
      setTimeout(() => renameInputRef.current?.focus(), 0);
      return;
    }
    setRenamingId(null);
    setOpenMenuId(null);
  };

  const handleAppNameSave = async () => {
    if (!editingAppId) return;
    const value = appNameDraft.trim();
    try {
      await invoke("update_recording_app_name", { id: editingAppId, appName: value });
      setRecordings((prev) =>
        prev.map((r) =>
          r.id === editingAppId ? { ...r, app_name: value } : r
        )
      );
    } catch (err) {
      console.error("Update app name failed:", err);
    }
    setEditingAppId(null);
  };

  const filtered = useMemo(() => {
    if (!search.trim()) return recordings;
    const q = search.toLowerCase();
    return recordings.filter(
      (r) =>
        r.title.toLowerCase().includes(q) ||
        r.app_name.toLowerCase().includes(q) ||
        (r.source_domain || "").toLowerCase().includes(q)
    );
  }, [recordings, search]);

  // ★ 只在「值真正变化」时回第 1 页；mount 时（从教程返回重新挂载）不能重置，
  //   否则 sessionStorage 恢复的页码会被这两处 effect 立刻打回第 1 页
  const prevSearchRef = useRef(search);
  const prevUserRef = useRef(siteUser?.id);
  useEffect(() => {
    if (prevSearchRef.current !== search) {
      prevSearchRef.current = search;
      setPage(1);
    }
  }, [search]);

  // 换账号 → 重置页码，避免 A 用户的"第 2 页"被 B 用户看到
  useEffect(() => {
    const uid = siteUser?.id;
    if (prevUserRef.current !== uid) {
      prevUserRef.current = uid;
      setPage(1);
    }
  }, [siteUser?.id]);

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);

  // ★ 全选/反选当前页（跨页选择保留；放在 paged 定义之后避免 TDZ）
  const pagedIds = paged.map((r) => r.id);
  const allPageSelected = pagedIds.length > 0 && pagedIds.every((id) => selected.includes(id));
  // ★ 本页「部分选中」⇒ 表头要显示 indeterminate（原生 checkbox 的这个属性只能手动设，
  //   否则勾了 5/6 时表头看起来和「一个都没选」完全一样）
  const somePageSelected = pagedIds.some((id) => selected.includes(id));
  useEffect(() => {
    if (headCheckRef.current) {
      headCheckRef.current.indeterminate = somePageSelected && !allPageSelected;
    }
  }, [somePageSelected, allPageSelected, batchMode]);
  const toggleSelectPage = () => {
    setSelected((prev) =>
      allPageSelected
        ? prev.filter((id) => !pagedIds.includes(id))
        : [...new Set([...prev, ...pagedIds])]
    );
  };

  // 删除录制 / 搜索后总页数可能少于当前页：自动回弹到合法页码。
  // ★ 必须等列表加载完成（listLoaded）再回弹：刚从教程返回时 recordings 还是空数组，
  //   totalPages=1，若此时回弹会把 sessionStorage 恢复的页码清掉
  useEffect(() => {
    if (listLoaded && page > totalPages) {
      setPage(totalPages);
    }
  }, [listLoaded, page, totalPages]);

  const recStatus = (r: Recording) => {
    const parts: string[] = [];
    if (r.scan_redacted) parts.push("扫");
    if (r.text_redacted) parts.push("文");
    if (r.ai_generated) parts.push("ai");
    return parts.length ? parts.join("/") : "未生成";
  };

  return (
    <div className="page">
      {/* Header */}
      <div className="page-header">
        <h1>我的录制</h1>
        <div className="dashboard-search">
          <Search size={16} className="search-box-icon" />
          <input
            type="text"
            placeholder="搜索录制..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="search-box-input"
          />
        </div>
      </div>

      {/* Action card */}
      <div className="dashboard-action-card">
        <p className="dashboard-action-text">
          {recording
            ? "录制进行中，录步会自动捕获你在任何应用中的每一步操作。"
            : "点击「开始录制」后在任何应用中进行操作，录步会自动捕获每一步并生成操作指南。"}
        </p>
        {recording ? (
          <div className="dashboard-rec-controls">
            {isPaused ? (
              <button
                className="btn dashboard-action-btn rec-btn rec-resume"
                onClick={() => onResumeRecording?.()}
                data-tip={"恢复录制\n继续截图、记录你的操作"}
              >
                <Play size={14} fill="currentColor" />
                恢复
              </button>
            ) : (
              <button
                className="btn dashboard-action-btn rec-btn rec-pause"
                onClick={() => onPauseRecording?.()}
                data-tip={"暂停录制\n暂停期间不再截图，语音也一并停止"}
              >
                <Pause size={14} />
                暂停
              </button>
            )}
            <button
              className="btn dashboard-action-btn rec-btn rec-stop"
              onClick={() => onStopRecording?.()}
              data-tip={"停止录制并保存\n停止后会自动合并重复点击/连续输入，并把语音转成文字补进步骤"}
            >
              <Square size={13} fill="currentColor" />
              停止
            </button>
          </div>
        ) : (
          <button
            className="btn btn-primary dashboard-action-btn"
            onClick={() => onStartRecording()}
            data-tip={"开始录制\n之后在任何软件里正常操作即可，录步会自动截图并记录每一步；若已开「语音」，会一并开麦"}
          >
            <Circle size={14} fill="currentColor" />
            开始录制
          </button>
        )}
      </div>

      {/* Table section */}
      <div className="dashboard-table-section">
        <div className="dashboard-table-bar">
          <h3>录制列表</h3>
          <span className="dashboard-table-count">
            共 {filtered.length} 条记录
          </span>
          {/* ★ 批量操作栏：点「批量」进入后才出现。平时多选框整列不渲染，列表保持干净。 */}
          {batchMode ? (
            <div className="dash-batchbar">
              {batchProgress ? (
                <span className="dash-batchbar-count">
                  正在导出 {batchProgress.done}/{batchProgress.total}…
                </span>
              ) : (
                <span className="dash-batchbar-count">
                  {selected.length > 0 ? `已选 ${selected.length} 条` : "勾选要处理的录制"}
                </span>
              )}
              <button
                className="dash-batchbar-btn"
                disabled={selected.length === 0 || batchBusy}
                onClick={() => void handleBatchExportStart()}
                data-tip={"批量导出\n选中的录制各导出一个文件夹，最后打包成 ZIP"}
              >
                <Download size={13} />
                批量导出
              </button>
              <button
                className="dash-batchbar-btn dash-batchbar-del"
                disabled={selected.length === 0 || batchBusy}
                onClick={() => void handleBatchDelete()}
                data-tip={"删除选中的所有录制\n连同截图一起删除，无法恢复"}
              >
                <Trash2 size={13} />
                批量删除
              </button>
              <button
                className="dash-batchbar-btn"
                disabled={batchBusy}
                onClick={exitBatchMode}
                data-tip="退出批量，多选框收起"
              >
                <X size={13} />
                取消
              </button>
            </div>
          ) : (
            <button
              className="dash-batchbar-btn"
              onClick={() => setBatchMode(true)}
              data-tip={"批量处理\n进入后可以勾选多条录制，批量导出或批量删除"}
            >
              <ListChecks size={13} />
              批量
            </button>
          )}
        </div>

        {paged.length === 0 ? (
          <div className="empty-state">
            <div className="empty-state-icon">
              <Play size={28} />
            </div>
            <h3>还没有录制任何操作流程</h3>
            <p>点击下方按钮开始你的第一次录制，录步会自动捕获每一步并生成指南。</p>
            {recording ? (
              <div className="dashboard-rec-controls dashboard-rec-controls--center">
                {isPaused ? (
                  <button
                    className="btn rec-btn rec-resume"
                    onClick={() => onResumeRecording?.()}
                    data-tip={"恢复录制\n继续截图、记录你的操作"}
                  >
                    <Play size={14} fill="currentColor" />
                    恢复
                  </button>
                ) : (
                  <button
                    className="btn rec-btn rec-pause"
                    onClick={() => onPauseRecording?.()}
                    data-tip={"暂停录制\n暂停期间不再截图，语音也一并停止"}
                  >
                    <Pause size={14} />
                    暂停
                  </button>
                )}
                <button
                  className="btn rec-btn rec-stop"
                  onClick={() => onStopRecording?.()}
                  data-tip={"停止录制并保存\n停止后会自动合并重复点击/连续输入，并把语音转成文字补进步骤"}
                >
                  <Square size={13} fill="currentColor" />
                  停止
                </button>
              </div>
            ) : (
              <button
                className="empty-state-btn"
                onClick={() => onStartRecording()}
                data-tip={"开始录制\n之后在任何软件里正常操作即可，录步会自动截图并记录每一步；若已开「语音」，会一并开麦"}
              >
                <Circle size={8} fill="currentColor" />
                开始录制
              </button>
            )}
          </div>
        ) : (
          <>
            <div className="table-container">
              <table className="table">
                <thead>
                  <tr>
                    {batchMode && (
                      <th style={{ width: 36 }}>
                        <input
                          type="checkbox"
                          className="dash-check"
                          ref={headCheckRef}
                          checked={allPageSelected}
                          onChange={toggleSelectPage}
                          data-tip="全选/取消本页"
                        />
                      </th>
                    )}
                    <th>标题</th>
                    <th style={{ width: 120 }}>应用</th>
                    <th style={{ width: 100 }}>状态</th>
                    <th style={{ width: 140 }}>时间</th>
                    <th style={{ width: 60 }}></th>
                  </tr>
                </thead>
                <tbody>
                  {paged.map((r) => (
                    <tr
                      key={r.id}
                      /* 批量模式下行点击 = 切换勾选；平时 = 打开编辑器（选中列不存在时保持旧行为） */
                      onClick={() => (batchMode ? toggleSelected(r.id) : onEdit(r.id))}
                      style={{ cursor: "pointer" }}
                      className={selected.includes(r.id) ? "dash-row-selected" : ""}
                    >
                      {batchMode && (
                        <td onClick={(e) => e.stopPropagation()}>
                          <input
                            type="checkbox"
                            className="dash-check"
                            checked={selected.includes(r.id)}
                            onChange={() => toggleSelected(r.id)}
                          />
                        </td>
                      )}
                      <td onClick={(e) => renamingId === r.id && e.stopPropagation()}>
                        {renamingId === r.id ? (
                          <input
                            ref={renameInputRef}
                            className="dashboard-rename-input"
                            value={renameDraft}
                            onChange={(e) => setRenameDraft(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") handleRename();
                              if (e.key === "Escape") setRenamingId(null);
                            }}
                            onBlur={handleRename}
                          />
                        ) : (
                          <span className="dashboard-title-link">
                            {r.title || "未命名录制"}
                          </span>
                        )}
                      </td>
                      <td onClick={(e) => editingAppId === r.id && e.stopPropagation()}>
                        {editingAppId === r.id ? (
                          <input
                            ref={appNameInputRef}
                            className="dashboard-appname-input"
                            value={appNameDraft}
                            onChange={(e) => setAppNameDraft(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === "Enter") handleAppNameSave();
                              if (e.key === "Escape") setEditingAppId(null);
                            }}
                            onBlur={handleAppNameSave}
                          />
                        ) : (
                          <span
                            className="dashboard-app-name"
                            onClick={(e) => {
                              e.stopPropagation();
                              setEditingAppId(r.id);
                              setAppNameDraft(r.app_name || "");
                            }}
                            data-tip="点击修改识别到的应用名"
                          >
                            {r.app_name || "—"}
                          </span>
                        )}
                        {r.source_domain && (
                          <div className="dashboard-source" data-tip="来源网站（步骤页面域名）">
                            来自 {r.source_domain}
                          </div>
                        )}
                      </td>
                      <td>
                        <span
                          className={`status-badge ${
                            r.ai_generated || r.text_redacted || r.scan_redacted
                              ? "status-done"
                              : "status-pending"
                          }`}
                        >
                          <span className="status-dot" />
                          {recStatus(r)}
                        </span>
                      </td>
                      <td>
                        <span className="dashboard-time">
                          {formatTime(r.created_at)}
                        </span>
                      </td>
                      <td>
                        <button
                          className="btn-icon"
                          data-tip="更多操作：重命名、导出、删除"
                          onClick={(e) => {
                            e.stopPropagation();
                            if (openMenuId === r.id) {
                              setOpenMenuId(null);
                            } else {
                              const rect = e.currentTarget.getBoundingClientRect();
                              menuAnchorRef.current = rect;
                              setMenuPlaced(false);
                              // 占位值，真实 left/top 由下面的 useLayoutEffect 按实测尺寸覆盖（绘制前完成）
                              setMenuPos({ top: rect.bottom + 4, left: Math.max(8, rect.right - 96) });
                              setOpenMenuId(r.id);
                            }
                          }}
                        >
                          <MoreVertical size={16} />
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {/* Pagination */}
            {totalPages > 1 && (
              <div className="pagination">
                <button
                  className="pagination-btn"
                  disabled={page <= 1}
                  onClick={() => setPage((p) => Math.max(1, p - 1))}
                  data-tip="上一页"
                >
                  上一页
                </button>
                {Array.from({ length: totalPages }, (_, i) => i + 1).map(
                  (p) => (
                    <button
                      key={p}
                      className={`pagination-num ${
                        p === page ? "active" : ""
                      }`}
                      onClick={() => setPage(p)}
                      data-tip={`第 ${p} 页`}
                    >
                      {p}
                    </button>
                  )
                )}
                <button
                  className="pagination-btn"
                  disabled={page >= totalPages}
                  onClick={() =>
                    setPage((p) => Math.min(totalPages, p + 1))
                  }
                  data-tip="下一页"
                >
                  下一页
                </button>
              </div>
            )}
          </>
        )}
      </div>

      {/* Dropdown portal — rendered outside table to avoid overflow clipping */}
      {openMenuId && (
        <div
          className="dropdown-menu dropdown-fixed"
          ref={menuRef}
          style={{ top: menuPos.top, left: menuPos.left, visibility: menuPlaced ? "visible" : "hidden" }}
        >
          <button
            className="dropdown-item"
            onClick={() => handleDelete(openMenuId)}
            data-tip="删除这条录制（连同截图，无法恢复）"
          >
            <Trash2 size={14} />
            删除
          </button>
          <button
            className="dropdown-item"
            onClick={() => {
              const rec = recordings.find(r => r.id === openMenuId);
              setRenamingId(openMenuId);
              setRenameDraft(rec?.title || "");
              setOpenMenuId(null);
            }}
            data-tip="给这条录制改个名字"
          >
            <Pencil size={14} />
            重命名
          </button>
          <div className="dropdown-divider" />
          <button
            className="dropdown-item"
            onClick={() => handleExport(openMenuId)}
            data-tip="导出为 PDF / Word / PPT / HTML / 视频 / 图片等"
          >
            <Download size={14} />
            导出
          </button>
        </div>
      )}

      {/* 批量导出：选格式/选项后直接导出 ZIP 到固定导出目录 */}
      {batchExportOpen && (
        <ExportDialog
          isOpen={true}
          onClose={() => setBatchExportOpen(false)}
          onSelect={(format, options) => void handleBatchExportFormat(format, options)}
        />
      )}

      {/* 单条导出：选格式/选项后直接导出到固定导出目录 */}
      {exportDialog && (
        <ExportDialog
          isOpen={true}
          onClose={() => setExportDialog(null)}
          onSelect={(format, options) => void handleSingleFormatSelect(format, options)}
          steps={exportSteps ?? undefined}
        />
      )}

      {/* Toast notification */}
      {toast && createPortal(
        <div
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            zIndex: 9999,
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
          {toast.type === "success"
            ? <CheckCircle2 size={18} color="#ff6b35" />
            : <XCircle size={18} color="#dc2626" />
          }
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
    </div>
  );
}
