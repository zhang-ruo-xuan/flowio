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
}

/// Run the AI pipeline on a blocking thread.
/// Does CPU-heavy base64 encoding of screenshots and network I/O.
fn run_ai_pipeline_sync(
    config: &AiConfig,
    step_rows: &[StepRow],
    recording_id: String,
) -> Result<AiResult, String> {
    let step_inputs: Vec<StepInput> = step_rows
        .iter()
        .map(|row| {
            let read_base64 = |path: &str| -> String {
                if path.is_empty() {
                    String::new()
                } else {
                    std::fs::read(path)
                        .ok()
                        .map(|data| base64::engine::general_purpose::STANDARD.encode(&data))
                        .unwrap_or_default()
                }
            };

            StepInput {
                title: row.title.clone(),
                screenshot_base64: read_base64(&row.screenshot_path),
                before_screenshot_base64: read_base64(&row.before_screenshot_path),
                description: row.description.clone(),
                click_x: row.x,
                click_y: row.y,
                action_type: row.action_type.clone(),
                window_title: row.window_title.clone(),
            }
        })
        .collect();

    let job = AiJob {
        recording_id,
        steps: step_inputs,
    };

    pipeline::run_pipeline(config, job)
}

// ── Tauri Commands ────────────────────────────────────────────

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
/// Returns 'zhipu' if no default is set.
#[tauri::command]
pub fn get_default_provider(db: State<DbState>) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = 'default_ai_provider'")
        .map_err(|e| e.to_string())?;
    let result: Option<String> = stmt.query_row([], |row| row.get(0)).ok();
    Ok(result.unwrap_or_else(|| "zhipu".to_string()))
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
) -> Result<String, String> {
    // ── Phase 1: Load config and steps from DB (main thread) ──
    let config: AiConfig = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;

        let default_id: Option<String> = conn
            .prepare("SELECT value FROM settings WHERE key = 'default_ai_provider'")
            .ok()
            .and_then(|mut stmt| stmt.query_row([], |row| row.get(0)).ok());

        let mut maybe_cfg: Option<AiConfig> = None;
        if let Some(ref id) = default_id {
            let key = format!("ai_config_{}", id);
            if let Ok(mut stmt) = conn.prepare("SELECT value FROM settings WHERE key = ?1") {
                if let Ok(json) = stmt.query_row(params![key], |row| row.get::<_, String>(0)) {
                    if !json.is_empty() {
                        maybe_cfg = serde_json::from_str::<AiConfig>(&json).ok();
                    }
                }
            }
        }

        if let Some(cfg) = maybe_cfg {
            cfg
        } else {
            let rows: Vec<(String, String)> = conn
                .prepare("SELECT key, value FROM settings WHERE key LIKE 'ai_config_%'")
                .map_err(|e| e.to_string())?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?
                .filter_map(|r| r.ok())
                .collect();

            match rows.into_iter().next() {
                Some((_, json)) if !json.is_empty() => {
                    serde_json::from_str::<AiConfig>(&json).map_err(|e| e.to_string())?
                }
                _ => {
                    return Err(
                        "AI 配置未设置，请先在设置中配置 AI 服务商。".to_string(),
                    );
                }
            }
        }
    };

    let step_rows: Vec<StepRow> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, screenshot_path, \
                 before_screenshot_path, after_screenshot_path, action_type, window_title, x, y \
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
                })
            })
            .map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| e.to_string())?);
        }
        result
    };

    if step_rows.is_empty() {
        return Err("No steps found for this recording".to_string());
    }

    let total_steps = step_rows.len();

    // ── Phase 2: Run pipeline on background thread ──────────
    let rid = recording_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_ai_pipeline_sync(&config, &step_rows, rid)
    })
    .await
    .map_err(|e| format!("AI 生成任务异常: {}", e))?
    .map_err(|e| e)?;

    let generated = result.steps.len();
    let error_count = result.errors.len();

    // ── Phase 3: Persist results to DB (back on main thread) ──
    apply_pipeline_result(db.inner(), &recording_id, &result);

    if generated == 0 && error_count > 0 {
        let mut unique_errors: Vec<&str> = Vec::new();
        for err in &result.errors {
            let msg = err.as_str();
            if !unique_errors.contains(&msg) && unique_errors.len() < 3 {
                unique_errors.push(msg);
            }
        }
        let detail = unique_errors.join("；");
        let trunc_detail = if detail.len() > 120 {
            format!("{}...", &detail[..117])
        } else {
            detail
        };
        Err(format!(
            "{} 个步骤全部失败。原因：{}",
            error_count, trunc_detail
        ))
    } else if generated == 0 {
        Err("未生成任何步骤。请检查 AI 服务商配置".to_string())
    } else if error_count > 0 {
        Ok(format!(
            "AI 生成完成：{} / {} 个步骤已生成（{} 个失败）",
            generated, total_steps, error_count
        ))
    } else {
        Ok(format!("AI 生成完成：全部 {} 个步骤已生成", generated))
    }
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
        Some(s) if !s.is_empty() => {
            let config: AiConfig = serde_json::from_str(&s).map_err(|e| e.to_string())?;
            Ok(Some(config))
        }
        _ => Ok(None),
    }
}

/// Retrieve the first available AI configuration across all providers.
#[tauri::command]
pub fn get_first_ai_config(db: State<DbState>) -> Result<Option<AiConfig>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT key, value FROM settings WHERE key LIKE 'ai_config_%'")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    match rows.into_iter().next() {
        Some((_, json)) if !json.is_empty() => {
            let config: AiConfig = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            Ok(Some(config))
        }
        _ => Ok(None),
    }
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
    // Determine base_url: try built-in first, then look up custom provider
    let (test_url, is_post) = match provider_id.as_str() {
        "openai" => ("https://api.openai.com/v1/models".to_string(), false),
        "deepseek" => ("https://api.deepseek.com/v1/models".to_string(), false),
        "moonshot" => ("https://api.moonshot.cn/v1/models".to_string(), false),
        "zhipu" => ("https://open.bigmodel.cn/api/paas/v4/models".to_string(), false),
        "anthropic" => ("https://api.anthropic.com/v1/models".to_string(), false),
        "gemini" => (
            format!(
                "https://generativelanguage.googleapis.com/v1beta/models?key={}",
                api_key
            ),
            false,
        ),
        "qianfan" => (
            "https://qianfan.baidubce.com/v2/chat/completions".to_string(),
            true,
        ),
        "qwen" => ("https://dashscope.aliyuncs.com/compatible-mode/v1/models".to_string(), false),
        "doubao" => ("https://ark.cn-beijing.volces.com/api/v3/models".to_string(), false),
        "minimax" => ("https://api.minimax.chat/v1/models".to_string(), false),
        "yi" => ("https://api.lingyiwanwu.com/v1/models".to_string(), false),
        "baichuan" => ("https://api.baichuan-ai.com/v1/models".to_string(), false),
        _ => {
            // Look up custom provider from settings
            let custom = get_custom_provider_inner(&db, &provider_id)
                .map_err(|e| format!("Unknown provider: {} ({})", provider_id, e))?;
            let url = format!("{}/models", custom.base_url.trim_end_matches('/'));
            (url, false)
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
    match provider_id.as_str() {
        "anthropic" => {
            request = request
                .header("x-api-key", &api_key)
                .header("anthropic-version", "2023-06-01");
        }
        "gemini" => {
            // API key is already in the URL query string
        }
        _ => {
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
        if status_code == 401 || status_code == 403 {
            Err("API Key 无效，请检查后重试".to_string())
        } else {
            Err(format!("连接失败 (HTTP {}): {}", status_code, body))
        }
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
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": "gpt-3.5-turbo",
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

    match resp.status().as_u16() {
        200..=299 | 400 => Ok("连接成功".to_string()),
        401 | 403 => Err("API Key 无效，请检查后重试".to_string()),
        s => Err(format!("服务器返回异常状态码 {}，请检查 API 地址是否正确", s)),
    }
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
fn read_custom_providers(db: &DbState) -> Result<Vec<CustomProvider>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .map_err(|e| e.to_string())?;
    let json: Option<String> = stmt.query_row(params![CUSTOM_PROVIDERS_KEY], |row| row.get(0)).ok();
    match json {
        Some(s) if !s.is_empty() => serde_json::from_str(&s).map_err(|e| e.to_string()),
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

    let model = provider.models.first().cloned().unwrap_or_else(|| "default".to_string());
    let ai_config = AiConfig {
        provider: AiProvider {
            id: provider.id.clone(),
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            model,
        },
        api_key,
    };
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
        "UPDATE recordings SET status = 'completed' WHERE id = ?1",
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
