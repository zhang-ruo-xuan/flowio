pub mod activity;
pub mod capture;
pub mod commands;
pub mod ime;
pub mod state;
pub mod screenshot;
pub mod selection_bridge;
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
    /// Focused element rectangle width (screen pixels), for Scribe-style crop.
    #[serde(default)]
    pub crop_w: Option<i32>,
    /// Focused element rectangle height (screen pixels), for Scribe-style crop.
    #[serde(default)]
    pub crop_h: Option<i32>,
    /// Path to the post-action ("after") JPEG screenshot
    pub after_screenshot_path: Option<String>,
    /// Unix epoch milliseconds when the step was recorded
    pub timestamp: i64,
    /// Active window title at the time of capture
    pub window_title: String,
    /// 此步骤是否涉及敏感输入（如密码框）。敏感输入的明文值不会记录/导出，
    /// 描述会被替换为 `[密码已隐藏]`。缺省为 false 以兼容旧数据。
    #[serde(default)]
    pub is_sensitive: bool,
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

    /// When `true`, clicks/keystrokes inside Flowio's own window are
    /// recorded too (used for authoring "录步使用手册" tutorials).
    /// Default `false`: Flowio's own window is self-filtered.
    #[serde(default)]
    pub record_self: bool,

    /// When `Some(process_name)` (录制指定软件), only events inside that app's
    /// window are recorded and captured frames are cropped to that window.
    #[serde(default)]
    pub target_window: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step_data(id: &str, n: i32) -> StepData {
        StepData {
            id: id.into(),
            order_index: n,
            step_number: n,
            action_type: "click".into(),
            title: format!("Title {}", n),
            description: format!("Desc {}", n),
            tip: Some(format!("Tip {}", n)),
            screenshot_path: format!("shot_{}.png", n),
            screenshot_base64: None,
            annotated_path: format!("annotated_{}.png", n),
            before_screenshot_path: format!("before_{}.png", n),
            x: Some(n),
            y: Some(n * 2),
            crop_w: None,
            crop_h: None,
            after_screenshot_path: Some(format!("after_{}.png", n)),
            timestamp: 1_700_000_000_000 + n as i64,
            window_title: format!("Window {}", n),
            is_sensitive: false,
        }
    }

    #[test]
    fn step_data_serde_roundtrip() {
        let s = step_data("step-1", 3);
        let json = serde_json::to_string(&s).unwrap();
        let back: StepData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "step-1");
        assert_eq!(back.order_index, 3);
        assert_eq!(back.action_type, "click");
        assert_eq!(back.x, Some(3));
        assert_eq!(back.y, Some(6));
        assert_eq!(back.after_screenshot_path.as_deref(), Some("after_3.png"));
        assert_eq!(back.timestamp, 1_700_000_000_003);
        assert_eq!(back.window_title, "Window 3");
        assert_eq!(back.screenshot_base64, None);
        assert_eq!(back.is_sensitive, false);
    }

    #[test]
    fn step_data_deserializes_missing_optionals_as_null() {
        let json = r#"{
            "id":"s2","order_index":0,"step_number":1,
            "action_type":"key","title":"T","description":"D",
            "tip":null,"screenshot_path":"shot.png","screenshot_base64":null,
            "annotated_path":"ann.png","before_screenshot_path":"before.png",
            "x":null,"y":null,"after_screenshot_path":null,
            "timestamp":0,"window_title":"W"
        }"#;
        let s: StepData = serde_json::from_str(json).unwrap();
        assert_eq!(s.tip, None);
        assert_eq!(s.x, None);
        assert_eq!(s.y, None);
        assert_eq!(s.after_screenshot_path, None);
        // is_sensitive 是新增字段，缺省时按 false 解析（兼容旧数据）
        assert_eq!(s.is_sensitive, false);
    }

    #[test]
    fn step_data_roundtrip_is_stable() {
        let s = step_data("step-stable", 7);
        let json = serde_json::to_string(&s).unwrap();
        let back: StepData = serde_json::from_str(&json).unwrap();
        let json2 = serde_json::to_string(&back).unwrap();
        assert_eq!(json, json2);
    }

    #[test]
    fn recorder_serde_roundtrip_skips_thread_fields() {
        let mut r = Recorder::new("rec-1".into(), "My Recording".into());
        r.state = RecordingState::Recording;
        r.started_at = Some("2026-08-20T10:00:00+08:00".into());
        r.steps.push(step_data("s1", 1));
        r.steps.push(step_data("s2", 2));

        let json = serde_json::to_string(&r).unwrap();
        // skip 字段不得出现在序列化结果中
        assert!(!json.contains("event_thread"));
        assert!(!json.contains("running_flag"));
        assert!(!json.contains("captured_steps"));

        let back: Recorder = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "rec-1");
        assert_eq!(back.title, "My Recording");
        assert_eq!(back.state, RecordingState::Recording);
        assert_eq!(back.steps.len(), 2);
        assert_eq!(back.steps[1].id, "s2");
        assert!(back.event_thread.is_none());
        assert!(back.running_flag.is_none());
        assert!(back.captured_steps.is_none());
    }

    #[test]
    fn recorder_serde_ignores_runtime_fields_when_present() {
        // 即使运行时字段已填充，序列化也必须忽略（防死锁/循环引用）
        let r = Recorder {
            id: "rec-rt".into(),
            title: "RT".into(),
            state: RecordingState::Paused,
            steps: vec![step_data("s1", 1)],
            started_at: None,
            paused_at: Some("2026-08-20T11:00:00+08:00".into()),
            event_thread: None,
            running_flag: Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true))),
            captured_steps: Some(std::sync::Arc::new(Mutex::new(vec![step_data("s2", 2)]))),
            record_self: false,
            target_window: Some("wps.exe".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("running_flag"));
        assert!(!json.contains("captured_steps"));
        let back: Recorder = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state, RecordingState::Paused);
        assert_eq!(back.paused_at.as_deref(), Some("2026-08-20T11:00:00+08:00"));
        assert_eq!(back.target_window.as_deref(), Some("wps.exe"));
    }

    #[test]
    fn recording_state_serde_variants() {
        let cases = [
            (RecordingState::Idle, "\"Idle\""),
            (RecordingState::Recording, "\"Recording\""),
            (RecordingState::Paused, "\"Paused\""),
            (RecordingState::Completed, "\"Completed\""),
            (RecordingState::Cancelled, "\"Cancelled\""),
        ];
        for (state, expected) in cases {
            assert_eq!(serde_json::to_string(&state).unwrap(), expected);
        }
    }

    #[test]
    fn recorder_new_defaults() {
        let r = Recorder::new("rec-new".into(), "New".into());
        assert_eq!(r.state, RecordingState::Idle);
        assert!(r.steps.is_empty());
        assert_eq!(r.started_at, None);
        assert_eq!(r.paused_at, None);
        assert!(r.event_thread.is_none());
        assert!(r.running_flag.is_none());
        assert!(r.captured_steps.is_none());
    }
}
