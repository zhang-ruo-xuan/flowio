use serde::{Deserialize, Serialize};

/// AI provider configuration (model vendor).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
}

/// Complete AI configuration including provider and API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: AiProvider,
    pub api_key: String,
}

/// Input for a single step in the AI pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInput {
    pub title: String,
    /// Base64-encoded marked screenshot (JPEG) — with red circle annotation for clicks.
    pub screenshot_base64: String,
    /// Base64-encoded clean before screenshot (JPEG) — no annotations, full context.
    pub before_screenshot_base64: String,
    pub description: String,
    /// Click coordinates from the recording hook (primary monitor pixels).
    pub click_x: Option<i32>,
    pub click_y: Option<i32>,
    /// Action type from recording: click / type / scroll / drag / navigate.
    pub action_type: String,
    /// Window title of the target application when the step was recorded.
    pub window_title: String,
}

/// AI-generated output for a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepOutput {
    pub title: String,
    pub description: String,
    pub action_type: String,
    pub tip: String,
}

/// A batch job to send through the AI pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiJob {
    pub recording_id: String,
    pub steps: Vec<StepInput>,
}

/// Result returned by the AI pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResult {
    pub steps: Vec<StepOutput>,
    pub errors: Vec<String>,
}
