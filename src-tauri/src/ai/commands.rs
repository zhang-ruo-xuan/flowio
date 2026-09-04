// v2: per-provider config with default provider
use base64::Engine;
use rusqlite::params;
use std::time::Duration;
use tauri::State;

use crate::ai::pipeline;
use crate::ai::types::{AiConfig, AiJob, AiProvider, AiResult, StepInput};
use crate::DbState;

// ── Data structures ──────────────────────────────────────────

struct StepRow {
    id: String,
    title: String,
    description: String,
    screenshot_path: String,
    before_screenshot_path: String,
    after_screenshot_path: String,
    action_type: String,
    window_title: String,
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
}

/// 读取文件并编码为 base64；空路径或读取失败返回空字符串。
fn read_file_base64(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        std::fs::read(path)
            .ok()
            .map(|data| base64::engine::general_purpose::STANDARD.encode(&data))
            .unwrap_or_default()
    }
}

/// 将数据库行转换为 AI 管道的步骤输入。
fn row_to_step_input(row: &StepRow) -> StepInput {
    StepInput {
        title: row.title.clone(),
        screenshot_base64: read_file_base64(&row.screenshot_path),
        before_screenshot_base64: read_file_base64(&row.before_screenshot_path),
        description: row.description.clone(),
        click_x: row.x,
        click_y: row.y,
        crop_w: row.crop_w,
        crop_h: row.crop_h,
        action_type: row.action_type.clone(),
        window_title: row.window_title.clone(),
    }
}

/// Run the AI pipeline on a blocking thread.
/// Does CPU-heavy base64 encoding of screenshots and network I/O.
fn run_ai_pipeline_sync(
    config: &AiConfig,
    step_rows: &[StepRow],
    recording_id: String,
    domain_mode: Option<String>,
    target_app: Option<String>,
) -> Result<AiResult, String> {
    let step_inputs: Vec<StepInput> = step_rows.iter().map(row_to_step_input).collect();

    let job = AiJob {
        recording_id,
        steps: step_inputs,
        domain_mode,
        target_app,
    };

    pipeline::run_pipeline(config, job)
}

// ── Tauri Commands ────────────────────────────────────────────

/// 归一化并聚合错误：剥离「Step N 」前缀让同类错误合并，按出现次数降序取前 3 条，
/// 格式「消息（×N）」，总长按字符安全截断到 160。
fn summarize_error_reasons(errors: &[String]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for err in errors {
        // 剥离 "Step 12 " 前缀（LLM error / parse error / validation 共用）
        let norm = match err.strip_prefix("Step ") {
            Some(rest) => match rest.find(' ') {
                Some(pos) => &rest[pos + 1..],
                None => err.as_str(),
            },
            None => err.as_str(),
        };
        match counts.iter_mut().find(|(m, _)| m == norm) {
            Some((_, c)) => *c += 1,
            None => counts.push((norm.to_string(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let parts: Vec<String> = counts
        .iter()
        .take(3)
        .map(|(m, c)| {
            if *c > 1 {
                format!("{}（×{}）", m, c)
            } else {
                m.clone()
            }
        })
        .collect();
    let detail = parts.join("；");
    if detail.chars().count() > 160 {
        format!("{}...", detail.chars().take(157).collect::<String>())
    } else {
        detail
    }
}

/// Append AI pipeline errors to the Flowio debug log (same file as the
/// recorder's capture log; release builds have no console/stderr).
pub(crate) fn ai_dbg_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(r"D:\录步\flowio_debug.log")
    {
        let _ = writeln!(f, "{}", msg);
    }
}

/// 汇总 AI 生成结果，返回面向用户的状态提示语。
/// - 全部失败：聚合原因（最多 3 条）后返回 Err
/// - 未生成任何步骤：返回固定 Err
/// - 部分失败：返回 Ok 摘要 + 聚合原因（不再黑箱只报数量）
/// - 全部成功：返回 Ok 摘要
fn summarize_generate_result(
    generated: usize,
    total_steps: usize,
    error_count: usize,
    errors: &[String],
) -> Result<String, String> {
    if generated == 0 && error_count > 0 {
        Err(format!(
            "{} 个步骤全部失败。原因：{}",
            error_count,
            summarize_error_reasons(errors)
        ))
    } else if generated == 0 {
        Err("未生成任何步骤。请检查 AI 服务商配置".to_string())
    } else if error_count > 0 {
        Ok(format!(
            "AI 生成完成：{} / {} 个步骤已生成（{} 个失败）。原因：{}",
            generated,
            total_steps,
            error_count,
            summarize_error_reasons(errors)
        ))
    } else {
        Ok(format!("AI 生成完成：全部 {} 个步骤已生成", generated))
    }
}

/// Set the default AI provider in the settings table.
#[tauri::command]
pub fn set_default_provider(db: State<DbState>, provider_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES ('default_ai_provider', ?1)",
        params![provider_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Get the default AI provider from the settings table.
/// Returns 'flowio-free' if no default is set.
#[tauri::command]
pub fn get_default_provider(db: State<DbState>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = 'default_ai_provider'")
        .map_err(|e| e.to_string())?;
    let result: Option<String> = stmt.query_row([], |row| row.get(0)).ok();
    Ok(result.unwrap_or_else(|| "flowio-free".to_string()))
}

/// Built-in free-tier AI configuration (Flowio free trial).
///
/// Uses Zhipu GLM-4.6V-Flash (vision-capable) with a built-in API key.
/// New users get this out-of-the-box — no setup required.
/// Matches the extension's `flowio-free` provider in modelEnum.ts.
pub(crate) fn get_builtin_free_config() -> AiConfig {
    AiConfig {
        provider: AiProvider {
            id: "flowio-free".into(),
            name: "Flowio 免费试用 (GLM-4.6V-Flash)".into(),
            base_url: "https://open.bigmodel.cn/api/paas/v4/chat/completions".into(),
            model: "glm-4.6v-flash".into(),
        },
        api_key: "874191e5443c479f968aafd3a07f257c.6LksFwbwhCrAfW8U".into(),
    }
}

/// 解析一条 AI 配置 JSON。先按标准 JSON 解析；失败则修正非法转义后重试。
///
/// 历史上（旧版本）写入过一批转义坏掉的 JSON，例如把 `{"custom_patterns":[]}`
/// 写成 `{" custom_patterns\:[],...}`。这类脏数据至今仍留在老用户的 settings
/// 表里，所以读取侧必须容错，不能因为一行脏数据就把整份配置判死。
fn parse_ai_config_json(json: &str) -> Option<AiConfig> {
    if json.is_empty() {
        return None;
    }
    if let Ok(cfg) = serde_json::from_str::<AiConfig>(json) {
        return Some(cfg);
    }
    let sanitized = crate::ai::parse::sanitize_json_escapes(json);
    serde_json::from_str::<AiConfig>(&sanitized).ok()
}

/// 扫描 `ai_config_%` 行，返回**第一条能解析**的配置。
///
/// ⚠️ 不能只取查询结果的第一行：SQLite 无 ORDER BY 时按 rowid 返回，而老库里
/// 那些脏行（`ai_config::__local__::custom_xxx`）的 rowid 往往比用户真正配置的
/// `ai_config_<provider_id>` 更小，直接 `.next()` 会拿脏行当有效配置，
/// 轻则静默回退到内置免费通道、重则直接报错说"配置损坏"。
fn scan_first_usable_ai_config(
    conn: &rusqlite::Connection,
) -> Option<(String, AiConfig)> {
    let mut stmt = conn
        .prepare("SELECT key, value FROM settings WHERE key LIKE 'ai_config_%'")
        .ok()?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok()?
        .filter_map(|r| r.ok())
        .collect();

    let mut skipped = 0usize;
    for (key, json) in rows {
        if let Some(cfg) = parse_ai_config_json(&json) {
            return Some((key, cfg));
        }
        skipped += 1;
        log::warn!(
            "跳过无法解析的 AI 配置行 {}（前 120 字符：{}）",
            key,
            &json[..json.len().min(120)]
        );
    }
    if skipped > 0 {
        log::warn!("共 {} 条 AI 配置行损坏且无可用配置", skipped);
    }
    None
}

/// Resolve the effective AI configuration from the settings table.
///
/// 优先级：默认服务商（`default_ai_provider`）→ 任意**可解析的**已存配置 → 内置免费通道。
/// `flowio-free` 且 key 为空时填入内置 key；配置 JSON 损坏时 sanitize 后重试，
/// 仍失败则跳过该行继续找下一条；一条可用的都没有才回退内置免费通道。
pub(crate) fn resolve_ai_config(conn: &rusqlite::Connection) -> AiConfig {
    let default_id: Option<String> = conn
        .prepare("SELECT value FROM settings WHERE key = 'default_ai_provider'")
        .ok()
        .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)).ok());

    let mut maybe_cfg: Option<AiConfig> = None;
    if let Some(ref id) = default_id {
        let key = format!("ai_config_{}", id);
        if let Ok(mut stmt) = conn.prepare("SELECT value FROM settings WHERE key = ?1") {
            if let Ok(json) = stmt.query_row(params![key], |row| row.get::<_, String>(0)) {
                maybe_cfg = parse_ai_config_json(&json);
            }
        }
    }

    if let Some(mut cfg) = maybe_cfg {
        // flowio-free with empty key → fill in built-in key
        if cfg.provider.id == "flowio-free" && cfg.api_key.is_empty() {
            cfg.api_key = get_builtin_free_config().api_key;
        }
        return cfg;
    }

    match scan_first_usable_ai_config(conn) {
        Some((_, cfg)) => cfg,
        None => {
            // No usable user-configured AI provider → use built-in free tier
            get_builtin_free_config()
        }
    }
}

/// Trigger AI-powered step generation for a recording.
///
/// Reads all steps from the database for the given `recording_id`,
/// builds an [`AiJob`], and runs the AI pipeline on a background
/// thread to keep the UI responsive. When the pipeline completes,
/// step titles, descriptions, action types, and tips are written
/// back to the database.
#[tauri::command]
pub async fn generate_ai_steps(
    db: State<'_, DbState>,
    recording_id: String,
    domain_mode: Option<String>,
) -> Result<String, String> {
    // ── Phase 1: Load config and steps from DB (main thread) ──
    let config: AiConfig = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        resolve_ai_config(&conn)
    };

    let step_rows: Vec<StepRow> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, screenshot_path, \
                 before_screenshot_path, after_screenshot_path, action_type, window_title, x, y, \
                 crop_w, crop_h \
                 FROM steps WHERE recording_id = ?1 ORDER BY order_index",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![recording_id], |row| {
                Ok(StepRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    screenshot_path: row.get(3)?,
                    before_screenshot_path: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    after_screenshot_path: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    action_type: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    window_title: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    x: row.get::<_, Option<i32>>(8)?,
                    y: row.get::<_, Option<i32>>(9)?,
                    crop_w: row.get::<_, Option<i32>>(10)?,
                    crop_h: row.get::<_, Option<i32>>(11)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| e.to_string())?);
        }
        result
    };

    let target_app: Option<String> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT target_app FROM recordings WHERE id = ?1",
            params![recording_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    };

    if step_rows.is_empty() {
        return Err("No steps found for this recording".to_string());
    }

    let total_steps = step_rows.len();

    // ── Phase 2: Run pipeline on background thread ──────────
    let rid = recording_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_ai_pipeline_sync(&config, &step_rows, rid, domain_mode, target_app)
    })
    .await
    .map_err(|e| format!("AI 生成任务异常: {}", e))?
    .map_err(|e| e)?;

    let generated = result.steps.len();
    let error_count = result.errors.len();

    // 逐条落调试日志（release 无 console，log:: 宏无处可去）
    for err in &result.errors {
        ai_dbg_log(&format!("[AI] {}", err));
    }

    // ── Phase 3: Persist results to DB (back on main thread) ──
    apply_pipeline_result(db.inner(), &recording_id, &result);

    summarize_generate_result(generated, total_steps, error_count, &result.errors)
}

/// Retrieve the stored AI configuration for a specific provider.
#[tauri::command]
pub fn get_ai_config(db: State<DbState>, provider_id: String) -> Result<Option<AiConfig>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let key = format!("ai_config_{}", provider_id);
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .map_err(|e| e.to_string())?;

    let json: Option<String> = stmt.query_row(params![key], |row| row.get(0)).ok();

    match json {
        Some(s) if !s.is_empty() => match parse_ai_config_json(&s) {
            Some(config) => Ok(Some(config)),
            None => Err(format!(
                "AI 配置数据损坏，请重新保存配置（key={}）。原文前 120 字符：{}",
                key,
                &s[..s.len().min(120)]
            )),
        },
        _ => Ok(None),
    }
}

/// Retrieve the first **usable** AI configuration across all providers.
///
/// 扫描顺序同 [`scan_first_usable_ai_config`]：跳过解析不了的历史脏行，
/// 而不是拿第一行（很可能是脏行）直接判"配置损坏"。
#[tauri::command]
pub fn get_first_ai_config(db: State<DbState>) -> Result<Option<AiConfig>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(scan_first_usable_ai_config(&conn).map(|(_, cfg)| cfg))
}

/// Persist the AI configuration into the settings table.
/// The key is scoped by provider ID so each provider has its own config.
#[tauri::command]
pub fn set_ai_config(db: State<DbState>, config: AiConfig) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
    let key = format!("ai_config_{}", config.provider.id);

    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![key, json],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Remove AI configuration for a provider from the settings table.
#[tauri::command]
pub fn remove_ai_config(db: State<DbState>, provider_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let key = format!("ai_config_{}", provider_id);
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Test endpoint / error classification helpers ────────────

/// 解析内置服务商的 API 测试端点；未知服务商返回 None（由调用方走自定义 provider 查找）。
/// 返回 (url, is_post)，qianfan 使用 chat/completions POST，其余为 GET /models。
fn resolve_provider_test_endpoint(provider_id: &str, api_key: &str) -> Option<(String, bool)> {
    match provider_id {
        "openai" => Some(("https://api.openai.com/v1/models".to_string(), false)),
        "deepseek" => Some(("https://api.deepseek.com/v1/models".to_string(), false)),
        "moonshot" => Some(("https://api.moonshot.cn/v1/models".to_string(), false)),
        "zhipu" | "flowio-free" => Some(("https://open.bigmodel.cn/api/paas/v4/models".to_string(), false)),
        "anthropic" => Some(("https://api.anthropic.com/v1/models".to_string(), false)),
        "gemini" => Some((
            format!(
                "https://generativelanguage.googleapis.com/v1beta/models?key={}",
                api_key
            ),
            false,
        )),
        "qianfan" => Some((
            "https://qianfan.baidubce.com/v2/chat/completions".to_string(),
            true,
        )),
        "qwen" => Some(("https://dashscope.aliyuncs.com/compatible-mode/v1/models".to_string(), false)),
        "doubao" => Some(("https://ark.cn-beijing.volces.com/api/v3/models".to_string(), false)),
        "minimax" => Some(("https://api.minimax.chat/v1/models".to_string(), false)),
        "yi" => Some(("https://api.lingyiwanwu.com/v1/models".to_string(), false)),
        "baichuan" => Some(("https://api.baichuan-ai.com/v1/models".to_string(), false)),
        _ => None,
    }
}

/// 测试请求的认证方案：Bearer（默认）/ ApiKey（anthropic）/ None（gemini，key 在 URL 中）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestAuth {
    Bearer,
    ApiKey,
    None,
}

fn test_auth_scheme(provider_id: &str) -> TestAuth {
    match provider_id {
        "anthropic" => TestAuth::ApiKey,
        "gemini" => TestAuth::None,
        _ => TestAuth::Bearer,
    }
}

/// 自定义服务商测试端点：{base_url}/models（去掉尾部斜杠）。
fn custom_test_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

/// 聊天补全端点：{base_url}/chat/completions（去掉尾部斜杠）。
fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

/// 将 test_api_key 的失败响应分类为面向用户的错误信息（仅非 2xx 时调用）。
fn classify_test_api_error(status_code: u16, body: &str) -> String {
    if status_code == 401 || status_code == 403 {
        let lower = body.to_lowercase();
        if lower.contains("access denied")
            || lower.contains("accessdenied")
            || lower.contains("forbidden")
            || lower.contains("arrears")
            || lower.contains("欠费")
            || lower.contains("account")
        {
            format!(
                "账户或权限异常（HTTP {}）：{}",
                status_code,
                extract_error_message(body)
            )
        } else if lower.contains("invalid")
            || lower.contains("incorrect")
            || lower.contains("unauthorized")
            || lower.contains("authentication")
            || lower.contains("auth")
        {
            "API Key 无效，请检查后重试".to_string()
        } else {
            format!(
                "API 验证失败（HTTP {}）：{}",
                status_code,
                extract_error_message(body)
            )
        }
    } else {
        format!("连接失败 (HTTP {}): {}", status_code, body)
    }
}

/// 将 validate_custom_api 的响应分类为 Result（200 成功，其余为面向用户的错误）。
fn classify_validate_api_error(
    status_code: u16,
    body: &str,
    model_name: &str,
) -> Result<String, String> {
    match status_code {
        200..=299 => Ok("连接成功".to_string()),
        400 | 401 | 403 | 404 => {
            let lower = body.to_lowercase();

            // 模型类错误（不存在 / 未购买 / 无权限）：优先识别，避免误报成 Key 问题
            const MODEL_ERR_KEYWORDS: [&str; 19] = [
                "model not found", "model does not exist", "model not exist", "modelerror",
                "not supported", "no such model", "access to model denied", "accessdenied",
                "unpurchased", "not purchased", "not eligible", "model access denied",
                "model permission", "not allowed to access model", "model unavailable",
                "does not exist", "not_found_error", "模型不存在", "do not have access",
            ];
            // Key 类错误
            const KEY_ERR_KEYWORDS: [&str; 9] = [
                "invalid api key", "invalid api-key", "incorrect api key", "incorrect api-key",
                "api key not valid", "invalid key", "autherror", "authentication", "unauthorized",
            ];
            // 账户类错误（欠费 / 被禁用 / 配额）
            const ACCOUNT_ERR_KEYWORDS: [&str; 8] = [
                "access denied", "forbidden", "arrears", "欠费", "quota",
                "account", "被禁用", "insufficient",
            ];

            if MODEL_ERR_KEYWORDS.iter().any(|k| lower.contains(k)) {
                Err(format!(
                    "模型错误：{}——{}",
                    model_name,
                    extract_error_message(body)
                ))
            } else if KEY_ERR_KEYWORDS.iter().any(|k| lower.contains(k)) {
                Err("API Key 无效，请检查后重试".to_string())
            } else if ACCOUNT_ERR_KEYWORDS.iter().any(|k| lower.contains(k)) {
                Err(format!(
                    "账户或权限异常（HTTP {}）：{}",
                    status_code,
                    extract_error_message(body)
                ))
            } else if status_code == 404 {
                Err("接口地址或路径错误（HTTP 404），请检查 base_url 是否填写正确（应为基础地址，不含 /chat/completions）".to_string())
            } else {
                Err(format!(
                    "API 验证失败（HTTP {}）：{}",
                    status_code,
                    extract_error_message(body)
                ))
            }
        }
        s => Err(format!(
            "服务器返回异常状态码 {}，请检查 API 地址是否正确",
            s
        )),
    }
}

/// Test API key validity by sending a lightweight request to the provider.
///
/// For most providers this is a GET to the /models endpoint with Bearer auth.
/// Custom providers pass `base_url` explicitly; built-in providers are resolved by id.
/// Returns `"连接成功"` on 2xx, or a `String` error describing the failure.
#[tauri::command]
pub fn test_api_key(
    db: State<DbState>,
    provider_id: String,
    api_key: String,
) -> Result<String, String> {
    // "builtin" sentinel from frontend → use actual built-in key for flowio-free
    let api_key = if api_key == "builtin" && provider_id == "flowio-free" {
        get_builtin_free_config().api_key
    } else {
        api_key
    };

    // Determine base_url: try built-in first, then look up custom provider
    let (test_url, is_post) = match resolve_provider_test_endpoint(&provider_id, &api_key) {
        Some(ep) => ep,
        None => {
            let custom = get_custom_provider_inner(&db, &provider_id)
                .map_err(|e| format!("Unknown provider: {} ({})", provider_id, e))?;
            (custom_test_url(&custom.base_url), false)
        }
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let mut request = if is_post {
        let body = serde_json::json!({
            "model": "ernie-4.0-turbo-8k",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 1
        });
        client.post(&test_url).json(&body)
    } else {
        client.get(&test_url)
    };

    // Set auth header per provider convention
    match test_auth_scheme(&provider_id) {
        TestAuth::ApiKey => {
            request = request
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
        }
        TestAuth::None => {
            // API key is already in the URL query string
        }
        TestAuth::Bearer => {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }
    }

    let response = request
        .send()
        .map_err(|e| format!("Connection failed: {}", e))?;

    let status = response.status();
    if status.is_success() {
        Ok("连接成功".to_string())
    } else {
        let status_code = status.as_u16();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        Err(classify_test_api_error(status_code, &body))
    }
}

/// Validate a custom provider's API key by sending a minimal POST to
/// {base_url}/chat/completions.  Many providers don't enforce auth on
/// the /models endpoint, so a chat request catches bad keys reliably.
///
/// Takes `base_url` and `api_key` directly (no DB lookup needed).
/// Returns `"连接成功"` on 2xx, or an error describing the failure.
#[tauri::command]
pub fn validate_custom_api(
    base_url: String,
    api_key: String,
    model: Option<String>,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let model_name = model.unwrap_or_else(|| "gpt-3.5-turbo".to_string());
    let url = chat_completions_url(&base_url);
    let body = serde_json::json!({
        "model": model_name,
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 1
    });
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("网络请求失败: {}", e))?;

    let code = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    classify_validate_api_error(code, &text, &model_name)
}

/// Extract a friendly error message from a provider error body.
/// Prefers `error.message`, then `error`, then `message` JSON fields.
fn extract_error_message(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = v
            .pointer("/error/message")
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return err.to_string();
        }
        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
    }
    text.chars().take(200).collect()
}

// ── Custom Provider Management ────────────────────────────────

const CUSTOM_PROVIDERS_KEY: &str = "custom_providers";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub models: Vec<String>,
}

/// Read the custom providers list from the settings table.
///
/// Gracefully handles legacy/corrupt data: if the stored JSON is not a
/// valid array, returns an empty list (and overwrites the bad data so
/// the error doesn't recur).
fn read_custom_providers(db: &DbState) -> Result<Vec<CustomProvider>, String> {
    let json = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT value FROM settings WHERE key = ?1")
            .map_err(|e| e.to_string())?;
        stmt.query_row(params![CUSTOM_PROVIDERS_KEY], |row| row.get::<_, String>(0)).ok()
    };
    // Lock released here before potential write below.
    match json {
        Some(s) if !s.is_empty() => {
            match serde_json::from_str::<Vec<CustomProvider>>(&s) {
                Ok(providers) => Ok(providers),
                Err(_) => {
                    // Legacy/corrupt data (object instead of array, etc.):
                    // overwrite with empty array so it doesn't block future saves.
                    let _ = write_custom_providers(db, &[]);
                    Ok(Vec::new())
                }
            }
        }
        _ => Ok(Vec::new()),
    }
}

/// Write the custom providers list back to the settings table.
fn write_custom_providers(db: &DbState, providers: &[CustomProvider]) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let json = serde_json::to_string(providers).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![CUSTOM_PROVIDERS_KEY, json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Look up a single custom provider by id (internal helper).
fn get_custom_provider_inner(db: &DbState, provider_id: &str) -> Result<CustomProvider, String> {
    let providers = read_custom_providers(db)?;
    providers
        .into_iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| format!("Custom provider not found: {}", provider_id))
}

/// 由自定义服务商构造 AiConfig：model 取 models 列表第一个，空列表回退 "default"。
fn build_custom_provider_config(provider: &CustomProvider, api_key: String) -> AiConfig {
    let model = provider
        .models
        .first()
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    AiConfig {
        provider: AiProvider {
            id: provider.id.clone(),
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            model,
        },
        api_key,
    }
}

/// List all custom providers.
#[tauri::command]
pub fn list_custom_providers(db: State<DbState>) -> Result<Vec<CustomProvider>, String> {
    read_custom_providers(&db)
}

/// Add a custom provider.
#[tauri::command]
pub fn add_custom_provider(
    db: State<DbState>,
    provider: CustomProvider,
    api_key: String,
) -> Result<(), String> {
    let mut providers = read_custom_providers(&db)?;
    // Check for duplicate name (case-insensitive)
    if providers
        .iter()
        .any(|p| p.name.to_lowercase() == provider.name.to_lowercase())
    {
        return Err("服务商名称已存在".to_string());
    }
    providers.push(provider.clone());

    // Use a single mutex lock to persist both the provider list AND the API key config atomically
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let providers_json = serde_json::to_string(&providers).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![CUSTOM_PROVIDERS_KEY, providers_json],
    )
    .map_err(|e| e.to_string())?;

    let ai_config = build_custom_provider_config(&provider, api_key);
    let config_json = serde_json::to_string(&ai_config).map_err(|e| e.to_string())?;
    let config_key = format!("ai_config_{}", provider.id);
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![config_key, config_json],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Remove a custom provider by id.
#[tauri::command]
pub fn remove_custom_provider(
    db: State<DbState>,
    provider_id: String,
) -> Result<(), String> {
    let providers = read_custom_providers(&db)?;
    let filtered: Vec<CustomProvider> = providers
        .into_iter()
        .filter(|p| p.id != provider_id)
        .collect();
    write_custom_providers(&db, &filtered)
}

// ── Internal helpers ──────────────────────────────────────────

/// Write pipeline results back to the database.
fn apply_pipeline_result(
    db_state: &DbState,
    recording_id: &str,
    result: &AiResult,
) {
    let conn = match db_state.0.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[ai] Failed to lock DB for persisting results: {}", e);
            return;
        }
    };
    // AI 生成会整体改写步骤标题/描述 → 先压全局撤销栈，生成结果可一键撤销
    let _ = crate::editor::commands::push_edit_history(&conn, recording_id);

    // Collect step IDs in order
    let step_ids: Vec<String> = {
        let mut stmt = match conn.prepare(
            "SELECT id FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[ai] Failed to query step IDs: {}", e);
                return;
            }
        };

        let rows = match stmt.query_map(params![recording_id], |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[ai] Failed to map step IDs: {}", e);
                return;
            }
        };

        rows.filter_map(|r| r.ok()).collect()
    };

    for (i, output) in result.steps.iter().enumerate() {
        if i >= step_ids.len() {
            break;
        }

        if let Err(e) = conn.execute(
            "UPDATE steps SET title = ?1, description = ?2, \
             action_type = ?3, tip = ?4 WHERE id = ?5",
            params![
                output.title,
                output.description,
                output.action_type,
                output.tip,
                step_ids[i],
            ],
        ) {
            eprintln!(
                "[ai] Failed to update step {} ({}): {}",
                i + 1,
                step_ids[i],
                e
            );
        }
    }

    // Update recording status
    if let Err(e) = conn.execute(
        "UPDATE recordings SET status = 'completed', ai_generated = 1 WHERE id = ?1",
        params![recording_id],
    ) {
        eprintln!("[ai] Failed to update recording status: {}", e);
    }

    if !result.errors.is_empty() {
        eprintln!(
            "[ai] Pipeline completed with {} error(s):",
            result.errors.len()
        );
        for err in &result.errors {
            eprintln!("  - {}", err);
        }
    }
}

#[cfg(test)]
mod ai_commands_tests {
    use super::*;
    use crate::ai::types::AiProvider;

    fn text_provider() -> AiProvider {
        AiProvider {
            id: "zhipu".into(),
            name: "智谱".into(),
            base_url: "https://open.bigmodel.cn/api/paas/v4/chat/completions".into(),
            model: "glm-4".into(),
        }
    }

    fn step_row_with_screenshot(path: String) -> StepRow {
        StepRow {
            id: "st1".into(),
            title: "点击".into(),
            description: "描述".into(),
            screenshot_path: path,
            before_screenshot_path: String::new(),
            after_screenshot_path: String::new(),
            action_type: "click".into(),
            window_title: "窗口".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
        }
    }

    #[test]
    fn run_ai_pipeline_sync_empty_steps_returns_empty() {
        let cfg = AiConfig { provider: text_provider(), api_key: "sk-test".into() };
        let result = run_ai_pipeline_sync(&cfg, &[], "rec-1".into(), None, None).unwrap();
        assert!(result.steps.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn run_ai_pipeline_sync_screenshot_with_text_model_errors_early() {
        // 真实 PNG + 非视觉模型 → 应在发起网络请求前直接报错
        let dir = std::env::temp_dir().join("flowio_ai_test");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        image::DynamicImage::new_rgb8(8, 8).save(&png).unwrap();
        let cfg = AiConfig { provider: text_provider(), api_key: "sk-test".into() };
        let rows = vec![step_row_with_screenshot(png.to_string_lossy().to_string())];
        let err = run_ai_pipeline_sync(&cfg, &rows, "rec-1".into(), None, None).unwrap_err();
        assert!(err.contains("不支持图片识别"), "err: {}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_ai_pipeline_sync_before_screenshot_triggers_vision_check() {
        let dir = std::env::temp_dir().join("flowio_ai_test2");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("before.png");
        image::DynamicImage::new_rgb8(8, 8).save(&png).unwrap();
        let cfg = AiConfig { provider: text_provider(), api_key: "sk-test".into() };
        let mut row = step_row_with_screenshot(String::new());
        row.before_screenshot_path = png.to_string_lossy().to_string();
        let err = run_ai_pipeline_sync(&cfg, &[row], "rec-1".into(), None, None).unwrap_err();
        assert!(err.contains("不支持图片识别"), "err: {}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_provider_sql_roundtrip_and_fallback() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn);
        conn.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('default_ai_provider', ?1)", params!["deepseek"]).unwrap();
        let v: String = conn.query_row("SELECT value FROM settings WHERE key = 'default_ai_provider'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "deepseek");
        let conn2 = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn2);
        let d: Option<String> = conn2.query_row("SELECT value FROM settings WHERE key = 'default_ai_provider'", [], |r| r.get(0)).ok();
        assert_eq!(d, None, "缺省时无默认 provider 行（命令层回退 flowio-free）");
    }

    #[test]
    fn ai_config_invalid_json_rejected() {
        let bad = "not-json{";
        assert!(serde_json::from_str::<AiConfig>(bad).is_err());
        let empty = "";
        assert!(serde_json::from_str::<AiConfig>(empty).is_err());
    }

    // ── 历史脏数据防线 ────────────────────────────────────────
    //
    // 老版本曾把「脱敏配置」序列化错（引号被写成反斜杠）后，批量写进了一批
    // 不相干的 settings key，其中就包括 `ai_config::__local__::custom_*`。
    // 这些行的 rowid 比用户真正的 `ai_config_<provider_id>` 更小，
    // 早先 `rows.into_iter().next()` 会先拿到它们并判死。
    // 下面三条锁住「跳过脏行、取真正可用的那条」这个行为。

    /// 真实脏数据原文（199 字节，取自老用户库）。引号全被替换成反斜杠，
    /// 连 sanitize 都救不回来 —— 只能跳过，不能当有效配置。
    const CORRUPT_LEGACY_VALUE: &str = "{\" custom_patterns\\:[],\\enabled\\:[\\id_card\\,\\phone\\,\
        \\bank_card\\,\\email\\,\\api_key\\,\\credit_code\\,\\qq_wechat\\,\\password_field\\,\
        \\ip_mac\\,\\url_param\\,\\custom\\],\\mode\\:\\blur\\,\\redact_before_export\\:false}";

    #[test]
    fn corrupt_legacy_value_is_unrecoverable() {
        assert!(parse_ai_config_json(CORRUPT_LEGACY_VALUE).is_none());
    }

    #[test]
    fn scan_skips_corrupt_rows_and_returns_usable_config() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn);
        // 先插脏行（rowid 更小），再插用户真实配置
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ai_config::__local__::custom_1787202913961', ?1)",
            params![CORRUPT_LEGACY_VALUE],
        )
        .unwrap();
        let good = AiConfig { provider: text_provider(), api_key: "sk-real".into() };
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ai_config_zhipu', ?1)",
            params![serde_json::to_string(&good).unwrap()],
        )
        .unwrap();

        let (key, cfg) = scan_first_usable_ai_config(&conn).expect("应跳过脏行找到可用配置");
        assert_eq!(key, "ai_config_zhipu");
        assert_eq!(cfg.api_key, "sk-real");
    }

    #[test]
    fn resolve_ai_config_ignores_corrupt_legacy_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn);
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ai_config::__local__::custom_1', ?1)",
            params![CORRUPT_LEGACY_VALUE],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ai_config::__local__::custom_2', ?1)",
            params![CORRUPT_LEGACY_VALUE],
        )
        .unwrap();
        let good = AiConfig { provider: text_provider(), api_key: "sk-real".into() };
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ai_config_zhipu', ?1)",
            params![serde_json::to_string(&good).unwrap()],
        )
        .unwrap();

        // 没设 default_ai_provider 时走扫描兜底：必须拿到真实配置，
        // 而不是被脏行顶掉后回退内置免费通道。
        let cfg = resolve_ai_config(&conn);
        assert_eq!(cfg.provider.id, "zhipu", "脏行不得顶掉用户真实配置");
        assert_eq!(cfg.api_key, "sk-real");
    }

    #[test]
    fn resolve_ai_config_falls_back_to_free_when_all_rows_corrupt() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn);
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ai_config::__local__::custom_1', ?1)",
            params![CORRUPT_LEGACY_VALUE],
        )
        .unwrap();

        let cfg = resolve_ai_config(&conn);
        assert_eq!(cfg.provider.id, "flowio-free", "全坏时应回退内置免费通道");
        assert!(!cfg.api_key.is_empty(), "回退时必须带上内置 key");
    }

    // ── read_file_base64 / row_to_step_input ─────────────────

    #[test]
    fn read_file_base64_empty_path_returns_empty() {
        assert_eq!(read_file_base64(""), "");
    }

    #[test]
    fn read_file_base64_missing_file_returns_empty() {
        assert_eq!(read_file_base64("Z:/definitely/not/a/file.png"), "");
    }

    #[test]
    fn read_file_base64_encodes_file_content() {
        let dir = std::env::temp_dir().join("flowio_ai_b64");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("data.bin");
        let bytes: Vec<u8> = (0..64u8).collect();
        std::fs::write(&f, &bytes).unwrap();
        let expected = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(read_file_base64(&f.to_string_lossy()), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn row_to_step_input_maps_fields_and_base64() {
        let dir = std::env::temp_dir().join("flowio_ai_row");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("shot.png");
        std::fs::write(&f, b"abc").unwrap();
        let row = StepRow {
            id: "st9".into(),
            title: "标题".into(),
            description: "描述".into(),
            screenshot_path: f.to_string_lossy().to_string(),
            before_screenshot_path: String::new(),
            after_screenshot_path: "after.png".into(),
            action_type: "click".into(),
            window_title: "窗口".into(),
            x: Some(12),
            y: Some(-3),
            crop_w: Some(180),
            crop_h: Some(60),
        };
        let input = row_to_step_input(&row);
        assert_eq!(input.title, "标题");
        assert_eq!(input.description, "描述");
        assert_eq!(
            input.screenshot_base64,
            base64::engine::general_purpose::STANDARD.encode(b"abc")
        );
        assert_eq!(input.before_screenshot_base64, "");
        assert_eq!(input.action_type, "click");
        assert_eq!(input.window_title, "窗口");
        assert_eq!(input.click_x, Some(12));
        assert_eq!(input.click_y, Some(-3));
        assert_eq!(input.crop_w, Some(180));
        assert_eq!(input.crop_h, Some(60));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── resolve_provider_test_endpoint ───────────────────────

    #[test]
    fn resolve_provider_test_endpoint_builtin() {
        let (url, is_post) = resolve_provider_test_endpoint("openai", "k").unwrap();
        assert_eq!(url, "https://api.openai.com/v1/models");
        assert!(!is_post);

        let (url, is_post) = resolve_provider_test_endpoint("qianfan", "k").unwrap();
        assert_eq!(url, "https://qianfan.baidubce.com/v2/chat/completions");
        assert!(is_post);

        for p in ["deepseek", "moonshot", "zhipu", "flowio-free", "anthropic", "qwen", "doubao", "minimax", "yi", "baichuan"] {
            assert!(resolve_provider_test_endpoint(p, "k").is_some(), "{}", p);
        }
    }

    #[test]
    fn resolve_provider_test_endpoint_gemini_embeds_key() {
        let (url, is_post) = resolve_provider_test_endpoint("gemini", "sk-abc").unwrap();
        assert!(url.contains("sk-abc"));
        assert!(url.starts_with("https://generativelanguage.googleapis.com/v1beta/models?key="));
        assert!(!is_post);
    }

    #[test]
    fn resolve_provider_test_endpoint_unknown_is_none() {
        assert!(resolve_provider_test_endpoint("my-custom", "k").is_none());
        assert!(resolve_provider_test_endpoint("", "k").is_none());
    }

    // ── test_auth_scheme ─────────────────────────────────────

    #[test]
    fn test_auth_scheme_variants() {
        assert_eq!(test_auth_scheme("anthropic"), TestAuth::ApiKey);
        assert_eq!(test_auth_scheme("gemini"), TestAuth::None);
        assert_eq!(test_auth_scheme("openai"), TestAuth::Bearer);
        assert_eq!(test_auth_scheme("whatever"), TestAuth::Bearer);
    }

    // ── URL builders ─────────────────────────────────────────

    #[test]
    fn custom_test_url_trims_trailing_slash() {
        assert_eq!(
            custom_test_url("https://api.example.com"),
            "https://api.example.com/models"
        );
        assert_eq!(
            custom_test_url("https://api.example.com/"),
            "https://api.example.com/models"
        );
        assert_eq!(custom_test_url(""), "/models");
    }

    #[test]
    fn chat_completions_url_trims_trailing_slash() {
        assert_eq!(
            chat_completions_url("https://api.example.com/v1"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://api.example.com/v1/"),
            "https://api.example.com/v1/chat/completions"
        );
    }

    // ── classify_test_api_error ──────────────────────────────

    #[test]
    fn classify_test_api_error_401_account_keyword() {
        let msg = classify_test_api_error(401, r#"{"error":{"message":"access denied"}}"#);
        assert!(msg.starts_with("账户或权限异常（HTTP 401）"), "{}", msg);
        assert!(msg.contains("access denied"));
    }

    #[test]
    fn classify_test_api_error_401_invalid_key() {
        let msg = classify_test_api_error(401, "invalid api key");
        assert_eq!(msg, "API Key 无效，请检查后重试");
    }

    #[test]
    fn classify_test_api_error_403_forbidden_body() {
        let msg = classify_test_api_error(403, "Forbidden: quota exceeded");
        assert!(msg.starts_with("账户或权限异常（HTTP 403）"), "{}", msg);
    }

    #[test]
    fn classify_test_api_error_401_unknown_body() {
        let msg = classify_test_api_error(401, "something else happened");
        assert!(msg.starts_with("API 验证失败（HTTP 401）"), "{}", msg);
    }

    #[test]
    fn classify_test_api_error_500_returns_connection_failed() {
        let msg = classify_test_api_error(500, "boom");
        assert_eq!(msg, "连接失败 (HTTP 500): boom");
    }

    #[test]
    fn classify_test_api_error_priority_account_over_invalid() {
        // 同时命中 account 与 invalid 关键词时，优先 account 分支
        let msg = classify_test_api_error(403, "forbidden: invalid api key");
        assert!(msg.starts_with("账户或权限异常"), "{}", msg);
    }

    // ── classify_validate_api_error ──────────────────────────

    #[test]
    fn classify_validate_api_error_success() {
        assert_eq!(
            classify_validate_api_error(200, "{}", "gpt-3.5-turbo").unwrap(),
            "连接成功"
        );
        assert_eq!(
            classify_validate_api_error(204, "", "m").unwrap(),
            "连接成功"
        );
    }

    #[test]
    fn classify_validate_api_error_model_error() {
        let msg = classify_validate_api_error(400, r#"{"error":{"message":"model not found"}}"#, "my-model").unwrap_err();
        assert!(msg.starts_with("模型错误：my-model——"), "{}", msg);
    }

    #[test]
    fn classify_validate_api_error_key_error() {
        let msg = classify_validate_api_error(401, "invalid api key", "m").unwrap_err();
        assert_eq!(msg, "API Key 无效，请检查后重试");
    }

    #[test]
    fn classify_validate_api_error_account_error() {
        let msg = classify_validate_api_error(403, "insufficient quota", "m").unwrap_err();
        assert!(msg.starts_with("账户或权限异常（HTTP 403）"), "{}", msg);
    }

    #[test]
    fn classify_validate_api_error_404_without_keyword_is_path_error() {
        let msg = classify_validate_api_error(404, "not found", "m").unwrap_err();
        assert!(msg.contains("接口地址或路径错误（HTTP 404）"), "{}", msg);
    }

    #[test]
    fn classify_validate_api_error_404_with_account_keyword_prioritizes_account() {
        let msg = classify_validate_api_error(404, "access denied", "m").unwrap_err();
        assert!(msg.starts_with("账户或权限异常"), "{}", msg);
    }

    #[test]
    fn classify_validate_api_error_400_unknown_body() {
        let msg = classify_validate_api_error(400, "weird error", "m").unwrap_err();
        assert!(msg.starts_with("API 验证失败（HTTP 400）"), "{}", msg);
    }

    #[test]
    fn classify_validate_api_error_other_status() {
        let msg = classify_validate_api_error(500, "oops", "m").unwrap_err();
        assert!(msg.contains("服务器返回异常状态码 500"), "{}", msg);
        let msg = classify_validate_api_error(429, "rate limited", "m").unwrap_err();
        assert!(msg.contains("服务器返回异常状态码 429"), "{}", msg);
    }

    // ── extract_error_message ────────────────────────────────

    #[test]
    fn extract_error_message_nested_message() {
        let s = r#"{"error":{"message":"bad key"}}"#;
        assert_eq!(extract_error_message(s), "bad key");
    }

    #[test]
    fn extract_error_message_error_string() {
        let s = r#"{"error":"direct error"}"#;
        assert_eq!(extract_error_message(s), "direct error");
    }

    #[test]
    fn extract_error_message_top_message() {
        let s = r#"{"message":"top level"}"#;
        assert_eq!(extract_error_message(s), "top level");
    }

    #[test]
    fn extract_error_message_non_json_truncated_to_200() {
        let s = "x".repeat(500);
        let out = extract_error_message(&s);
        assert_eq!(out.len(), 200);
        assert_eq!(out, s[..200]);
    }

    #[test]
    fn extract_error_message_json_without_known_fields_falls_back_to_raw() {
        let s = r#"{"code":42}"#;
        assert_eq!(extract_error_message(s), s);
    }

    // ── summarize_generate_result ────────────────────────────

    #[test]
    fn summarize_generate_result_all_failed_dedup() {
        let err = summarize_generate_result(
            0,
            5,
            3,
            &["网络错误".to_string(), "网络错误".to_string(), "超时".to_string()],
        )
        .unwrap_err();
        assert!(err.starts_with("3 个步骤全部失败。原因："), "{}", err);
        assert!(err.contains("网络错误"));
        assert!(err.contains("超时"));
        // 去重后只有 2 条
        assert_eq!(err.matches("；").count(), 1);
    }

    #[test]
    fn summarize_generate_result_all_failed_truncated() {
        let long: String = "e".repeat(100);
        let errors: Vec<String> = (0..5).map(|i| format!("{}#{}", long, i)).collect();
        let err = summarize_generate_result(0, 5, 5, &errors).unwrap_err();
        assert!(err.ends_with("..."));
        // 原因段：去重 3 条 + 分隔符；截断到 117 + "..."
        assert!(err.contains("120") || err.contains("..."));
    }

    #[test]
    fn summarize_generate_result_all_failed_multibyte_truncate_no_panic() {
        // 中文错误信息 + 长度超 120：按字符安全截断，不能 panic
        let long: String = "错误".repeat(80); // 240 字符
        let errors: Vec<String> = vec![long, "网络超时".into(), "连接失败".into()];
        let err = summarize_generate_result(0, 3, 3, &errors).unwrap_err();
        assert!(err.ends_with("..."));
        assert!(err.contains("错误"));
        // 截断段应为合法 UTF-8（构造即合法），且不出现非法字符截断
        assert!(err.is_char_boundary(err.len()));
    }

    #[test]
    fn summarize_generate_result_none_generated_no_errors() {
        let err = summarize_generate_result(0, 3, 0, &[]).unwrap_err();
        assert_eq!(err, "未生成任何步骤。请检查 AI 服务商配置");
    }

    #[test]
    fn summarize_generate_result_partial() {
        let ok = summarize_generate_result(3, 5, 2, &["x".to_string()]).unwrap();
        assert!(
            ok.starts_with("AI 生成完成：3 / 5 个步骤已生成（2 个失败）。原因："),
            "{}",
            ok
        );
        assert!(ok.contains("x"), "部分失败也应透出原因: {}", ok);
    }

    #[test]
    fn summarize_error_reasons_aggregates_step_prefix() {
        // 15 个同类 429 错误（不同步骤号）应聚合成一条「（×15）」
        let errors: Vec<String> = (1..=15)
            .map(|i| format!("Step {} LLM error: LLM API 返回错误（HTTP 429）：该模型当前访问量过大", i))
            .chain(std::iter::once("Step 16 parse error: bad json".to_string()))
            .collect();
        let reasons = summarize_error_reasons(&errors);
        assert!(reasons.contains("（×15）"), "{}", reasons);
        assert!(reasons.contains("HTTP 429"), "{}", reasons);
        assert!(reasons.starts_with("LLM error"), "{}", reasons);
        // 次数降序：429 在前，parse error 在后
        let pos429 = reasons.find("HTTP 429").unwrap();
        let pos_parse = reasons.find("parse error").unwrap();
        assert!(pos429 < pos_parse);
    }

    #[test]
    fn summarize_generate_result_all_success() {
        let ok = summarize_generate_result(5, 5, 0, &[]).unwrap();
        assert_eq!(ok, "AI 生成完成：全部 5 个步骤已生成");
    }

    // ── build_custom_provider_config ─────────────────────────

    #[test]
    fn build_custom_provider_config_uses_first_model() {
        let p = CustomProvider {
            id: "cp1".into(),
            name: "自定义".into(),
            base_url: "https://api.cp1.com/v1".into(),
            models: vec!["m1".into(), "m2".into()],
        };
        let cfg = build_custom_provider_config(&p, "sk-1".into());
        assert_eq!(cfg.provider.id, "cp1");
        assert_eq!(cfg.provider.name, "自定义");
        assert_eq!(cfg.provider.base_url, "https://api.cp1.com/v1");
        assert_eq!(cfg.provider.model, "m1");
        assert_eq!(cfg.api_key, "sk-1");
    }

    #[test]
    fn build_custom_provider_config_empty_models_default() {
        let p = CustomProvider {
            id: "cp2".into(),
            name: "空模型".into(),
            base_url: "https://api.cp2.com".into(),
            models: vec![],
        };
        let cfg = build_custom_provider_config(&p, "sk-2".into());
        assert_eq!(cfg.provider.model, "default");
    }
}

