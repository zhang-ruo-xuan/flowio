/**
 * 语音输入采集器（桌面端）。
 *
 * 管线：getUserMedia → AudioContext(16kHz) → ScriptProcessorNode 收集
 * Float32 PCM → 停止时转 i16 → 打包裸 PCM 分块（base64）交给 Rust。
 * WAV 头由 Rust 侧在送 ASR 前统一补齐（voice/audio.rs::pcm_to_wav）。
 *
 * 设计要点：
 * - ScriptProcessorNode 已废弃但 Chromium 完整支持；比 AudioWorklet 免去
 *   独立模块文件的加载时序问题，且 WebView2 最小化时音频回调不中断。
 * - 暂停录制时调用 pause()/resume()：暂停期间的 PCM 直接丢弃
 *   （PRD：切片按绝对时间对齐，丢弃段自然静音，不影响步骤映射）。
 * - startedAt 取 AudioContext 成功创建并收到首帧数据的时刻（epoch ms），
 *   与步骤的 timestamp（epoch ms）同源，偏移计算无需时钟换算。
 */

const TARGET_SAMPLE_RATE = 16000;
/** 每 ~0.5s 上传一块（16k × i16 × 0.5s ≈ 16KB） */
const CHUNK_UPLOAD_INTERVAL_MS = 500;

export interface VoiceStopResult {
  /** 裸 PCM 分块（base64），按顺序 voice_append */
  chunks: string[];
  /** 采集起点（epoch ms） */
  startedAtMs: number;
  /** 采样率（与后端 audio.rs 约定一致） */
  sampleRate: number;
  /** 采集总时长（毫秒） */
  durationMs: number;
}

export class VoiceRecorder {
  private stream: MediaStream | null = null;
  private ctx: AudioContext | null = null;
  private processor: ScriptProcessorNode | null = null;
  private source: MediaStreamAudioSourceNode | null = null;

  private buffers: Float32Array[] = [];
  private pendingChunk: Float32Array[] = [];
  private pendingChunkSamples = 0;
  private totalSamples = 0;

  private startedAtMs = 0;
  private paused = false;
  private active = false;

  /** 采样率（与后端 voice/audio.rs::SAMPLE_RATE 约定一致） */
  readonly sampleRateSafe = TARGET_SAMPLE_RATE;

  /** 采集是否进行中 */
  get isActive(): boolean {
    return this.active;
  }

  /** 是否处于暂停（暂停期间丢弃 PCM） */
  get isPaused(): boolean {
    return this.paused;
  }

  /**
   * 开始采集。解析失败 / 权限被拒时抛错（调用方按 PRD V3 降级为纯录制）。
   * 返回值即音频起点（epoch ms）。
   */
  async start(): Promise<number> {
    if (this.active) return this.startedAtMs;
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: 1,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
    });
    // 采样率不满足期望时（设备不支持重采样）仍可工作，只是体积变大；
    // 后端 sample_rate 由 voice_begin 传入，无需两端强一致。
    this.ctx = new AudioContext({ sampleRate: TARGET_SAMPLE_RATE });
    this.source = this.ctx.createMediaStreamSource(this.stream);
    this.processor = this.ctx.createScriptProcessor(4096, 1, 1);
    this.processor.onaudioprocess = (ev) => {
      if (!this.active || this.paused) return;
      const input = ev.inputBuffer.getChannelData(0);
      const copy = new Float32Array(input.length);
      copy.set(input);
      this.buffers.push(copy);
      this.pendingChunk.push(copy);
      this.pendingChunkSamples += copy.length;
      this.totalSamples += copy.length;
    };
    this.source.connect(this.processor);
    // ScriptProcessor 需要连接到 destination 才会驱动回调；接一个零增益
    // 节点避免把采集声音外放。
    const mute = this.ctx.createGain();
    mute.gain.value = 0;
    this.processor.connect(mute);
    mute.connect(this.ctx.destination);

    await this.ctx.resume();
    this.buffers = [];
    this.pendingChunk = [];
    this.pendingChunkSamples = 0;
    this.totalSamples = 0;
    this.paused = false;
    this.startedAtMs = Date.now();
    this.active = true;
    return this.startedAtMs;
  }

  /** 暂停采集：丢弃后续 PCM（暂停期间不上传） */
  pause(): void {
    this.paused = true;
  }

  /** 恢复采集 */
  resume(): void {
    this.paused = false;
  }

  /** 停止采集并产出 PCM 分块 */
  async stop(): Promise<VoiceStopResult> {
    this.active = false;
    try {
      this.processor?.disconnect();
      this.source?.disconnect();
    } catch {
      /* noop */
    }
    try {
      await this.ctx?.close();
    } catch {
      /* noop */
    }
    this.stream?.getTracks().forEach((t) => t.stop());
    this.processor = null;
    this.source = null;
    this.ctx = null;
    this.stream = null;

    const sampleRate = TARGET_SAMPLE_RATE;
    const durationMs = Math.round((this.totalSamples * 1000) / sampleRate);
    const chunks = this.pendingChunk.map(encodeChunkB64);
    this.buffers = [];
    this.pendingChunk = [];
    this.pendingChunkSamples = 0;
    this.totalSamples = 0;
    return { chunks, startedAtMs: this.startedAtMs, sampleRate, durationMs };
  }

  /**
   * 判断 pendingChunk 是否该上传（由调用方定时驱动）。
   * 返回 null 表示不足一块；否则返回 base64 并清空 pending。
   */
  takeChunkIfReady(): string | null {
    const readySamples = Math.round((CHUNK_UPLOAD_INTERVAL_MS / 1000) * TARGET_SAMPLE_RATE);
    if (this.pendingChunkSamples < readySamples) return null;
    return this.takePendingChunk();
  }

  /** 立即取走 pending 数据（不足一块时也取） */
  takePendingChunk(): string | null {
    if (this.pendingChunkSamples === 0) return null;
    const b64 = this.pendingChunk.map(encodeChunkB64).join("");
    this.pendingChunk = [];
    this.pendingChunkSamples = 0;
    return b64;
  }
}

/** Float32Array → base64（i16 LE PCM） */
function encodeChunkB64(chunk: Float32Array): string {
  const pcm = float32ToInt16(chunk);
  let binary = "";
  const bytes = new Uint8Array(pcm.buffer);
  const STEP = 0x8000;
  for (let i = 0; i < bytes.length; i += STEP) {
    binary += String.fromCharCode.apply(
      null,
      Array.from(bytes.subarray(i, Math.min(i + STEP, bytes.length))) as unknown as number[]
    );
  }
  return btoa(binary);
}

function float32ToInt16(input: Float32Array): Int16Array {
  const out = new Int16Array(input.length);
  for (let i = 0; i < input.length; i++) {
    const s = Math.max(-1, Math.min(1, input[i]));
    out[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
  }
  return out;
}
