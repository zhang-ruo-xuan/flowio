import { useState, useEffect, useCallback, useRef } from "react";
import { createPortal } from "react-dom";
import {
  Folder,
  ChevronRight,
  HardDrive,
  X,
  Search,
  File,
  CornerDownRight,
  Monitor,
  FolderOpen,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

/* ================================================================
   Type definitions
   ================================================================ */

interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  modified: number;
}

interface DriveInfo {
  name: string;
  path: string;
}

interface KnownFolder {
  name: string;
  path: string;
}

interface Filter {
  name: string;
  extensions: string[];
}

interface FileSaveDialogProps {
  isOpen: boolean;
  onClose: () => void;
  onSave: (filePath: string) => void;
  title: string;
  defaultName: string;
  filters: Filter[];
}

/* ================================================================
   Icons
   ================================================================ */

/* eslint-disable @typescript-eslint/no-explicit-any */
const iconMap: Record<string, any> = {
  desktop: Monitor,
  documents: File,
  downloads: FolderOpen,
  pictures: Folder,
  videos: Folder,
  music: Folder,
  user: Folder,
};

function pickIcon(name: string) {
  for (const [key, Icon] of Object.entries(iconMap)) {
    if (name.includes(key)) return Icon;
  }
  return Folder;
}

/* ================================================================
   Helpers
   ================================================================ */

function formatDateTime(ts: number): string {
  if (ts === 0) return "";
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function parentPath(p: string): string {
  const s = p.replace(/[\\/]+$/, "");
  const i = s.lastIndexOf("\\");
  return i > 0 ? s.slice(0, i + 1) : s.slice(0, 1) + ":\\";
}

/* ================================================================
   Component
   ================================================================ */

export default function FileSaveDialog({
  isOpen,
  onClose,
  onSave,
  title,
  defaultName,
  filters,
}: FileSaveDialogProps) {
  const [knownFolders, setKnownFolders] = useState<KnownFolder[]>([]);
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [currentDir, setCurrentDir] = useState("");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [selectedFile, setSelectedFile] = useState<FileEntry | null>(null);
  const [fileName, setFileName] = useState(defaultName);
  const [formatIdx, setFormatIdx] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [activeSidebar, setActiveSidebar] = useState("");
  const [overwriteTarget, setOverwriteTarget] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  /* ── Init ─────────────────────────────────────────────── */

  useEffect(() => {
    if (!isOpen) return;
    (async () => {
      try {
        const [folders, d] = await Promise.all([
          invoke<KnownFolder[]>("get_known_folders"),
          invoke<DriveInfo[]>("get_drives"),
        ]);
        setKnownFolders(folders);
        setDrives(d);

        // Default to the first known folder (Desktop) or C:\
        const initial = folders.length > 0 ? folders[0].path : d.length > 0 ? d[0].path : "C:\\";
        setCurrentDir(initial);
        setActiveSidebar(initial);
        setSelectedFile(null);
        setFileName(defaultName);
        setFormatIdx(0);
        setError("");
      } catch (e) {
        setError("Failed to initialize: " + String(e));
      }
    })();
  }, [isOpen]);

  /* ── Load directory entries ───────────────────────────── */

  useEffect(() => {
    if (!currentDir) return;
    (async () => {
      setLoading(true);
      setError("");
      try {
        const list = await invoke<FileEntry[]>("list_directory", { path: currentDir });
        setEntries(list);
      } catch (e) {
        setError("Failed to load directory: " + String(e));
        setEntries([]);
      } finally {
        setLoading(false);
      }
    })();
  }, [currentDir]);

  /* ── Focus input on open ──────────────────────────────── */

  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isOpen]);

  /* ── Handlers ─────────────────────────────────────────── */

  const navigateTo = useCallback((path: string) => {
    setCurrentDir(path);
    setActiveSidebar(path);
    setSelectedFile(null);
  }, []);

  const handleSidebarClick = useCallback((path: string) => {
    navigateTo(path);
  }, [navigateTo]);

  const handleBreadcrumbClick = useCallback((path: string) => {
    navigateTo(path);
  }, [navigateTo]);

  const handleEntryDoubleClick = useCallback(
    (entry: FileEntry) => {
      if (entry.is_dir) {
        navigateTo(entry.path + "\\");
      }
    },
    [navigateTo]
  );

  const handleEntryClick = useCallback((entry: FileEntry) => {
    if (!entry.is_dir) {
      setSelectedFile(entry);
      setFileName(entry.name);
    }
  }, []);

  const handleSave = useCallback(() => {
    const ext = filters[formatIdx]?.extensions?.[0] || "";
    let finalName = fileName.trim();
    if (!finalName) return;
    // Auto-append extension if missing
    if (ext && !finalName.toLowerCase().endsWith("." + ext)) {
      finalName += "." + ext;
    }
    const dir = currentDir.endsWith("\\") ? currentDir : currentDir + "\\";
    const fullPath = dir + finalName;

    // Check if file already exists
    const exists = entries.some(
      (e) => !e.is_dir && e.name.toLowerCase() === finalName.toLowerCase()
    );
    if (exists) {
      setOverwriteTarget(fullPath);
      return;
    }

    onSave(fullPath);
  }, [fileName, currentDir, formatIdx, filters, entries, onSave]);

  const confirmOverwrite = useCallback(() => {
    if (overwriteTarget) {
      onSave(overwriteTarget);
      setOverwriteTarget(null);
    }
  }, [overwriteTarget, onSave]);

  const cancelOverwrite = useCallback(() => {
    setOverwriteTarget(null);
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter") {
        handleSave();
      } else if (e.key === "Escape") {
        onClose();
      }
    },
    [handleSave, onClose]
  );

  /* ── Breadcrumb parts ─────────────────────────────────── */

  const breadcrumbs: { label: string; path: string }[] = [];
  {
    const parts = currentDir.replace(/[\\/]+$/, "").split("\\");
    let acc = "";
    for (let i = 0; i < parts.length; i++) {
      acc += parts[i] + "\\";
      breadcrumbs.push({ label: parts[i] || "此电脑", path: acc });
    }
  }

  /* ── Don't render if not open ─────────────────────────── */

  if (!isOpen) return null;

  /* ============================================================
     Styles (inline to keep component self-contained)
     ============================================================ */

  const s: Record<string, React.CSSProperties> = {
    overlay: {
      position: "fixed",
      inset: 0,
      zIndex: 10000,
      background: "rgba(0, 0, 0, 0.35)",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
    },
    dialog: {
      width: 800,
      height: 550,
      background: "#fff",
      borderRadius: 8,
      display: "flex",
      flexDirection: "column",
      boxShadow: "0 12px 40px rgba(0,0,0,0.18)",
      border: "1px solid #d0d7de",
      overflow: "hidden",
    },
    header: {
      display: "flex",
      alignItems: "center",
      justifyContent: "space-between",
      padding: "10px 16px",
      borderBottom: "1px solid #d8dee4",
      background: "#f6f8fa",
      fontSize: 13,
      fontWeight: 600,
      color: "#1f2328",
      flexShrink: 0,
    },
    body: {
      display: "flex",
      flex: 1,
      overflow: "hidden",
    },
    sidebar: {
      width: 200,
      borderRight: "1px solid #d8dee4",
      background: "#f6f8fa",
      overflowY: "auto",
      flexShrink: 0,
      padding: "6px 0",
    },
    sidebarItem: {
      display: "flex",
      alignItems: "center",
      gap: 8,
      padding: "6px 14px",
      fontSize: 12,
      cursor: "pointer",
      whiteSpace: "nowrap",
      overflow: "hidden",
      textOverflow: "ellipsis",
      color: "#1f2328",
      transition: "background 0.1s",
    },
    sidebarItemActive: {
      background: "#ddf4ff",
      color: "#0969da",
      fontWeight: 600,
    },
    sidebarSection: {
      fontSize: 11,
      fontWeight: 700,
      color: "#656d76",
      padding: "10px 14px 4px",
      textTransform: "uppercase" as const,
      letterSpacing: "0.5px",
    },
    content: {
      flex: 1,
      display: "flex",
      flexDirection: "column",
      overflow: "hidden",
    },
    breadcrumb: {
      display: "flex",
      alignItems: "center",
      gap: 2,
      padding: "6px 12px",
      borderBottom: "1px solid #d8dee4",
      background: "#fff",
      fontSize: 12,
      flexShrink: 0,
      overflowX: "auto",
      whiteSpace: "nowrap" as const,
    },
    crumb: {
      cursor: "pointer",
      color: "#0969da",
      padding: "2px 4px",
      borderRadius: 4,
      whiteSpace: "nowrap" as const,
    },
    crumbLast: {
      color: "#1f2328",
      fontWeight: 600,
      cursor: "default",
      padding: "2px 4px",
      whiteSpace: "nowrap" as const,
    },
    fileList: {
      flex: 1,
      overflowY: "auto",
      fontSize: 12,
    },
    fileRow: {
      display: "flex",
      alignItems: "center",
      gap: 8,
      padding: "4px 12px",
      cursor: "pointer",
      borderBottom: "1px solid #f0f2f5",
    },
    fileRowHover: {},
    fileName: {
      flex: 1,
      overflow: "hidden",
      textOverflow: "ellipsis",
      whiteSpace: "nowrap" as const,
    },
    fileDate: {
      color: "#656d76",
      fontSize: 11,
      flexShrink: 0,
    },
    footer: {
      borderTop: "1px solid #d8dee4",
      padding: "12px 16px",
      background: "#f6f8fa",
      display: "flex",
      gap: 12,
      flexShrink: 0,
    },
    footerLeft: {
      flex: 1,
      display: "flex",
      flexDirection: "column" as const,
      gap: 8,
    },
    footerRow: {
      display: "flex",
      alignItems: "center",
      gap: 8,
    },
    footerRight: {
      display: "flex",
      flexDirection: "column" as const,
      gap: 6,
      justifyContent: "flex-end" as const,
    },
    label: {
      fontSize: 12,
      color: "#1f2328",
      flexShrink: 0,
      minWidth: 70,
      textAlign: "right" as const,
    },
    input: {
      flex: 1,
      height: 30,
      padding: "0 10px",
      border: "1px solid #d0d7de",
      borderRadius: 6,
      fontSize: 13,
      outline: "none",
    },
    select: {
      height: 30,
      padding: "0 8px",
      border: "1px solid #d0d7de",
      borderRadius: 6,
      fontSize: 12,
      background: "#fff",
      cursor: "pointer",
    },
    btn: {
      height: 30,
      padding: "0 16px",
      border: "1px solid #d0d7de",
      borderRadius: 6,
      fontSize: 12,
      fontWeight: 600,
      cursor: "pointer",
      background: "#f6f8fa",
      color: "#24292f",
    },
    btnPrimary: {
      background: "#1f883d",
      color: "#fff",
      border: "1px solid #1a7f37",
    },
    empty: {
      padding: 40,
      textAlign: "center",
      color: "#656d76",
      fontSize: 13,
    },
  };

  /* ============================================================
     Render
     ============================================================ */

  return createPortal(
    <div style={s.overlay} onKeyDown={handleKeyDown}>
      <div style={s.dialog} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div style={s.header}>
          <span>{title}</span>
          <X size={16} style={{ cursor: "pointer", color: "#656d76" }} onClick={onClose} />
        </div>

        {/* Overwrite confirmation overlay */}
        {overwriteTarget && (
          <div style={{
            position: "absolute", inset: 0, zIndex: 10,
            background: "rgba(255,255,255,0.92)",
            display: "flex", alignItems: "center", justifyContent: "center",
          }}>
            <div style={{
              background: "#fff", border: "1px solid #d0d7de", borderRadius: 8,
              padding: "20px 24px", boxShadow: "0 8px 24px rgba(0,0,0,0.15)",
              maxWidth: 380, textAlign: "center",
            }}>
              <p style={{ fontSize: 13, color: "#1f2328", marginBottom: 4, fontWeight: 600 }}>
                确认另存为
              </p>
              <p style={{ fontSize: 12, color: "#656d76", marginBottom: 16, wordBreak: "break-all" }}>
                {overwriteTarget.split("\\").pop()} 已存在。<br />是否覆盖？
              </p>
              <div style={{ display: "flex", gap: 8, justifyContent: "center" }}>
                <button style={{ ...s.btn, ...s.btnPrimary }} onClick={confirmOverwrite}>是(Y)</button>
                <button style={s.btn} onClick={cancelOverwrite}>否(N)</button>
              </div>
            </div>
          </div>
        )}

        {/* Body */}
        <div style={s.body}>
          {/* Sidebar */}
          <div style={s.sidebar}>
            <div style={s.sidebarSection}>常用文件夹</div>
            {knownFolders.map((f) => {
              const Icon = pickIcon(f.name);
              const active = activeSidebar === f.path;
              return (
                <div
                  key={f.path}
                  style={{
                    ...s.sidebarItem,
                    ...(active ? s.sidebarItemActive : {}),
                  }}
                  onClick={() => handleSidebarClick(f.path)}
                >
                  <Icon className="w-4 h-4" size={14} />
                  <span>{f.name}</span>
                </div>
              );
            })}
            <div style={s.sidebarSection}>磁盘驱动器</div>
            {drives.map((d) => {
              const active = activeSidebar === d.path;
              return (
                <div
                  key={d.path}
                  style={{
                    ...s.sidebarItem,
                    ...(active ? s.sidebarItemActive : {}),
                  }}
                  onClick={() => handleSidebarClick(d.path)}
                >
                  <HardDrive size={14} />
                  <span>{d.name}</span>
                </div>
              );
            })}
          </div>

          {/* Content */}
          <div style={s.content}>
            {/* Breadcrumb */}
            <div style={s.breadcrumb}>
              {breadcrumbs.map((b, idx) =>
                idx === breadcrumbs.length - 1 ? (
                  <span key={idx} style={s.crumbLast}>
                    {b.label}
                  </span>
                ) : (
                  <span key={idx} style={{ display: "flex", alignItems: "center", gap: 0 }}>
                    <span style={s.crumb} onClick={() => handleBreadcrumbClick(b.path)}>
                      {b.label}
                    </span>
                    <ChevronRight size={12} style={{ color: "#8b949e", margin: "0 2px", flexShrink: 0 }} />
                  </span>
                )
              )}
            </div>

            {/* File list */}
            <div style={s.fileList}>
              {loading && <div style={s.empty}>加载中...</div>}
              {error && <div style={{ ...s.empty, color: "#cf222e" }}>{error}</div>}
              {!loading && !error && entries.length === 0 && (
                <div style={s.empty}>此文件夹为空</div>
              )}
              {!loading &&
                entries.map((entry) => (
                  <div
                    key={entry.path}
                    style={{
                      ...s.fileRow,
                      ...(selectedFile?.path === entry.path
                        ? { background: "#ddf4ff" }
                        : {}),
                    }}
                    onClick={() => handleEntryClick(entry)}
                    onDoubleClick={() => handleEntryDoubleClick(entry)}
                  >
                    {entry.is_dir ? (
                      <Folder size={16} style={{ color: "#54aeff", flexShrink: 0 }} />
                    ) : (
                      <File size={16} style={{ color: "#656d76", flexShrink: 0 }} />
                    )}
                    <span style={s.fileName}>{entry.name}</span>
                    <span style={s.fileDate}>{formatDateTime(entry.modified)}</span>
                  </div>
                ))}
            </div>
          </div>
        </div>

        {/* Footer */}
        <div style={s.footer}>
          <div style={s.footerLeft}>
            <div style={s.footerRow}>
              <span style={s.label}>文件名(N):</span>
              <input
                ref={inputRef}
                style={s.input}
                value={fileName}
                onChange={(e) => setFileName(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder="输入文件名"
              />
            </div>
            <div style={s.footerRow}>
              <span style={s.label}>保存类型(T):</span>
              <select
                style={s.select}
                value={formatIdx}
                onChange={(e) => setFormatIdx(Number(e.target.value))}
              >
                {filters.map((f, i) => (
                  <option key={i} value={i}>
                    {f.name} 文件 (*.{f.extensions[0]})
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div style={s.footerRight}>
            <button
              style={{ ...s.btn, ...s.btnPrimary, width: 80 }}
              onClick={handleSave}
              disabled={!fileName.trim()}
            >
              保存(S)
            </button>
            <button style={{ ...s.btn, width: 80 }} onClick={onClose}>
              取消
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body
  );
}
