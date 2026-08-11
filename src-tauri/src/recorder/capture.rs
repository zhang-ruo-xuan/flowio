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
pub struct CapturedFrame {
    pub timestamp: Instant,
    pub image: DynamicImage,
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
pub fn start_capture_thread(
    running: Arc<AtomicBool>,
    frame_tx: mpsc::Sender<CapturedFrame>,
    interval: Duration,
) {
    std::thread::spawn(move || {
        eprintln!(
            "[CAPTURE] continuous capture thread started (interval: {:?})",
            interval
        );
        while running.load(Ordering::Relaxed) {
            let loop_start = Instant::now();

            match screenshot::capture_fullscreen() {
                Ok(img) => {
                    let frame = CapturedFrame {
                        timestamp: Instant::now(),
                        image: img,
                    };
                    if frame_tx.send(frame).is_err() {
                        eprintln!("[CAPTURE] frame channel closed, exiting");
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("[CAPTURE] frame capture failed: {}", e);
                }
            }

            let elapsed = loop_start.elapsed();
            if elapsed < interval {
                std::thread::sleep(interval - elapsed);
            }
        }
        eprintln!("[CAPTURE] capture thread exiting");
    });
}
