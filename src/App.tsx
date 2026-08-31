import { BrowserRouter, Routes, Route, useNavigate, useLocation } from "react-router-dom";
import { useState, useCallback, useEffect, useRef } from "react";
import { Settings, Minus, Square, Copy, X, Pause, Play, User, LayoutDashboard } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Dashboard from "./pages/Dashboard";
import SettingsPage from "./pages/Settings";
import StepEditor from "./pages/StepEditor";
import ProfilePage from "./pages/Profile";
import { useAuthStore } from "./store/authStore";

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
  const { setAuth, clearAuth, siteUser, siteToken } = useAuthStore();

  useEffect(() => {
    const savedFontSize = localStorage.getItem("fontSize") || "14";
    document.body.style.zoom = `${parseInt(savedFontSize) / 14 * 100}%`;
  }, []);

  // 网站 token 自动刷新（PocketBase auth-refresh）
  useEffect(() => {
    if (!siteToken) return;
    (async () => {
      try {
        const res = await fetch("http://47.102.154.233/api/collections/users/auth-refresh", {
          method: "POST",
          headers: { Authorization: siteToken },
        });
        if (!res.ok) {
          console.log("[flowio] 网站 token 已过期，清除登录状态");
          clearAuth();
          return;
        }
        const data = await res.json();
        if (data.token && data.record?.id) {
          setAuth(data.token, {
            id: data.record.id,
            email: data.record.email || "",
            display_name: data.record.display_name || "",
            bio: data.record.bio || "",
            avatar: data.record.avatar || "",
          });
          console.log("[flowio] 网站 token 刷新成功:", data.record.email);
        }
      } catch (err) {
        console.warn("[flowio] token 刷新失败:", err);
      }
    })();
  }, []);

  // 启动时处理挂起的 deep-link（登录回调）
  useEffect(() => {
    (async () => {
      try {
        const dl = await invoke<{ token: string; id: string; email: string; display_name?: string; bio?: string; avatar?: string } | null>("get_pending_deep_link");
        if (dl) {
          console.log("[flowio] 启动时发现未处理的 deep-link:", dl);
          setAuth(dl.token, { id: dl.id, email: dl.email, display_name: dl.display_name, bio: dl.bio, avatar: dl.avatar });
        }
      } catch (err) {
        console.warn("[flowio] get_pending_deep_link 失败:", err);
      }
    })();
  }, [setAuth]);

  // 监听 deep-link 登录回调
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      unlisten = await listen("deep-link-auth", (event: any) => {
        console.log("[deep-link-auth] 收到事件:", event.payload);
        const { token, id, email, display_name, bio, avatar } = event.payload;
        setAuth(token, { id, email, display_name, bio, avatar });
      });
    })();
    return () => { unlisten?.(); };
  }, [setAuth]);

  // 轮询 fallback：每 2 秒检查一次 PendingDeepLink（事件未收到时兜底）
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const dl = await invoke<{ token: string; id: string; email: string; display_name?: string; bio?: string; avatar?: string } | null>("get_pending_deep_link");
        if (dl) {
          console.log("[flowio] 轮询发现未处理的 deep-link:", dl);
          setAuth(dl.token, { id: dl.id, email: dl.email, display_name: dl.display_name, bio: dl.bio, avatar: dl.avatar });
        }
      } catch {
        // 静默失败
      }
    }, 2000);
    return () => clearInterval(interval);
  }, [setAuth]);

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
      // record-self preference now lives in Settings (persisted in localStorage)
      const recordSelf = localStorage.getItem("flowio_record_self") === "1";
      const id = await invoke<string>("start_recording", { recordSelf });
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

  const handleEdit = useCallback((id: string) => { navigate(`/editor/${id}`); }, [navigate]);
  const formatTime = (sec: number) => `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, "0")}`;

  return (
    <div className={`app-layout ${darkMode ? "dark" : ""}`}>
      <div className="titlebar">
        <div className="titlebar-left">
          <div data-tauri-drag-region className="titlebar-drag">
            <span className="titlebar-logo">录步</span>
            {recording && (
              <span className={`titlebar-rec-badge ${isPaused ? "paused" : "recording"}`}>
                <span className="titlebar-rec-dot"></span>
                {isPaused ? "PAUSED" : "REC"} {formatTime(elapsed)}
              </span>
            )}
          </div>
          {isEditor && <div id="titlebar-editor-left" className="titlebar-editor-left" />}
        </div>
        <div className="titlebar-right">
          {isEditor && <div id="titlebar-editor-actions" className="titlebar-editor-actions" />}
          {!isEditor && recording && (
            <>
              {isPaused ? (
                <button className="titlebar-btn titlebar-btn-resume" onClick={handleResumeRecording} title="恢复录制">
                  <Play size={13} /> 恢复
                </button>
              ) : (
                <button className="titlebar-btn titlebar-btn-pause" onClick={handlePauseRecording} title="暂停录制">
                  <Pause size={13} /> 暂停
                </button>
              )}
              <button className="titlebar-btn titlebar-btn-stop" onClick={handleStopRecording} title="停止录制">STOP</button>
            </>
          )}
          {!isEditor && (
            <button
              className="titlebar-btn titlebar-btn-login"
              onClick={() => siteToken ? navigate("/profile") : invoke("open_external_url", { url: "http://47.102.154.233/author/login?redirect=flowio://auth" })}
              title={siteToken ? `已登录 ${siteUser?.email}（点击进入个人中心）` : "登录网站账号"}
            >
              {siteToken ? (
                <span className="titlebar-avatar">{((siteUser?.email) || "U")[0].toUpperCase()}</span>
              ) : (
                <><User size={15} /><span className="titlebar-login-text">登录</span></>
              )}
            </button>
          )}
          {!isEditor && (
            <button className="titlebar-win-btn" onClick={() => navigate("/settings")} title="设置"><Settings size={15} /></button>
          )}
          <button className="titlebar-win-btn" onClick={() => appWindow.minimize()} title="最小化"><Minus size={15} /></button>
          <button className="titlebar-win-btn" onClick={() => appWindow.toggleMaximize()} title={isMaximized ? "还原" : "最大化"}>{isMaximized ? <Copy size={13} /> : <Square size={13} />}</button>
          <button className="titlebar-win-btn titlebar-close-btn" onClick={() => appWindow.close()} title="关闭"><X size={15} /></button>
        </div>
      </div>

      <div className="app-body">
        <div className="body">
          <Routes>
            <Route path="/" element={<Dashboard onEdit={handleEdit} onStartRecording={handleStartRecording} locationKey={location.key} />} />
            <Route path="/settings" element={<SettingsPage darkMode={darkMode} onToggleDark={() => setDarkMode(!darkMode)} />} />
            <Route path="/profile" element={<ProfilePage />} />
            <Route path="/editor/:id" element={<StepEditor siteToken={siteToken} siteUser={siteUser} />} />
          </Routes>
        </div>
      </div>

      <div className="statusbar">
        <span className="statusbar-left">Flowio v2.0</span>
        <span className="statusbar-right">
          <span className={`status-dot ${recording ? "" : "inactive"}`}></span>
          <span>{recording ? "录制中" : "就绪"}</span>
        </span>
      </div>
    </div>
  );
}

export default function WrappedApp() {
  return <BrowserRouter><App /></BrowserRouter>;
}
