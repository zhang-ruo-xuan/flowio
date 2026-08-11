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
        self.steps.push(step);
        Ok(())
    }
}
