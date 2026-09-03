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
import { getCachedAvatar, cacheAvatar, avatarFileUrl } from "./lib/avatarCache";

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
  const [avatarLoadFailed, setAvatarLoadFailed] = useState(false);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const recordingIdRef = useRef<string | null>(null);
  const isPausedRef = useRef(false);
  const { setAuth, clearAuth, siteUser, siteToken } = useAuthStore();

  // 网站用户换了头像 / token 刷新后，旧的 url 失效 → 重新尝试加载图片
  useEffect(() => { setAvatarLoadFailed(false); }, [siteUser?.avatar, siteUser?.id]);

  // 头像缓存预热：启动即回填（不等 img onLoad 再触发），杜绝「应用刚启动
  // 就进出编辑器」时缓存未落地 → 重挂又走网络 → 闪绿底的竞态窗口
  useEffect(() => {
    if (siteToken && siteUser?.id && siteUser?.avatar) {
      cacheAvatar(siteUser.id, siteUser.avatar);
    }
  }, [siteToken, siteUser?.id, siteUser?.avatar]);

  // 登录态同步到本地 DB：仅写 settings.current_user 隔离标记，不做批量迁移
  const syncLocalUser = useCallback((userId: string) => {
    invoke("set_current_user", { userId })
      .then(() => console.log("[flowio] set_current_user 成功:", userId))
      .catch((e) =>
        console.warn("[flowio] set_current_user 失败:", e)
      );
  }, []);

  // 登录态本地同步（不依赖网络）：只要 WebView 已有有效登录态，就把用户 ID
  // 写入本地 DB。auth-refresh 走网络可能失败/超时，本地同步保证
  // settings.current_user 隔离标记始终与登录态一致（修复"登录没同步"）。
  useEffect(() => {
    if (siteToken && siteUser?.id) {
      syncLocalUser(siteUser.id);
    }
  }, [siteToken, siteUser?.id, syncLocalUser]);

  // 兜底：persist rehydrate 完成后（或首次同步失败/未触发时）再补一次同步。
  // 防止站点 token 存在但 effect 因依赖未变化而从未执行的情况。
  useEffect(() => {
    const t = setTimeout(() => {
      const { siteUser: u, siteToken: tok } = useAuthStore.getState();
      if (tok && u?.id) {
        syncLocalUser(u.id);
      }
    }, 800);
    return () => clearTimeout(t);
  }, [syncLocalUser]);

  useEffect(() => {
    const savedFontSize = localStorage.getItem("fontSize") || "14";
    document.body.style.zoom = `${parseInt(savedFontSize) / 14 * 100}%`;
  }, []);

  // 网站 token 自动刷新（PocketBase auth-refresh）
  // 合并策略：只覆盖服务端返回了非空值的字段，避免把已有头像/昵称清空。
  // 窗口获焦时也刷新，保证用户在网站改了头像/简介后切回桌面端能自动同步。
  const doAuthRefresh = useCallback(async () => {
    if (!siteToken) return;
    try {
      const res = await fetch("https://flowio.cn/api/collections/users/auth-refresh", {
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
        const prev = siteUser;
        setAuth(data.token, {
          id: data.record.id,
          email: data.record.email || prev?.email || "",
          display_name: data.record.display_name || prev?.display_name || "",
          bio: data.record.bio || prev?.bio || "",
          avatar: data.record.avatar || prev?.avatar || "",
        });
        syncLocalUser(data.record.id);
        console.log("[flowio] 网站 token 刷新成功:", data.record.email);
      }
    } catch (err) {
      console.warn("[flowio] token 刷新失败:", err);
    }
  }, [siteToken, siteUser, syncLocalUser]);

  useEffect(() => { doAuthRefresh(); }, []);
  // 窗口从后台切回前台时也刷新一次（用户可能在网站改了头像/简介）
  useEffect(() => {
    const handleFocus = () => { doAuthRefresh(); };
    window.addEventListener('focus', handleFocus);
    return () => window.removeEventListener('focus', handleFocus);
  }, [doAuthRefresh]);

  // 启动时处理挂起的 deep-link（登录回调）
  useEffect(() => {
    (async () => {
      try {
        const dl = await invoke<{ token: string; id: string; email: string; display_name?: string; bio?: string; avatar?: string } | null>("get_pending_deep_link");
        if (dl) {
          setAuth(dl.token, { id: dl.id, email: dl.email, display_name: dl.display_name, bio: dl.bio, avatar: dl.avatar });
          syncLocalUser(dl.id);
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
        const { token, id, email, display_name, bio, avatar } = event.payload;
        setAuth(token, { id, email, display_name, bio, avatar });
        syncLocalUser(id);
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
          syncLocalUser(dl.id);
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
      // record-self / record-target preferences live in Settings (persisted in localStorage)
      const recordSelf = localStorage.getItem("flowio_record_self") === "1";
      const recordTarget = localStorage.getItem("flowio_record_target") === "1";
      const targetProcess = localStorage.getItem("flowio_record_target_process") || "";
      const targetWindow = recordTarget && targetProcess ? targetProcess : null;
      const id = await invoke<string>("start_recording", { recordSelf, targetWindow });
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
      // 未登录时记录"录制完成"标记，编辑页自动弹出登录引导弹窗（LoginSaveModal）
      if (!siteToken) {
        sessionStorage.setItem("flowio_show_login_prompt", "1");
      } else {
        sessionStorage.removeItem("flowio_show_login_prompt");
      }
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
  }, [navigate, siteToken]);

  const handleEdit = useCallback((id: string) => { navigate(`/editor/${id}`); }, [navigate]);

  // 全局快捷键事件（Rust 侧 hotkey 模块 emit：shortcut-record / shortcut-pause / shortcut-stop）
  useEffect(() => {
    let unlisteners: Array<() => void> = [];
    // 简单去抖：同一事件 300ms 内只处理一次，避免系统级重复触发
    const lastTriggerAt: Record<string, number> = {};
    const debounced = (name: string, fn: () => void) => {
      const now = Date.now();
      if (lastTriggerAt[name] && now - lastTriggerAt[name] < 300) return;
      lastTriggerAt[name] = now;
      fn();
    };

    (async () => {
      unlisteners.push(await listen("shortcut-record", () => {
        debounced("shortcut-record", () => {
          if (recordingIdRef.current === null) {
            handleStartRecording();
          } else if (isPausedRef.current) {
            handleResumeRecording();
          }
          // 录制中且未暂停：忽略，避免误停
        });
      }));
      unlisteners.push(await listen("shortcut-pause", () => {
        debounced("shortcut-pause", () => {
          if (recordingIdRef.current === null) return;
          if (isPausedRef.current) {
            handleResumeRecording();
          } else {
            handlePauseRecording();
          }
        });
      }));
      unlisteners.push(await listen("shortcut-stop", () => {
        debounced("shortcut-stop", () => {
          if (recordingIdRef.current === null) return;
          handleStopRecording();
        });
      }));
    })();
    return () => { unlisteners.forEach((u) => u()); };
  }, [handleStartRecording, handlePauseRecording, handleResumeRecording, handleStopRecording]);
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
          <button
            className="titlebar-btn titlebar-btn-login"
            style={isEditor ? { display: "none" } : undefined}
            onClick={() => siteToken ? navigate("/profile") : invoke("open_external_url", { url: "https://flowio.cn/author/login?redirect=flowio://auth" })}
            title={siteToken ? `${siteUser?.display_name || siteUser?.email || '用户'}（点击进入个人中心）` : "登录网站账号"}
          >
            {siteToken ? (
              (() => {
                const avatarFile = siteUser?.avatar || "";
                // 优先同步命中本地缓存（dataURL）→ 页面切换零网络、不闪绿色底
                const cached = avatarFile ? getCachedAvatar(siteUser!.id, avatarFile) : null;
                return avatarFile && !avatarLoadFailed ? (
                <img
                  className="titlebar-avatar-img"
                  src={cached ?? avatarFileUrl(siteUser!.id, avatarFile)}
                  alt={siteUser?.display_name || siteUser?.email || "avatar"}
                  onError={() => setAvatarLoadFailed(true)}
                  onLoad={() => { if (!cached) cacheAvatar(siteUser!.id, avatarFile); }}
                />
              ) : (
                <img
                  className="titlebar-avatar-img"
                  src="https://flowio.cn/flowio-icon.png"
                  alt="avatar"
                />
              );
              })()
            ) : (
              <><User size={15} /><span className="titlebar-login-text">登录</span></>
            )}
          </button>
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
            <Route path="/" element={<Dashboard onEdit={handleEdit} onStartRecording={handleStartRecording} recording={recording} isPaused={isPaused} onPauseRecording={handlePauseRecording} onResumeRecording={handleResumeRecording} onStopRecording={handleStopRecording} locationKey={location.key} />} />
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
