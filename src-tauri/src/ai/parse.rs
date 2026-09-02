use crate::ai::types::StepOutput;

/// Extract a `StepOutput` from a raw AI response string.
///
/// Handles common LLM output patterns:
/// - Pure JSON: `{"title": "...", ...}`
/// - Markdown code-fenced JSON: ` ```json ... ``` `
/// - Loose code-fenced JSON: ` ``` ... ``` `
///
/// Also sanitizes invalid escape sequences that some LLMs produce
/// (e.g. `\s`, `\(`, `\.`) which are valid in regex strings but
/// **not** in JSON — they cause `serde_json` to error with
/// "invalid escape at line 1 column N".
///
/// After successful parsing, performs an `action_type` consistency check:
/// if the AI returned `action_type` differs from `original_action_type`,
/// appends `[AI检测异常]` to the title and logs a warning — but still
/// returns the step (never discarded due to action_type mismatch alone).
pub fn parse_step_output(raw: &str, original_action_type: &str) -> Result<StepOutput, String> {
    let trimmed = raw.trim();

    // Sanitize invalid escape sequences once, before any parsing attempt.
    // Some LLMs emit non-JSON escapes (\s, \(, \., etc.) that cause
    // serde_json to error with "invalid escape at line 1 column N".
    let sanitized = sanitize_json_escapes(trimmed);

    // Try direct JSON parse first
    let mut step = if let Ok(step) = serde_json::from_str::<StepOutput>(&sanitized) {
        step
    } else {
        // Try to extract JSON from markdown code blocks
        let json_str = extract_json_from_markdown(&sanitized)?;
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

/// Sanitize invalid JSON escape sequences produced by some LLMs.
///
/// JSON only allows these escapes: `\" \\ \/ \b \f \n \r \t \uXXXX`.
/// LLMs sometimes emit regex-style escapes like `\s` `\(` `\.` or
/// Chinese-punctuation escapes, which cause `serde_json::from_str` to fail
/// with "invalid escape at line 1 column N".
///
/// This function scans for `\` followed by an invalid escape character and
/// replaces it with `\\` (escaped backslash) + the character — effectively
/// treating the backslash as a literal rather than an escape introducer.
///
/// It is careful to **not** break valid `\uXXXX` sequences (exactly 4 hex
/// digits after `\u`) and preserves all standard JSON escapes.
fn sanitize_json_escapes(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            // Check if this is a valid JSON escape
            let is_valid = match next {
                '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' => true,
                'u' => {
                    // \u must be followed by exactly 4 hex digits
                    let mut ok = true;
                    for j in 2..=5 {
                        if i + j >= chars.len() || !chars[i + j].is_ascii_hexdigit() {
                            ok = false;
                            break;
                        }
                    }
                    ok
                }
                _ => false,
            };
            if is_valid {
                // Valid JSON escape — keep as-is
                out.push('\\');
                out.push(next);
                // For \uXXXX, also copy the 4 hex digits
                if next == 'u' {
                    for j in 2..=5 {
                        out.push(chars[i + j]);
                    }
                    i += 6;
                    continue;
                }
                i += 2;
                continue;
            }
            // Invalid escape: convert `\x` → `\\x`
            out.push_str("\\\\");
            out.push(next);
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
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


    #[test]
    fn test_no_fence_no_braces_errors() {
        assert!(parse_step_output("没有 JSON 内容", "click").is_err());
        assert!(parse_step_output("", "click").is_err());
        assert!(parse_step_output("   ", "click").is_err());
    }

    #[test]
    fn test_multiple_code_blocks_first_wins() {
        let raw = "```json\n{\"title\":\"第一个\",\"description\":\"\",\"action_type\":\"click\",\"tip\":\"\"}\n```\n```json\n{\"title\":\"第二个\",\"description\":\"\",\"action_type\":\"click\",\"tip\":\"\"}\n```";
        let result = parse_step_output(raw, "click").unwrap();
        assert_eq!(result.title, "第一个");
    }

    #[test]
    fn test_unclosed_fence_takes_rest() {
        let raw = "```json\n{\"title\":\"未闭合\",\"description\":\"\",\"action_type\":\"click\",\"tip\":\"\"}";
        let result = parse_step_output(raw, "click").unwrap();
        assert_eq!(result.title, "未闭合");
    }

    #[test]
    fn test_trailing_text_after_brace_ignored() {
        let raw = "{\"title\":\"标题\",\"description\":\"\",\"action_type\":\"click\",\"tip\":\"\"} 后面有垃圾文字";
        let result = parse_step_output(raw, "click").unwrap();
        assert_eq!(result.title, "标题");
    }

    #[test]
    fn test_non_object_json_errors() {
        assert!(parse_step_output("[1,2,3]", "click").is_err());
    }

    #[test]
    fn test_normalize_action_type_unknown_and_empty() {
        assert_eq!(normalize_action_type("SCROLL"), "scroll");
        assert_eq!(normalize_action_type("drag"), "drag");
        assert_eq!(normalize_action_type(""), "");
        assert_eq!(normalize_action_type("Key"), "type");
        assert_eq!(normalize_action_type("PRESS"), "click");
    }

    #[test]
    fn test_action_type_mismatch_empty_original_no_marker() {
        let raw = r#"{"title":"点击","description":"","action_type":"click","tip":""}"#;
        let result = parse_step_output(raw, "").unwrap();
        assert_eq!(result.title, "点击");
        assert!(!result.title.contains("[AI检测异常]"));
    }

    // ── sanitize_json_escapes ─────────────────────────────────

    #[test]
    fn test_sanitize_preserves_valid_escapes() {
        // All standard JSON escapes must pass through unchanged
        let input = r#"{"a":"\" \\ \/ \b \f \n \r \t"}"#;
        let out = sanitize_json_escapes(input);
        assert_eq!(out, input);
    }

    #[test]
    fn test_sanitize_fixes_invalid_escapes() {
        // \s \( \) \. are NOT valid JSON escapes
        let input = r#"{"title":"测试\s内容\(重要\)"}"#;
        let out = sanitize_json_escapes(input);
        // After sanitization: \s → \\s, \( → \\(, \) → \\)
        assert!(!out.contains("\\s") || out.contains("\\\\s")); // should be double-backslash now
        // Must be parseable as JSON
        let v: serde_json::Value = serde_json::from_str(&out).expect("sanitized JSON should parse");
        assert_eq!(v["title"].as_str().unwrap(), "测试\\s内容\\(重要\\)");
    }

    #[test]
    fn test_sanitize_handles_invalid_u_escape() {
        // \u not followed by 4 hex digits
        let input = r#"{"desc":"error\u123"}"#;
        let out = sanitize_json_escapes(input);
        // Should not panic and should produce valid JSON
        let v: serde_json::Value = serde_json::from_str(&out).expect("should parse after sanitize");
        assert!(v["desc"].as_str().unwrap().contains("\\u"));
    }

    #[test]
    fn test_parse_with_invalid_escapes_succeeds() {
        // Full integration: LLM output with invalid escapes should parse OK
        let raw = r#"{"title":"在\s输入框\(\)中输入","description":"点击\.按钮","action_type":"type","tip":""}"#;
        let result = parse_step_output(raw, "type").unwrap();
        assert!(result.title.contains("输入框"));
        assert_eq!(result.action_type, "type");
    }
}
