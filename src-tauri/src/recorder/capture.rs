//! Continuous screenshot capture ring buffer.
//!
//! Scribe-style architecture: a dedicated background thread captures
//! fullscreen frames at 4 FPS and pushes them into an mpsc channel.
//! The event thread maintains a VecDeque-based ring buffer of recent
//! frames, queryable by timestamp (closest match) or by recency (latest).
//!
//! This decouples screenshot capture from hook callbacks, eliminating
//! hook callback latency and unifying the click/key screenshot pipeline.

use image::DynamicImage;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::screenshot;

/// A timestamped frame from the continuous capture thread.
///
/// `origin_x / origin_y` is the captured monitor's top-left corner in
/// virtual-screen coordinates — needed to translate global input coordinates
/// (click points, UIA rectangles) into this image's space.
pub struct CapturedFrame {
    pub timestamp: Instant,
    pub image: DynamicImage,
    pub origin_x: i32,
    pub origin_y: i32,
}

/// Ring buffer of recent frames, queryable by timestamp or recency.
pub struct FrameRing {
    frames: VecDeque<CapturedFrame>,
    capacity: usize,
}

impl FrameRing {
    pub fn new(capacity: usize) -> Self {
        FrameRing {
            frames: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push a frame, evicting oldest if at capacity.
    pub fn push(&mut self, frame: CapturedFrame) {
        // Zero capacity means "store nothing" — keep the ring empty
        // instead of overflowing into a 1-element buffer.
        if self.capacity == 0 {
            return;
        }
        if self.frames.len() >= self.capacity {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    /// Most recent frame (for keyboard flush — shows typed text).
    pub fn latest(&self) -> Option<&CapturedFrame> {
        self.frames.back()
    }

    /// Frame closest to the given timestamp (for click — shows pre-click state).
    /// Uses binary search since frames are always in timestamp order.
    pub fn closest_to(&self, target: Instant) -> Option<&CapturedFrame> {
        if self.frames.is_empty() {
            return None;
        }
        let idx = self.frames.partition_point(|f| f.timestamp < target);
        if idx == 0 {
            return self.frames.front();
        }
        if idx >= self.frames.len() {
            return self.frames.back();
        }
        let prev = &self.frames[idx - 1];
        let next = &self.frames[idx];
        if target.duration_since(prev.timestamp) <= next.timestamp.duration_since(target) {
            Some(prev)
        } else {
            Some(next)
        }
    }
}

/// Start the continuous capture thread.
///
/// Captures a fullscreen frame every `interval` (default 250ms = 4 FPS)
/// and sends it via `frame_tx`. Runs until `running` is set to false.
///
/// When `target_window` is `Some(process_name)` (录制指定软件), each frame is
/// cropped to the target window's screen rectangle and `origin_x/origin_y`
/// record where in screen space the crop starts. If the target window is not
/// visible/available, the frame is skipped entirely so the ring never holds
/// unrelated desktop content.
pub fn start_capture_thread(
    running: Arc<AtomicBool>,
    frame_tx: mpsc::Sender<CapturedFrame>,
    interval: Duration,
    target_window: Option<String>,
) {
    std::thread::spawn(move || {
        super::recorder::capture_dbg_log(&format!(
            "[CAPTURE] continuous capture thread started (interval: {:?}, target: {:?})",
            interval, target_window
        ));
        while running.load(Ordering::Relaxed) {
            let loop_start = Instant::now();

            // In target-window mode, use PrintWindow (captures GPU-accelerated
            // windows like WPS Office that GDI BitBlt renders as black).
            let capture_result = match &target_window {
                Some(target) => screenshot::capture_target_window_composite(target),
                None => screenshot::capture_fullscreen_with_origin(),
            };

            match capture_result {
                Ok((img, ox, oy)) => {

                    let frame = CapturedFrame {
                        timestamp: Instant::now(),
                        image: img,
                        origin_x: ox,
                        origin_y: oy,
                    };
                    if frame_tx.send(frame).is_err() {
                        super::recorder::capture_dbg_log("[CAPTURE] frame channel closed, exiting");
                        break;
                    }
                }
                Err(e) => {
                    super::recorder::capture_dbg_log(&format!("[CAPTURE] frame capture failed: {}", e));
                }
            }

            let elapsed = loop_start.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
        }
        super::recorder::capture_dbg_log("[CAPTURE] capture thread exiting");
    });
}

/// Sleep out the rest of the interval (used when a target-mode frame is skipped).
fn skip_frame(loop_start: &Instant, interval: Duration) {
    let elapsed = loop_start.elapsed();
    if elapsed < interval {
        std::thread::sleep(interval - elapsed);
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    fn frame_at(ts: Instant) -> CapturedFrame {
        CapturedFrame {
            timestamp: ts,
            image: DynamicImage::new_rgb8(4, 4),
            origin_x: 0,
            origin_y: 0,
        }
    }

    #[test]
    fn push_evicts_oldest_when_full() {
        let mut ring = FrameRing::new(2);
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        ring.push(frame_at(t0 + Duration::from_millis(10)));
        ring.push(frame_at(t0 + Duration::from_millis(20)));

        assert_eq!(ring.frames.len(), 2);
        assert_eq!(
            ring.latest().unwrap().timestamp,
            t0 + Duration::from_millis(20)
        );
        // 最旧帧被淘汰，只剩第 2、3 帧
        assert_eq!(
            ring.frames.front().unwrap().timestamp,
            t0 + Duration::from_millis(10)
        );
    }

    #[test]
    fn latest_returns_newest_frame() {
        let mut ring = FrameRing::new(4);
        assert!(ring.latest().is_none());
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        ring.push(frame_at(t0 + Duration::from_millis(5)));
        assert_eq!(ring.latest().unwrap().timestamp, t0 + Duration::from_millis(5));
    }

    #[test]
    fn closest_to_empty_returns_none() {
        let ring = FrameRing::new(4);
        assert!(ring.closest_to(Instant::now()).is_none());
    }

    #[test]
    fn closest_to_before_all_returns_first() {
        let mut ring = FrameRing::new(4);
        let t0 = Instant::now();
        ring.push(frame_at(t0 + Duration::from_millis(100)));
        ring.push(frame_at(t0 + Duration::from_millis(200)));
        // 目标早于所有帧 → 第一帧
        assert_eq!(
            ring.closest_to(t0).unwrap().timestamp,
            t0 + Duration::from_millis(100)
        );
    }

    #[test]
    fn closest_to_after_all_returns_last() {
        let mut ring = FrameRing::new(4);
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        ring.push(frame_at(t0 + Duration::from_millis(100)));
        // 目标晚于所有帧 → 最后一帧
        assert_eq!(
            ring.closest_to(t0 + Duration::from_millis(500)).unwrap().timestamp,
            t0 + Duration::from_millis(100)
        );
    }

    #[test]
    fn closest_to_middle_prefers_nearer_frame() {
        let mut ring = FrameRing::new(4);
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        ring.push(frame_at(t0 + Duration::from_millis(100)));
        // 距前帧 60ms、距后帧 40ms → 后帧
        assert_eq!(
            ring.closest_to(t0 + Duration::from_millis(60)).unwrap().timestamp,
            t0 + Duration::from_millis(100)
        );
        // 距前帧 40ms、距后帧 60ms → 前帧
        assert_eq!(
            ring.closest_to(t0 + Duration::from_millis(40)).unwrap().timestamp,
            t0
        );
        // 等距时取前帧
        assert_eq!(
            ring.closest_to(t0 + Duration::from_millis(50)).unwrap().timestamp,
            t0
        );
    }

    #[test]
    fn closest_to_exact_hit_returns_that_frame() {
        let mut ring = FrameRing::new(4);
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        ring.push(frame_at(t0 + Duration::from_millis(100)));
        ring.push(frame_at(t0 + Duration::from_millis(200)));
        assert_eq!(
            ring.closest_to(t0 + Duration::from_millis(100)).unwrap().timestamp,
            t0 + Duration::from_millis(100)
        );
    }


    #[test]
    fn capacity_one_keeps_only_latest() {
        let mut ring = FrameRing::new(1);
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        ring.push(frame_at(t0 + Duration::from_millis(10)));
        assert_eq!(ring.frames.len(), 1);
        assert_eq!(ring.latest().unwrap().timestamp, t0 + Duration::from_millis(10));
    }

    #[test]
    fn capacity_zero_push_does_not_panic() {
        // 容量 0 语义：不存储任何帧，保持空队（修复前会退化为 1 帧缓冲）
        let mut ring = FrameRing::new(0);
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        assert_eq!(ring.frames.len(), 0);
        assert!(ring.latest().is_none());
    }

    #[test]
    fn closest_to_single_frame_any_target() {
        let mut ring = FrameRing::new(4);
        let t0 = Instant::now();
        ring.push(frame_at(t0));
        assert_eq!(ring.closest_to(t0 - Duration::from_secs(10)).unwrap().timestamp, t0);
        assert_eq!(ring.closest_to(t0 + Duration::from_secs(10)).unwrap().timestamp, t0);
    }

    #[test]
    fn push_many_evicts_and_latest_correct() {
        let mut ring = FrameRing::new(3);
        let t0 = Instant::now();
        for i in 0..10u64 {
            ring.push(frame_at(t0 + Duration::from_millis(i * 10)));
        }
        assert_eq!(ring.frames.len(), 3);
        assert_eq!(ring.latest().unwrap().timestamp, t0 + Duration::from_millis(90));
        assert_eq!(
            ring.closest_to(t0 + Duration::from_millis(81)).unwrap().timestamp,
            t0 + Duration::from_millis(80)
        );
    }
}
