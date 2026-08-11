pub mod activity;
pub mod capture;
pub mod commands;
pub mod ime;
pub mod state;
pub mod screenshot;
pub mod keyboard;
pub mod mouse;
pub mod hook;
pub mod recorder;

use serde::{Deserialize, Serialize};
use state::RecordingState;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Debug, Serialize, Deserialize)]
pub struct StepData {
    pub id: String,
    pub order_index: i32,
    pub step_number: i32,
    pub action_type: String,
    pub title: String,
    pub description: String,
    pub tip: Option<String>,
    pub screenshot_path: String,
    pub screenshot_base64: Option<String>,
    pub annotated_path: String,
    pub before_screenshot_path: String,
    /// Click / tap coordinate X (screen pixels)
    pub x: Option<i32>,
    /// Click / tap coordinate Y (screen pixels)
    pub y: Option<i32>,
    /// Path to the post-action ("after") JPEG screenshot
    pub after_screenshot_path: Option<String>,
    /// Unix epoch milliseconds when the step was recorded
    pub timestamp: i64,
    /// Active window title at the time of capture
    pub window_title: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Recorder {
    pub id: String,
    pub title: String,
    pub state: RecordingState,
    pub steps: Vec<StepData>,
    pub started_at: Option<String>,
    pub paused_at: Option<String>,

    /// Background event-processing thread handle.
    #[serde(skip)]
    pub event_thread: Option<std::thread::JoinHandle<()>>,

    /// Atomic flag to signal the event thread to stop.
    #[serde(skip)]
    pub running_flag: Option<Arc<AtomicBool>>,

    /// Steps pushed by the event thread during an active recording
    /// session are accumulated here.  `finish_recording` drains them
    /// into `self.steps`.
    #[serde(skip)]
    pub captured_steps: Option<Arc<Mutex<Vec<StepData>>>>,
}
