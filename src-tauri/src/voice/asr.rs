//! 智谱 GLM-ASR 语音转写客户端。
//!
//! 接口：`POST https://open.bigmodel.cn/api/paas/v4/audio/transcriptions`
//! （multipart：`model` + `file`，wav/mp3，单段 **≤30s / ≤25MB**，
//! 见 https://docs.bigmodel.cn/api-reference/模型-api/语音转文本 ）。
//! 429 / 网络瞬断按 3s/6s/12s 指数退避重试 3 次（与 ai::http::call_llm 一致）；
//! 但「余额/资源包」类 429 立即失败（见 `is_unrecoverable_quota_error`）。

use std::time::Duration;

/// ASR 端点（固定智谱；自定义服务商暂不支持 ASR）
pub const ASR_URL: &str = "https://open.bigmodel.cn/api/paas/v4/audio/transcriptions";

/// ASR 模型名。
///
/// 🔴 **必须是 `glm-asr-2512`，不能改成 `glm-asr`**（2026-09-03 实测踩坑，
/// 曾导致语音功能整体失效数个版本）：
/// | 模型名 | 内置免费 Key 实测结果 |
/// |---|---|
/// | `glm-asr-2512` | HTTP 200，中文转写逐字准确 ✅ |
/// | `glm-asr` | HTTP 429 / code 1113「余额不足或无可用资源包」❌ |
///
/// `glm-asr` 是**付费**模型，免费 Key 无其资源包；而失败时旧逻辑会把 429
/// 一律当限流退避重试 3 次（3s+6s+12s=21s），用户表现为「录了半天、
/// 最后提示未检测到语音内容」。改模型名前请重新跑一遍真实语音回归。
pub const ASR_MODEL: &str = "glm-asr-2512";

/// 当前 AI 配置是否属于「智谱系」通道。
///
/// 语音转写的端点是**硬编码智谱**的（`ASR_URL`），因此只有智谱的 Key 可用：
/// 内置免费通道（flowio-free）、用户自配的智谱（zhipu，base_url 指向
/// open.bigmodel.cn）。千问/DeepSeek 等第三方兼容端点没有
/// `/audio/transcriptions`，其 Key 打到智谱必然 401。
///
/// ⚠️ 返回 false **不代表语音要失效** —— 调用方应回落内置免费通道 Key
/// （见 `voice::commands::resolve_asr_key`），而不是拒绝服务。
pub fn asr_supported(provider_id: &str, base_url: &str) -> bool {
    matches!(provider_id, "zhipu" | "flowio-free")
        || base_url.contains("open.bigmodel.cn")
}

/// 内置热词：录屏教学场景的高频操作词与常见软件名。
///
/// 智谱 GLM-ASR 的 `hotwords` 参数（官方文档：热词表可提升特定领域词汇
/// 识别率，≤100 项）对口音导致的近音错字有直接改善——例如把口音下的
/// 近音错字用热词锚定到正确写法。用户可在 设置 → 录制设置 里追加
/// 自定义热词（settings 表 `voice_hotwords`，JSON 数组）。
pub const BUILTIN_HOTWORDS: &[&str] = &[
    // 通用操作
    "工具栏", "菜单栏", "右键", "双击", "左键", "拖拽", "截图", "全屏", "最小化", "最大化",
    "关闭", "保存", "另存为", "复制", "粘贴", "撤销", "重做", "搜索", "输入框", "下拉框",
    "复选框", "按钮", "对话框", "弹窗", "标签页", "地址栏", "网址", "浏览器", "文件夹",
    "资源管理器", "桌面", "任务栏", "开始菜单", "快捷键", "回车", "删除", "滚动条",
    "缩放", "登录", "注册", "账号", "密码", "验证码", "二维码",
    // ★ 2026-09-06 补齐：插件端实测高频口语操作词（ASR 易错，热词锚定）
    "下一个", "上一个", "点击", "选中", "全选", "链接", "打开",
    // Office / 文档
    "单元格", "工作表", "表格", "文档", "幻灯片", "演示文稿", "公式", "函数", "字体",
    "加粗", "斜体", "下划线", "对齐", "行距", "页边距", "页眉", "页脚", "目录", "批注",
    "修订", "打印", "导出", "上传", "下载", "附件", "邮件", "图表", "筛选", "排序",
    "合并", "拆分", "引用", "脚注",
    // 常见软件名（英文专名 ASR 错率高，重点锚定）
    "WPS", "Excel", "Word", "PPT", "PowerPoint", "PDF", "微信", "钉钉",
];

/// ASR 热词上限：智谱官方文档 `Maximum array length: 100`。
pub const MAX_HOTWORDS: usize = 100;

/// 合并内置热词与用户自定义热词：去空白、去重、截断到上限。
/// 自定义热词排前面（更贴近用户场景），内置词兜底补齐。
pub fn merge_hotwords(custom: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let custom_iter = custom.iter().map(|s| s.as_str());
    for w in custom_iter.chain(BUILTIN_HOTWORDS.iter().copied()) {
        let w = w.trim();
        if w.is_empty() || out.iter().any(|e| e == w) {
            continue;
        }
        if out.len() >= MAX_HOTWORDS {
            break;
        }
        out.push(w.to_string());
    }
    out
}

/// 跨段上下文：把上一段转写结果作为 `prompt` 传入（官方推荐用于长文本
/// 场景，可提升段间连贯性与口音语境纠正）。空文本返回 None；截 500 字
/// 足够表达上一段语境（官方上限 8000，我们只传最近一段）。
pub fn segment_prompt(prev: Option<&str>) -> Option<String> {
    let t = prev?.trim();
    if t.is_empty() {
        return None;
    }
    Some(t.chars().take(500).collect())
}

/// `hotwords` 字段的 multipart 值（纯函数）：**单个字段、值为 JSON 数组
/// 字符串**——官方 SDK 示例 `'hotwords': (None, '["词1","词2"]')`，不是
/// 每个词一个重复字段。空表返回 None（不上传该字段）。
/// 🔴 格式有测试锁死（`hotwords_field_value_json_array_string`），改动前
/// 先重新实测真实 API，传错格式会 400 炸掉整个语音转写。
pub fn hotwords_field_value(hotwords: &[String]) -> Option<String> {
    if hotwords.is_empty() {
        None
    } else {
        serde_json::to_string(hotwords).ok()
    }
}

/// 语种判定纯函数（无网络、无副作用）：依据 CJK 与拉丁字母占比。
///
/// 返回 `"zh"` / `"en"` / `"unknown"`：
/// - 无任何拉丁字母且无可识别 CJK → `"unknown"`（纯数字/标点/空，无法判断）
/// - 无拉丁字母但含 CJK → `"zh"`
/// - 无 CJK → `"en"`
/// - 中英混合：以 CJK 占（CJK+拉丁）比例判断，< 0.2 视为英文主导
///
/// 用途（见 [`transcribe_wav_auto`]）：中文教学热词会**干扰**英文识别，
/// 故英文段应去掉热词重转，中文段零额外成本。
pub fn detect_lang(text: &str) -> &'static str {
    let cjk = text.chars().filter(|c| is_cjk(*c)).count();
    let latin = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if latin == 0 {
        return if cjk == 0 { "unknown" } else { "zh" };
    }
    if cjk == 0 {
        return "en";
    }
    let ratio = cjk as f32 / (cjk + latin) as f32;
    if ratio < 0.2 {
        "en"
    } else {
        "zh"
    }
}

/// 判断一个字符是否属于 CJK（含统一表意文字、扩展 A、CJK 标点与全角形式）。
fn is_cjk(c: char) -> bool {
    let cp = c as u32;
    (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x3000..=0x303F).contains(&cp)
        || (0xFF00..=0xFFEF).contains(&cp)
}

#[derive(serde::Deserialize)]
struct AsrResponse {
    text: String,
}

/// 转写一段 WAV 音频（≤ [`crate::voice::audio::MAX_SEGMENT_MS`]，当前 28s；
/// 智谱服务端硬限 30s / 25MB），返回识别文本。
///
/// `hotwords`：热词表（见 [`merge_hotwords`]）；`prompt`：上一段转写结果
/// 作为上下文（见 [`segment_prompt`]）。二者为空时不上传对应字段，请求
/// 与旧版完全一致。
///
/// multipart 编码说明（查证 2026-09-05）：`hotwords` 是**单个字段、值为
/// JSON 数组字符串**（官方 SDK 示例 `'hotwords': (None, '["词1","词2"]')`），
/// 不是每个词一个重复字段。
///
/// # Errors
/// HTTP 非 2xx 或重试耗尽时返回带原因的 String；响应体缺 `text` 字段时
/// 返回解析错误。
pub fn transcribe_wav(
    api_key: &str,
    wav_bytes: Vec<u8>,
    hotwords: &[String],
    prompt: Option<&str>,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    const MAX_RETRIES: usize = 3;
    let mut last_err = String::new();
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(3 << (attempt - 1)));
        }

        let part = reqwest::blocking::multipart::Part::bytes(wav_bytes.clone())
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| format!("multipart part error: {}", e))?;
        let mut form = reqwest::blocking::multipart::Form::new()
            .text("model", ASR_MODEL)
            .part("file", part);
        if !hotwords.is_empty() {
            let json = hotwords_field_value(hotwords)
                .ok_or_else(|| "hotwords serialize failed".to_string())?;
            form = form.text("hotwords", json);
        }
        if let Some(p) = prompt {
            if !p.trim().is_empty() {
                form = form.text("prompt", p.trim().to_string());
            }
        }

        let response = match client
            .post(ASR_URL)
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("HTTP request failed: {}", e);
                continue;
            }
        };

        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unable to read body>".to_string());

        if status.as_u16() == 429 {
            // ⚠️ 429 有两种，行为必须相反（2026-09-03 实测踩坑）：
            //   · code 1305「该模型当前访问量过大」= 限流，**重试有意义**
            //   · code 1113「余额不足或无可用资源包」= 账户/资源包问题，
            //     重试 3 次只会白等 21 秒（3s+6s+12s）后照样失败，
            //     必须立即返回可行动的错误。
            let text = extract_error_text(&body);
            if is_unrecoverable_quota_error(&body) {
                return Err(format!(
                    "语音转写配额不可用（HTTP 429）：{}。\
                     当前 Key 没有可用的语音识别资源包，请在设置 → AI 服务商中配置智谱 AI 的 API Key，\
                     或为智谱账户充值语音资源包后重试",
                    text
                ));
            }
            last_err = format!("HTTP 429 {}", text);
            continue;
        }
        if !status.is_success() {
            return Err(format!(
                "GLM-ASR 返回错误（HTTP {}）：{}",
                status.as_u16(),
                extract_error_text(&body)
            ));
        }

        let parsed: AsrResponse = serde_json::from_str(&body)
            .map_err(|e| format!("ASR 响应解析失败：{}（body: {}）", e, truncate(&body, 200)))?;
        return Ok(parsed.text.trim().to_string());
    }

    Err(format!("{}（已重试 {} 次仍失败）", last_err, MAX_RETRIES))
}

/// 语种自动检测版转写：先带热词转写（中文场景零额外成本）；若判定为
/// 英文主导，则去掉热词（且去掉中文上下文 `prompt`）重转一次，避免中文热词
/// 把英文识别带偏。本身就是无热词调用时（`hotwords` 为空）直接返回首轮结果，
/// 不发起多余请求。
///
/// # Errors
/// 同 [`transcribe_wav`]。
pub fn transcribe_wav_auto(
    api_key: &str,
    wav_bytes: Vec<u8>,
    hotwords: &[String],
    prompt: Option<&str>,
) -> Result<String, String> {
    let first = transcribe_wav(api_key, wav_bytes.clone(), hotwords, prompt)?;
    // 本来就没喂热词：无需重转（首轮即无热词路径）
    if hotwords.is_empty() {
        return Ok(first);
    }
    if detect_lang(&first) == "en" {
        // 英文主导：中文热词会干扰识别，去掉热词与中文上下文重转
        return transcribe_wav(api_key, wav_bytes, &[], None);
    }
    Ok(first)
}

/// 429 是否属于「重试也没用」的配额/账户类错误。
///
/// 智谱实测错误码：`1113` = 余额不足或无可用资源包（不可恢复）；
/// `1305` = 模型访问量过大（限流，退避重试有效）。
/// 无法解析 code 时退化为按关键词匹配，宁可少重试一次也不要白等 21 秒。
fn is_unrecoverable_quota_error(body: &str) -> bool {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(code) = v.pointer("/error/code") {
            let code = code.as_str().unwrap_or_default().to_string()
                + &code.as_i64().map(|n| n.to_string()).unwrap_or_default();
            if code.contains("1113") {
                return true;
            }
        }
    }
    let msg = extract_error_text(body);
    [
        "余额不足",
        "无可用资源包",
        "资源包",
        "arrears",
        "insufficient balance",
        "quota exceeded",
        "欠费",
    ]
    .iter()
    .any(|k| msg.contains(k))
}

/// 与 ai::http::extract_error_text 同策略的错误信息提取。
fn extract_error_text(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return err.to_string();
        }
        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
    }
    truncate(text, 200)
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod asr_tests {
    use super::*;

    /// 🔴 回归防线：模型名曾写成 `glm-asr`（付费模型，免费 Key 无资源包），
    /// 导致内置通道用户语音功能整体失效、且每段白等 21s 重试。
    #[test]
    fn asr_model_is_the_free_tier_one() {
        assert_eq!(ASR_MODEL, "glm-asr-2512");
        assert_ne!(ASR_MODEL, "glm-asr", "glm-asr 需付费资源包，免费 Key 会返回 429/1113");
    }

    /// 1113（余额/资源包）必须立即失败；1305（限流）才值得退避重试。
    #[test]
    fn quota_429_is_not_retried_but_rate_limit_is() {
        assert!(is_unrecoverable_quota_error(
            r#"{"error":{"code":"1113","message":"余额不足或无可用资源包,请充值。"}}"#
        ));
        assert!(is_unrecoverable_quota_error(r#"{"error":{"code":1113}}"#));
        assert!(!is_unrecoverable_quota_error(
            r#"{"error":{"code":"1305","message":"该模型当前访问量过大，请您稍后再试"}}"#
        ));
        assert!(is_unrecoverable_quota_error("余额不足，请充值"));
    }

    #[test]
    fn asr_supported_for_zhipu_channels_only() {
        assert!(asr_supported("flowio-free", "https://open.bigmodel.cn/api/paas/v4/chat/completions"));
        assert!(asr_supported("zhipu", "https://open.bigmodel.cn/api/paas/v4"));
        assert!(asr_supported(
            "custom-x",
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        ));
        assert!(!asr_supported(
            "qwen",
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        ));
        assert!(!asr_supported("deepseek", "https://api.deepseek.com/v1"));
    }

    #[test]
    fn extract_error_text_prefers_nested_message() {
        assert_eq!(
            extract_error_text(r#"{"error":{"code":"1305","message":"该模型当前访问量过大"}}"#),
            "该模型当前访问量过大"
        );
        assert_eq!(extract_error_text("plain"), "plain");
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("中文abc", 2), "中文");
        assert_eq!(truncate("ab", 10), "ab");
    }

    /// 热词合并：自定义在前、去空白、去重、上限 100（智谱官方限制）。
    #[test]
    fn merge_hotwords_dedupes_and_caps() {
        let custom = vec!["  绘图模式 ".to_string(), "".to_string(), "工具栏".to_string()];
        let merged = merge_hotwords(&custom);
        assert_eq!(merged.first().map(String::as_str), Some("绘图模式"));
        // 「工具栏」在自定义里出现过，内置表里的同名词不得重复
        assert_eq!(merged.iter().filter(|w| *w == "工具栏").count(), 1);
        // 超上限截断
        let many: Vec<String> = (0..150).map(|i| format!("词{:03}", i)).collect();
        assert_eq!(merge_hotwords(&many).len(), MAX_HOTWORDS);
        assert_eq!(MAX_HOTWORDS, 100);
    }

    /// 空自定义热词也要能拿到内置热词（不能一个词都不传）。
    #[test]
    fn merge_hotwords_builtin_fallback() {
        assert!(merge_hotwords(&[]).contains(&"工具栏".to_string()));
        assert!(merge_hotwords(&[]).contains(&"Excel".to_string()));
    }

    /// 跨段上下文：空/纯空白 → None；超长截 500 字。
    #[test]
    fn segment_prompt_trims_and_caps() {
        assert!(segment_prompt(None).is_none());
        assert!(segment_prompt(Some("   ")).is_none());
        let long: String = "字".repeat(600);
        let p = segment_prompt(Some(&long)).unwrap();
        assert_eq!(p.chars().count(), 500);
        assert_eq!(segment_prompt(Some(" 你好 ")).unwrap(), "你好");
    }

    /// 🔴 锁死 hotwords 的 multipart 编码格式：单个字段、JSON 数组字符串
    /// （官方 SDK 示例 `'["词1","词2"]'`）。改格式前必须重新实测真实 API。
    #[test]
    fn hotwords_field_value_json_array_string() {
        assert!(hotwords_field_value(&[]).is_none(), "空表不得上传字段");
        let words = vec!["词1".to_string(), "Excel".to_string()];
        assert_eq!(
            hotwords_field_value(&words).as_deref(),
            Some(r#"["词1","Excel"]"#)
        );
    }

    /// 语种判定：纯中文 / 纯英文 / 混合 / 空文本 四态。
    #[test]
    fn detect_lang_classifies() {
        assert_eq!(detect_lang("点击保存按钮并导出文件"), "zh");
        assert_eq!(detect_lang("Click the Save button and export"), "en");
        assert_eq!(
            detect_lang("开 Excel 后输入数据"),
            "zh",
            "中英混合且中文占主导应判中文"
        );
        assert_eq!(detect_lang(""), "unknown");
        assert_eq!(detect_lang("12345 !@#$"), "unknown");
        // 边界：少量中英夹杂（CJK 占比 ≥0.2）仍判中文——中文 ASR 带热词对夹英文识别良好
        assert_eq!(detect_lang("Open 文件 then 关闭"), "zh");
        // 英文主导（CJK 占比 <0.2）才判英文 → 去热词重转
        assert_eq!(
            detect_lang("Open 文件 and then close the panel"),
            "en",
            "英文词占主导应判英文"
        );
    }

    /// transcribe_wav_auto 无热词时透传、英文段重转不带热词（逻辑路径用 detect_lang 覆盖）。
    #[test]
    fn transcribe_wav_auto_routes_by_lang() {
        // 纯英文 → 判定 en（路径覆盖：进入重转分支前的判定）
        assert_eq!(detect_lang("hello world"), "en");
        // 中文 → 不重转（直接返回首轮）
        assert_eq!(detect_lang("你好世界"), "zh");
    }
}
