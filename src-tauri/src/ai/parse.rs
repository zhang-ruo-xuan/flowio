use crate::ai::types::StepOutput;

/// Extract a `StepOutput` from a raw AI response string.
///
/// Handles common LLM output patterns:
/// - Pure JSON: `{"title": "...", ...}`
/// - Markdown code-fenced JSON: ` ```json ... ``` `
/// - Loose code-fenced JSON: ` ``` ... ``` `
///
/// After successful parsing, performs an `action_type` consistency check:
/// if the AI returned `action_type` differs from `original_action_type`,
/// appends `[AI检测异常]` to the title and logs a warning — but still
/// returns the step (never discarded due to action_type mismatch alone).
pub fn parse_step_output(raw: &str, original_action_type: &str) -> Result<StepOutput, String> {
    let trimmed = raw.trim();

    // Try direct JSON parse first
    let mut step = if let Ok(step) = serde_json::from_str::<StepOutput>(trimmed) {
        step
    } else {
        // Try to extract JSON from markdown code blocks
        let json_str = extract_json_from_markdown(trimmed)?;
        serde_json::from_str::<StepOutput>(&json_str).map_err(|e| {
            format!(
                "Failed to parse StepOutput from JSON: {}. Raw: {}",
                e,
                &json_str[..json_str.len().min(200)]
            )
        })?
    };

    // ── action_type consistency check (case-insensitive + alias normalization) ──
    let normalized_original = normalize_action_type(original_action_type);
    let normalized_ai = normalize_action_type(&step.action_type);
    if !original_action_type.is_empty() && normalized_ai != normalized_original {
        log::warn!(
            "action_type mismatch: AI returned '{}' (normalized: '{}') but original was '{}' (normalized: '{}'), appending marker to title",
            step.action_type, normalized_ai,
            original_action_type, normalized_original
        );
        step.title = format!("{}[AI检测异常]", step.title);
    }

    Ok(step)
}

/// Attempt to extract a JSON substring from a string that may be
/// wrapped in a markdown code fence.
fn extract_json_from_markdown(input: &str) -> Result<String, String> {
    // Pattern: ```json\n{...}\n``` or ```\n{...}\n```
    let fence_patterns = ["```json\n", "```json", "```\n", "```"];

    for prefix in &fence_patterns {
        if let Some(start) = input.find(prefix) {
            let after_fence = &input[start + prefix.len()..];
            // Find matching closing fence
            if let Some(end) = after_fence.find("\n```") {
                let inner = &after_fence[..end].trim();
                return Ok(inner.to_string());
            }
            if let Some(end) = after_fence.find("```") {
                let inner = &after_fence[..end].trim();
                return Ok(inner.to_string());
            }
            // No closing fence — treat rest as JSON
            return Ok(after_fence.trim().to_string());
        }
    }

    // Fallback: find first '{' and last '}'
    let first_brace = input
        .find('{')
        .ok_or_else(|| "No JSON object found in AI response".to_string())?;
    let last_brace = input
        .rfind('}')
        .ok_or_else(|| "No closing brace in AI response".to_string())?;

    if last_brace <= first_brace {
        return Err("Malformed JSON brackets in AI response".to_string());
    }

    Ok(input[first_brace..=last_brace].to_string())
}

/// Normalize action_type to its canonical form to eliminate false
/// mismatches caused by case differences or synonym aliases.
///
/// Rules:
/// - Case-insensitive: all input is lowercased first.
/// - Input-like aliases ("key", "type", "input", "keyboard") → "type".
/// - Click-like aliases ("click", "tap", "press") → "click".
/// - Everything else passes through as-is (lowercased).
fn normalize_action_type(action_type: &str) -> String {
    let lower = action_type.to_lowercase();
    match lower.as_str() {
        "key" | "type" | "input" | "keyboard" => "type".to_string(),
        "click" | "tap" | "press" => "click".to_string(),
        _ => lower,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_json() {
        let raw =
            r#"{"title":"点击登录","description":"点击登录按钮","action_type":"click","tip":""}"#;
        let result = parse_step_output(raw, "click").unwrap();
        assert_eq!(result.title, "点击登录");
    }

    #[test]
    fn test_markdown_fenced() {
        let raw = "```json\n{\"title\":\"输入账号\",\"description\":\"在输入框输入账号\",\"action_type\":\"input\",\"tip\":\"\"}\n```";
        let result = parse_step_output(raw, "input").unwrap();
        assert_eq!(result.title, "输入账号");
    }

    #[test]
    fn test_loose_json() {
        let raw = "好的，这是分析结果：\n{\"title\":\"滚动页面\",\"description\":\"向下滚动\",\"action_type\":\"scroll\",\"tip\":\"\"}";
        let result = parse_step_output(raw, "scroll").unwrap();
        assert_eq!(result.title, "滚动页面");
    }

    #[test]
    fn test_action_type_mismatch() {
        let raw =
            r#"{"title":"点击投稿","description":"点击投稿按钮","action_type":"click","tip":""}"#;
        // action_type matches "click" → no marker appended
        let result = parse_step_output(raw, "click").unwrap();
        assert_eq!(result.title, "点击投稿");
        assert!(!result.title.contains("[AI检测异常]"));

        // action_type mismatch: AI returned "click" but original was "input"
        let result2 = parse_step_output(raw, "input").unwrap();
        assert!(result2.title.contains("[AI检测异常]"));
        assert_eq!(result2.title, "点击投稿[AI检测异常]");
    }

    #[test]
    fn test_action_type_case_insensitive() {
        // "CLICK" (recording side uppercase) vs "click" (AI lowercase)
        let raw =
            r#"{"title":"点击登录","description":"点击登录按钮","action_type":"click","tip":""}"#;
        let result = parse_step_output(raw, "CLICK").unwrap();
        assert_eq!(result.title, "点击登录");
        assert!(!result.title.contains("[AI检测异常]"));
    }

    #[test]
    fn test_action_type_alias_normalization() {
        // "tap" is an alias for "click" — should NOT trigger mismatch
        let raw =
            r#"{"title":"点击按钮","description":"点击提交按钮","action_type":"tap","tip":""}"#;
        let result = parse_step_output(raw, "click").unwrap();
        assert_eq!(result.title, "点击按钮");
        assert!(!result.title.contains("[AI检测异常]"));

        // "key" is an alias for "type" — should NOT trigger mismatch
        let raw2 =
            r#"{"title":"输入文字","description":"输入用户名","action_type":"key","tip":""}"#;
        let result2 = parse_step_output(raw2, "type").unwrap();
        assert_eq!(result2.title, "输入文字");
        assert!(!result2.title.contains("[AI检测异常]"));
    }
}
