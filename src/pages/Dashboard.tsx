import { useEffect, useLayoutEffect, useState, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { Circle, Search, MoreVertical, Play, Pencil, FileText, Code, FileType, CheckCircle2, XCircle, Trash2, Pause, Square } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import FileSaveDialog from "../components/FileSaveDialog";
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
  const [menuPos, setMenuPos] = useState({ top: 0, left: 0 });
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [toast, setToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const [exportDialog, setExportDialog] = useState<{ id: string; format: string } | null>(null);
  const [editingAppId, setEditingAppId] = useState<string | null>(null);
  const [appNameDraft, setAppNameDraft] = useState("");
  const appNameInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  // ★ 行操作菜单：底部空间不足时向上弹出（记录触发按钮位置 + 定位完成前隐藏防闪烁）
  const menuAnchorRef = useRef<DOMRect | null>(null);
  const [menuPlaced, setMenuPlaced] = useState(false);
  const renameInputRef = useRef<HTMLInputElement>(null);

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

  // ★ 菜单渲染后测量实际高度：底部放不下则改为向上弹出（菜单底边贴按钮顶边）
  useLayoutEffect(() => {
    const el = menuRef.current;
    const anchor = menuAnchorRef.current;
    if (!openMenuId || !el || !anchor) return;
    const h = el.offsetHeight;
    // 仅在仍处于「默认向下」位置时判断翻转，避免 setState 回路
    if (menuPos.top === anchor.bottom + 4) {
      if (anchor.bottom + 4 + h > window.innerHeight - 8) {
        setMenuPos({ top: Math.max(8, anchor.top - h - 4), left: anchor.right - 160 });
        return;
      }
    }
    setMenuPlaced(true);
  }, [openMenuId, menuPos]);

  const handleExport = (id: string, format: string) => {
    setOpenMenuId(null);
    setExportDialog({ id, format });
  };

  const handleSaveConfirm = async (savedPath: string) => {
    const { id, format } = exportDialog!;
    setExportDialog(null);
    try {
      const path = await invoke<string>("export_recording", { id, format, outputPath: savedPath });
      setToast({ type: "success", message: `导出成功: ${path}` });
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
        r.app_name.toLowerCase().includes(q)
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
          {/* ★ 批量操作栏：有选中项时显示 */}
          {selected.length > 0 && (
            <div className="dash-batchbar">
              <span className="dash-batchbar-count">已选 {selected.length} 条</span>
              <button className="dash-batchbar-btn dash-batchbar-del" onClick={() => void handleBatchDelete()} data-tip={"删除选中的所有录制\n连同截图一起删除，无法恢复"}>
                <Trash2 size={13} />
                批量删除
              </button>
              <button className="dash-batchbar-btn" onClick={() => setSelected([])} data-tip="清空选中，不删除任何东西">
                取消选择
              </button>
            </div>
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
                    <th style={{ width: 36 }}>
                      <input
                        type="checkbox"
                        className="dash-check"
                        checked={allPageSelected}
                        onChange={toggleSelectPage}
                        data-tip="全选/取消本页"
                      />
                    </th>
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
                      onClick={() => onEdit(r.id)}
                      style={{ cursor: "pointer" }}
                      className={selected.includes(r.id) ? "dash-row-selected" : ""}
                    >
                      <td onClick={(e) => e.stopPropagation()}>
                        <input
                          type="checkbox"
                          className="dash-check"
                          checked={selected.includes(r.id)}
                          onChange={() => toggleSelected(r.id)}
                        />
                      </td>
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
                              setMenuPos({ top: rect.bottom + 4, left: rect.right - 160 });
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
            onClick={() => handleExport(openMenuId, "pdf")}
            data-tip="导出成 PDF，适合直接发给别人看或打印"
          >
            <FileText size={14} />
            导出 PDF
          </button>
          <button
            className="dropdown-item"
            onClick={() => handleExport(openMenuId, "html")}
            data-tip="导出成单个 HTML 文件，带目录，双击即可打开"
          >
            <Code size={14} />
            导出 HTML
          </button>
          <button
            className="dropdown-item"
            onClick={() => handleExport(openMenuId, "docx")}
            data-tip="导出成 Word 文档，方便交给同事继续编辑"
          >
            <FileType size={14} />
            导出 Word 文档
          </button>
        </div>
      )}

      {/* Custom Save Dialog */}
      {exportDialog && (() => {
        const formatLabels: Record<string, string> = { html: "HTML", markdown: "Markdown", pdf: "PDF" };
        const formatExt: Record<string, string> = { html: "html", markdown: "md", pdf: "pdf" };
        const fmt = exportDialog.format;
        const rec = recordings.find((r) => r.id === exportDialog.id);
        const defaultName = (rec?.title || "recording")
          .replace(/[\\/:*?"<>|]/g, "_")
          .substring(0, 120) + "." + (formatExt[fmt] || fmt);
        return (
          <FileSaveDialog
            isOpen={true}
            title={`导出为 ${formatLabels[fmt] || fmt}`}
            defaultName={defaultName}
            filters={[{ name: formatLabels[fmt] || fmt, extensions: [formatExt[fmt] || fmt] }]}
            onClose={() => setExportDialog(null)}
            onSave={handleSaveConfirm}
          />
        );
      })()}

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
