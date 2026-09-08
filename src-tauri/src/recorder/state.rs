use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RecordingState {
    Idle,
    Recording,
    Paused,
    Completed,
    Cancelled,
}

use super::{Recorder, StepData};

/// 单次录制最大步骤数（`spec.md:160`「超出 200 步提示并自动停止」）。
pub const MAX_RECORDING_STEPS: usize = 200;

impl Recorder {
    pub fn new(id: String, title: String) -> Self {
        Recorder {
            id,
            title,
            state: RecordingState::Idle,
            steps: Vec::new(),
            started_at: None,
            paused_at: None,
            event_thread: None,
            running_flag: None,
            captured_steps: None,
            record_self: false,
            target_window: None,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.state != RecordingState::Idle {
            return Err(format!(
                "Cannot start recording from state {:?}",
                self.state
            ));
        }
        self.state = RecordingState::Recording;
        self.started_at = Some(chrono::Utc::now().to_rfc3339());
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), String> {
        if self.state != RecordingState::Recording {
            return Err(format!(
                "Cannot pause recording from state {:?}",
                self.state
            ));
        }
        // Signal all background threads to stop
        if let Some(ref flag) = self.running_flag {
            flag.store(false, Ordering::Relaxed);
        }
        // Wait for the event-processing thread to exit
        if let Some(handle) = self.event_thread.take() {
            eprintln!("[pause] waiting for event thread to exit...");
            let _ = handle.join();
            eprintln!("[pause] event thread joined");
        }
        self.state = RecordingState::Paused;
        self.paused_at = Some(chrono::Utc::now().to_rfc3339());
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), String> {
        if self.state != RecordingState::Paused {
            return Err(format!(
                "Cannot resume recording from state {:?}",
                self.state
            ));
        }
        self.state = RecordingState::Recording;
        self.paused_at = None;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if self.state != RecordingState::Recording && self.state != RecordingState::Paused {
            return Err(format!(
                "Cannot finish recording from state {:?}",
                self.state
            ));
        }
        self.state = RecordingState::Completed;
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), String> {
        self.state = RecordingState::Cancelled;
        Ok(())
    }

    pub fn push_step(&mut self, step: StepData) -> Result<(), String> {
        if self.state != RecordingState::Recording {
            return Err(format!(
                "Cannot push step while recording state is {:?}",
                self.state
            ));
        }
        // B5: 超出 200 步上限时拒绝（spec.md:160 约定「超出则提示并停止」）。
        if self.steps.len() >= MAX_RECORDING_STEPS {
            return Err(format!(
                "录制步骤数已达上限（{} 步），请结束本次录制",
                MAX_RECORDING_STEPS
            ));
        }
        self.steps.push(step);
        Ok(())
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::recorder::StepData;

    fn make_step() -> StepData {
        StepData {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None,
            id: "s1".into(),
            order_index: 0,
            step_number: 1,
            action_type: "screenshot".into(),
            title: "t".into(),
            description: String::new(),
            tip: None,
            screenshot_path: String::new(),
            screenshot_base64: None,
            annotated_path: String::new(),
            before_screenshot_path: String::new(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            after_screenshot_path: None,
            timestamp: 0,
            window_title: String::new(),
            is_sensitive: false,
            hotkey_combo: String::new(),
            uia_anchor: String::new(),
            mouse_button: String::new(),
        }
    }

    #[test]
    fn new_starts_idle() {
        let r = Recorder::new("id1".into(), "title".into());
        assert_eq!(r.state, RecordingState::Idle);
        assert!(r.steps.is_empty());
        assert!(r.started_at.is_none());
        assert!(r.paused_at.is_none());
    }

    #[test]
    fn start_sets_recording_and_started_at() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        assert_eq!(r.state, RecordingState::Recording);
        assert!(r.started_at.is_some());
    }

    #[test]
    fn start_from_non_idle_fails() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        let err = r.start().unwrap_err();
        assert!(err.contains("Cannot start recording"));
        assert_eq!(r.state, RecordingState::Recording);
    }

    #[test]
    fn pause_only_from_recording() {
        let mut r = Recorder::new("id1".into(), "t".into());
        assert!(r.pause().is_err()); // Idle 不能 pause
        r.start().unwrap();
        r.pause().unwrap();
        assert_eq!(r.state, RecordingState::Paused);
        assert!(r.paused_at.is_some());
    }

    #[test]
    fn resume_clears_paused_at() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.pause().unwrap();
        assert!(r.paused_at.is_some());
        r.resume().unwrap();
        assert_eq!(r.state, RecordingState::Recording);
        assert!(r.paused_at.is_none());
    }

    #[test]
    fn resume_only_from_paused() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        let err = r.resume().unwrap_err();
        assert!(err.contains("Cannot resume"));
    }

    #[test]
    fn finish_from_recording_or_paused() {
        let mut r = Recorder::new("id1".into(), "t".into());
        assert!(r.finish().is_err()); // Idle 不能 finish
        r.start().unwrap();
        r.finish().unwrap();
        assert_eq!(r.state, RecordingState::Completed);
        assert!(r.finish().is_err()); // Completed 不能再次 finish
    }

    #[test]
    fn finish_from_paused_ok() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.pause().unwrap();
        r.finish().unwrap();
        assert_eq!(r.state, RecordingState::Completed);
    }

    #[test]
    fn cancel_any_state_ok() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.cancel().unwrap();
        assert_eq!(r.state, RecordingState::Cancelled);
        let mut r2 = Recorder::new("id2".into(), "t".into());
        r2.start().unwrap();
        r2.cancel().unwrap();
        assert_eq!(r2.state, RecordingState::Cancelled);
    }

    #[test]
    fn cancel_allows_clearing_steps() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.push_step(make_step()).unwrap();
        assert_eq!(r.steps.len(), 1);
        r.cancel().unwrap();
        assert_eq!(r.state, RecordingState::Cancelled);
        // 模拟 commands::cancel_recording 行为：cancel 后清空步骤
        r.steps.clear();
        assert!(r.steps.is_empty());
    }

    #[test]
    fn push_step_only_when_recording() {
        let mut r = Recorder::new("id1".into(), "t".into());
        assert!(r.push_step(make_step()).is_err()); // Idle 不允许
        r.start().unwrap();
        r.push_step(make_step()).unwrap();
        assert_eq!(r.steps.len(), 1);
        r.pause().unwrap();
        assert!(r.push_step(make_step()).is_err()); // Paused 不允许
    }


    #[test]
    fn finish_then_push_step_fails() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.finish().unwrap();
        assert!(r.push_step(make_step()).is_err());
        assert_eq!(r.steps.len(), 0);
    }

    #[test]
    fn cancel_then_finish_fails() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.cancel().unwrap();
        let err = r.finish().unwrap_err();
        assert!(err.contains("Cannot finish"));
    }

    #[test]
    fn pause_then_resume_then_pause_cycle() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.pause().unwrap();
        r.resume().unwrap();
        r.pause().unwrap();
        assert_eq!(r.state, RecordingState::Paused);
        assert!(r.paused_at.is_some());
    }

    #[test]
    fn cancel_after_completed_keeps_cancelled() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.finish().unwrap();
        r.cancel().unwrap();
        assert_eq!(r.state, RecordingState::Cancelled);
    }

    #[test]
    fn start_keeps_original_id_and_title() {
        let mut r = Recorder::new("my-id".into(), "我的录制".into());
        r.start().unwrap();
        assert_eq!(r.id, "my-id");
        assert_eq!(r.title, "我的录制");
    }

    #[test]
    fn finish_preserves_steps() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        r.push_step(make_step()).unwrap();
        r.push_step(make_step()).unwrap();
        r.finish().unwrap();
        assert_eq!(r.steps.len(), 2);
        assert_eq!(r.state, RecordingState::Completed);
    }

    // B5 回归：单次录制 200 步上限——第 200 步允许入录，第 201 步被拒绝且总数不变
    #[test]
    fn push_step_enforces_max_recording_steps() {
        let mut r = Recorder::new("id1".into(), "t".into());
        r.start().unwrap();
        for _ in 0..MAX_RECORDING_STEPS {
            r.push_step(make_step()).unwrap();
        }
        assert_eq!(r.steps.len(), MAX_RECORDING_STEPS);
        let err = r.push_step(make_step()).unwrap_err();
        assert!(err.contains("上限") || err.contains("200"), "err: {}", err);
        assert_eq!(r.steps.len(), MAX_RECORDING_STEPS);
    }
}
