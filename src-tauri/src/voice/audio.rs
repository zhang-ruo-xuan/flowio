//! 语音输入音频处理：PCM→WAV 封装、按步骤切片、静音检测。
//!
//! 采集端是 `voice/capture.rs`（Rust + cpal/WASAPI），产出 16kHz / 单声道
//! / i16 PCM，直接写入后端缓冲；本模块只做纯信号/几何处理，无 IO，
//! 全部函数可独立单测。

/// 采集采样率（与前端 AudioContext(sampleRate) 约定一致）
pub const SAMPLE_RATE: u32 = 16_000;
/// 单段送 ASR 的时长上限。
///
/// ⚠️ 智谱官方文档（docs.bigmodel.cn 语音转文本）写死的限制是
/// **「文件大小 ≤ 25MB、音频时长 ≤ 30 秒」** —— 不是 60 秒。
/// 早先按 60s 理解取了 55_000，导致连续讲超过 30 秒的段必然被服务端
/// 拒绝（表现为「一直在说但一个字都没转出来」）。取 28_000（留 2s 余量）。
///
/// 体积校验：16kHz/16bit 单声道 = 32KB/s，28s ≈ 896KB，远低于 25MB。
/// 改大这个值前先看 `max_segment_ms_respects_asr_limit` 那条测试。
pub const MAX_SEGMENT_MS: i64 = 28_000;
/// 短于该时长的切片直接丢弃（≈0.4s，无法成句）
pub const MIN_SEGMENT_MS: i64 = 400;
/// RMS 静音阈值（≈ -47 dBFS）：低于该值视为该步骤无有效语音
pub const SILENCE_RMS: f32 = 0.0045;
/// 全局信号判定阈值（≈ -66 dBFS）：整段 RMS 低于该值视为「麦克风根本
/// 没拾到声音」（数字静音/底噪水平），此时不做 ASR 兜底（送了也白送）。
/// 高于它但 VAD 检不出区段（音量过低/短句被最小段过滤）→ 兜底整段送 ASR。
pub const FALLBACK_MIN_RMS: f32 = 0.0005;
/// 缓冲上限（字节）：30min@16k/i16 ≈ 57.6MB，超限停止追加
pub const MAX_BUFFER_BYTES: usize = 96 * 1024 * 1024;

/// 由原始 PCM（i16 LE）构造标准 44 字节头的 WAV 文件字节。
///
/// 前端分块上传的是裸 PCM；ASR 接口只收 .wav/.mp3，所以在送请求前
/// 统一补 WAV 头。无 RIFF 额外块，绝大多数解析器都能接受。
pub fn pcm_to_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let file_len = 36 + data_len;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&file_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // fmt chunk
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2; // mono * 2 bytes
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    // data chunk
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// PCM 字节长度 → 时长（毫秒）
pub fn pcm_duration_ms(pcm_len: usize, sample_rate: u32) -> i64 {
    let samples = (pcm_len / 2) as i64;
    samples * 1000 / sample_rate as i64
}

/// 毫秒 → PCM 字节偏移（2 字节对齐）
fn ms_to_byte_offset(ms: i64, sample_rate: u32) -> usize {
    let off = (ms * sample_rate as i64 * 2 / 1000) as usize;
    off - (off % 2)
}

/// i16 样本序列的 RMS（归一化到 [-1,1]）
pub fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let v = s as f64 / 32768.0;
            v * v
        })
        .sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// 整段 PCM 的 RMS（流式计算，不复制样本缓冲）。用于诊断「麦克风是否
/// 拾到过任何声音」：数字静音 ≈ 0.0，正常人声 ≥ 0.01，过低音量在
/// `FALLBACK_MIN_RMS` 与 `SILENCE_RMS` 之间。
pub fn overall_rms(pcm: &[u8]) -> f32 {
    let n = pcm.len() / 2;
    if n == 0 {
        return 0.0;
    }
    let sum_sq: f64 = pcm
        .chunks_exact(2)
        .map(|c| {
            let v = i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0;
            v * v
        })
        .sum();
    (sum_sq / n as f64).sqrt() as f32
}

/// 帧长（毫秒）：语音区段检测的扫描精度
const VAD_FRAME_MS: i64 = 50;
/// 区段合并间隙（毫秒）：小于该值的静音间隙视为句内停顿，不拆区段
const GAP_MERGE_MS: i64 = 300;

fn slice_is_silent(pcm: &[u8]) -> bool {
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    rms_i16(&samples) < SILENCE_RMS
}

/// 检测语音区段（毫秒区间列表）。
///
/// 扫描流程：按 50ms 帧×滑动窗口计算每帧的 PCM 字节区间 → 单帧 RMS
/// 判定 → 相邻语音帧按 `GAP_MERGE_MS` 合并成区段 → 丢弃短于
/// `MIN_SEGMENT_MS` 的区段。
pub fn speech_regions(pcm: &[u8], sample_rate: u32) -> Vec<(i64, i64)> {
    if pcm.is_empty() {
        return Vec::new();
    }
    let total_ms = pcm_duration_ms(pcm.len(), sample_rate);
    let frame_samples = (sample_rate as i64 * VAD_FRAME_MS / 1000) as usize;
    let frame_bytes = frame_samples * 2;

    // 单帧语音判定
    let mut speech_frames: Vec<bool> = Vec::new();
    let mut pos = 0usize;
    while pos < pcm.len() {
        let end = (pos + frame_bytes).min(pcm.len());
        speech_frames.push(!slice_is_silent(&pcm[pos..end]));
        pos = end;
    }

    // 合并相邻语音帧（间隙 ≤ GAP_MERGE_MS 视为句内停顿）
    let mut regions: Vec<(i64, i64)> = Vec::new();
    let mut current: Option<(i64, i64)> = None; // (start_ms, last_speech_end_ms)
    for (fi, &spoke) in speech_frames.iter().enumerate() {
        let f_start = fi as i64 * VAD_FRAME_MS;
        let f_end = (f_start + VAD_FRAME_MS).min(total_ms);
        if spoke {
            match &mut current {
                Some((_, last_end)) => *last_end = f_end,
                None => current = Some((f_start, f_end)),
            }
        } else if let Some((s, e)) = &mut current {
            // 间隙超过 GAP_MERGE_MS → 结束当前区段
            if f_end - *e > GAP_MERGE_MS {
                let (s, e) = (*s, *e);
                if e - s >= MIN_SEGMENT_MS {
                    regions.push((s, e));
                }
                current = None;
            }
        }
    }
    if let Some((s, e)) = current {
        if e - s >= MIN_SEGMENT_MS {
            regions.push((s, e));
        }
    }
    regions
}

/// 把一段毫秒区间切成不超过 `MAX_SEGMENT_MS` 的子区间。
fn split_long_range(start_ms: i64, end_ms: i64) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut s = start_ms;
    while s < end_ms {
        let e = (s + MAX_SEGMENT_MS).min(end_ms);
        out.push((s, e));
        s = e;
    }
    out
}

/// 按步骤时间戳把整段 PCM 切成「步骤 → 若干 PCM 段」。
///
/// `step_offsets_ms[i]` = 第 i 步截图时刻相对音频起点的毫秒偏移
/// （`step.timestamp - audio_started_at_ms`，可为负，内部 clamp）。
///
/// 切片规则（对应 PRD V6/V7/V10/V11；2026-09-06 归属规则改版）：
/// - **操作前的语音归这个操作的步骤**：区段起点之后第一个步骤截图时刻
///   即该语音所属步骤——做某步操作之前说的话 → 归那一步（V10 改）；
/// - 话没说完就做了操作（区段跨过后续步骤边界）不拆句，整段仍留在
///   该步骤；说完后的下一段语音再归下一个操作的步骤；
/// - 最后一步之后说的话没有更晚的步骤 → 归最后一步；
/// - 该步骤没有任何区段 → 丢弃（无语音，V7）；
/// - 区段超过 `MAX_SEGMENT_MS` → 等分细分，结果按顺序拼接（V11）。
///
/// 返回 `(step_index, chunks)`，step_index 为 `step_offsets_ms` 的下标，
/// 只包含有有效语音的步骤，且按步骤顺序升序。
pub fn slice_for_steps(
    pcm: &[u8],
    sample_rate: u32,
    step_offsets_ms: &[i64],
) -> Vec<(usize, Vec<Vec<u8>>)> {
    if pcm.is_empty() || step_offsets_ms.is_empty() {
        return Vec::new();
    }
    let total_ms = pcm_duration_ms(pcm.len(), sample_rate);

    // 区段 → 起点之后第一个步骤（操作前的语音归这个操作）。
    // 没有更晚的步骤（最后一步之后才开口）→ 归最后一步。
    let mut grouped: std::collections::BTreeMap<usize, Vec<(i64, i64)>> =
        std::collections::BTreeMap::new();
    for (rs, re) in speech_regions(pcm, sample_rate) {
        let step_idx = step_offsets_ms
            .iter()
            .position(|&off| off.clamp(0, total_ms) > rs)
            .unwrap_or(step_offsets_ms.len() - 1);
        grouped.entry(step_idx).or_default().push((rs, re));
    }

    let mut out: Vec<(usize, Vec<Vec<u8>>)> = Vec::new();
    for (step_idx, regions) in grouped {
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        for (rs, re) in regions {
            for (s, e) in split_long_range(rs, re) {
                let bs = ms_to_byte_offset(s, sample_rate);
                let be = ms_to_byte_offset(e, sample_rate).min(pcm.len());
                if be > bs {
                    chunks.push(pcm[bs..be].to_vec());
                }
            }
        }
        if !chunks.is_empty() {
            out.push((step_idx, chunks));
        }
    }
    out
}

/// 兜底切片（VAD 兜底）：不做静音检测，直接按步骤时间戳把整段音频
/// 切成「步骤 → 若干 PCM 段」。
///
/// 背景：VAD 阈值按正常人声标定（-47 dBFS），麦克风音量过低、或说话
/// 全是 <400ms 的短句时，`slice_for_steps` 会返回空 → 「未检测到语音
/// 内容」。此时若整段信号非数字静音，按时间戳硬切送 ASR 是最后一道
/// 防线——真无声时 ASR 返回空文本，代价只是几次请求。
///
/// 切分规则（2026-09-06 与 `slice_for_steps` 归属规则对齐）：
/// - 步骤 0 覆盖 `[0, clamp(offset_0))`，步骤 i 覆盖
///   `[clamp(offset_{i-1}), clamp(offset_i))`，最后一步覆盖
///   `[clamp(offset_{n-2}), 音频末尾)`——操作前的语音归这个操作；
/// - 区间为空（步骤边界相同 / 无更早音频）→ 跳过；
/// - 负偏移 clamp 到 0，超过音频长度的步骤按 clamp 后区间取；
/// - 单段超过 `MAX_SEGMENT_MS` 等分细分，短于 `MIN_SEGMENT_MS` 的段丢弃。
pub fn fallback_slice_all(
    pcm: &[u8],
    sample_rate: u32,
    step_offsets_ms: &[i64],
) -> Vec<(usize, Vec<Vec<u8>>)> {
    if pcm.is_empty() || step_offsets_ms.is_empty() {
        return Vec::new();
    }
    let total_ms = pcm_duration_ms(pcm.len(), sample_rate);
    let n = step_offsets_ms.len();
    let mut out: Vec<(usize, Vec<Vec<u8>>)> = Vec::new();
    for i in 0..n {
        let start = if i == 0 {
            0
        } else {
            step_offsets_ms[i - 1].clamp(0, total_ms)
        };
        let end = if i + 1 == n {
            total_ms
        } else {
            step_offsets_ms[i].clamp(0, total_ms)
        };
        if end <= start {
            continue;
        }
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        for (s, e) in split_long_range(start, end) {
            if e - s < MIN_SEGMENT_MS {
                continue;
            }
            let bs = ms_to_byte_offset(s, sample_rate);
            let be = ms_to_byte_offset(e, sample_rate).min(pcm.len());
            if be > bs {
                chunks.push(pcm[bs..be].to_vec());
            }
        }
        if !chunks.is_empty() {
            out.push((i, chunks));
        }
    }
    out
}

#[cfg(test)]
mod audio_tests {
    use super::*;

    fn sine_pcm(ms: i64, sample_rate: u32, freq: f32, amp: f32) -> Vec<u8> {
        let n = (sample_rate as f32 * ms as f32 / 1000.0) as usize;
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = (amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
                .clamp(-1.0, 1.0);
            out.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
        }
        out
    }

    // ── pcm_to_wav ─────────────────────────────────────────────

    #[test]
    fn wav_header_layout_is_standard() {
        let pcm = vec![0u8; 8];
        let wav = pcm_to_wav(&pcm, 16_000);
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        // fmt size = 16
        assert_eq!(u32::from_le_bytes([wav[16], wav[17], wav[18], wav[19]]), 16);
        // PCM mono
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        // sample rate + byte rate
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16_000);
        assert_eq!(u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]), 32_000);
        // bits per sample
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 8);
    }

    #[test]
    fn wav_roundtrip_header_matches_hound_layout() {
        // RIFF size 字段必须 = 36 + data 长度，读回时才能完整解出 payload
        let pcm = vec![1u8; 1000];
        let wav = pcm_to_wav(&pcm, 44_100);
        let riff_size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        assert_eq!(riff_size as usize, 36 + 1000);
        assert_eq!(wav.len() - 44, 1000);
    }

    // ── duration / rms ─────────────────────────────────────────

    #[test]
    fn duration_math() {
        assert_eq!(pcm_duration_ms(32_000, 16_000), 1_000); // 16000 samples * 2B
        assert_eq!(pcm_duration_ms(0, 16_000), 0);
        assert_eq!(pcm_duration_ms(31, 16_000), 0); // 不足一个样本
    }

    #[test]
    fn rms_loud_vs_silent() {
        let loud = sine_pcm(500, 16_000, 440.0, 0.5);
        let samples: Vec<i16> = loud
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert!(rms_i16(&samples) > 0.3);
        let quiet = vec![0u8; 32_000];
        let qs: Vec<i16> = quiet
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(rms_i16(&qs), 0.0);
    }

    // ── slice_for_steps ────────────────────────────────────────

    #[test]
    fn slice_v7_silent_step_is_dropped() {
        // 3 秒音频：0-1s 静音、1-2s 440Hz 语音、2-3s 静音；
        // 语音 1-2s 起点之后第一个步骤是下标 2（2s 处截图）→ 归第 3 步
        let mut pcm = vec![0u8; 16_000 * 2]; // 0-1s
        pcm.extend(sine_pcm(1_000, 16_000, 440.0, 0.5)); // 1-2s
        pcm.extend(vec![0u8; 16_000 * 2]); // 2-3s
        let offsets = [0, 1_000, 2_000];
        let out = slice_for_steps(&pcm, 16_000, &offsets);
        let idxs: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, vec![2], "操作前（2s 截图前）的语音归第 3 步");
    }

    #[test]
    fn slice_v10_speech_crossing_boundary_stays_with_upcoming_step() {
        // 2 秒语音覆盖两步边界（0.5s 处第二步截图）→ 话没说完就操作：
        // 整段不拆，归起点之后的第一个步骤（第 2 步，下标 1）
        let pcm = sine_pcm(2_000, 16_000, 440.0, 0.5);
        let offsets = [0, 500];
        let out = slice_for_steps(&pcm, 16_000, &offsets);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 1);
    }

    #[test]
    fn slice_v11_long_segment_is_split_into_max_chunks() {
        // 70 秒单步语音 → 3 段（28/28/14），每段都必须 ≤ 智谱 30s 上限
        let pcm = sine_pcm(70_000, 16_000, 440.0, 0.5);
        let out = slice_for_steps(&pcm, 16_000, &[0]);
        assert_eq!(out.len(), 1);
        let chunks = &out[0].1;
        assert_eq!(chunks.len(), 3);
        let durs: Vec<i64> = chunks.iter().map(|c| pcm_duration_ms(c.len(), 16_000)).collect();
        assert_eq!(durs, vec![28_000, 28_000, 14_000]);
    }

    /// 服务端硬限制：单段 ≤30 秒、文件 ≤25MB。改常量时这条会先炸。
    #[test]
    fn max_segment_respects_zhipu_asr_limits() {
        assert!(
            MAX_SEGMENT_MS <= 30_000,
            "智谱语音转文本单段上限 30 秒，MAX_SEGMENT_MS={} 超限",
            MAX_SEGMENT_MS
        );
        // 16kHz / 16bit / 单声道 → 32KB/s
        let max_bytes = MAX_SEGMENT_MS as usize * SAMPLE_RATE as usize * 2 / 1000;
        assert!(
            max_bytes <= 25 * 1024 * 1024,
            "单段 WAV {} 字节超过 25MB 上限",
            max_bytes
        );
    }

    /// 送出去的每一段（含 WAV 头）都要同时满足时长与体积上限。
    #[test]
    fn every_chunk_stays_within_asr_limits() {
        // 一段 5 分钟不停顿的讲话（兜底路径会整段硬切）
        let pcm = sine_pcm(300_000, 16_000, 440.0, 0.5);
        let out = fallback_slice_all(&pcm, 16_000, &[0]);
        let chunks: Vec<&Vec<u8>> = out.iter().flat_map(|(_, cs)| cs.iter()).collect();
        assert!(!chunks.is_empty());
        for c in chunks {
            let ms = pcm_duration_ms(c.len(), 16_000);
            assert!(ms <= 30_000, "段长 {}ms 超过 30s 上限", ms);
            assert!(
                pcm_to_wav(c, 16_000).len() <= 25 * 1024 * 1024,
                "段 WAV 超过 25MB 上限"
            );
        }
    }

    #[test]
    fn slice_negative_offset_clamps_to_zero() {
        // 第 1 步截图早于音频启动（时间戳偏移为负）→ clamp 到 0；
        // 连续语音 0-3s 不拆句 → 归起点之后第一个步骤（1s 处截图的第 2 步）
        let pcm = sine_pcm(3_000, 16_000, 440.0, 0.5);
        let offsets = [-5_000, 1_000];
        let out = slice_for_steps(&pcm, 16_000, &offsets);
        let idxs: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, vec![1]);
        // 整段 3000ms 语音都归第 2 步
        assert_eq!(pcm_duration_ms(out[0].1[0].len(), 16_000), 3_000);
    }

    #[test]
    fn slice_v7b_speech_across_pause_gap_stays_one_region() {
        // 1s 语音 → 200ms 停顿（<300ms 句内停顿）→ 1s 语音：同一区段不拆
        let mut pcm = sine_pcm(1_000, 16_000, 440.0, 0.5);
        pcm.extend(vec![0u8; 200 * 16]); // 200ms 静音
        pcm.extend(sine_pcm(1_000, 16_000, 440.0, 0.5));
        let regions = speech_regions(&pcm, 16_000);
        assert_eq!(regions.len(), 1);
        assert!(regions[0].1 - regions[0].0 >= 2_000);
    }

    #[test]
    fn slice_audio_shorter_than_offsets_clamps_to_end() {
        // 音频中途用户关掉了开关（V13）：音频只有 1s，语音 0-1s 起点之后
        // 第一个步骤是下标 1（2s 截图 clamp 到音频末尾 1s > 0）→ 归第 2 步
        let pcm = sine_pcm(1_000, 16_000, 440.0, 0.5);
        let offsets = [0, 2_000, 4_000];
        let out = slice_for_steps(&pcm, 16_000, &offsets);
        let idxs: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, vec![1]);
    }

    #[test]
    fn slice_empty_inputs_are_safe() {
        assert!(slice_for_steps(&[], 16_000, &[0]).is_empty());
        let pcm = sine_pcm(500, 16_000, 440.0, 0.5);
        assert!(slice_for_steps(&pcm, 16_000, &[]).is_empty());
    }

    // ── overall_rms / fallback_slice_all ───────────────────────

    #[test]
    fn overall_rms_loud_vs_digital_silence() {
        let loud = overall_rms(&sine_pcm(1_000, 16_000, 440.0, 0.5));
        assert!(loud > 0.3, "0.5 幅度正弦的 RMS 应 ≈0.35，实际 {}", loud);
        assert_eq!(overall_rms(&vec![0u8; 32_000]), 0.0);
        assert_eq!(overall_rms(&[]), 0.0);
        // 奇数字节长度安全
        assert_eq!(overall_rms(&[1u8, 0, 2]), overall_rms(&[1u8, 0]));
    }

    #[test]
    fn fallback_assigns_segments_by_step() {
        // 新归属：步骤 i 的区间 = [上一步边界, 本步边界)。offsets [0,1000,2000]：
        // 步骤 0 区间 [0,0) 为空跳过；步骤 1 [0,1000)；步骤 2（最后）[1000, 3000)
        let pcm = sine_pcm(3_000, 16_000, 440.0, 0.5);
        let out = fallback_slice_all(&pcm, 16_000, &[0, 1_000, 2_000]);
        let idxs: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, vec![1, 2]);
        assert_eq!(pcm_duration_ms(out[0].1[0].len(), 16_000), 1_000);
        assert_eq!(pcm_duration_ms(out[1].1[0].len(), 16_000), 2_000);
    }

    #[test]
    fn fallback_equal_offsets_assign_to_last_step_only() {
        // 边界全部相同 → 前面步骤区间为空，语音整段归最后一步
        let pcm = sine_pcm(3_000, 16_000, 440.0, 0.5);
        let out = fallback_slice_all(&pcm, 16_000, &[0, 0, 0]);
        let idxs: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, vec![2]);
        assert_eq!(pcm_duration_ms(out[0].1[0].len(), 16_000), 3_000);
    }

    #[test]
    fn fallback_negative_offset_clamps_and_last_extends_to_end() {
        // offsets [-5000, 1000] → clamp 后 [0, 1000]：步骤 0 区间 [0,0) 为空，
        // 最后一步覆盖 [0, 末尾) → 整段 3s 归最后一步（下标 1）
        let pcm = sine_pcm(3_000, 16_000, 440.0, 0.5);
        let out = fallback_slice_all(&pcm, 16_000, &[-5_000, 1_000]);
        let idxs: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, vec![1]);
        assert_eq!(pcm_duration_ms(out[0].1[0].len(), 16_000), 3_000);
    }

    #[test]
    fn fallback_offset_beyond_audio_drops_step_and_drops_tiny_audio() {
        // 第二步偏移(5s)超出音频长度(1s) → clamp 到末尾：步骤 0 区间空，
        // 语音整段归最后一步（下标 1）
        let pcm = sine_pcm(1_000, 16_000, 440.0, 0.5);
        let out = fallback_slice_all(&pcm, 16_000, &[0, 5_000]);
        let idxs: Vec<usize> = out.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, vec![1]);
        // 音频本身过短（<400ms）→ 全部丢弃
        let tiny = sine_pcm(300, 16_000, 440.0, 0.5);
        assert!(fallback_slice_all(&tiny, 16_000, &[0]).is_empty());
    }

    #[test]
    fn split_long_range_boundaries() {
        // 正好等于上限 → 一段；超过 → 上限段 + 余量段
        assert_eq!(split_long_range(0, 28_000), vec![(0, 28_000)]);
        assert_eq!(
            split_long_range(0, 29_000),
            vec![(0, 28_000), (28_000, 29_000)]
        );
        assert!(split_long_range(5_000, 5_000).is_empty());
    }
}
