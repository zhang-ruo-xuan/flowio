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
    /// Focused element rect size (screen px); anchors Scribe-style adaptive crop.
    #[serde(default)]
    pub crop_w: Option<i32>,
    #[serde(default)]
    pub crop_h: Option<i32>,
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
    /// 用户手动标记的「表格软件模式」：识别时使用全图 + 表格专项 prompt，
    /// 即使窗口标题不含 Excel/WPS 等关键词也能精准识别单元格/函数。
    #[serde(default)]
    pub spreadsheet_mode: bool,
}

/// Result returned by the AI pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResult {
    pub steps: Vec<StepOutput>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod types_tests {
    use super::*;

    #[test]
    fn ai_config_json_roundtrip() {
        let cfg = AiConfig {
            provider: AiProvider {
                id: "zhipu".into(),
                name: "智谱".into(),
                base_url: "https://open.bigmodel.cn/api/paas/v4/chat/completions".into(),
                model: "glm-4v".into(),
            },
            api_key: "sk-xxxx".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: AiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider.id, "zhipu");
        assert_eq!(back.provider.model, "glm-4v");
        assert_eq!(back.api_key, "sk-xxxx");
    }

    #[test]
    fn step_input_optional_coords_roundtrip() {
        let s = StepInput {
            title: "t".into(),
            screenshot_base64: "".into(),
            before_screenshot_base64: "".into(),
            description: "d".into(),
            click_x: Some(100),
            click_y: None,
            crop_w: None,
            crop_h: None,
            action_type: "click".into(),
            window_title: "w".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: StepInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.click_x, Some(100));
        assert_eq!(back.click_y, None);
    }

    #[test]
    fn ai_result_roundtrip() {
        let r = AiResult {
            steps: vec![StepOutput {
                title: "t".into(),
                description: "d".into(),
                action_type: "click".into(),
                tip: "tip".into(),
            }],
            errors: vec!["e1".into()],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: AiResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.steps.len(), 1);
        assert_eq!(back.steps[0].action_type, "click");
        assert_eq!(back.errors, vec!["e1".to_string()]);
    }

    #[test]
    fn step_output_missing_optional_fields_not_allowed() {
        // StepOutput 无 #[serde(default)] 字段，缺字段应解析失败（前端必须传齐）
        let json = r#"{"title":"t"}"#;
        assert!(serde_json::from_str::<StepOutput>(json).is_err());
    }
}
