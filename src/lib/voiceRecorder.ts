/**
 * 语音输入采集器（桌面端）—— 薄封装，真正的采集在 Rust 侧。
 *
 * 🔴 为什么不用 getUserMedia + AudioContext（历史包袱，别再走回头路）：
 * 录制一开始主窗口就 minimize()（把屏幕让给被录的应用），WebView2 会停掉
 * 指向真实输出设备的音频渲染线程 —— ScriptProcessorNode 的 onaudioprocess
 * 随之停止（实测只采到最小化前的 512ms）。试过 AudioContext({sinkId:"none"})
 * 让上下文不挂输出设备，结果更糟：null sink 没有设备时钟驱动渲染，
 * **一次回调都没跑**（样本数全程 0）。
 * 采集不该依赖渲染进程，所以搬到了 Rust：`src-tauri/src/voice/capture.rs`
 * 用 cpal（Windows 走 WASAPI）直接采，窗口最小化/遮挡/隐藏都不影响。
 *
 * 代价：丢掉 getUserMedia 的 echoCancellation / noiseSuppression，麦克风
 * 会拾到喇叭外放的声音 —— 戴耳机或调低外放音量即可。
 *
 * 本类只负责：起停采集（voice_mic_start/stop）、暂停恢复（补静音保时间轴）、
 * 以及给看门狗查样本数（voice_mic_samples）。PCM 直接进 Rust 的缓冲，
 * 不再经 IPC 传 base64 分块。
 */

import { invoke } from "@tauri-apps/api/core";

const TARGET_SAMPLE_RATE = 16000;

export interface VoiceStopResult {
  /** 保留字段：Rust 侧采集不再经前端上传，恒为空 */
  chunks: string[];
  /** 采集起点（epoch ms） */
  startedAtMs: number;
  /** 采样率（与后端 voice/audio.rs::SAMPLE_RATE 约定一致） */
  sampleRate: number;
  /** 采集总时长（毫秒） */
  durationMs: number;
}

export class VoiceRecorder {
  private active = false;
  private pausedFlag = false;
  private startedAtMs = 0;

  /** 采样率（与后端 voice/audio.rs::SAMPLE_RATE 约定一致） */
  readonly sampleRateSafe = TARGET_SAMPLE_RATE;

  /** 采集是否进行中 */
  get isActive(): boolean {
    return this.active;
  }

  /** 是否处于暂停（暂停期间丢弃 PCM，恢复时补等长静音） */
  get isPaused(): boolean {
    return this.pausedFlag;
  }

  /**
   * 已采集样本数（看门狗用：连续不变说明采集停摆）。
   * 异步是因为计数在 Rust 侧的原子变量里。
   */
  async samples(): Promise<number> {
    try {
      return await invoke<number>("voice_mic_samples");
    } catch {
      return 0;
    }
  }

  /**
   * 开始采集。解析失败 / 权限被拒时抛错（调用方按 PRD V3 降级为纯录制）。
   * 返回值即音频起点（epoch ms，由 Rust 在流真正启动的瞬间取）。
   */
  async start(recordingId: string): Promise<number> {
    if (this.active) return this.startedAtMs;
    this.startedAtMs = await invoke<number>("voice_mic_start", { recordingId });
    this.active = true;
    this.pausedFlag = false;
    return this.startedAtMs;
  }

  /** 暂停采集：暂停期间的音频不写入缓冲（恢复时补等长静音保住时间轴） */
  async pause(): Promise<void> {
    this.pausedFlag = true;
    await invoke("voice_mic_set_paused", { paused: true }).catch(() => {});
  }

  /** 恢复采集 */
  async resume(): Promise<void> {
    this.pausedFlag = false;
    await invoke("voice_mic_set_paused", { paused: false }).catch(() => {});
  }

  /** 停止采集（会等 Rust 把管道里剩余 PCM 落进缓冲） */
  async stop(): Promise<VoiceStopResult> {
    if (!this.active) {
      return { chunks: [], startedAtMs: this.startedAtMs, sampleRate: TARGET_SAMPLE_RATE, durationMs: 0 };
    }
    this.active = false;
    this.pausedFlag = false;
    let totalSamples = 0;
    try {
      totalSamples = await invoke<number>("voice_mic_stop");
    } catch (err) {
      console.warn("[voice] mic stop failed:", err);
    }
    return {
      chunks: [],
      startedAtMs: this.startedAtMs,
      sampleRate: TARGET_SAMPLE_RATE,
      durationMs: Math.round((totalSamples * 1000) / TARGET_SAMPLE_RATE),
    };
  }
}
