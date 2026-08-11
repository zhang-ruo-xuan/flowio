import { useEffect, useState, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { Circle, Search, MoreVertical, Play, Pencil, FileText, Code, FileType, CheckCircle2, XCircle, Trash2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import FileSaveDialog from "../components/FileSaveDialog";

interface Recording {
  id: string;
  title: string;
  app_name: string;
  status: string;
  total_steps: number;
  created_at: string;
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
  locationKey?: string;
}

const PAGE_SIZE = 10;

export default function Dashboard({ onEdit, onStartRecording, locationKey }: DashboardProps) {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(1);
  const [openMenuId, setOpenMenuId] = useState<string | null>(null);
  const [menuPos, setMenuPos] = useState({ top: 0, left: 0 });
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [toast, setToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const [exportDialog, setExportDialog] = useState<{ id: string; format: string } | null>(null);
  const [editingAppId, setEditingAppId] = useState<string | null>(null);
  const [appNameDraft, setAppNameDraft] = useState("");
  const appNameInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const renameInputRef = useRef<HTMLInputElement>(null);

  // Auto-dismiss toast after 4s
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 2000);
    return () => clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    invoke<Recording[]>("list_recordings")
      .then(setRecordings)
      .catch((err) => console.error("Failed to list recordings:", err));
  }, [locationKey]);

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
      console.error("Rename failed:", err);
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

  useEffect(() => {
    setPage(1);
  }, [search]);

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);

  const statusLabel = (s: string) => (s ? "已生成" : "未生成");

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
          点击「开始录制」后在任何应用中进行操作，录步会自动捕获每一步并生成操作指南。
        </p>
        <button
          className="btn btn-primary dashboard-action-btn"
          onClick={onStartRecording}
        >
          <Circle size={14} fill="currentColor" />
          开始录制
        </button>
      </div>

      {/* Table section */}
      <div className="dashboard-table-section">
        <div className="dashboard-table-bar">
          <h3>录制列表</h3>
          <span className="dashboard-table-count">
            共 {filtered.length} 条记录
          </span>
        </div>

        {paged.length === 0 ? (
          <div className="empty-state">
            <div className="empty-state-icon">
              <Play size={28} />
            </div>
            <h3>还没有录制任何操作流程</h3>
            <p>点击下方按钮开始你的第一次录制，录步会自动捕获每一步并生成指南。</p>
            <button
              className="empty-state-btn"
              onClick={onStartRecording}
            >
              <Circle size={8} fill="currentColor" />
              开始录制
            </button>
          </div>
        ) : (
          <>
            <div className="table-container">
              <table className="table">
                <thead>
                  <tr>
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
                    >
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
                            title="点击编辑应用名"
                          >
                            {r.app_name || "—"}
                          </span>
                        )}
                      </td>
                      <td>
                        <span
                          className={`status-badge ${
                            r.status ? "status-done" : "status-pending"
                          }`}
                        >
                          <span className="status-dot" />
                          {statusLabel(r.status)}
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
                          onClick={(e) => {
                            e.stopPropagation();
                            if (openMenuId === r.id) {
                              setOpenMenuId(null);
                            } else {
                              const rect = e.currentTarget.getBoundingClientRect();
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
          style={{ top: menuPos.top, left: menuPos.left }}
        >
          <button
            className="dropdown-item"
            onClick={() => handleDelete(openMenuId)}
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
          >
            <Pencil size={14} />
            重命名
          </button>
          <div className="dropdown-divider" />
          <button
            className="dropdown-item"
            onClick={() => handleExport(openMenuId, "pdf")}
          >
            <FileText size={14} />
            导出 PDF
          </button>
          <button
            className="dropdown-item"
            onClick={() => handleExport(openMenuId, "html")}
          >
            <Code size={14} />
            导出 HTML
          </button>
          <button
            className="dropdown-item"
            onClick={() => handleExport(openMenuId, "docx")}
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
          {toast.type === "success"
            ? <CheckCircle2 size={18} color="#16a34a" />
            : <XCircle size={18} color="#dc2626" />
          }
          <span>{toast.message}</span>
        </div>,
        document.body
      )}
    </div>
  );
}
