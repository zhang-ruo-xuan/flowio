use rusqlite::params;
use tauri::State;

use crate::DbState;

// ── Appearance configuration ─────────────────────────────

const APPEARANCE_CONFIG_KEY: &str = "appearance_config";

fn default_appearance_config() -> serde_json::Value {
    serde_json::json!({
        "dark_mode": false,
        "font_size": 14,
    })
}

/// 读取 settings 表中 key 对应的 JSON 配置；缺失/非法 JSON 时返回默认值。
fn read_config_json(
    conn: &rusqlite::Connection,
    key: &str,
    default: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .map_err(|e| e.to_string())?;
    let json: Option<String> = stmt.query_row(params![key], |row| row.get(0)).ok();
    match json {
        Some(s) if !s.is_empty() => {
            // "null" 字面量视为非法/空配置，同样回退默认值
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(v) if !v.is_null() => Ok(v),
                _ => Ok(default),
            }
        }
        _ => Ok(default),
    }
}

/// 与已存配置（缺失/非法时用默认值）合并后写入 settings 表，
/// 避免部分更新丢字段或重置此前已保存的非默认值。
fn write_config_json(
    conn: &rusqlite::Connection,
    key: &str,
    config: &serde_json::Value,
    default: serde_json::Value,
) -> Result<(), String> {
    let mut merged = read_config_json(conn, key, default)?;
    if let Some(obj) = config.as_object() {
        for (k, v) in obj {
            merged[k] = v.clone();
        }
    }
    let json = serde_json::to_string(&merged).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![key, json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Appearance configuration ──────────────────────────────────

#[tauri::command]
pub fn get_appearance_config(db: State<DbState>) -> Result<serde_json::Value, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config())
}

#[tauri::command]
pub fn set_appearance_config(db: State<DbState>, config: serde_json::Value) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    write_config_json(&conn, APPEARANCE_CONFIG_KEY, &config, default_appearance_config())
}

const REDACTION_CONFIG_KEY: &str = "redaction_config";

pub fn read_recorder_bool(conn: &rusqlite::Connection, key: &str) -> Option<bool> {
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .ok()?;
    let json: Option<String> = stmt
        .query_row(params!["recorder_config"], |row| row.get(0))
        .ok();
    let value = match json {
        Some(s) if !s.is_empty() => serde_json::from_str::<serde_json::Value>(&s).ok(),
        _ => Some(serde_json::json!({})),
    };
    value?.get(key).and_then(|v| v.as_bool())
}

fn default_redaction_config() -> serde_json::Value {
    let mut v = crate::redaction::RedactionConfig::default_all().to_json();
    // 导出前自动脱敏默认关闭：日常导出通常就是给团队/客户看的原始录屏；
    // 真要脱敏在编辑器用「文本脱敏」「扫描脱敏」按需手动触发。
    if let Some(obj) = v.as_object_mut() {
        obj.insert("redact_before_export".to_string(), serde_json::json!(false));
    }
    v
}

#[tauri::command]
pub fn get_redaction_config(db: State<DbState>) -> Result<serde_json::Value, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    refresh_auto_redact_policy(&conn);
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .map_err(|e| e.to_string())?;
    let json: Option<String> = stmt
        .query_row(params![REDACTION_CONFIG_KEY], |row| row.get(0))
        .ok();
    match json {
        Some(s) if !s.is_empty() => {
            serde_json::from_str(&s).map_err(|e| e.to_string())
        }
        _ => Ok(default_redaction_config()),
    }
}

#[tauri::command]
pub fn set_redaction_config(db: State<DbState>, config: serde_json::Value) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // Merge with defaults so partial updates don't drop other keys
    let mut merged = default_redaction_config();
    if let Some(obj) = config.as_object() {
        for (k, v) in obj {
            merged[k] = v.clone();
        }
    }
    let json = serde_json::to_string(&merged).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        params![REDACTION_CONFIG_KEY, json],
    )
    .map_err(|e| e.to_string())?;
    refresh_auto_redact_policy(&conn);
    Ok(())
}

/// 读取脱敏配置（供 redaction 命令使用），缺失时返回默认全量配置。
pub fn read_redaction_config(conn: &rusqlite::Connection) -> crate::redaction::RedactionConfig {
    let mut stmt = match conn.prepare("SELECT value FROM settings WHERE key = ?1") {
        Ok(s) => s,
        Err(_) => return crate::redaction::RedactionConfig::default_all(),
    };
    let json: Option<String> = stmt
        .query_row(params![REDACTION_CONFIG_KEY], |row| row.get(0))
        .ok();
    match json {
        Some(s) if !s.is_empty() => serde_json::from_str::<serde_json::Value>(&s)
            .map(|v| crate::redaction::RedactionConfig::from_json(&v))
            .unwrap_or_else(|_| crate::redaction::RedactionConfig::default_all()),
        _ => crate::redaction::RedactionConfig::default_all(),
    }
}

/// 刷新全局自动脱敏策略缓存（开关 + 覆盖方式 + 类别配置）。
/// 由本模块各配置读写命令在持有 DB 连接时调用，录制线程只读缓存。
pub fn refresh_auto_redact_policy(conn: &rusqlite::Connection) {
    let enabled = read_recorder_bool(conn, "auto_redact_recording").unwrap_or(false);
    let config = read_redaction_config(conn);
    let mode = {
        let mut stmt = match conn.prepare("SELECT value FROM settings WHERE key = ?1") {
            Ok(s) => s,
            Err(_) => return,
        };
        let json: Option<String> = stmt
            .query_row(params![REDACTION_CONFIG_KEY], |row| row.get(0))
            .ok();
        match json {
            Some(s) if !s.is_empty() => serde_json::from_str::<serde_json::Value>(&s)
                .ok()
                .and_then(|v| v.get("mode").and_then(|x| x.as_str()).map(|m| m.to_string())),
            _ => None,
        }
    };
    crate::redaction::set_auto_redact_policy(
        enabled,
        crate::redaction::RedactMode::from_key(mode.as_deref()),
        config,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn);
        conn
    }

    #[test]
    fn read_recorder_bool_missing_key_returns_none() {
        let conn = mem_db();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), None);
    }

    #[test]
    fn read_recorder_bool_missing_config_row_returns_none() {
        let conn = mem_db();
        // settings 中根本没有 recorder_config 行
        assert_eq!(read_recorder_bool(&conn, "anything"), None);
    }

    #[test]
    fn read_recorder_bool_true_false() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('recorder_config', ?1)",
            params![r#"{"auto_redact_recording": true, "other": false}"#],
        )
        .unwrap();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), Some(true));
        assert_eq!(read_recorder_bool(&conn, "other"), Some(false));
        assert_eq!(read_recorder_bool(&conn, "missing_key"), None);
    }

    #[test]
    fn read_recorder_bool_invalid_json_returns_none() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('recorder_config', 'not-json')",
            [],
        )
        .unwrap();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), None);
    }

    #[test]
    fn read_recorder_bool_non_boolean_value_returns_none() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('recorder_config', ?1)",
            params![r#"{"auto_redact_recording": "yes"}"#],
        )
        .unwrap();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), None);
    }

    #[test]
    fn read_recorder_bool_empty_string_means_empty_object() {
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('recorder_config', '')", [])
            .unwrap();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), None);
    }

    #[test]
    fn read_recorder_bool_array_and_null_value_returns_none() {
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('recorder_config', ?1)", params![r#"{"auto_redact_recording": [true]}"#]).unwrap();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), None);
        conn.execute("UPDATE settings SET value = ?1 WHERE key = 'recorder_config'", params![r#"{"auto_redact_recording": null}"#]).unwrap();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), None);
    }

    #[test]
    fn read_recorder_bool_false_value_returns_false() {
        // 不触发 refresh_auto_redact_policy 的全局副作用（并行测试下会竞态），只验证读取语义
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('recorder_config', ?1)", params![r#"{"auto_redact_recording": false}"#]).unwrap();
        assert_eq!(read_recorder_bool(&conn, "auto_redact_recording"), Some(false));
    }

    #[test]
    fn default_redaction_config_has_export_flag_and_all_kinds() {
        let v = default_redaction_config();
        assert!(v.is_object());
        assert_eq!(v["redact_before_export"], serde_json::json!(false));
        let enabled = v
            .get("enabled")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(!enabled.is_empty(), "default config should enable all kinds");
        assert!(v.get("custom_patterns").is_some());
    }

    #[test]
    fn read_redaction_config_default_when_missing() {
        let conn = mem_db();
        let cfg = read_redaction_config(&conn);
        assert!(!cfg.enabled.is_empty());
        assert!(cfg.custom_patterns.is_empty());
    }

    #[test]
    fn read_redaction_config_invalid_json_falls_back_to_default() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('redaction_config', '{bad json')",
            [],
        )
        .unwrap();
        let cfg = read_redaction_config(&conn);
        assert!(!cfg.enabled.is_empty());
    }

    #[test]
    fn read_redaction_config_roundtrip() {
        let conn = mem_db();
        let cfg = crate::redaction::RedactionConfig {
            enabled: vec![crate::redaction::SensitiveKind::from_key("phone").unwrap()],
            custom_patterns: vec!["abc-\\d{3}".to_string()],
        };
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('redaction_config', ?1)",
            params![cfg.to_json().to_string()],
        )
        .unwrap();
        let loaded = read_redaction_config(&conn);
        assert_eq!(loaded.enabled.len(), 1);
        assert_eq!(loaded.enabled[0].key(), "phone");
        assert_eq!(loaded.custom_patterns, vec!["abc-\\d{3}".to_string()]);
    }

    #[test]
    fn read_redaction_config_empty_enabled_falls_back_all() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('redaction_config', ?1)",
            params![r#"{"enabled": []}"#],
        )
        .unwrap();
        let cfg = read_redaction_config(&conn);
        assert!(!cfg.enabled.is_empty(), "empty enabled should fall back to default_all");
    }

    #[test]
    fn read_redaction_config_partial_enabled_roundtrip() {
        let conn = mem_db();
        let cfg = crate::redaction::RedactionConfig {
            enabled: vec![
                crate::redaction::SensitiveKind::from_key("phone").unwrap(),
                crate::redaction::SensitiveKind::from_key("email").unwrap(),
            ],
            custom_patterns: vec![],
        };
        conn.execute("INSERT INTO settings (key, value) VALUES ('redaction_config', ?1)", params![cfg.to_json().to_string()]).unwrap();
        let loaded = read_redaction_config(&conn);
        assert_eq!(loaded.enabled.len(), 2);
        assert!(loaded.enabled.iter().any(|k| k.key() == "phone"));
        assert!(loaded.enabled.iter().any(|k| k.key() == "email"));
    }

    #[test]
    fn read_redaction_config_custom_only_keeps_all_enabled() {
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('redaction_config', ?1)", params![r#"{"enabled": [], "custom_patterns": ["x-\\d{3}"]}"#]).unwrap();
        let cfg = read_redaction_config(&conn);
        assert!(!cfg.enabled.is_empty(), "empty enabled falls back to all kinds");
        assert_eq!(cfg.custom_patterns, vec!["x-\\d{3}".to_string()]);
    }

    #[test]
    fn read_redaction_config_non_object_json_falls_back_default() {
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('redaction_config', ?1)", params!["[1,2,3]"]).unwrap();
        let cfg = read_redaction_config(&conn);
        assert!(!cfg.enabled.is_empty());
    }

    #[test]
    fn refresh_auto_redact_policy_disabled_by_default() {
        let conn = mem_db();
        refresh_auto_redact_policy(&conn);
        assert!(crate::redaction::auto_redact_policy().is_none());
    }

    #[test]
    fn refresh_auto_redact_policy_enabled_when_switch_on() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('recorder_config', ?1)",
            params![r#"{"auto_redact_recording": true}"#],
        )
        .unwrap();
        refresh_auto_redact_policy(&conn);
        assert!(crate::redaction::auto_redact_policy().is_some());
        // 恢复全局缓存，避免影响其他测试
        conn.execute(
            "UPDATE settings SET value = ?1 WHERE key = 'recorder_config'",
            params![r#"{}"#],
        )
        .unwrap();
        refresh_auto_redact_policy(&conn);
    }

    

    

    

    

    

    
#[test]
    fn default_appearance_config_values() {
        let v = default_appearance_config();
        assert_eq!(v["dark_mode"], serde_json::json!(false));
        assert_eq!(v["font_size"], serde_json::json!(14));
    }

    
#[test]
    fn read_appearance_config_missing_and_invalid_falls_back_default() {
        let conn = mem_db();
        let v = read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config()).unwrap();
        assert_eq!(v["dark_mode"], serde_json::json!(false));
        assert_eq!(v["font_size"], serde_json::json!(14));

        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('appearance_config', 'not-json')",
            [],
        )
        .unwrap();
        let v = read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config()).unwrap();
        assert_eq!(v["dark_mode"], serde_json::json!(false));
        assert_eq!(v["font_size"], serde_json::json!(14));
    }

    
#[test]
    fn appearance_config_roundtrip_and_partial_update() {
        let conn = mem_db();
        write_config_json(
            &conn,
            APPEARANCE_CONFIG_KEY,
            &serde_json::json!({ "dark_mode": true }),
            default_appearance_config(),
        )
        .unwrap();
        let v = read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config()).unwrap();
        assert_eq!(v["dark_mode"], serde_json::json!(true));
        assert_eq!(v["font_size"], serde_json::json!(14));

        // 部分更新 font_size 不应丢掉 dark_mode
        write_config_json(
            &conn,
            APPEARANCE_CONFIG_KEY,
            &serde_json::json!({ "font_size": 16 }),
            default_appearance_config(),
        )
        .unwrap();
        let v = read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config()).unwrap();
        assert_eq!(v["dark_mode"], serde_json::json!(true));
        assert_eq!(v["font_size"], serde_json::json!(16));
    }


    
#[test]
    fn write_config_json_preserves_other_keys_in_db() {
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('other_key', 'keep')", []).unwrap();
        write_config_json(&conn, APPEARANCE_CONFIG_KEY, &serde_json::json!({"font_size": 18}), default_appearance_config()).unwrap();
        let v: String = conn.query_row("SELECT value FROM settings WHERE key = 'other_key'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "keep");
    }

    
#[test]
    fn write_config_json_then_invalid_db_value_falls_back_default() {
        let conn = mem_db();
        write_config_json(&conn, APPEARANCE_CONFIG_KEY, &serde_json::json!({"dark_mode": true}), default_appearance_config()).unwrap();
        conn.execute("UPDATE settings SET value = 'not-json{' WHERE key = 'appearance_config'", []).unwrap();
        let v = read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config()).unwrap();
        assert_eq!(v["dark_mode"], serde_json::json!(false));
        assert_eq!(v["font_size"], serde_json::json!(14));
    }

    
#[test]
    fn read_config_json_null_json_value_returns_default() {
        let conn = mem_db();
        conn.execute("INSERT INTO settings (key, value) VALUES ('appearance_config', 'null')", []).unwrap();
        let v = read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config()).unwrap();
        // 修复后：DB 值 'null' 视为空配置，回退默认值
        assert_eq!(v["dark_mode"], serde_json::json!(false));
        assert_eq!(v["font_size"], serde_json::json!(14));
    }

    
#[test]
    fn write_config_json_empty_object_keeps_defaults() {
        let conn = mem_db();
        write_config_json(&conn, APPEARANCE_CONFIG_KEY, &serde_json::json!({}), default_appearance_config()).unwrap();
        let v = read_config_json(&conn, APPEARANCE_CONFIG_KEY, default_appearance_config()).unwrap();
        assert_eq!(v["dark_mode"], serde_json::json!(false));
        assert_eq!(v["font_size"], serde_json::json!(14));
    }
}
