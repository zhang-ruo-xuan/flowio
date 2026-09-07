import { BrowserRouter, Routes, Route, useNavigate, useLocation } from "react-router-dom";
import { useState, useCallback, useEffect, useRef } from "react";
import { Settings, Minus, Square, Copy, X, Pause, Play, User, LayoutDashboard, Mic, MicOff } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Dashboard from "./pages/Dashboard";
import SettingsPage from "./pages/Settings";
import StepEditor from "./pages/StepEditor";
import ProfilePage from "./pages/Profile";
import { useAuthStore } from "./store/authStore";
import { useAppStore } from "./store/appStore";
import { getCachedAvatar, cacheAvatar, avatarFileUrl } from "./lib/avatarCache";
import { VoiceRecorder } from "./lib/voiceRecorder";
import Tooltip from "./components/Tooltip";
import logoIcon from "./logo-32.png";

// 长任务操作 → 中文标签（全局完成提醒卡片用）
const JOB_OP_LABELS: Record<string, string> = {
  ai_simplify: "AI 精简",
  translate: "翻译",
  voice: "语音转写",
  scan_redact: "扫描脱敏",
  export: "导出",
};

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
  // ── 语音输入（PRD F1）────────────────────────────────────────
  const [voiceEnabled, setVoiceEnabled] = useState(() => localStorage.getItem("flowio_voice_input") === "1");
  const [voiceActive, setVoiceActive] = useState(false);   // 麦克风是否采集中
  const [voiceTranscribing, setVoiceTranscribing] = useState(false); // 停止后转写中
  const [voiceToast, setVoiceToast] = useState<{ type: "success" | "error"; message: string } | null>(null);
  const voiceRecorderRef = useRef<VoiceRecorder | null>(null);
  const voiceChunkTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  // 看门狗：采集样本数连续多秒不增长 = 音频回调停摆（最小化场景），记入 debug log
  const voiceStallRef = useRef<{ last: number; ticks: number; logged: boolean }>({ last: 0, ticks: 0, logged: false });
  const voiceToastTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const recordingIdRef = useRef<string | null>(null);
  const isPausedRef = useRef(false);
  const { setAuth, clearAuth, siteUser, siteToken } = useAuthStore();

  // ── 长任务完成全局提醒（AI 精简/翻译/语音/扫描脱敏/导出，任意页面弹窗）──
  const [jobToasts, setJobToasts] = useState<{
    id: number; recordingId: string; title: string; op: string; ok: boolean; summary: string;
  }[]>([]);
  const jobIdRef = useRef(0);

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

  // 长任务完成全局提醒：监听后端 flowio://job-done，任意页面右上角堆叠卡片，
  // 带教程名，点击跳教程；AI 精简完成时清掉全局「进行中」标记。
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listen<{ recording_id: string; title: string; op: string; ok: boolean; summary: string }>(
      "flowio://job-done",
      (ev) => {
        const p = ev.payload;
        const id = ++jobIdRef.current;
        setJobToasts((prev) => [
          ...prev,
          { id, recordingId: p.recording_id, title: p.title, op: p.op, ok: p.ok, summary: p.summary },
        ]);
        if (p.ok) {
          // 成功卡片：9 秒后自动消失（纯信息性，无需用户操作）
          window.setTimeout(() => {
            setJobToasts((prev) => prev.filter((t) => t.id !== id));
          }, 9000);
        }
        // 失败卡片：常驻直到用户点击右上角 × 关闭，避免 9 秒内被忽略而错过失败原因
        if (p.op === "ai_simplify") {
          useAppStore.getState().setSimplifyingId(null);
        }
      }
    ).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
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

  // ── 语音输入（PRD F1）────────────────────────────────────────
  const showVoiceToast = useCallback((t: { type: "success" | "error"; message: string }) => {
    setVoiceToast(t);
    if (voiceToastTimerRef.current) clearTimeout(voiceToastTimerRef.current);
    voiceToastTimerRef.current = setTimeout(() => setVoiceToast(null), 10000);
  }, []);

  // voice://done：转写完成 → 提示（StepEditor 自己监听并刷新步骤）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      unlisten = await listen<{ recordingId: string; transcribed: number; failed: number; ok: boolean; summary: string }>(
        "voice://done",
        (ev) => {
          setVoiceTranscribing(false);
          showVoiceToast({
            type: ev.payload.ok && ev.payload.failed === 0 ? "success" : "error",
            message: ev.payload.summary,
          });
        }
      );
    })();
    return () => { unlisten?.(); };
  }, [showVoiceToast]);

  // 监听设置页语音开关切换，同步标题栏显示（避免「设置页开 / 标题栏显示关」）
  useEffect(() => {
    const handler = (e: CustomEvent<{ enabled: boolean }>) => {
      setVoiceEnabled(e.detail.enabled);
    };
    window.addEventListener("flowio:voice-toggle", handler as EventListener);
    return () => window.removeEventListener("flowio:voice-toggle", handler as EventListener);
  }, []);

  // 语音开关：改 localStorage（V1 记忆）；录制中切换 → V13 语义
  const toggleVoice = useCallback(async () => {
    const next = !voiceEnabled;
    setVoiceEnabled(next);
    localStorage.setItem("flowio_voice_input", next ? "1" : "0");
    // 通知设置页同步（如果设置页正挂载着）
    window.dispatchEvent(new CustomEvent("flowio:voice-toggle", { detail: { enabled: next } }));
    const recId = recordingIdRef.current;
    if (!recId) return; // 未在录制，仅记忆开关
    if (!next && voiceRecorderRef.current?.isActive) {
      // 录制中关麦：停止采集，已采集部分照常转写
      voiceRecorderRef.current = null;
      setVoiceActive(false);
      if (voiceChunkTimerRef.current) { clearInterval(voiceChunkTimerRef.current); voiceChunkTimerRef.current = null; }
    } else if (next && !voiceRecorderRef.current) {
      // 录制中开麦：从现在开始采集（Rust 侧 cpal，PCM 直接进后端缓冲）
      try {
        const rec = new VoiceRecorder();
        await rec.start(recId);
        voiceRecorderRef.current = rec;
        setVoiceActive(true);
        voiceStallRef.current = { last: 0, ticks: 0, logged: false };
        voiceChunkTimerRef.current = setInterval(async () => {
          const r = voiceRecorderRef.current;
          if (!r || r.isPaused) return;
          const now = await r.samples();
          const s = voiceStallRef.current;
          if (now === s.last) {
            s.ticks += 1;
            if (s.ticks >= 5 && !s.logged) {
              s.logged = true;
              invoke("voice_dbg_log", { message: `[VOICE] 采集停摆：样本数 ${now} 已 5 秒未增长，本次录制语音将不完整` }).catch(() => {});
            }
          } else {
            s.last = now;
            s.ticks = 0;
          }
        }, 1000);
      } catch (err) {
        console.warn("[voice] mid-recording start failed:", err);
        setVoiceEnabled(false);
        localStorage.setItem("flowio_voice_input", "0");
        window.dispatchEvent(new CustomEvent("flowio:voice-toggle", { detail: { enabled: false } }));
        showVoiceToast({ type: "error", message: `麦克风开启失败：${String(err)}，本次录制将以无语音方式继续` });
      }
    }
  }, [voiceEnabled, showVoiceToast]);

  // 开始录制前开麦（V2/V3）；失败降级为纯录制，不阻塞
  const startVoiceIfEnabled = useCallback(async (id: string) => {
    if (!voiceEnabled || voiceRecorderRef.current) return;
    try {
      const rec = new VoiceRecorder();
      await rec.start(id);
      voiceRecorderRef.current = rec;
      setVoiceActive(true);
      voiceStallRef.current = { last: 0, ticks: 0, logged: false };
      // 看门狗：非暂停状态下样本数 5 秒不增长 → 采集停摆，只报一次。
      // （Rust 侧采集不受窗口最小化影响；真停了说明设备被拔掉/被独占）
      voiceChunkTimerRef.current = setInterval(async () => {
        const r = voiceRecorderRef.current;
        if (!r || r.isPaused) return;
        const now = await r.samples();
        const s = voiceStallRef.current;
        if (now === s.last) {
          s.ticks += 1;
          if (s.ticks >= 5 && !s.logged) {
            s.logged = true;
            invoke("voice_dbg_log", { message: `[VOICE] 采集停摆：样本数 ${now} 已 5 秒未增长（麦克风被占用或设备异常），本次录制语音将不完整` }).catch(() => {});
          }
        } else {
          s.last = now;
          s.ticks = 0;
        }
      }, 1000);
    } catch (err) {
      console.warn("[voice] start failed:", err);
      showVoiceToast({ type: "error", message: `麦克风开启失败：${String(err)}，本次录制将以无语音方式继续` });
    }
  }, [voiceEnabled, showVoiceToast]);

  // 停止采集并触发转写（在 finish_recording 成功后调用；V5/V9 不阻塞）
  const stopVoiceAndTranscribe = useCallback(async (id: string) => {
    if (voiceChunkTimerRef.current) { clearInterval(voiceChunkTimerRef.current); voiceChunkTimerRef.current = null; }
    const rec = voiceRecorderRef.current;
    voiceRecorderRef.current = null;
    setVoiceActive(false);
    if (!rec) return;
    try {
      // Rust 侧采集会把剩余 PCM 全部落进缓冲后再返回，前端无需补传
      await rec.stop();
      setVoiceTranscribing(true);
      await invoke("voice_finish", { recordingId: id });
    } catch (err) {
      console.warn("[voice] finish failed:", err);
      setVoiceTranscribing(false);
      showVoiceToast({ type: "error", message: "语音转写失败: " + String(err) });
    }
  }, [showVoiceToast]);

  // 卸载兜底清理
  useEffect(() => {
    return () => {
      if (voiceChunkTimerRef.current) clearInterval(voiceChunkTimerRef.current);
      voiceRecorderRef.current?.stop().catch(() => {});
    };
  }, []);

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
      await startVoiceIfEnabled(id);
      appWindow.minimize();
    } catch (err) {
      console.error("Failed to start recording:", err);
    }
  }, [startVoiceIfEnabled]);

  const handlePauseRecording = useCallback(async () => {
    const id = recordingIdRef.current;
    if (!id) return;
    try {
      await invoke("pause_recording", { id });
      isPausedRef.current = true;
      setIsPaused(true);
      void voiceRecorderRef.current?.pause(); // 暂停期间的音频不进缓冲（恢复时补等长静音）
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
      void voiceRecorderRef.current?.resume();
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
      // 语音转写（后台执行，不阻塞跳转编辑器；V5/V9）
      await stopVoiceAndTranscribe(id);
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
      // 兜底：流程异常中断时也要把麦克风收掉，否则采集线程一直跑
      voiceRecorderRef.current?.stop().catch(() => {});
      voiceRecorderRef.current = null;
    }
  }, [navigate, siteToken, stopVoiceAndTranscribe]);

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
            {!isEditor && (
              <span className="titlebar-logo" style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <img src={logoIcon} alt="" style={{ width: 18, height: 18, borderRadius: 3 }} />
                录步 Flowio
              </span>
            )}
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
          {/* 语音输入开关（V1/V2/V13）：录制中=🎙指示灯；未录制=预设开关 */}
          {!isEditor && (
            <button
              className={`titlebar-btn titlebar-btn-voice${voiceEnabled ? " on" : ""}`}
              style={voiceActive && recording ? { color: "#ff5b5b" } : undefined}
              onClick={toggleVoice}
              data-tip={
                recording
                  ? voiceActive ? "语音输入已开启（点击在本段录制中关闭，已采集内容照常转写）" : "语音输入已关闭（点击在本段录制中开启）"
                  : voiceEnabled ? "语音输入：开（录制时边说边记，文字自动补充到对应步骤）" : "语音输入：关（点击开启）"
              }
            >
              {voiceEnabled ? <Mic size={14} /> : <MicOff size={14} />}
              <span className="titlebar-voice-text">{recording && voiceActive ? "语音" : voiceEnabled ? "语音开" : "语音关"}</span>
            </button>
          )}
          {!isEditor && recording && (
            <>
              {isPaused ? (
                <button className="titlebar-btn titlebar-btn-resume" onClick={handleResumeRecording} data-tip={"恢复录制\n继续截图、记录你的操作"}>
                  <Play size={13} /> 恢复
                </button>
              ) : (
                <button className="titlebar-btn titlebar-btn-pause" onClick={handlePauseRecording} data-tip={"暂停录制\n暂停期间不再截图，语音也一并停止；用来中场休息或处理别的事"}>
                  <Pause size={13} /> 暂停
                </button>
              )}
              <button className="titlebar-btn titlebar-btn-stop" onClick={handleStopRecording} data-tip={"停止录制并保存\n停止后会自动合并重复点击/连续输入，并把你说的语音转成文字补进步骤"}>STOP</button>
            </>
          )}
          <button
            className="titlebar-btn titlebar-btn-login"
            style={isEditor ? { display: "none" } : undefined}
            onClick={() => siteToken ? navigate("/profile") : invoke("open_external_url", { url: "https://flowio.cn/author/login?redirect=flowio://auth" })}
            data-tip={siteToken ? `${siteUser?.display_name || siteUser?.email || '用户'}（点击进入个人中心）` : "登录网站账号"}
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
            <button className="titlebar-win-btn" onClick={() => navigate("/settings")} data-tip={"设置\nAI 服务商、快捷键、截图与隐私选项"}><Settings size={15} /></button>
          )}
          <button className="titlebar-win-btn" onClick={() => appWindow.minimize()}><Minus size={15} /></button>
          <button className="titlebar-win-btn" onClick={() => appWindow.toggleMaximize()}>{isMaximized ? <Copy size={13} /> : <Square size={13} />}</button>
          <button className="titlebar-win-btn titlebar-close-btn" onClick={() => appWindow.close()}><X size={15} /></button>
        </div>
      </div>

      <div className="app-body">
        <div className="body">
          <Routes>
            <Route path="/" element={<Dashboard onEdit={handleEdit} onStartRecording={handleStartRecording} recording={recording} isPaused={isPaused} onPauseRecording={handlePauseRecording} onResumeRecording={handleResumeRecording} onStopRecording={handleStopRecording} locationKey={location.key} />} />
            <Route path="/settings" element={<SettingsPage darkMode={darkMode} onToggleDark={() => setDarkMode(!darkMode)} />} />
            <Route path="/profile" element={<ProfilePage />} />
            <Route path="/editor/:id" element={<StepEditor siteToken={siteToken} siteUser={siteUser} voiceTranscribing={voiceTranscribing} />} />
          </Routes>
        </div>
      </div>

      <div className="statusbar">
        <span className="statusbar-left">Flowio v2.0</span>
        <span className="statusbar-right">
          <span className={`status-dot ${recording ? "" : "inactive"}`}></span>
          <span>{voiceTranscribing ? "语音转写中…" : recording ? "录制中" : "就绪"}</span>
        </span>
      </div>

      {/* 语音输入非阻塞 toast（V3/V9） */}
      {voiceToast && (
        <div
          style={{
            position: "fixed", bottom: 44, right: 20, zIndex: 9999,
            display: "flex", alignItems: "flex-start", gap: 10,
            padding: "8px 12px", borderRadius: 6, fontSize: 13, maxWidth: 400,
            boxShadow: "0 4px 14px rgba(0,0,0,0.18)",
            background: voiceToast.type === "success" ? "#f0f9f0" : "#fdf0f0",
            border: `1px solid ${voiceToast.type === "success" ? "#4caf50" : "#e57373"}`,
            color: voiceToast.type === "success" ? "#2e7d32" : "#c62828",
          }}
        >
          <span style={{ wordBreak: "break-word", lineHeight: 1.5 }}>{voiceToast.message}</span>
          <button
            onClick={() => setVoiceToast(null)}
            title="关闭"
            style={{
              flexShrink: 0,
              width: 20,
              height: 20,
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              padding: 0,
              border: "none",
              borderRadius: "50%",
              background: "rgba(0,0,0,0.08)",
              color: "inherit",
              fontSize: 11,
              lineHeight: 1,
              cursor: "pointer",
            }}
          >
            ✕
          </button>
        </div>
      )}

      {/* 长任务完成全局提醒：任意页面右上角堆叠卡片，带教程名，点击跳教程 */}
      {jobToasts.length > 0 && (
        <div style={{ position: "fixed", top: 52, right: 16, zIndex: 10000, display: "flex", flexDirection: "column", gap: 8, maxWidth: 340 }}>
          {jobToasts.map((t) => (
            <div
              key={t.id}
              onClick={() => navigate(`/editor/${t.recordingId}`)}
              style={{
                background: t.ok ? "#ffffff" : "#fff5f5",
                border: `1px solid ${t.ok ? "#4caf50" : "#e57373"}`,
                borderLeft: `4px solid ${t.ok ? "#4caf50" : "#e57373"}`,
                borderRadius: 8, padding: "10px 12px", cursor: "pointer",
                boxShadow: "0 4px 14px rgba(0,0,0,0.16)",
              }}
            >
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
                <span style={{ fontWeight: 600, fontSize: 13, color: t.ok ? "#2e7d32" : "#c62828" }}>
                  {JOB_OP_LABELS[t.op] ?? t.op}{t.ok ? "完成" : "失败"}
                </span>
                <button
                  onClick={(e) => { e.stopPropagation(); setJobToasts((prev) => prev.filter((x) => x.id !== t.id)); }}
                  style={{ background: "transparent", border: "none", cursor: "pointer", fontSize: 14, lineHeight: 1, color: "#888" }}
                >×</button>
              </div>
              <div style={{ fontSize: 12, color: "#333", marginTop: 3, fontWeight: 500 }}>
                《{t.title || "未命名教程"}》
              </div>
              <div style={{ fontSize: 12, color: "#555", marginTop: 2 }}>{t.summary}</div>
              <div style={{ fontSize: 11, color: "#999", marginTop: 4 }}>点击查看教程</div>
            </div>
          ))}
        </div>
      )}

      {/* 全局悬浮提示层：任何元素加 data-tip="…" 即可出气泡 */}
      <Tooltip />
    </div>
  );
}

export default function WrappedApp() {
  return <BrowserRouter><App /></BrowserRouter>;
}
