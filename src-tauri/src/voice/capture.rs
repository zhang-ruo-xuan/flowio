//! 麦克风采集（cpal：Windows 走 WASAPI，macOS 走 CoreAudio）。
//!
//! ## 为什么不用浏览器的 getUserMedia + AudioContext
//! 录制一开始主窗口就会 `minimize()`（把屏幕让给被录的应用），WebView2 随即
//! 停掉指向真实输出设备的音频渲染线程 —— ScriptProcessorNode 的
//! `onaudioprocess` 跟着停（实测只采到最小化前的 512ms），这就是
//! 「一直在说但检测不到」的根因。
//! 试过 `AudioContext({ sinkId: "none" })` 让上下文不挂输出设备，结果更糟：
//! null sink 没有设备时钟驱动渲染，**一次回调都没跑**（样本数全程 0）。
//! 结论：采集不能寄生在渲染进程里，搬到 Rust 侧。
//!
//! ## 代价
//! 丢掉 getUserMedia 的 echoCancellation / noiseSuppression（浏览器侧信号
//! 处理），麦克风会拾到喇叭外放的声音。戴耳机或调低外放音量即可。
//!
//! ## 线程模型
//! ```text
//! cpal 音频回调（高优先级，不碰锁/IO）
//!   → 降混单声道 → 重采样到 16kHz → i16
//!   → mpsc::channel（无界，不丢）
//!   → 采集线程（每 50ms 排空一次）→ VoiceCaptureState 的 VoiceBuffer
//! ```
//! cpal 的 `Stream` 由采集线程持有（drop 即停止），不进 Tauri State。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SampleRate, Stream, StreamConfig, SupportedStreamConfig};
use tauri::State;

use super::audio;
use super::commands::{VoiceBuffer, VoiceCaptureState};
use crate::DbState;

/// 采集线程收到的控制命令。
enum MicCmd {
    /// 停止采集并退出线程
    Stop,
    /// 暂停 / 恢复（暂停期间丢弃 PCM，恢复时补等长静音）
    SetPaused(bool),
}

/// 已启动的采集句柄（进 Tauri State，需要 Send + Sync，所以要 pub）。
///
/// 注意：`cpal::Stream` 不在这里 —— 它由采集线程持有，线程退出即释放。
pub struct MicHandle {
    cmd_tx: Mutex<Sender<MicCmd>>,
    done_rx: Mutex<Receiver<()>>,
    /// 已采集样本数（看门狗 / 停止时统计时长）
    samples: Arc<AtomicU64>,
}

/// 全局麦克风采集状态（同时只允许一路）。
#[derive(Default)]
pub struct MicCaptureState(pub Mutex<Option<MicHandle>>);

/// 采集排空周期：50ms 足够，通道无界不会丢数据。
const DRAIN_INTERVAL: Duration = Duration::from_millis(50);
/// 启动握手超时（枚举设备 + 打开 WASAPI 流偶尔要几秒）
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// 开始采集并建立该录制的缓冲。
///
/// 返回实际采集起点（epoch ms），由 Rust 在流真正 `play()` 的瞬间取，
/// 与步骤的 `timestamp`（epoch ms）同源。
#[tauri::command]
pub fn voice_mic_start(
    _app: tauri::AppHandle,
    _db: State<'_, DbState>,
    state: State<'_, MicCaptureState>,
    buffers: State<'_, VoiceCaptureState>,
    recording_id: String,
) -> Result<i64, String> {
    {
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err("麦克风已在采集中".to_string());
        }
    }
    let buffers = buffers.inner().clone();

    let (init_tx, init_rx) = mpsc::channel::<Result<i64, String>>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<MicCmd>();
    let (pcm_tx, pcm_rx) = mpsc::channel::<Vec<i16>>();
    let (done_tx, done_rx) = mpsc::channel::<()>();

    let paused = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(AtomicU64::new(0));

    let t_paused = Arc::clone(&paused);
    let t_samples = Arc::clone(&samples);
    thread::spawn(move || {
        let _com = ComGuard::init();
        run_capture(
            buffers,
            recording_id,
            t_paused,
            t_samples,
            pcm_tx,
            pcm_rx,
            cmd_rx,
            init_tx,
            done_tx,
        );
    });

    let started_at = init_rx
        .recv_timeout(START_TIMEOUT)
        .map_err(|_| "麦克风启动超时（15 秒内没有响应）".to_string())??;

    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    *guard = Some(MicHandle {
        cmd_tx: Mutex::new(cmd_tx),
        done_rx: Mutex::new(done_rx),
        samples,
    });
    Ok(started_at)
}

/// 暂停 / 恢复采集。暂停期间丢弃 PCM（不写入缓冲）。
#[tauri::command]
pub fn voice_mic_set_paused(state: State<'_, MicCaptureState>, paused: bool) -> Result<(), String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(h) = guard.as_ref() {
        if let Ok(tx) = h.cmd_tx.lock() {
            let _ = tx.send(MicCmd::SetPaused(paused));
        }
    }
    Ok(())
}

/// 已采集样本数（前端看门狗用：连续不变 = 采集停摆）。
#[tauri::command]
pub fn voice_mic_samples(state: State<'_, MicCaptureState>) -> Result<u64, String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    Ok(guard
        .as_ref()
        .map(|h| h.samples.load(Ordering::Relaxed))
        .unwrap_or(0))
}

/// 停止采集。会等采集线程把管道里剩余 PCM 全部落进缓冲后才返回。
#[tauri::command]
pub fn voice_mic_stop(state: State<'_, MicCaptureState>) -> Result<u64, String> {
    let handle = {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        guard.take()
    };
    let Some(h) = handle else {
        return Ok(0);
    };
    if let Ok(tx) = h.cmd_tx.lock() {
        let _ = tx.send(MicCmd::Stop);
    }
    // 等线程排空收尾，避免最后几百毫秒的音频丢失
    if let Ok(rx) = h.done_rx.lock() {
        let _ = rx.recv_timeout(Duration::from_secs(3));
    }
    Ok(h.samples.load(Ordering::Relaxed))
}

/// 采集线程主体：建流 → play → 排空 PCM + 处理命令 → 退出。
#[allow(clippy::too_many_arguments)]
fn run_capture(
    buffers: VoiceCaptureState,
    recording_id: String,
    paused: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    pcm_tx: Sender<Vec<i16>>,
    pcm_rx: Receiver<Vec<i16>>,
    cmd_rx: Receiver<MicCmd>,
    init_tx: Sender<Result<i64, String>>,
    done_tx: Sender<()>,
) {
    let gain_bits = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let setup = build_stream_for_default_device(
        Arc::clone(&paused),
        Arc::clone(&samples),
        Arc::clone(&gain_bits),
        pcm_tx,
    );
    let (stream, info) = match setup {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            let _ = done_tx.send(());
            return;
        }
    };

    // 先建缓冲再 play：回调可能在 play 返回前就投递数据
    {
        if let Ok(mut map) = buffers.0.lock() {
            map.insert(
                recording_id.clone(),
                VoiceBuffer {
                    started_at_ms: chrono::Utc::now().timestamp_millis(),
                    sample_rate: audio::SAMPLE_RATE,
                    pcm: Vec::new(),
                    last_heartbeat_len: 0,
                },
            );
        }
    }
    if let Err(e) = stream.play() {
        if let Ok(mut map) = buffers.0.lock() {
            map.remove(&recording_id);
        }
        let _ = init_tx.send(Err(format!("启动麦克风失败: {}", e)));
        let _ = done_tx.send(());
        return;
    }
    let started_at = chrono::Utc::now().timestamp_millis();
    crate::ai::commands::ai_dbg_log(&format!(
        "[VOICE] mic started: device={} dev_rate={} ch={} fmt={:?} -> {}Hz mono i16",
        info.name, info.dev_rate, info.channels, info.format, audio::SAMPLE_RATE
    ));
    let _ = init_tx.send(Ok(started_at));

    let mut paused_since: Option<Instant> = None;
    loop {
        // 1) 排空已采集的 PCM
        while let Ok(chunk) = pcm_rx.try_recv() {
            append_pcm(&buffers, &recording_id, &chunk);
        }
        // 2) 处理命令（50ms 超时后回到 1）
        match cmd_rx.recv_timeout(DRAIN_INTERVAL) {
            Ok(MicCmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(MicCmd::SetPaused(true)) => {
                if paused_since.is_none() {
                    paused_since = Some(Instant::now());
                }
                paused.store(true, Ordering::Relaxed);
            }
            Ok(MicCmd::SetPaused(false)) => {
                // 补等长静音：直接丢 PCM 会让缓冲比墙钟短，步骤偏移会整体前移
                if let Some(t) = paused_since.take() {
                    let ms = t.elapsed().as_millis() as u64;
                    let n = (ms * audio::SAMPLE_RATE as u64 / 1000) as usize;
                    if n > 0 {
                        append_pcm(&buffers, &recording_id, &vec![0i16; n]);
                    }
                }
                paused.store(false, Ordering::Relaxed);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    // 收尾：排空剩余
    while let Ok(chunk) = pcm_rx.try_recv() {
        append_pcm(&buffers, &recording_id, &chunk);
    }
    let final_gain = f32::from_bits(gain_bits.load(Ordering::Relaxed));
    crate::ai::commands::ai_dbg_log(&format!(
        "[VOICE] mic stopped: final AGC gain=×{:.1}（远低于 ×4 说明麦克风输入电平偏低，建议戴耳机或调大系统麦克风电平）",
        final_gain
    ));
    drop(stream);
    let _ = done_tx.send(());
}

/// 实际选中的设备参数（只用于日志）
struct StreamInfo {
    name: String,
    dev_rate: u32,
    channels: u16,
    format: SampleFormat,
}

/// 用系统默认麦克风建一条输入流。
fn build_stream_for_default_device(
    paused: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    gain_bits: Arc<AtomicU32>,
    tx: Sender<Vec<i16>>,
) -> Result<(Stream, StreamInfo), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "系统里没有可用的麦克风（没有默认输入设备）".to_string())?;
    let name = device.name().unwrap_or_else(|_| "(未知设备)".to_string());
    let cfg = pick_input_config(&device)?;
    let info = StreamInfo {
        name: name.clone(),
        dev_rate: cfg.sample_rate().0,
        channels: cfg.channels(),
        format: cfg.sample_format(),
    };
    match build_stream(
        &device,
        &cfg,
        Arc::clone(&paused),
        Arc::clone(&samples),
        Arc::clone(&gain_bits),
        tx.clone(),
    ) {
        Ok(stream) => Ok((stream, info)),
        Err(first_err) => {
            // 有的声卡在枚举里声称支持 16kHz，真正建流却失败（尤其 USB 耳机）。
            // 退回设备默认格式，靠回调里的重采样补到 16kHz，别让麦克风直接开不了。
            let fallback = device
                .default_input_config()
                .map_err(|e| format!("{}（退回设备默认格式也失败：{}）", first_err, e))?;
            if fallback.sample_rate() == cfg.sample_rate()
                && fallback.sample_format() == cfg.sample_format()
            {
                // 默认配置跟刚才失败的是同一个，重试没意义
                return Err(first_err);
            }
            crate::ai::commands::ai_dbg_log(&format!(
                "[VOICE] 16kHz 建流失败，退回默认格式 {}Hz/{:?}：{}",
                fallback.sample_rate().0,
                fallback.sample_format(),
                first_err
            ));
            let info = StreamInfo {
                name,
                dev_rate: fallback.sample_rate().0,
                channels: fallback.channels(),
                format: fallback.sample_format(),
            };
            let stream = build_stream(&device, &fallback, paused, samples, gain_bits, tx)?;
            Ok((stream, info))
        }
    }
}

/// 挑一个能用的输入配置：优先 16kHz 原生，否则用设备默认配置（回调里重采样）。
fn pick_input_config(device: &Device) -> Result<SupportedStreamConfig, String> {
    let wanted = SampleRate(audio::SAMPLE_RATE);
    match device.supported_input_configs() {
        Ok(mut ranges) => {
            if let Some(range) = ranges.find(|r| {
                r.channels() >= 1 && r.min_sample_rate() <= wanted && wanted <= r.max_sample_rate()
            }) {
                return Ok(range.with_sample_rate(wanted));
            }
        }
        Err(e) => {
            crate::ai::commands::ai_dbg_log(&format!(
                "[VOICE] supported_input_configs 查询失败（用默认配置兜底）: {}",
                e
            ));
        }
    }
    device
        .default_input_config()
        .map_err(|e| format!("取不到麦克风默认格式: {}", e))
}

/// 建 cpal 输入流。回调只做纯计算 + 无锁 channel 投递，不碰 Mutex / IO。
fn build_stream(
    device: &Device,
    cfg: &SupportedStreamConfig,
    paused: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    gain_bits: Arc<AtomicU32>,
    tx: Sender<Vec<i16>>,
) -> Result<Stream, String> {
    let sc: StreamConfig = cfg.config();
    let channels = cfg.channels() as usize;
    let dev_rate = cfg.sample_rate().0;
    let fmt = cfg.sample_format();
    let err_fn = move |e: cpal::StreamError| {
        crate::ai::commands::ai_dbg_log(&format!("[VOICE] cpal 采集流错误: {}", e));
    };

    match fmt {
        SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
            &sc,
            make_callback::<f32>(channels, dev_rate, |s| s, Arc::clone(&paused), Arc::clone(&samples), Arc::clone(&gain_bits), tx),
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
            &sc,
            make_callback::<i16>(channels, dev_rate, |s| s as f32 / 32768.0, Arc::clone(&paused), Arc::clone(&samples), Arc::clone(&gain_bits), tx),
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
            &sc,
            make_callback::<u16>(channels, dev_rate, |s| (s as f32 - 32768.0) / 32768.0, Arc::clone(&paused), Arc::clone(&samples), Arc::clone(&gain_bits), tx),
            err_fn,
            None,
        ),
        other => return Err(format!("麦克风的采样格式 {:?} 暂不支持", other)),
    }
    .map_err(|e| format!("打开麦克风失败（可能被系统权限或其他程序占用）: {}", e))
}

/// 软件自动增益（AGC）。
///
/// 为什么需要：浏览器 getUserMedia 默认自带 AGC / 降噪，换 cpal 裸采后
/// 这层全没了 —— 低电平麦克风（实测整体 RMS 0.004 上下，贴着静音阈值
/// SILENCE_RMS=0.0045）会让 VAD 判不出说话区段，ASR 收到的是「耳语级」
/// 信号，识别质量极差。这里用慢速 EMA 跟踪语音电平，把增益朝目标缓推：
/// 只放大不压低（gain ≥ 1），限幅防爆音，快攻慢放避免把静音段也放大。
pub struct Agc {
    gain: f32, // 当前增益（1.0 = 不放大）
    level: f32, // 信号 RMS 的慢速滑动平均
}

impl Agc {
    const TARGET_RMS: f32 = 0.06; // 目标语音电平（约 -24 dBFS，ASR 舒适区）
    const MAX_GAIN: f32 = 16.0; // 上限 +24dB，再大主要在放大底噪
    const MIN_LEVEL: f32 = 1e-4; // 低于此视为纯静音，不参与电平估计

    pub fn new() -> Self {
        Self { gain: 1.0, level: 0.0 }
    }

    /// 对一块 16kHz 单声道 i16 样本就地增益，并更新电平估计与增益。
    pub fn process(&mut self, buf: &mut [i16]) {
        if buf.is_empty() {
            return;
        }
        let sum: f64 = buf.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
        let rms = (sum / buf.len() as f64).sqrt() as f32;
        if rms > Self::MIN_LEVEL {
            // 快攻慢放：语音到来快速跟上电平，静音段缓慢回落
            let alpha = if rms > self.level { 0.15 } else { 0.01 };
            self.level += (rms - self.level) * alpha;
        }
        let want = if self.level > Self::MIN_LEVEL {
            (Self::TARGET_RMS / self.level).clamp(1.0, Self::MAX_GAIN)
        } else {
            Self::MAX_GAIN
        };
        self.gain += (want - self.gain) * 0.02; // 增益缓变，避免呼吸声/泵感
        for s in buf.iter_mut() {
            *s = (*s as f32 * self.gain).clamp(-32767.0, 32767.0) as i16;
        }
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }
}

/// 一阶高通滤波器（滤除风扇、空调等低频背景噪音）。
///
/// 截止频率约 80Hz（人声基频通常 > 85Hz），用最简 IIR 实现：
///   y[n] = α * (y[n-1] + x[n] - x[n-1])
struct HighPassFilter {
    prev_input: f32,
    prev_output: f32,
    alpha: f32, // = rc / (rc + dt)，越接近 1 截止越低
}

impl HighPassFilter {
    /// 新建：cutoff_hz 为截止频率，sample_rate 为采样率
    fn new(cutoff_hz: f32, sample_rate: u32) -> Self {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        let dt = 1.0 / sample_rate.max(1) as f32;
        let alpha = rc / (rc + dt);
        Self { prev_input: 0.0, prev_output: 0.0, alpha }
    }

    /// 处理单个样本，返回滤波后值
    fn process(&mut self, input: f32) -> f32 {
        let output = self.alpha * (self.prev_output + input - self.prev_input);
        self.prev_input = input;
        self.prev_output = output;
        output
    }
}

/// 噪声门：当信号能量低于阈值时直接归零，只放过「有人声」的段。
///
/// 用滑动窗口 RMS 检测，低于 gate_threshold 时输出静音；高于时原样通过。
/// 窗口 10ms（160 样本 @16kHz），避免切掉语音起首的弱辅音。
pub struct NoiseGate {
    buf: Vec<f32>,      // 滑动窗口
    gate_threshold: f32, // RMS 阈值（低于此 → 静音）
    window_size: usize,
}

impl NoiseGate {
    /// threshold_rms：RMS 阈值，建议 0.002~0.005（介于 FALLBACK_MIN_RMS 和 SILENCE_RMS 之间）
    pub fn new(threshold_rms: f32, sample_rate: u32) -> Self {
        let ws = ((sample_rate as f64) * 0.01).max(32.0) as usize; // 10ms 窗口
        Self { buf: Vec::with_capacity(ws), gate_threshold: threshold_rms, window_size: ws }
    }

    /// 处理一个样本：窗口内 RMS 低于阈值 → 返回 0；否则返回原值
    pub fn process(&mut self, sample: f32) -> f32 {
        self.buf.push(sample);
        if self.buf.len() > self.window_size {
            self.buf.remove(0);
        }
        if self.buf.len() < self.window_size {
            return sample; // 窗口未填满，先放行
        }
        let sum_sq: f32 = self.buf.iter().map(|&x| x * x).sum();
        let rms = (sum_sq / self.window_size as f32).sqrt();
        if rms < self.gate_threshold { 0.0 } else { sample }
    }
}

/// 造一个 cpal 数据回调：降混 → 重采样 → AGC → i16 → 投进 channel。
fn make_callback<T>(
    channels: usize,
    dev_rate: u32,
    to_f32: fn(T) -> f32,
    paused: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    gain_bits: Arc<AtomicU32>,
    tx: Sender<Vec<i16>>,
) -> impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static
where
    T: Copy + Send + 'static,
{
    let mut res = Resampler::new(dev_rate, audio::SAMPLE_RATE);
    let mut agc = Agc::new();
    let mut hpf = HighPassFilter::new(120.0, audio::SAMPLE_RATE); // 120Hz 高通滤除风扇/空调低频噪音
    let mut gate = NoiseGate::new(0.003, audio::SAMPLE_RATE);   // 噪声门阈值
    let mut mono: Vec<f32> = Vec::new();
    move |data: &[T], _: &cpal::InputCallbackInfo| {
        if paused.load(Ordering::Relaxed) {
            return;
        }
        mono.clear();
        downmix(data, channels, &mut mono, to_f32);
        let mut out: Vec<i16> = Vec::with_capacity(mono.len() + 8);
        res.push(&mono, &mut out);
        if out.is_empty() {
            return;
        }
        // 信号处理链：AGC → 高通滤波 → 噪声门 → i16
        agc.process(&mut out);
        // 高通滤波（在 i16 上做 f32 运算）
        for s in out.iter_mut() {
            let sf = *s as f32 / 32768.0;
            let filtered = hpf.process(sf);
            let gated = gate.process(filtered);
            *s = (gated.clamp(-1.0, 1.0) * 32767.0) as i16;
        }
        gain_bits.store(agc.gain().to_bits(), Ordering::Relaxed);
        samples.fetch_add(out.len() as u64, Ordering::Relaxed);
        let _ = tx.send(out);
    }
}

/// 把 PCM 落进该录制的缓冲（采集线程侧调用）。
fn append_pcm(state: &VoiceCaptureState, recording_id: &str, samples: &[i16]) {
    let Ok(mut map) = state.0.lock() else {
        return;
    };
    let Some(buf) = map.get_mut(recording_id) else {
        return;
    };
    let mut bytes: Vec<u8> = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    super::commands::append_pcm_bytes(buf, &bytes, recording_id);
}

/// 多声道降混为单声道（取各声道均值）。
fn downmix<T: Copy>(data: &[T], channels: usize, out: &mut Vec<f32>, to_f32: fn(T) -> f32) {
    if channels <= 1 {
        out.extend(data.iter().map(|&s| to_f32(s)));
        return;
    }
    for frame in data.chunks(channels) {
        let sum: f32 = frame.iter().map(|&s| to_f32(s)).sum();
        out.push(sum / frame.len() as f32);
    }
}

/// 线性插值重采样（设备采样率 → 16kHz）。ratio == 1 时直通。
///
/// 约定：输入样本 k 位于连续坐标 k，输出第 j 个样本取自坐标 `j * ratio`，
/// 由左右两个相邻输入样本线性插值得到。
/// 跨块边界用一个 `pending` 队列兜住「本块还没消费完」的样本，
/// 这样插值始终是标准的两点插值（不依赖上一块的尾样本做近似）。
pub struct Resampler {
    /// 每输出一个样本要前进的输入样本数（dev_rate / 16000）
    ratio: f64,
    /// 下一个输出样本在 `pending` 中的浮点位置
    pos: f64,
    /// 未消费的输入样本（跨块保留）
    pending: VecDeque<f32>,
}

impl Resampler {
    pub fn new(src_rate: u32, dst_rate: u32) -> Self {
        Self {
            ratio: src_rate as f64 / dst_rate.max(1) as f64,
            pos: 0.0,
            pending: VecDeque::new(),
        }
    }

    /// 输入一段单声道 f32，输出重采样后的 i16 PCM。
    pub fn push(&mut self, input: &[f32], out: &mut Vec<i16>) {
        if input.is_empty() {
            return;
        }
        self.pending.extend(input.iter().copied());
        if (self.ratio - 1.0).abs() < 1e-9 {
            // 同采样率：直通，不做插值
            for s in self.pending.drain(..) {
                out.push(to_i16(s));
            }
            self.pos = 0.0;
            return;
        }
        let n = self.pending.len();
        let mut i = self.pos;
        // 需要 i0 与 i0+1 两个点，所以要求 i + 1 < n
        while (i + 1.0) < n as f64 {
            let i0 = i.floor() as usize;
            let frac = (i - i0 as f64) as f32;
            let s0 = self.pending[i0];
            let s1 = self.pending[i0 + 1];
            out.push(to_i16(s0 + (s1 - s0) * frac));
            i += self.ratio;
        }
        // 丢掉已消费完的样本，并把位置换算到剩余队列
        let consumed = (i.floor() as usize).min(self.pending.len());
        if consumed > 0 {
            self.pending.drain(..consumed);
            i -= consumed as f64;
        }
        self.pos = i;
    }
}

fn to_i16(s: f32) -> i16 {
    let s = if s > 1.0 {
        1.0
    } else if s < -1.0 {
        -1.0
    } else {
        s
    };
    (if s < 0.0 { s * 32768.0 } else { s * 32767.0 }) as i16
}

/// Windows 下 WASAPI 需要 COM 初始化（新线程不继承主线程的 COM）。
struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        #[cfg(windows)]
        {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            // S_OK / S_FALSE（本线程已初始化过）都算成功
            return Self(hr.is_ok());
        }
        #[cfg(not(windows))]
        Self(false)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.0 {
            use windows::Win32::System::Com::CoUninitialize;
            unsafe { CoUninitialize() };
        }
        #[cfg(not(windows))]
        {
            let _ = self.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_passthrough_when_rates_match() {
        let mut r = Resampler::new(16_000, 16_000);
        let input = vec![0.0f32, 0.5, -0.5, 1.0];
        let mut out = Vec::new();
        r.push(&input, &mut out);
        assert_eq!(out, vec![0, 16_383, -16_384, 32_767]);
    }

    #[test]
    fn resampler_downsamples_by_two() {
        // 32kHz → 16kHz：步长 2，坐标落在整数位 → 插值结果等于该点本身
        let mut r = Resampler::new(32_000, 16_000);
        let input: Vec<f32> = (0..8).map(|i| i as f32 / 100.0).collect();
        let mut out = Vec::new();
        r.push(&input, &mut out);
        assert_eq!(out.len(), 4, "32k→16k 应减半，实际 {}", out.len());
        assert_eq!(out[0], to_i16(0.00));
        assert_eq!(out[1], to_i16(0.02));
        assert_eq!(out[2], to_i16(0.04));
    }

    #[test]
    fn resampler_upsamples_without_drift() {
        // 8kHz → 16kHz：10 秒输入应产出 20 秒（16 万样本）
        let mut r = Resampler::new(8_000, 16_000);
        let mut out = Vec::new();
        for _ in 0..10 {
            let block: Vec<f32> = (0..8000).map(|i| (i % 100) as f32 / 100.0).collect();
            r.push(&block, &mut out);
        }
        let expected = 160_000usize;
        let diff = (out.len() as i64 - expected as i64).abs();
        assert!(
            diff <= 4,
            "8k→16k 10 秒应得 ~{} 样本，实际 {}",
            expected,
            out.len()
        );
    }

    #[test]
    fn resampler_interpolates_midpoint() {
        // 24k→16k：步长 1.5，第二个输出落在坐标 1.5 → 取 p[1]、p[2] 的中点
        let mut r = Resampler::new(24_000, 16_000);
        let input = vec![0.0f32, 0.2, 0.6, 1.0];
        let mut out = Vec::new();
        r.push(&input, &mut out);
        assert_eq!(out.len(), 2, "24k→16k 4 个样本应得 2 个，实际 {}", out.len());
        assert_eq!(out[0], to_i16(0.0), "坐标 0 处");
        assert_eq!(out[1], to_i16(0.4), "坐标 1.5 处应为 0.2 与 0.6 的中点");
    }

    #[test]
    fn resampler_is_continuous_across_blocks() {
        // 同一段信号切成小块喂，结果应与整块喂一致（跨块不能丢/重样本）
        let signal: Vec<f32> = (0..6000).map(|i| (i % 97) as f32 / 97.0).collect();
        let mut whole = Resampler::new(48_000, 16_000);
        let mut a = Vec::new();
        whole.push(&signal, &mut a);

        let mut chunked = Resampler::new(48_000, 16_000);
        let mut b = Vec::new();
        for blk in signal.chunks(331) {
            chunked.push(blk, &mut b);
        }
        let diff = (a.len() as i64 - b.len() as i64).abs();
        assert!(diff <= 2, "分块与整块样本数应基本一致：{} vs {}", a.len(), b.len());
        assert_eq!(&a[..b.len().min(a.len())], &b[..b.len().min(a.len())]);
    }

    #[test]
    fn downmix_averages_channels() {
        let stereo = [1.0f32, 0.0, -0.5, 0.5];
        let mut out = Vec::new();
        downmix(&stereo, 2, &mut out, |s: f32| s);
        assert_eq!(out, vec![0.5, 0.0]);
    }

    /// 真机麦克风 smoke 测试（需要真实设备，默认跳过）。
    ///
    /// ```text
    /// cargo test --lib voice::capture::tests::mic_capture_smoke -- --ignored --nocapture
    /// ```
    /// 用来验证 cpal/WASAPI 这一层在本机确实能采到数据 —— 排查「采集停摆」
    /// 时先跑它，能一眼区分是驱动/设备问题还是业务链路问题。
    #[test]
    #[ignore = "需要真实麦克风，手动跑"]
    fn mic_capture_smoke() {
        let (tx, rx) = mpsc::channel::<Vec<i16>>();
        let paused = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(AtomicU64::new(0));
        let gain_bits = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let (stream, info) = build_stream_for_default_device(
            Arc::clone(&paused),
            Arc::clone(&samples),
            gain_bits,
            tx,
        )
        .expect("打开麦克风失败");
        println!(
            "设备={} 原始采样率={}Hz 声道={} 格式={:?}",
            info.name, info.dev_rate, info.channels, info.format
        );
        stream.play().expect("play 失败");

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = 0usize;
        let mut peak: i16 = 0;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => {
                    for &s in &chunk {
                        if s.abs() > peak.abs() {
                            peak = s;
                        }
                    }
                    got += chunk.len();
                }
                Err(_) => {}
            }
        }
        println!(
            "3 秒采集到 {} 样本（16kHz 下 = {:.2} 秒），峰值幅度 {}",
            got,
            got as f64 / 16_000.0,
            peak
        );
        assert!(
            got > 16_000,
            "3 秒应至少采到 1 秒的量（16000 样本），实际 {} —— 设备没送数据？",
            got
        );
    }

    // ── AGC ───────────────────────────────────────────────────

    fn sine_i16(n: usize, amplitude: f32) -> Vec<i16> {
        (0..n)
            .map(|i| ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 16_000.0).sin() * amplitude * 32767.0) as i16)
            .collect()
    }

    fn rms_i16(buf: &[i16]) -> f32 {
        let sum: f64 = buf.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
        (sum / buf.len().max(1) as f64).sqrt() as f32
    }

    #[test]
    fn agc_boosts_quiet_input_toward_target() {
        let mut agc = Agc::new();
        let in_rms = rms_i16(&sine_i16(1600, 0.008)); // 整体 RMS ≈ 0.0057 的安静输入
        let mut last = Vec::new();
        // 模拟真实流式：每轮回调送来「新鲜的、未被放大过」的原始样本
        for _ in 0..300 {
            let mut buf = sine_i16(1600, 0.008);
            agc.process(&mut buf);
            last = buf;
        }
        assert!(agc.gain() > 4.0, "低电平输入 3 秒后增益应 >4，实际 ×{:.2}", agc.gain());
        let out_rms = rms_i16(&last);
        assert!(
            out_rms > in_rms * 3.0,
            "输出应被显著放大：in {:.5} out {:.5}",
            in_rms,
            out_rms
        );
    }

    #[test]
    fn agc_never_attenuates_loud_input() {
        let mut agc = Agc::new();
        let in_rms = rms_i16(&sine_i16(1600, 0.5)); // 正常响度
        let mut last = Vec::new();
        for _ in 0..300 {
            let mut buf = sine_i16(1600, 0.5);
            agc.process(&mut buf);
            last = buf;
        }
        assert!(
            agc.gain() <= 1.05,
            "响亮输入不应再放大，实际 ×{:.2}",
            agc.gain()
        );
        assert!(
            (rms_i16(&last) - in_rms).abs() / in_rms < 0.05,
            "响亮输入应近似直通"
        );
    }

    #[test]
    fn agc_output_never_wraps() {
        let mut agc = Agc::new();
        // 先把增益推上去，再喂一个突发的响信号（削波场景）
        let mut quiet = sine_i16(1600, 0.005);
        for _ in 0..300 {
            agc.process(&mut quiet);
        }
        let mut loud = vec![32000i16; 1600];
        for _ in 0..50 {
            agc.process(&mut loud);
            assert!(
                loud.iter().all(|&s| s >= -32767 && s <= 32767),
                "增益后样本必须夹在 i16 范围内"
            );
        }
    }

    // ── 高通滤波器 ─────────────────────────────────────

    #[test]
    fn hpf_blocks_dc_offset() {
        let mut f = HighPassFilter::new(80.0, 16_000);
        // 直流偏移应被滤除
        let mut out = 0.0f32;
        for _ in 0..2000 {
            out = f.process(0.5); // 恒定直流
        }
        assert!(out.abs() < 0.01, "直流应被高通滤除，实际 {:.4}", out);
    }

    #[test]
    fn hpf_passes_sine_above_cutoff() {
        let mut f = HighPassFilter::new(120.0, 16_000);
        // 440Hz 正弦波应在通带（远高于 120Hz 截止）
        let mut sum_sq = 0.0f64;
        let mut n = 0u32;
        for i in 0..4000 {
            let x = ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 16_000.0).sin()) * 0.5;
            let y = f.process(x);
            if i > 500 { // 跳过建立期
                sum_sq += (y as f64).powi(2);
                n += 1;
            }
        }
        let rms = (sum_sq / n.max(1) as f64).sqrt();
        // 440Hz 在通带，输出 RMS 应 > 0.1（输入 RMS = 0.35）
        assert!(rms > 0.1, "440Hz 应通过高通滤波器，输出 RMS={:.5}", rms);
    }

    #[test]
    fn hpf_attenuates_low_frequency() {
        let mut f_low = HighPassFilter::new(120.0, 16_000);
        let mut f_high = HighPassFilter::new(120.0, 16_000);
        let mut rms_low = 0.0f64;
        let mut rms_high = 0.0f64;
        const N: usize = 4000;
        for i in 0..N {
            // 30Hz 低频（应在阻带）
            let x_low = ((i as f32 * 30.0 * 2.0 * std::f32::consts::PI / 16_000.0).sin()) * 0.5;
            let y_low = f_low.process(x_low);
            if i > 1000 { rms_low += (y_low as f64).powi(2); }

            // 440Hz 高频（应在通带）
            let x_high = ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 16_000.0).sin()) * 0.5;
            let y_high = f_high.process(x_high);
            if i > 1000 { rms_high += (y_high as f64).powi(2); }
        }
        let n_f = (N - 1000) as f64;
        rms_low = (rms_low / n_f).sqrt();
        rms_high = (rms_high / n_f).sqrt();
        // 一阶高通 -6dB/倍频程，30Hz 距 120Hz 约 2 个倍频程 → 衰减 ~12dB
        // 440Hz 在通带几乎无损，比值应 > 2 倍
        assert!(
            rms_high > rms_low * 2.0,
            "高频(440Hz) RMS 应大于低频(30Hz): {:.5} vs {:.5}",
            rms_high, rms_low
        );
    }

    // ── 噪声门 ─────────────────────────────────────────

    #[test]
    fn noise_gate_passes_loud_signal() {
        let mut gate = NoiseGate::new(0.001, 16_000); // 低阈值
        let amplitude = 0.1f32; // 响亮信号
        let mut passed = 0usize;
        let mut total = 0usize;
        for i in 0..8000 {
            let s = ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 16_000.0).sin()) * amplitude;
            let out = gate.process(s);
            total += 1;
            if out != 0.0 { passed += 1; }
        }
        // 大部分样本应通过
        assert!(passed > total * 9 / 10, "响亮信号应大部分通过: {}/{}", passed, total);
    }

    #[test]
    fn noise_gate_silences_quiet_signal() {
        let mut gate = NoiseGate::new(0.01, 16_000); // 较高阈值
        let amplitude = 0.0005f32; // 极弱信号（底噪级）
        let mut silenced = 0usize;
        let total = 4000usize;
        for i in 0..total {
            let s = ((i as f32 * 60.0 * 2.0 * std::f32::consts::PI / 16_000.0).sin()) * amplitude;
            let out = gate.process(s);
            if out == 0.0 { silenced += 1; }
        }
        // 大部分样本应被静音
        assert!(silenced > total * 8 / 10, "弱信号应大部分被静音: {}/{}", silenced, total);
    }
}
