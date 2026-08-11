import { BrowserRouter, Routes, Route, useNavigate, useLocation } from "react-router-dom";
import { useState, useCallback, useEffect, useRef } from "react";
import { Settings, Minus, Square, Copy, X, Pause, Play } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Dashboard from "./pages/Dashboard";
import SettingsPage from "./pages/Settings";
import StepEditor from "./pages/StepEditor";

function App() {
  const [darkMode, setDarkMode] = useState(false);
  const [recording, setRecording] = useState(false);
  const [recordingId, setRecordingId] = useState<string | null>(null);
  const [isPaused, setIsPaused] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [stepCount, setStepCount] = useState(0);
  const navigate = useNavigate();
  const location = useLocation();
  const isEditor = location.pathname.startsWith("/editor/");
  const appWindow = getCurrentWindow();
  const [isMaximized, setIsMaximized] = useState(false);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const recordingIdRef = useRef<string | null>(null);
  const isPausedRef = useRef(false);

  useEffect(() => {
    const savedFontSize = localStorage.getItem("fontSize") || "14";
    document.body.style.zoom = `${parseInt(savedFontSize) / 14 * 100}%`;
  }, []);

  useEffect(() => {
    appWindow.isMaximized().then(setIsMaximized);
    let unlisten: (() => void) | undefined;
    appWindow.onResized(() => {
      appWindow.isMaximized().then(setIsMaximized);
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      unlisten = await appWindow.onFocusChanged(({ payload: focused }) => {
        if (focused) {
          invoke<{ id: string; is_paused: boolean } | null>("get_active_recording")
            .then((info) => {
              if (info) {
                recordingIdRef.current = info.id;
                setRecordingId(info.id);
                setRecording(true);
                isPausedRef.current = info.is_paused;
                setIsPaused(info.is_paused);
              }
            })
            .catch((e) => console.error("get_active_recording:", e));
        }
      });
    })();
    return () => { unlisten?.(); };
  }, []);

  useEffect(() => {
    if (recording) {
      let seconds = elapsed;
      timerRef.current = setInterval(() => {
        seconds++;
        setElapsed(seconds);
      }, 1000);
      return () => { if (timerRef.current) clearInterval(timerRef.current); };
    } else {
      if (timerRef.current) clearInterval(timerRef.current);
    }
  }, [recording]);

  const handleStartRecording = useCallback(async () => {
    try {
      const id = await invoke<string>("start_recording");
      console.log("Recording started:", id);
      recordingIdRef.current = id;
      setRecordingId(id);
      setRecording(true);
      setIsPaused(false);
      isPausedRef.current = false;
      setElapsed(0);
      setStepCount(0);
      appWindow.minimize();
    } catch (err) {
      console.error("Failed to start recording:", err);
    }
  }, []);

  const handlePauseRecording = useCallback(async () => {
    const id = recordingIdRef.current;
    if (!id) return;
    try {
      await invoke("pause_recording", { id });
      isPausedRef.current = true;
      setIsPaused(true);
      await appWindow.unminimize();
      await appWindow.show();
      await appWindow.setFocus();
    } catch (err) {
      console.error("Failed to pause recording:", err);
    }
  }, []);

  const handleResumeRecording = useCallback(async () => {
    const id = recordingIdRef.current;
    if (!id) return;
    try {
      await invoke("resume_recording", { id });
      console.log("[resume] invoke succeeded, id=" + id);
      isPausedRef.current = false;
      setIsPaused(false);
      // small delay to let resume_recording settle before minimizing
      await new Promise(r => setTimeout(r, 200));
      appWindow.minimize().catch(e => console.error("minimize on resume failed:", e));
    } catch (err: any) {
      alert("恢复录制失败: " + (err?.message || err));
      console.error("Failed to resume recording:", err);
    }
  }, []);

  const handleStopRecording = useCallback(async () => {
    let id: string | null = recordingIdRef.current;

    // If ref is null (possible after webview rebuild), try to recover from backend
    if (!id) {
      alert("录制状态异常，正在尝试恢复...");
      try {
        const result = await invoke<{ id: string; is_paused: boolean } | null>("get_active_recording");
        if (result) {
          id = result.id;
          recordingIdRef.current = result.id;
          setRecordingId(result.id);
          isPausedRef.current = result.is_paused;
          setIsPaused(result.is_paused);
          setRecording(true);
        }
      } catch (e) {
        console.error("Recovery attempt failed:", e);
      }
      if (!id) {
        alert("无法恢复录制状态，请重启应用后重试");
        return;
      }
    }

    try {
      await invoke("finish_recording", { id });
      recordingIdRef.current = null;
      setRecordingId(null);
      setRecording(false);
      setIsPaused(false);
      isPausedRef.current = false;
      // Window restore — best-effort, must not block navigation to editor
      try {
        await appWindow.unminimize();
        await appWindow.show();
        await appWindow.setFocus();
      } catch (e) {
        console.error("Window restore failed (non-fatal):", e);
      }
      try {
        navigate(`/editor/${id}`);
      } catch (navErr: any) {
        alert("跳转编辑页面失败: " + (navErr?.message || navErr));
        console.error("Navigate to editor failed:", navErr);
      }
    } catch (err: any) {
      alert("停止录制失败: " + (err?.message || err));
      console.error("Failed to finish recording:", err);
    }
  }, [navigate]);

  // ── Global shortcut event listeners ──
  useEffect(() => {
    const unlistens: (() => void)[] = [];
    (async () => {
      unlistens.push(
        await listen("shortcut-record", () => {
          if (recordingIdRef.current) {
            if (isPausedRef.current) {
              handleResumeRecording();
            } else {
              handleStopRecording();
            }
          } else {
            handleStartRecording();
          }
        })
      );
      unlistens.push(
        await listen("shortcut-pause", () => {
          if (!recordingIdRef.current) return;
          handlePauseRecording();
        })
      );
      unlistens.push(
        await listen("shortcut-stop", () => {
          if (recordingIdRef.current) handleStopRecording();
        })
      );
    })();
    return () => { unlistens.forEach((fn) => fn()); };
  }, []);

  const handleEdit = useCallback((id: string) => { navigate(`/editor/${id}`); }, [navigate]);

  return (
    <div className={`app-layout ${darkMode ? "dark" : ""}`}>
      <div className="titlebar" style={{ height: 40, background: "#24292e", display: "flex", alignItems: "center", justifyContent: "space-between", padding: "0 4px" }}>
        {isEditor ? (
          <div id="titlebar-editor-left" style={{ flex: 1, height: "100%", display: "flex", alignItems: "center", paddingLeft: 8 }} />
        ) : (
          <div data-tauri-drag-region style={{ flex: 1, height: "100%", display: "flex", alignItems: "center", paddingLeft: 8 }}>
            <span style={{ color: "#e6edf3", fontSize: 14, fontWeight: 600, userSelect: "none" }}>录步</span>
            {recording && (
              <>
                <span style={{ color: isPaused ? "#f0883e" : "#f85149", fontSize: 12, marginLeft: 10 }}>
                  {isPaused ? "PAUSED" : "REC"} {Math.floor(elapsed / 60)}:{String(elapsed % 60).padStart(2, "0")}
                </span>
              </>
            )}
          </div>
        )}
        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
          {isEditor ? (
            <div id="titlebar-editor-actions" style={{ display: "flex", alignItems: "center", gap: 4 }} />
          ) : recording ? (
            <>
              {!isPaused ? (
                <button onClick={handlePauseRecording} title="暂停录制 (Ctrl+Shift+P)" style={{ background: "#f0883e", border: "none", color: "#fff", cursor: "pointer", padding: "4px 10px", borderRadius: 4, fontSize: 12, fontWeight: 600 }}>
                  <Pause size={14} style={{ verticalAlign: "middle", marginRight: 2 }} /> 暂停
                </button>
              ) : (
                <button onClick={handleResumeRecording} title="恢复录制 (Ctrl+Shift+P)" style={{ background: "#2da44e", border: "none", color: "#fff", cursor: "pointer", padding: "4px 10px", borderRadius: 4, fontSize: 12, fontWeight: 600 }}>
                  <Play size={14} style={{ verticalAlign: "middle", marginRight: 2 }} /> 恢复
                </button>
              )}
              <button onClick={handleStopRecording} title="停止录制 (Ctrl+Shift+S)" style={{ background: "#cf222e", border: "none", color: "#fff", cursor: "pointer", padding: "4px 10px", borderRadius: 4, fontSize: 12, fontWeight: 600 }}>
                STOP
              </button>
            </>
          ) : null}
          <button onClick={() => navigate("/settings")} title="设置" style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "6px 8px", borderRadius: 4 }}>
            <Settings size={16} />
          </button>
          <button onClick={() => appWindow.minimize()} title="最小化" style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "6px 8px", borderRadius: 4 }}>
            <Minus size={16} />
          </button>
          <button onClick={() => appWindow.toggleMaximize()} title={isMaximized ? "还原" : "最大化"} style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "6px 8px", borderRadius: 4 }}>
            {isMaximized ? <Copy size={16} /> : <Square size={16} />}
          </button>
          <button onClick={() => appWindow.close()} title="关闭" style={{ background: "transparent", border: "none", color: "#e6edf3", cursor: "pointer", padding: "6px 8px", borderRadius: 4 }}>
            <X size={16} />
          </button>
        </div>
      </div>

      <div className="body" style={{ flex: 1, overflow: "auto" }}>
        <Routes>
          <Route path="/" element={<Dashboard onEdit={handleEdit} onStartRecording={handleStartRecording} locationKey={location.key} />} />
          <Route path="/settings" element={<SettingsPage darkMode={darkMode} onToggleDark={() => setDarkMode(!darkMode)} />} />
          <Route path="/editor/:id" element={<StepEditor />} />
        </Routes>
      </div>

      <div className="statusbar" style={{ height: 28, background: "#1f2428", display: "flex", alignItems: "center", justifyContent: "space-between", padding: "0 12px", color: "#8b949e", fontSize: 11 }}>
        <span>Flowio v2.0</span>
        <span style={{ display: "flex", alignItems: "center", gap: 4 }}>
          <span className={recording ? "status-dot" : "status-dot inactive"}></span>
          <span>{recording ? "录制中" : "就绪"}</span>
        </span>
      </div>
    </div>
  );
}

export default function WrappedApp() {
  return <BrowserRouter><App /></BrowserRouter>;
}
