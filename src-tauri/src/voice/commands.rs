//! 语音输入 Tauri 命令：采集缓冲（begin/append/finish/cancel）+ 后台转写。
//!
//! 流程（对应 PRD《voice-and-translation-prd.md》F1）：
//! 1. 前端在 start_recording 前开麦，`voice_mic_start`（voice/capture.rs，
//!    走 cpal 原生采集）建缓冲并记录音频起点（epoch ms）；
//! 2. PCM 由采集线程直接写入缓冲；`voice_append` 保留为手工追加入口
//!    （base64 分块，暂停期间的音频由采集侧补静音而非丢弃）；
//! 3. `voice_finish` 立即返回，后台线程读 DB 步骤时间戳 → 切片 → GLM-ASR
//!    → 文字以「【语音】…」追加进对应步骤描述 → emit `voice://done`；
//! 4. 转写失败不阻塞教程生成（V9），原因落 debug log 并聚合进事件。

use base64::Engine;
use rusqlite::params;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

use super::asr;
use super::audio;
use crate::DbState;

/// 单条录制的语音采集缓冲。
pub struct VoiceBuffer {
    /// 音频采集起点（epoch 毫秒）。步骤偏移 = step.timestamp - started_at_ms。
    pub started_at_ms: i64,
    pub sample_rate: u32,
    /// 裸 PCM（i16 LE，mono）
    pub pcm: Vec<u8>,
    /// 心跳水位：采集量每新增 ~10s 向 debug log 打一条（排障用，
    /// 可发现「窗口最小化后采集回调停摆」这类前端问题）
    pub last_heartbeat_len: usize,
    /// ★ 实时字幕：录制中已逐句转写的句子（start_ms 相对采集起点）。
    /// 停止录制时若非空 → 直接按时间戳归属步骤，不再整段转写（所见即所得）。
    pub live_segments: Vec<LiveSegment>,
    /// ★ 实时字幕：句级 VAD 游标（当前句的起点与计时）
    pub live_cursor: LiveCursor,
    /// 在途实时转写计数（0 = 全部落库）。停止/取缓冲前必须等它归零，
    /// 否则「缓冲已移除、句子转写完无处可写」→ 尾句/慢句静默丢失。
    pub live_pending: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

/// 实时字幕的一句转写结果。
#[derive(Clone, Debug)]
pub struct LiveSegment {
    /// 相对采集起点的毫秒偏移
    pub start_ms: i64,
    /// 该句音频时长（毫秒），用于「实时产物是否覆盖了检出语音」的完整性守卫
    pub duration_ms: i64,
    pub text: String,
}

/// 句级 VAD 游标：判断「一句话说完了没」。
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveCursor {
    /// 当前句起点（PCM 字节下标）
    pub start_byte: usize,
    pub speech_ms: i64,
    pub silence_ms: i64,
    pub total_ms: i64,
}

/// 句内判定阈值（RMS，与扩展端 offscreen / 整段 VAD 的 SILENCE_RMS 对齐）。
/// 曾用 0.006：比整段 VAD(0.0045) 高一截，低电平麦克风的轻声句子实时
/// 通道漏检、整段通道却能检出 → 实时产物缺句。对齐后两端灵敏度一致。
pub const LIVE_SPEECH_RMS: f32 = 0.0045;
/// 句末静音：≥600ms 且本句已有语音 → 吐句
pub const LIVE_END_SILENCE_MS: i64 = 600;
/// 单句最短 1.5s：更短的碎片与下一句合并（短句识别质量差）
pub const LIVE_MIN_SPEECH_MS: i64 = 1_500;
/// 单句最长 8s：说完不停也强制断句（控制出字延迟）
pub const LIVE_MAX_SPEECH_MS: i64 = 8_000;
/// 段总长硬上限 15s：防整段过大
pub const LIVE_HARD_MAX_MS: i64 = 15_000;

/// 句级 VAD 状态推进（纯函数，便于单测）。
///
/// 返回 `Some((start_byte, end_byte, start_ms))` 表示「这句话说完了，可以送 ASR」；
/// `start_ms` 是该句相对采集起点的起始毫秒偏移。
pub fn live_vad_update(
    cur: &mut LiveCursor,
    chunk_ms: i64,
    rms: f32,
    pcm_len_bytes: usize,
    sample_rate: u32,
) -> Option<(usize, usize, i64)> {
    if rms >= LIVE_SPEECH_RMS {
        cur.speech_ms += chunk_ms;
        cur.silence_ms = 0;
    } else if cur.speech_ms > 0 {
        cur.silence_ms += chunk_ms;
    } else {
        // 还没开口：起点跟着往前滑，避免把长静音算进句子
        cur.start_byte = pcm_len_bytes;
    }
    cur.total_ms += chunk_ms;
    let enough = cur.speech_ms >= LIVE_MIN_SPEECH_MS;
    let done = cur.speech_ms > 0
        && (cur.total_ms >= LIVE_HARD_MAX_MS
            || (enough
                && (cur.silence_ms >= LIVE_END_SILENCE_MS || cur.speech_ms >= LIVE_MAX_SPEECH_MS)));
    if !done {
        return None;
    }
    let start = cur.start_byte;
    let end = pcm_len_bytes;
    let start_ms = (start as i64 * 1000) / (sample_rate as i64 * 2);
    cur.start_byte = end;
    cur.speech_ms = 0;
    cur.silence_ms = 0;
    cur.total_ms = 0;
    if end <= start {
        return None;
    }
    Some((start, end, start_ms))
}

/// 按「操作前的语音归这个操作的步骤」把实时句子归到步骤（纯函数）。
///
/// `offsets[i]` = 第 i 步相对采集起点的毫秒偏移；返回 `(step_idx, 该步合并文本)`。
pub fn assign_live_segments(offsets: &[i64], segs: &[LiveSegment]) -> Vec<(usize, String)> {
    if offsets.is_empty() || segs.is_empty() {
        return Vec::new();
    }
    let mut grouped: std::collections::BTreeMap<usize, Vec<String>> =
        std::collections::BTreeMap::new();
    for s in segs {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }
        let rs = s.start_ms.max(0);
        let idx = offsets
            .iter()
            .position(|&off| off.max(0) > rs)
            .unwrap_or(offsets.len() - 1);
        grouped.entry(idx).or_default().push(text.to_string());
    }
    grouped
        .into_iter()
        .map(|(i, texts)| (i, texts.join("")))
        .collect()
}

/// 全部进行中的语音采集缓冲（recording_id → buffer）。
///
/// 用 Arc 包一层：cpal 采集线程（voice/capture.rs）需要持有一份克隆，
/// 而 Tauri State 只能给出引用。
#[derive(Clone, Default)]
pub struct VoiceCaptureState(pub Arc<Mutex<HashMap<String, VoiceBuffer>>>);

/// `voice://done` 事件载荷。
///
/// 🔴 必须 camelCase：前端（App.tsx / StepEditor.tsx）用 `ev.payload.recordingId` 读。
/// 蛇形的 `recording_id` 会让匹配失败 → 转写成功后编辑器不刷新、toast 不弹，
/// 用户看到的表现与「语音没生效」一模一样（2026-09-04 实测踩坑）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceDoneEvent {
    recording_id: String,
    transcribed: usize,
    failed: usize,
    ok: bool,
    summary: String,
}

/// 前端语音诊断写 debug log（release 无 console；voiceRecorder 看门狗用）。
#[tauri::command]
pub fn voice_dbg_log(message: String) {
    crate::ai::commands::ai_dbg_log(&message);
}

/// 开始语音采集：建立（或重置）该录制的缓冲。
#[tauri::command]
pub fn voice_begin(
    state: State<'_, VoiceCaptureState>,
    recording_id: String,
    started_at_ms: i64,
    sample_rate: Option<u32>,
) -> Result<(), String> {
    let mut map = state.0.lock().map_err(|e| e.to_string())?;
    map.insert(
        recording_id,
        VoiceBuffer {
            started_at_ms,
            sample_rate: sample_rate.unwrap_or(audio::SAMPLE_RATE),
            pcm: Vec::new(),
            last_heartbeat_len: 0,
            live_segments: Vec::new(),
            live_cursor: Default::default(),
            live_pending: Default::default(),
        },
    );
    Ok(())
}

/// 追加一块 PCM（base64 编码的 i16 LE 字节）。
#[tauri::command]
pub fn voice_append(
    state: State<'_, VoiceCaptureState>,
    recording_id: String,
    chunk_base64: String,
) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk_base64.as_bytes())
        .map_err(|e| format!("voice chunk base64 decode failed: {}", e))?;
    let mut map = state.0.lock().map_err(|e| e.to_string())?;
    let buffer = map
        .get_mut(&recording_id)
        .ok_or_else(|| "voice buffer not found (did you call voice_begin?)".to_string())?;
    append_pcm_bytes(buffer, &bytes, &recording_id);
    Ok(())
}

/// 把裸 PCM（i16 LE）写进缓冲，并打采集心跳。
///
/// 采集线程（voice/capture.rs）与 `voice_append` 共用这一份实现。
pub(crate) fn append_pcm_bytes(buffer: &mut VoiceBuffer, bytes: &[u8], recording_id: &str) {
    if buffer.pcm.len() + bytes.len() > audio::MAX_BUFFER_BYTES {
        // 超上限：丢弃多余数据（30 分钟语音已覆盖绝大多数场景），不报错中断录制
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE] buffer limit reached for {}, dropping {} bytes",
            recording_id,
            bytes.len()
        ));
        return;
    }
    buffer.pcm.extend_from_slice(bytes);
    // 采集心跳：每 ~10s（16kHz/16bit 单声道 ≈ 320KB）打一条，用于发现
    // 「开麦成功但采集停摆」这类问题
    const HEARTBEAT_BYTES: usize = 320_000;
    let hb_slot = buffer.pcm.len() / HEARTBEAT_BYTES;
    if hb_slot > buffer.last_heartbeat_len {
        buffer.last_heartbeat_len = hb_slot;
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE] capture {}: {}s / {} KB",
            recording_id,
            audio::pcm_duration_ms(buffer.pcm.len(), buffer.sample_rate) / 1000,
            buffer.pcm.len() / 1024
        ));
    }
}

/// 取消采集（用户放弃本次录制）：直接丢弃缓冲。
#[tauri::command]
pub fn voice_cancel(
    state: State<'_, VoiceCaptureState>,
    recording_id: String,
) -> Result<(), String> {
    if let Ok(mut map) = state.0.lock() {
        map.remove(&recording_id);
    }
    Ok(())
}

/// 结束采集并触发后台转写。
///
/// 立即返回（UI 显示「语音转写中…」）；结果经 `voice://done` 事件回传。
#[tauri::command]
pub fn voice_finish(
    db: State<'_, DbState>,
    app: AppHandle,
    state: State<'_, VoiceCaptureState>,
    recording_id: String,
) -> Result<(), String> {
    // 结束前先做一次 DB 读取，拿到录制标题便于日志定位（失败不致命）
    let buffer = {
        // 🔴 先等在途实时转写全部落库（不持锁等待），再取走缓冲。
        // 否则句子转写完时缓冲已不在 map 里 → 尾句/慢句静默丢失。
        let pending = state
            .0
            .lock()
            .ok()
            .and_then(|m| m.get(&recording_id).map(|b| b.live_pending.clone()));
        if let Some(p) = pending {
            let left = wait_live_pending(&p, std::time::Duration::from_secs(8));
            if left > 0 {
                crate::ai::commands::ai_dbg_log(&format!(
                    "[VOICE] finish {}: {} live ASR still in flight after 8s, proceeding (may lose tail text)",
                    recording_id, left
                ));
            }
        }
        let mut map = state.0.lock().map_err(|e| e.to_string())?;
        map.remove(&recording_id)
    };
    let Some(buffer) = buffer else {
        return Err("没有可转写的语音数据（本次录制未开启语音输入）".to_string());
    };
    // 读取教程标题，供完成提醒展示（失败不致命，降级为空串）
    let title = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT title FROM recordings WHERE id = ?1",
            rusqlite::params![recording_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default()
    };

    if buffer.pcm.is_empty() {
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE] finish for {}: buffer EMPTY (voice_begin was called but no PCM arrived; capture likely died or mic gave silence)",
            recording_id
        ));
        let summary = "麦克风没有采集到任何音频数据（开麦成功但全程无数据），请检查系统麦克风设备后重试"
            .to_string();
        let _ = app.emit(
            "voice://done",
            VoiceDoneEvent {
                recording_id: recording_id.clone(),
                transcribed: 0,
                failed: 0,
                ok: false,
                summary: summary.clone(),
            },
        );
        // 全局完成提醒（任意页面弹窗 + Windows 系统通知，带教程名）
        crate::job_notify::emit_job_done(
            &app,
            &recording_id,
            &title,
            crate::job_notify::JobOp::Voice,
            false,
            &summary,
        );
        return Ok(());
    }

    let app_handle = app.clone();
    let title2 = title;
    std::thread::spawn(move || {
        let result = transcribe_and_apply(&recording_id, buffer);
        match &result {
            Ok(ev) => {
                crate::ai::commands::ai_dbg_log(&format!(
                    "[VOICE] done for {}: {}",
                    recording_id, ev.summary
                ));
            }
            Err(msg) => {
                crate::ai::commands::ai_dbg_log(&format!(
                    "[VOICE] fatal for {}: {}",
                    recording_id, msg
                ));
            }
        }
        let (ok, summary) = match &result {
            Ok(ev) => (ev.ok, ev.summary.clone()),
            Err(msg) => (false, msg.clone()),
        };
        let event = match result {
            Ok(ev) => ev,
            Err(msg) => VoiceDoneEvent {
                recording_id: recording_id.clone(),
                transcribed: 0,
                failed: 0,
                ok: false,
                summary: msg,
            },
        };
        let _ = app_handle.emit("voice://done", event);
        // 全局完成提醒（任意页面弹窗 + Windows 系统通知，带教程名）
        crate::job_notify::emit_job_done(
            &app_handle,
            &recording_id,
            &title2,
            crate::job_notify::JobOp::Voice,
            ok,
            &summary,
        );
    });

    // 抑制 db 未使用告警：当前线程内自开连接（见 transcribe_and_apply），
    // 传入的 db 句柄保留在签名里以便未来复用主连接。
    let _ = &db;
    Ok(())
}

/// 解析语音转写实际使用的 API Key，返回 `(key, 通道名)`。
///
/// 🔴 **不能直接复用 `resolve_ai_config().api_key`**，两个原因：
///
/// ① ASR 端点硬编码智谱（`asr::ASR_URL`），拿千问 / DeepSeek 的 Key 去打
///    智谱必然 401 —— Key 必须与端点同族。
/// ② 用户把默认 AI 服务商设成第三方，通常只是为了换模型写步骤说明（省钱
///    / 效果更好），**不该顺带把语音功能整个废掉**。旧逻辑在这里直接拒绝
///    服务，是「配了第三方 AI 之后语音就再没效果」的隐藏原因。
///
/// 策略：当前配置是智谱系且 Key 非空 → 用它的；否则一律回落内置免费通道
/// Key（`glm-asr-2512` 在免费额度内可用，2026-09-03 实测中文转写准确）。
pub fn resolve_asr_key(cfg: &crate::ai::types::AiConfig) -> (String, &'static str) {
    if asr::asr_supported(&cfg.provider.id, &cfg.provider.base_url)
        && !cfg.api_key.trim().is_empty()
    {
        return (cfg.api_key.clone(), "用户配置");
    }
    let builtin = crate::ai::commands::get_builtin_free_config();
    (builtin.api_key, "内置免费通道")
}

/// ★ 实时字幕：采集线程每来一块 PCM 调一次。
///
/// 内部只做「记账 + 决定吐不吐句」；真正转写在独立线程里做（HTTP 阻塞
/// 不能压在采集线程上）。转写成功后：① 记进 `live_segments`（停止时直接
/// 归属落库）② emit `voice://caption` 推给前端显示。
pub fn feed_live_chunk(
    buffers: &VoiceCaptureState,
    recording_id: &str,
    chunk: &[i16],
    sample_rate: u32,
    asr_key: &str,
    hotwords: &[String],
    app: &AppHandle,
) {
    let chunk_ms = (chunk.len() as i64 * 1000) / sample_rate as i64;
    if chunk_ms <= 0 {
        return;
    }
    let rms = audio::rms_i16(chunk);

    // ① 先算要不要吐句（持锁时间极短）
    let pending = {
        let Ok(mut map) = buffers.0.lock() else {
            return;
        };
        let Some(buf) = map.get_mut(recording_id) else {
            return;
        };
        let pcm_len = buf.pcm.len();
        let hit = live_vad_update(
            &mut buf.live_cursor,
            chunk_ms,
            rms,
            pcm_len,
            sample_rate,
        );
        let (start, end, start_ms) = match hit {
            Some(v) => v,
            None => return,
        };
        if end <= start || end > pcm_len || (end - start) < (sample_rate as usize * 2 / 5) {
            return; // 段太短（<200ms）：不打扰 ASR
        }
        let duration_ms = ((end - start) as i64 * 1000) / (sample_rate as i64 * 2);
        (buf.pcm[start..end].to_vec(), start_ms, duration_ms)
    };
    let (pcm, start_ms, duration_ms) = pending;

    // ② 转写放后台线程（每句一次 HTTP，采集线程继续跑）
    spawn_live_transcribe(buffers, recording_id, pcm, start_ms, duration_ms, sample_rate, asr_key, hotwords, app);
}

/// 后台转写一句实时语音：完成后记进 `live_segments` + emit 字幕事件。
///
/// 🔴 在途计数（live_pending）：spawn 前 +1，线程退出前 -1。停止录制时
/// 必须等它归零再取走缓冲，否则「缓冲已被 voice_finish 移除、句子转写完
/// 无处可写」→ 尾句/慢句静默丢失（2026-09-06 实测「说的话丢了」根因之一）。
fn spawn_live_transcribe(
    buffers: &VoiceCaptureState,
    recording_id: &str,
    pcm: Vec<u8>,
    start_ms: i64,
    duration_ms: i64,
    sample_rate: u32,
    asr_key: &str,
    hotwords: &[String],
    app: &AppHandle,
) {
    let key = asr_key.to_string();
    let words = hotwords.to_vec();
    let rid = recording_id.to_string();
    let bufs = buffers.clone();
    let app = app.clone();
    // 计数 Arc：先取（缓冲可能刚被移除——取消场景，直接放弃即可）
    let pending = {
        let Ok(map) = bufs.0.lock() else { return; };
        match map.get(&rid) {
            Some(b) => b.live_pending.clone(),
            None => return,
        }
    };
    pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::thread::spawn(move || {
        let outcome = (|| -> Option<String> {
            let wav = audio::pcm_to_wav(&pcm, sample_rate);
            let prev = {
                let Ok(map) = bufs.0.lock() else { return None; };
                map.get(&rid)
                    .and_then(|b| b.live_segments.last().map(|s| s.text.clone()))
            };
            match asr::transcribe_wav_auto(
                &key, wav, &words, asr::segment_prompt(prev.as_deref()).as_deref(),
            ) {
                Ok(text) if !text.trim().is_empty() => Some(text),
                Ok(_) => None,
                Err(e) => {
                    crate::ai::commands::ai_dbg_log(&format!(
                        "[VOICE] live caption ASR failed: {}", e
                    ));
                    None
                }
            }
        })();
        if let Some(text) = outcome {
            {
                let Ok(mut map) = bufs.0.lock() else {
                    app_emit_caption(&app, &rid, text);
                    pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                };
                if let Some(b) = map.get_mut(&rid) {
                    b.live_segments.push(LiveSegment { start_ms, duration_ms, text: text.clone() });
                } else {
                    // 缓冲已被移除（极端竞态）：字幕还能发，但句子落不了库
                    crate::ai::commands::ai_dbg_log(&format!(
                        "[VOICE] live segment dropped (buffer gone): {} chars lost", text.len()
                    ));
                }
            }
            app_emit_caption(&app, &rid, text);
        }
        pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    });
}

/// 发一句字幕到前端（voice://caption）。
fn app_emit_caption(app: &AppHandle, recording_id: &str, text: String) {
    let _ = app.emit(
        "voice://caption",
        LiveCaptionEvent {
            recording_id: recording_id.to_string(),
            text,
        },
    );
}

/// 等待在途实时转写全部完成（不持锁轮询，最多 timeout），返回剩余数量。
/// 🔴 调用方绝不能持有 buffers 锁——转写线程结束时要拿锁写回。
pub(crate) fn wait_live_pending(
    pending: &std::sync::atomic::AtomicUsize,
    timeout: std::time::Duration,
) -> usize {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let n = pending.load(std::sync::atomic::Ordering::SeqCst);
        if n == 0 || std::time::Instant::now() >= deadline {
            return n;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// ★ 收尾：把当前没吐完的半句也吐出去（暂停 / 停止时调用）。
pub fn flush_live_chunk(
    buffers: &VoiceCaptureState,
    recording_id: &str,
    sample_rate: u32,
    asr_key: &str,
    hotwords: &[String],
    app: &AppHandle,
) {
    let tail = {
        let Ok(mut map) = buffers.0.lock() else {
            return;
        };
        let Some(buf) = map.get_mut(recording_id) else {
            return;
        };
        if buf.live_cursor.speech_ms <= 0 {
            return;
        }
        let start = buf.live_cursor.start_byte.min(buf.pcm.len());
        let end = buf.pcm.len();
        buf.live_cursor = LiveCursor {
            start_byte: end,
            ..Default::default()
        };
        if end <= start {
            return;
        }
        let duration_ms = ((end - start) as i64 * 1000) / (sample_rate as i64 * 2);
        (
            buf.pcm[start..end].to_vec(),
            (start as i64 * 1000) / (sample_rate as i64 * 2),
            duration_ms,
        )
    };
    let (pcm, start_ms, duration_ms) = tail;
    spawn_live_transcribe(buffers, recording_id, pcm, start_ms, duration_ms, sample_rate, asr_key, hotwords, app);
}

/// `voice://caption` 事件载荷（🔴 必须 camelCase）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LiveCaptionEvent {
    recording_id: String,
    text: String,
}

/// 用户自定义 ASR 热词的 settings 键（JSON 字符串数组）。
const HOTWORDS_KEY: &str = "voice_hotwords";

/// 读用户自定义热词（设置 → 录制设置）。容忍脏数据：解析失败记日志、
/// 返回空表（内置热词仍然生效），绝不能因热词坏掉让语音转写整体失败。
pub fn read_custom_hotwords(conn: &rusqlite::Connection) -> Vec<String> {
    let val: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params!(HOTWORDS_KEY),
            |r| r.get(0),
        )
        .ok();
    match val {
        Some(s) => match serde_json::from_str::<Vec<String>>(&s) {
            Ok(list) => list,
            Err(e) => {
                crate::ai::commands::ai_dbg_log(&format!(
                    "[VOICE] hotwords parse failed (ignored): {} (raw: {})",
                    e,
                    &s.chars().take(80).collect::<String>()
                ));
                Vec::new()
            }
        },
        None => Vec::new(),
    }
}

/// 获取用户自定义语音识别热词（设置页用；内置热词由 Rust 侧自行合并）。
#[tauri::command]
pub fn voice_get_hotwords(db: State<'_, DbState>) -> Result<Vec<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(read_custom_hotwords(&conn))
}

/// 保存用户自定义语音识别热词（空数组 = 清空，仅用内置热词）。
#[tauri::command]
pub fn voice_set_hotwords(db: State<'_, DbState>, words: Vec<String>) -> Result<(), String> {
    let list: Vec<String> = words
        .into_iter()
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty())
        .collect();
    if list.len() > asr::MAX_HOTWORDS {
        return Err(format!("热词最多 {} 个", asr::MAX_HOTWORDS));
    }
    let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params!(HOTWORDS_KEY, json),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 获取/设置「语音 AI 纠错」开关（设置 → 录制设置；默认开）。
#[tauri::command]
pub fn voice_get_correction_enabled(db: State<'_, DbState>) -> Result<bool, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(super::correct::correction_enabled(&conn))
}

#[tauri::command]
pub fn voice_set_correction_enabled(db: State<'_, DbState>, enabled: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES ('voice_ai_correct', ?1)",
        params!(if enabled { "1" } else { "0" }),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 单次转写任务的全量逻辑（在后台线程执行）。
///
/// 使用独立 DB 连接：`DbState`（Mutex<Connection>）不可跨线程移动，
/// 而转写要等所有 ASR 请求结束后才写库；SQLite 同文件多连接对这种
/// 低频短写完全安全。
fn transcribe_and_apply(recording_id: &str, buffer: VoiceBuffer) -> Result<VoiceDoneEvent, String> {
    let conn = crate::db::init_db_file();

    // ── 1. 读步骤（时间戳 + 当前描述）──────────────────────────
    let mut stmt = conn
        .prepare(
            "SELECT id, description, timestamp FROM steps \
             WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| format!("read steps failed: {}", e))?;
    let steps: Vec<(String, String, i64)> = stmt
        .query_map(params!(recording_id), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| format!("read steps failed: {}", e))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    if steps.is_empty() {
        return Ok(VoiceDoneEvent {
            recording_id: recording_id.to_string(),
            transcribed: 0,
            failed: 0,
            ok: true,
            summary: "本次录制没有步骤，语音内容已忽略".to_string(),
        });
    }

    // ── 2. 解析 ASR 通道与 Key ────────────────────────────────
    let config = crate::ai::commands::resolve_ai_config(&conn);
    let (asr_key, asr_channel) = resolve_asr_key(&config);
    if asr_key.trim().is_empty() {
        return Ok(VoiceDoneEvent {
            recording_id: recording_id.to_string(),
            transcribed: 0,
            failed: 0,
            ok: false,
            summary: "未配置可用的语音识别 Key，请在设置 → AI 服务商中配置智谱 AI 的 API Key".to_string(),
        });
    }
    crate::ai::commands::ai_dbg_log(&format!(
        "[VOICE] channel {}: provider={} ({}), model={}",
        recording_id, config.provider.id, asr_channel, asr::ASR_MODEL
    ));

    // ── 3. 切片（含 VAD 诊断与兜底）───────────────────────────
    let sample_rate = buffer.sample_rate;
    let audio_ms = audio::pcm_duration_ms(buffer.pcm.len(), sample_rate);
    let overall_rms = audio::overall_rms(&buffer.pcm);
    let regions = audio::speech_regions(&buffer.pcm, sample_rate);
    let speech_ms: i64 = regions.iter().map(|(s, e)| e - s).sum();
    let offsets: Vec<i64> = steps
        .iter()
        .map(|(_, _, ts)| ts - buffer.started_at_ms)
        .collect();
    crate::ai::commands::ai_dbg_log(&format!(
        "[VOICE] transcribe {}: audio {}ms ({} KB), overall_rms={:.5}, regions={} ({}ms speech), started_at={}, first_step_offset={}ms, steps={}",
        recording_id,
        audio_ms,
        buffer.pcm.len() / 1024,
        overall_rms,
        regions.len(),
        speech_ms,
        buffer.started_at_ms,
        offsets.first().copied().unwrap_or(-1),
        steps.len()
    ));

    // ★ 实时字幕产物优先：录制中已逐句转写完 → 直接归属，不再整段转写（屏幕所见即最终稿）
    // 🔴 完整性守卫：实时句子覆盖的音频时长不足检出语音的一半（在途丢失 /
    //    低电平漏检）→ 放弃实时产物，整段按步骤重转，保证「说过的话不丢」。
    let live = buffer.live_segments.clone();
    let live_ms: i64 = live.iter().map(|s| s.duration_ms).sum();
    let precomputed: Option<Vec<(usize, String)>> = if live.is_empty() {
        None
    } else if speech_ms > 0 && live_ms * 2 < speech_ms {
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE] transcribe {}: 实时产物疑似不完整（{} 句 / 覆盖 {}ms < 检出语音 {}ms 的一半）→ 整段兜底重转",
            recording_id,
            live.len(),
            live_ms,
            speech_ms
        ));
        None
    } else {
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE] transcribe {}: 使用实时字幕产物 {} 句（跳过整段转写）",
            recording_id,
            live.len()
        ));
        Some(assign_live_segments(&offsets, &live))
    };

    let mut used_fallback = false;
    let mut sliced = audio::slice_for_steps(&buffer.pcm, sample_rate, &offsets);
    if sliced.is_empty() && overall_rms > audio::FALLBACK_MIN_RMS {
        // VAD 一个区段都没检出、但整段有信号（音量过低 / 短句被 400ms
        // 最小段过滤）→ 不做静音过滤，按步骤时间戳整段送 ASR 兜底
        used_fallback = true;
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE] VAD found 0 regions but signal present (rms={:.5}) → fallback raw slicing",
            overall_rms
        ));
        sliced = audio::fallback_slice_all(&buffer.pcm, sample_rate, &offsets);
    }
    drop(buffer);

    if sliced.is_empty() && precomputed.is_none() {
        let summary = if overall_rms <= audio::FALLBACK_MIN_RMS {
            format!(
                "录音 {} 分钟全程接近静音：麦克风没有拾到声音，请检查系统麦克风设备选择与输入音量",
                (audio_ms as f64 / 60_000.0 * 10.0).round() / 10.0
            )
        } else {
            "录音时长过短，未检测到语音内容".to_string()
        };
        return Ok(VoiceDoneEvent {
            recording_id: recording_id.to_string(),
            transcribed: 0,
            failed: 0,
            ok: false,
            summary,
        });
    }

    // ── 4. 逐步骤转写并写库（V6/V9/V11）──────────────────────
    // 热词：内置 + 用户自定义（设置 → 录制设置），锚定口音近音错字。
    let custom_hotwords = read_custom_hotwords(&conn);
    let hotwords = asr::merge_hotwords(&custom_hotwords);
    crate::ai::commands::ai_dbg_log(&format!(
        "[VOICE] hotwords: {} custom + {} builtin = {} total",
        custom_hotwords.len(),
        asr::BUILTIN_HOTWORDS.len(),
        hotwords.len()
    ));
    let mut transcribed = 0usize;
    let mut failed = 0usize;
    let mut attempted = 0usize; // 实际送 ASR 的段数（用于区分「没送」和「送了但空」）
    let mut errors: Vec<String> = Vec::new();
    // 首次成功转写前压全局撤销栈（懒压：一条都没转出来就不制造无意义快照）
    let mut hist_pushed = false;
    // 跨段上下文：把最近一段的成功转写结果作为 prompt 传给下一段，
    // 提升段间连贯性（段是按 28s/步骤切的，独立识别会丢语境）。
    let mut last_text: Option<String> = None;
    // AI 纠错目标（方案③）：本转写回合写入了「【语音】」文本的步骤。
    let mut correction_targets: Vec<super::correct::CorrectionTarget> = Vec::new();

    // ★ 实时字幕产物优先：直接用已转好的句子，跳过整段 ASR（所见即所得）
    let mut final_texts: Vec<(usize, String)> = Vec::new();
    if let Some(pairs) = precomputed {
        for (idx, text) in pairs {
            if !text.trim().is_empty() {
                final_texts.push((idx, text));
            }
        }
        attempted = final_texts.len();
    } else {
    for (step_idx, chunks) in &sliced {
        let (step_id, desc, _) = &steps[*step_idx];
        let mut texts: Vec<String> = Vec::new();
        for chunk in chunks {
            attempted += 1;
            let wav = audio::pcm_to_wav(chunk, sample_rate);
            match asr::transcribe_wav_auto(
                &asr_key,
                wav,
                &hotwords,
                asr::segment_prompt(last_text.as_deref()).as_deref(),
            ) {
                Ok(text) if !text.is_empty() => {
                    last_text = Some(text.clone());
                    texts.push(text);
                }
                Ok(_) => {
                    // 识别为空（杂音/无人声）→ 记录但不计失败
                    crate::ai::commands::ai_dbg_log(&format!(
                        "[VOICE] step {} (order {}) ASR returned empty text (chunk {}/{})",
                        step_id,
                        step_idx,
                        attempted,
                        chunks.len()
                    ));
                }
                Err(e) => {
                    failed += 1;
                    errors.push(e.clone());
                    crate::ai::commands::ai_dbg_log(&format!(
                        "[VOICE] step {} (order {}) ASR failed: {}",
                        step_id, step_idx, e
                    ));
                    break; // 同一步骤后续段大概率同样失败，省时间
                }
            }
        }
        if texts.is_empty() {
            continue;
        }
        let joined = texts.join("").trim().to_string();
        if joined.is_empty() {
            continue;
        }
        final_texts.push((*step_idx, joined));
    }
    }

    for (step_idx, joined) in &final_texts {
        let (step_id, desc, _) = &steps[*step_idx];
        if !hist_pushed {
            let _ = crate::editor::commands::push_edit_history(&conn, &recording_id);
            hist_pushed = true;
        }
        // ★ 2026-09-06: 不再加「【语音】」前缀——说的话就是步骤说明本身（对齐 Scribe）。
        //   存量教程里已写入的旧前缀保持原样，不做迁移。
        let new_desc = if desc.trim().is_empty() {
            joined.clone()
        } else {
            format!("{}\n\n{}", desc, joined)
        };
        if let Err(e) = conn.execute(
            "UPDATE steps SET description = ?1 WHERE id = ?2",
            params!(new_desc, step_id),
        ) {
            failed += 1;
            errors.push(format!("写库失败：{}", e));
        } else {
            transcribed += 1;
            correction_targets.push(super::correct::CorrectionTarget {
                step_id: step_id.clone(),
                desc_prefix: desc.clone(),
                voice_text: joined.clone(),
            });
        }
    }

    // ── 5. AI 纠错（方案③，可关）────────────────────────────
    // 视觉 LLM 对照该步骤截图校对口音近音错字；失败/守卫拒绝一律保留原文。
    let mut ai_fixed = 0usize;
    if !correction_targets.is_empty() && super::correct::correction_enabled(&conn) {
        // 纠错要发带截图的视觉请求：第三方服务商回落内置免费视觉通道（同 ASR 策略）
        let corr_config =
            if super::correct::should_use_builtin_for_correction(&config) {
                crate::ai::commands::get_builtin_free_config()
            } else {
                config.clone()
            };
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE][AI-CORRECT] {} targets (cap {}), model={}, channel={}",
            correction_targets.len(),
            super::correct::CORRECTION_MAX_STEPS,
            corr_config.provider.model,
            if super::correct::should_use_builtin_for_correction(&config) { "内置免费通道" } else { "用户配置" }
        ));
        let (fixed, corr_errors) =
            super::correct::correct_voice_steps(&conn, &corr_config, &correction_targets);
        ai_fixed = fixed;
        for e in corr_errors {
            crate::ai::commands::ai_dbg_log(&format!("[VOICE][AI-CORRECT] failed: {}", e));
        }
        crate::ai::commands::ai_dbg_log(&format!(
            "[VOICE][AI-CORRECT] done: {} fixed",
            ai_fixed
        ));
    }

    // 汇总：0 转写 0 失败 → 不是错误，只是没识别到人声（可能是纯环境音/太轻）
    let summary = if transcribed == 0 && failed == 0 {
        if attempted == 0 {
            "录音时长过短，未检测到语音内容".to_string()
        } else {
            "未识别到清晰人声，已跳过语音补全".to_string()
        }
    } else {
        summarize_voice_result(transcribed, failed, &errors)
    };
    let summary = if ai_fixed > 0 {
        format!("{}，AI 已结合截图修正 {} 处疑似口音错字", summary, ai_fixed)
    } else {
        summary
    };
    let ok = failed == 0; // 只要没有 ASR 报错就算成功（无人声 ≠ 错误）
    crate::ai::commands::ai_dbg_log(&format!(
        "[VOICE] result {}: transcribed={}, failed={}, attempted={}, fallback={}",
        recording_id, transcribed, failed, attempted, used_fallback
    ));
    Ok(VoiceDoneEvent {
        recording_id: recording_id.to_string(),
        transcribed,
        failed,
        ok,
        summary,
    })
}

/// 聚合转写结果为面向用户的提示语（与 AI 生成的 summarize 风格一致）。
fn summarize_voice_result(transcribed: usize, failed: usize, errors: &[String]) -> String {
    let reason_part = if errors.is_empty() {
        String::new()
    } else {
        format!("。原因：{}", summarize_error_reasons(errors))
    };
    if transcribed == 0 && failed == 0 {
        "未检测到语音内容".to_string()
    } else if transcribed == 0 {
        format!("语音转写失败（{} 段）{}", failed, reason_part)
    } else if failed > 0 {
        format!(
            "语音转写完成：{} 个步骤已补充语音说明（{} 个失败）{}",
            transcribed, failed, reason_part
        )
    } else {
        format!("语音转写完成：{} 个步骤已补充语音说明", transcribed)
    }
}

/// 去重聚合错误原因（最多 3 条，重复原因带 ×N）。
fn summarize_error_reasons(errors: &[String]) -> String {
    use std::collections::HashMap as StdHashMap;
    let mut counts: StdHashMap<String, usize> = StdHashMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in errors {
        let key = e.trim().to_string();
        if key.is_empty() {
            continue;
        }
        if !counts.contains_key(&key) {
            order.push(key.clone());
        }
        *counts.entry(key).or_insert(0) += 1;
    }
    let parts: Vec<String> = order
        .into_iter()
        .take(3)
        .map(|m| {
            let c = counts.get(&m).copied().unwrap_or(1);
            if c > 1 {
                format!("{}（×{}）", m, c)
            } else {
                m
            }
        })
        .collect();
    let detail = parts.join("；");
    if detail.chars().count() > 160 {
        format!("{}...", detail.chars().take(157).collect::<String>())
    } else {
        detail
    }
}

#[cfg(test)]
mod voice_commands_tests {
    use super::*;

    fn feed(cur: &mut LiveCursor, ms: i64, rms: f32, pcm_len: usize, sr: u32) -> Option<(usize, usize, i64)> {
        live_vad_update(cur, ms, rms, pcm_len, sr)
    }

    /// 说完一句（静音 ≥600ms）才吐句；开口前的静音不算进句子。
    #[test]
    fn live_vad_emits_after_sentence_end_silence() {
        let sr = 16_000;
        let mut cur = LiveCursor::default();
        // 开口前 500ms 静音：起点应随之前移，不吐句
        assert!(feed(&mut cur, 500, 0.0, 0, sr).is_none());
        assert_eq!(cur.start_byte, 0);
        // 说 2s（每块 100ms）
        let mut len = 0usize;
        for _ in 0..20 {
            len += (sr as usize * 100 / 1000) * 2;
            assert!(feed(&mut cur, 100, 0.05, len, sr).is_none());
        }
        // 静音 300ms：还不够（<600ms）
        for _ in 0..3 {
            len += (sr as usize * 100 / 1000) * 2;
            assert!(feed(&mut cur, 100, 0.0, len, sr).is_none());
        }
        // 再静音 300ms（累计 600ms）→ 吐句
        len += (sr as usize * 300 / 1000) * 2;
        let hit = feed(&mut cur, 300, 0.0, len, sr).expect("静音满 600ms 应吐句");
        assert_eq!(hit.1, len, "句尾 = 当前 PCM 末尾");
        assert_eq!(cur.start_byte, len, "吐句后游标应重置到末尾");
    }

    /// 短句（<1.5s）不吐，避免碎片请求拖低识别质量。
    #[test]
    fn live_vad_skips_too_short_utterance() {
        let sr = 16_000;
        let mut cur = LiveCursor::default();
        let mut len = 0usize;
        for _ in 0..8 {
            len += (sr as usize * 100 / 1000) * 2;
            assert!(feed(&mut cur, 100, 0.05, len, sr).is_none());
        }
        assert!(cur.speech_ms < LIVE_MIN_SPEECH_MS);
        len += (sr as usize * 700 / 1000) * 2;
        assert!(feed(&mut cur, 700, 0.0, len, sr).is_none(), "短句不应吐出");
    }

    /// 归属规则：操作前说的话 → 归这个操作（起点之后第一个步骤）。
    #[test]
    fn assign_live_segments_uses_next_step_after_start() {
        let offsets = vec![0, 3_000, 6_000];
        let segs = vec![
            LiveSegment { start_ms: 1_000, duration_ms: 1_000, text: "点这里".into() },
            LiveSegment { start_ms: 3_500, duration_ms: 1_000, text: "再点那里".into() },
            LiveSegment { start_ms: 9_000, duration_ms: 1_000, text: "最后的话".into() },
        ];
        let out = assign_live_segments(&offsets, &segs);
        let got: Vec<(usize, String)> = out;
        // 第 2 句归步骤 2，第 3 句（最后一步之后说的）也归步骤 2 → 合并成一条
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (1, "点这里".to_string()));
        assert_eq!(got[1], (2, "再点那里最后的话".to_string()));
    }

    /// 同一步骤的多句要合并成一条文本。
    #[test]
    fn assign_live_segments_merges_same_step() {
        let offsets = vec![1_000, 5_000];
        let segs = vec![
            LiveSegment { start_ms: 100, duration_ms: 200, text: "先".into() },
            LiveSegment { start_ms: 600, duration_ms: 200, text: "后".into() },
        ];
        let out = assign_live_segments(&offsets, &segs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (0, "先后".to_string()));
    }

    /// 事件载荷必须 camelCase：前端按 `recordingId` 匹配，蛇形会让
    /// 转写完成后编辑器不刷新、toast 不弹（与「语音没生效」同表现）。
    #[test]
    fn voice_done_event_is_camel_case() {
        let ev = VoiceDoneEvent {
            recording_id: "rec-1".into(),
            transcribed: 1,
            failed: 0,
            ok: true,
            summary: "ok".into(),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert!(v.get("recordingId").is_some(), "必须输出 recordingId，实际：{}", v);
        assert!(v.get("recording_id").is_none());
    }

    #[test]
    fn summarize_all_transcribed() {
        assert_eq!(
            summarize_voice_result(5, 0, &[]),
            "语音转写完成：5 个步骤已补充语音说明"
        );
    }

    #[test]
    fn summarize_no_speech() {
        assert_eq!(summarize_voice_result(0, 0, &[]), "未检测到语音内容");
    }

    #[test]
    fn summarize_all_failed_with_dedup_reasons() {
        let s = summarize_voice_result(
            0,
            3,
            &vec!["HTTP 429 该模型当前访问量过大".to_string(); 3],
        );
        assert_eq!(
            s,
            "语音转写失败（3 段）。原因：HTTP 429 该模型当前访问量过大（×3）"
        );
    }

    #[test]
    fn summarize_partial_failure() {
        let s = summarize_voice_result(4, 1, &["网络超时".into()]);
        assert_eq!(
            s,
            "语音转写完成：4 个步骤已补充语音说明（1 个失败）。原因：网络超时"
        );
    }

    /// 🔴 回归防线：用户把默认 AI 服务商设成第三方（千问/DeepSeek）后，
    /// 语音不能跟着失效 —— 必须回落内置免费通道 Key，而不是拒绝服务。
    #[test]
    fn resolve_asr_key_falls_back_to_builtin_for_third_party_provider() {
        use crate::ai::types::{AiConfig, AiProvider};

        let third_party = AiConfig {
            provider: AiProvider {
                id: "custom_1788373546428".into(),
                name: "qwen3.7-plus".into(),
                base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1".into(),
                model: "qwen3.7-plus".into(),
            },
            api_key: "sk-third-party-key".into(),
        };
        let (key, channel) = resolve_asr_key(&third_party);
        assert_eq!(channel, "内置免费通道");
        // 绝不能拿第三方的 key 去打智谱端点（必然 401）
        assert_ne!(key, "sk-third-party-key");
        assert!(!key.trim().is_empty());
    }

    /// 用户自配智谱时应当优先用他自己的 Key（省内置额度、也更可控）。
    #[test]
    fn resolve_asr_key_prefers_user_zhipu_key() {
        use crate::ai::types::{AiConfig, AiProvider};

        let mine = AiConfig {
            provider: AiProvider {
                id: "zhipu".into(),
                name: "我的智谱".into(),
                base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
                model: "glm-4.6v-flash".into(),
            },
            api_key: "sk-my-zhipu".into(),
        };
        let (key, channel) = resolve_asr_key(&mine);
        assert_eq!(channel, "用户配置");
        assert_eq!(key, "sk-my-zhipu");
    }

    /// 智谱系但 Key 为空时也要能兜底（不能拿着空 Key 去请求）。
    #[test]
    fn resolve_asr_key_falls_back_when_zhipu_key_empty() {
        use crate::ai::types::{AiConfig, AiProvider};

        let empty = AiConfig {
            provider: AiProvider {
                id: "flowio-free".into(),
                name: "免费".into(),
                base_url: "https://open.bigmodel.cn/api/paas/v4/chat/completions".into(),
                model: "glm-4.6v-flash".into(),
            },
            api_key: "".into(),
        };
        let (key, channel) = resolve_asr_key(&empty);
        assert_eq!(channel, "内置免费通道");
        assert!(!key.trim().is_empty());
    }

    #[test]
    fn summarize_reasons_cap_at_three_and_160_chars() {
        let errs: Vec<String> = (0..5).map(|i| format!("原因{}", i)).collect();
        let s = summarize_voice_result(0, 5, &errs);
        assert!(s.contains("原因0") && s.contains("原因2") && !s.contains("原因3"));
        let long: Vec<String> = vec!["长".repeat(100); 2];
        let s2 = summarize_voice_result(0, 2, &long);
        assert!(s2.chars().count() <= 200);
    }
}
