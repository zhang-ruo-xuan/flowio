//! 隐私脱敏（P1-03）
//!
//! 正则 + 文本双通道检测身份证（18 位）/ 手机号（11 位）/ 银行卡（16-19 位）/
//! 邮箱，文本内容替换为 `[已隐藏]`；截图区域支持高斯模糊（区域坐标由调用方传入）。

pub mod commands;
pub mod ocr;

use image::{DynamicImage, RgbaImage};

/// 敏感信息类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveKind {
    IdCard,       // 身份证号
    Phone,        // 手机号
    BankCard,     // 银行卡号
    Email,        // 邮箱
    ApiKey,       // API Key / Token（sk-、ghp_、AKIA、Bearer）
    CreditCode,   // 统一社会信用代码（18 位）
    QqWechat,     // QQ 号 / 微信号
    PasswordField, // 密码口令键值对（密码:/password= 等）
    IpMac,        // 内网 IP / MAC 地址
    UrlParam,     // URL 敏感参数（token/key/secret/password 等）
    Custom,       // 用户自定义正则
}

/// 文本中检测到的一段敏感内容
#[derive(Debug, Clone)]
pub struct SensitiveSpan {
    pub kind: SensitiveKind,
    pub start: usize,
    pub end: usize,
}

impl SensitiveKind {
    pub const ALL: [SensitiveKind; 11] = [
        SensitiveKind::IdCard,
        SensitiveKind::Phone,
        SensitiveKind::BankCard,
        SensitiveKind::Email,
        SensitiveKind::ApiKey,
        SensitiveKind::CreditCode,
        SensitiveKind::QqWechat,
        SensitiveKind::PasswordField,
        SensitiveKind::IpMac,
        SensitiveKind::UrlParam,
        SensitiveKind::Custom,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SensitiveKind::IdCard => "身份证号",
            SensitiveKind::Phone => "手机号",
            SensitiveKind::BankCard => "银行卡号",
            SensitiveKind::Email => "邮箱",
            SensitiveKind::ApiKey => "API Key / Token",
            SensitiveKind::CreditCode => "统一社会信用代码",
            SensitiveKind::QqWechat => "QQ号 / 微信号",
            SensitiveKind::PasswordField => "密码 / 口令",
            SensitiveKind::IpMac => "内网IP / MAC",
            SensitiveKind::UrlParam => "URL 敏感参数",
            SensitiveKind::Custom => "自定义规则",
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            SensitiveKind::IdCard => "id_card",
            SensitiveKind::Phone => "phone",
            SensitiveKind::BankCard => "bank_card",
            SensitiveKind::Email => "email",
            SensitiveKind::ApiKey => "api_key",
            SensitiveKind::CreditCode => "credit_code",
            SensitiveKind::QqWechat => "qq_wechat",
            SensitiveKind::PasswordField => "password_field",
            SensitiveKind::IpMac => "ip_mac",
            SensitiveKind::UrlParam => "url_param",
            SensitiveKind::Custom => "custom",
        }
    }

    pub fn from_key(s: &str) -> Option<SensitiveKind> {
        match s {
            "id_card" => Some(SensitiveKind::IdCard),
            "phone" => Some(SensitiveKind::Phone),
            "bank_card" => Some(SensitiveKind::BankCard),
            "email" => Some(SensitiveKind::Email),
            "api_key" => Some(SensitiveKind::ApiKey),
            "credit_code" => Some(SensitiveKind::CreditCode),
            "qq_wechat" => Some(SensitiveKind::QqWechat),
            "password_field" => Some(SensitiveKind::PasswordField),
            "ip_mac" => Some(SensitiveKind::IpMac),
            "url_param" => Some(SensitiveKind::UrlParam),
            "custom" => Some(SensitiveKind::Custom),
            _ => None,
        }
    }
}

/// 脱敏配置：启用的类别 + 用户自定义正则（来自 settings 表 redaction_config）。
#[derive(Debug, Clone)]
pub struct RedactionConfig {
    pub enabled: Vec<SensitiveKind>,
    pub custom_patterns: Vec<String>,
}

impl RedactionConfig {
    pub fn default_all() -> Self {
        Self {
            enabled: SensitiveKind::ALL.to_vec(),
            custom_patterns: Vec::new(),
        }
    }

    pub fn is_enabled(&self, kind: SensitiveKind) -> bool {
        self.enabled.contains(&kind)
    }

    pub fn from_json(v: &serde_json::Value) -> Self {
        let mut cfg = Self::default_all();
        if let Some(arr) = v.get("enabled").and_then(|x| x.as_array()) {
            // 🔴 用户显式配置的类别列表**必须原样生效（含空数组 = 全部不勾 = 什么都不检测）**。
            // 之前空数组被当成「未配置」静默回落 default_all：
            // 设置里所有类别都不勾，扫描/文本脱敏仍按全量 11 类规则跑（2026-09-06 实锤：
            // 用户 DB enabled=[] 但扫描照样打码）。键缺失（旧配置无此字段）才保留全量默认。
            let enabled: Vec<SensitiveKind> = arr
                .iter()
                .filter_map(|x| x.as_str().and_then(SensitiveKind::from_key))
                .collect();
            cfg.enabled = enabled;
        }
        if let Some(arr) = v.get("custom_patterns").and_then(|x| x.as_array()) {
            cfg.custom_patterns = arr
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect();
        }
        cfg
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.enabled.iter().map(|k| k.key()).collect::<Vec<_>>(),
            "custom_patterns": self.custom_patterns,
        })
    }
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self::default_all()
    }
}

/// 扫描文本，返回所有敏感片段的位置（互不重叠，按出现顺序）。使用默认全量配置。
pub fn find_sensitive(text: &str) -> Vec<SensitiveSpan> {
    find_sensitive_with_config(text, &RedactionConfig::default_all())
}

/// 扫描文本（按配置过滤类别 + 自定义正则），返回所有敏感片段的位置（互不重叠，按出现顺序）。
///
/// 注意：当前依赖的 regex 编译配置不支持 look-around，因此这里用捕获组 + ASCII
/// 边界检查实现与原 look-around 等价的语义（前后非数字 / 前后非邮箱字符）。
pub fn find_sensitive_with_config(text: &str, config: &RedactionConfig) -> Vec<SensitiveSpan> {
    let mut candidates: Vec<SensitiveSpan> = Vec::new();
    let bytes = text.as_bytes();

    // 数字类正则（身份证/手机号/银行卡/QQ）要求前后非数字
    let is_digit_span = |m: &regex::Match<'_>| -> bool {
        if m.start() > 0 && bytes[m.start() - 1].is_ascii_digit() {
            return false;
        }
        if m.end() < text.len() && bytes[m.end()].is_ascii_digit() {
            return false;
        }
        true
    };

    // 邮箱要求前后非邮箱字符
    let is_email_char = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-');
    let is_email_span = |m: &regex::Match<'_>| -> bool {
        if m.start() > 0 && is_email_char(bytes[m.start() - 1]) {
            return false;
        }
        if m.end() < text.len() && is_email_char(bytes[m.end()]) {
            return false;
        }
        true
    };

    // 宽松边界：前后非字母数字下划线（用于 API Key / 信用代码 / 微信号 / 密码字段 / URL 参数等）
    let is_word_span = |m: &regex::Match<'_>| -> bool {
        let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        if m.start() > 0 && is_word(bytes[m.start() - 1]) {
            return false;
        }
        if m.end() < text.len() && is_word(bytes[m.end()]) {
            return false;
        }
        true
    };

    if config.is_enabled(SensitiveKind::IdCard) {
        let re = regex::Regex::new(
            r"([1-9][0-9]{5}(?:19|20)[0-9]{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12][0-9]|3[01])[0-9]{3}[0-9Xx])",
        )
        .unwrap();
        for m in re.find_iter(text) {
            if is_digit_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::IdCard, start: m.start(), end: m.end() });
            }
        }
    }
    if config.is_enabled(SensitiveKind::Phone) {
        let re = regex::Regex::new(r"(1[3-9][0-9]{9})").unwrap();
        for m in re.find_iter(text) {
            if is_digit_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::Phone, start: m.start(), end: m.end() });
            }
        }
    }
    if config.is_enabled(SensitiveKind::BankCard) {
        // B8: 从 13-19 收紧为 16-19——13~15 位连续数字多为订单号/流水号，
        // 过宽容易误伤普通数字串。
        let re = regex::Regex::new(r"([0-9]{16,19})").unwrap();
        for m in re.find_iter(text) {
            if is_digit_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::BankCard, start: m.start(), end: m.end() });
            }
        }
    }
    if config.is_enabled(SensitiveKind::Email) {
        let re = regex::Regex::new(r"([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})").unwrap();
        for m in re.find_iter(text) {
            if is_email_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::Email, start: m.start(), end: m.end() });
            }
        }
    }
    if config.is_enabled(SensitiveKind::ApiKey) {
        // sk-/ghp_/AKIA/Bearer 等常见密钥前缀
        let re = regex::Regex::new(
            r"(?i)(sk-[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|Bearer\s+[A-Za-z0-9._~+/=-]{16,})",
        )
        .unwrap();
        for m in re.find_iter(text) {
            if is_word_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::ApiKey, start: m.start(), end: m.end() });
            }
        }
    }
    if config.is_enabled(SensitiveKind::CreditCode) {
        // 统一社会信用代码：2 位登记管理部门 + 6 位机构代码 + 10 位主体标识码/校验码（18 位）
        let re = regex::Regex::new(r"([0-9A-HJ-NPQRTUWXY]{2}[0-9]{6}[0-9A-HJ-NPQRTUWXY]{10})").unwrap();
        for m in re.find_iter(text) {
            if is_word_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::CreditCode, start: m.start(), end: m.end() });
            }
        }
    }
    if config.is_enabled(SensitiveKind::QqWechat) {
        // QQ：6-10 位非 0 开头数字；微信号：wxid_ 前缀 或 "微信号:" 后跟
        let qq_re = regex::Regex::new(r"([1-9][0-9]{5,9})").unwrap();
        for m in qq_re.find_iter(text) {
            if is_digit_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::QqWechat, start: m.start(), end: m.end() });
            }
        }
        let wx_re = regex::Regex::new(r"(wxid_[A-Za-z0-9_-]{6,}|微信号[:：]\s*[A-Za-z][A-Za-z0-9_-]{5,19})").unwrap();
        for m in wx_re.find_iter(text) {
            if is_word_span(&m) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::QqWechat, start: m.start(), end: m.end() });
            }
        }
    }
    if config.is_enabled(SensitiveKind::PasswordField) {
        // 密码/口令键值对：password:/密码: 等后跟非空白值
        let re = regex::Regex::new(
            r"(?i)((?:密码|口令|password|pwd|passwd)\s*[:：=]\s*[^\s,，;；。]+)",
        )
        .unwrap();
        for m in re.find_iter(text) {
            candidates.push(SensitiveSpan { kind: SensitiveKind::PasswordField, start: m.start(), end: m.end() });
        }
    }
    if config.is_enabled(SensitiveKind::IpMac) {
        // IPv4（含内网/公网）+ MAC；OCR 可能把点识别成全角/空格，归一化后已转为半角
        let ip_re = regex::Regex::new(r"((?:[0-9]{1,3}\.){3}[0-9]{1,3})")
            .unwrap();
        for m in ip_re.find_iter(text) {
            candidates.push(SensitiveSpan { kind: SensitiveKind::IpMac, start: m.start(), end: m.end() });
        }
        let mac_re = regex::Regex::new(r"((?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2})").unwrap();
        for m in mac_re.find_iter(text) {
            candidates.push(SensitiveSpan { kind: SensitiveKind::IpMac, start: m.start(), end: m.end() });
        }
    }
    if config.is_enabled(SensitiveKind::UrlParam) {
        // URL 查询参数中携带凭据类字段
        let re = regex::Regex::new(
            r#"(?i)([?&](?:token|key|secret|password|pwd|passwd|sessionid|session_id|api_key|apikey|sign|sig|auth)=[^&\s"'<>]+)"#,
        )
        .unwrap();
        for m in re.find_iter(text) {
            candidates.push(SensitiveSpan { kind: SensitiveKind::UrlParam, start: m.start(), end: m.end() });
        }
    }
    for pat in &config.custom_patterns {
        let pat = pat.trim();
        if pat.is_empty() {
            continue;
        }
        if let Ok(re) = regex::Regex::new(pat) {
            for m in re.find_iter(text) {
                candidates.push(SensitiveSpan { kind: SensitiveKind::Custom, start: m.start(), end: m.end() });
            }
        }
    }

    // 排序并按区间去重（重叠时保留更长的，避免手机号被银行卡规则重复命中）
    candidates.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut spans: Vec<SensitiveSpan> = Vec::new();
    for c in candidates {
        if let Some(last) = spans.last() {
            if c.start < last.end {
                // 重叠：保留已收录的更长的区间（后到的若更长则替换）
                if c.end > last.end && c.start <= last.start {
                    spans.pop();
                    spans.push(c);
                }
                continue;
            }
        }
        spans.push(c);
    }
    spans
}

/// 将文本中的敏感内容全部替换为 `[已隐藏]`。使用默认全量配置。
pub fn redact_text(text: &str) -> String {
    redact_text_with_config(text, &RedactionConfig::default_all())
}

/// 将文本中的敏感内容全部替换为 `[已隐藏]`（按配置过滤类别）。
pub fn redact_text_with_config(text: &str, config: &RedactionConfig) -> String {
    let spans = find_sensitive_with_config(text, config);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for s in &spans {
        out.push_str(&text[cursor..s.start]);
        out.push_str("[已隐藏]");
        cursor = s.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// 判断文本是否包含敏感内容。
pub fn contains_sensitive(text: &str) -> bool {
    !find_sensitive(text).is_empty()
}

/// 对图片指定矩形区域做高斯模糊（近似 3 遍 box blur），其余区域保持不变。
pub fn redact_image_region(img: &DynamicImage, x: i32, y: i32, w: i32, h: i32) -> DynamicImage {
    let mut rgba = img.to_rgba8();
    blur_region_rgba(&mut rgba, x, y, w, h);
    DynamicImage::ImageRgba8(rgba)
}

/// 就地模糊 RgbaImage 的指定区域（3 遍 box blur，半径 3）。
pub fn blur_region_rgba(rgba: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32) {
    let img_w = rgba.width() as i32;
    let img_h = rgba.height() as i32;

    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(img_w);
    let y1 = (y + h).min(img_h);
    if x1 - x0 < 2 || y1 - y0 < 2 {
        return;
    }

    // 提取区域
    let rw = (x1 - x0) as u32;
    let rh = (y1 - y0) as u32;
    let mut region = RgbaImage::new(rw, rh);
    for py in 0..rh {
        for px in 0..rw {
            region.put_pixel(px, py, *rgba.get_pixel((x0 as u32) + px, (y0 as u32) + py));
        }
    }

    // 3 遍 box blur，半径 3（等效较强高斯模糊）
    for _ in 0..3 {
        region = box_blur(&region, 3);
    }

    // 写回
    for py in 0..rh {
        for px in 0..rw {
            rgba.put_pixel((x0 as u32) + px, (y0 as u32) + py, *region.get_pixel(px, py));
        }
    }
}

/// 就地填充实心色块（深灰），用于"实心色块覆盖"脱敏模式。
pub fn fill_block_rgba(rgba: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32) {
    let img_w = rgba.width() as i32;
    let img_h = rgba.height() as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(img_w);
    let y1 = (y + h).min(img_h);
    if x1 - x0 <= 0 || y1 - y0 <= 0 {
        return;
    }
    for py in y0..y1 {
        for px in x0..x1 {
            rgba.put_pixel(px as u32, py as u32, image::Rgba([33, 41, 54, 255]));
        }
    }
}

/// 截图脱敏的覆盖方式（Phase B）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactMode {
    /// 高斯模糊
    Blur,
    /// 实心色块覆盖
    Block,
}

impl RedactMode {
    pub fn from_key(s: Option<&str>) -> Self {
        match s {
            Some("block") => RedactMode::Block,
            _ => RedactMode::Blur,
        }
    }
}

// ── Phase C: 录制时自动脱敏的全局策略缓存 ─────────────────────
//
// 录制事件线程无法安全地持有 DB 锁（高频事件 + 长事务会互相阻塞），
// 因此设置读写命令负责在配置变化时刷新本缓存，录制线程只读缓存判断。

static AUTO_REDACT: std::sync::Mutex<Option<(bool, RedactMode, RedactionConfig)>> =
    std::sync::Mutex::new(None);

/// 由 settings 命令在读写脱敏/录制配置时刷新。
pub fn set_auto_redact_policy(enabled: bool, mode: RedactMode, config: RedactionConfig) {
    *AUTO_REDACT.lock().unwrap() = Some((enabled, mode, config));
}

/// 录制线程读取：开关开启时返回 `(mode, config)`。
pub fn auto_redact_policy() -> Option<(RedactMode, RedactionConfig)> {
    AUTO_REDACT
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(enabled, _, _)| *enabled)
        .map(|(_, mode, config)| (*mode, config.clone()))
}

/// OCR 行文本归一化：全角转半角 + 去除所有空白。
///
/// Windows OCR 对中文/标点常识别出“密 码 ： x”、“47 ． 102 ． 154 ． 233”等
/// 带空格/全角字符的文本，导致敏感正则无法命中。统一去空白、转半角后
/// 再匹配，可显著提升命中率（按行区域遮盖，归一化不影响遮盖坐标）。
fn normalize_ocr_text(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '　' => ' ',
            '：' => ':',
            '．' => '.',
            '，' => ',',
            '；' => ';',
            '。' => '.',
            '（' => '(',
            '）' => ')',
            '％' => '%',
            '＠' => '@',
            '＝' => '=',
            '＋' => '+',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<String>()
}

/// 对 OCR 文本行做命中检测并就地脱敏，返回命中行数（纯逻辑，便于单测）。
fn redact_hits_on_lines(
    rgba: &mut image::RgbaImage,
    lines: &[ocr::OcrLine],
    config: &RedactionConfig,
    mode: RedactMode,
) -> usize {
    let mut hits = 0usize;
    for line in lines {
        let text = normalize_ocr_text(&line.text);
        if text.is_empty() {
            continue;
        }
        if find_sensitive_with_config(&text, config).is_empty() {
            continue;
        }
        // 命中行：区域向外扩 4px，避免文字贴边，裁剪交给内部实现
        let x = (line.x - 4.0).floor().max(0.0) as i32;
        let y = (line.y - 4.0).floor().max(0.0) as i32;
        let w = (line.w + 8.0).ceil() as i32;
        let h = (line.h + 8.0).ceil() as i32;
        match mode {
            RedactMode::Blur => blur_region_rgba(rgba, x, y, w, h),
            RedactMode::Block => fill_block_rgba(rgba, x, y, w, h),
        }
        hits += 1;
    }
    hits
}

// 🔴 瓦片复查（第三级零命中兜底，与扩展端 v0.65.94 同策略）：
// 整图与灰度放大两级 OCR 都零命中且图够大时，把截图切成 grid×grid 带
// 重叠的瓦片逐片 OCR——小字（手机号/学号等）在瓦片里的有效字号放大 4 倍，
// 命中率显著提升。仅零命中图多花 4 次 OCR，不影响命中图的速度。
const TILE_MIN_LONG_EDGE: u32 = 1200;
const TILE_GRID: u32 = 2;
const TILE_OVERLAP: f64 = 0.12;

/// 长边是否达到阈值（纯逻辑，便于单测）。任一边为 0 视为无效图。
fn long_edge_at_least(w: u32, h: u32, min: u32) -> bool {
    if w == 0 || h == 0 {
        return false;
    }
    w.max(h) >= min
}

/// 计算瓦片原点与尺寸列表 `(x0, y0, tw, th)`（纯逻辑，便于单测）。
///
/// ★ v0.65.97 覆盖修复：旧实现 tw = ceil(w/grid) 再带重叠步长，grid 片总覆盖
/// 只有 w×(1-重叠+1/grid)——1920 宽时右侧漏 ~115px 条带，该区域永不被复查。
/// 新实现把瓦片本身放大 (1+重叠) 倍再切：总覆盖 = tw + (grid-1)×step ≥ w。
fn tile_origins(
    w: u32,
    h: u32,
    grid: u32,
    overlap: f64,
) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    if w == 0 || h == 0 || grid == 0 {
        return out;
    }
    let ov = overlap.clamp(0.0, 0.9);
    let tw = ((((w as f64) * (1.0 + ov)) / (grid as f64)).ceil() as u32).clamp(1, w);
    let th = ((((h as f64) * (1.0 + ov)) / (grid as f64)).ceil() as u32).clamp(1, h);
    let step_x = ((tw as f64) * (1.0 - ov)).round().max(1.0) as u32;
    let step_y = ((th as f64) * (1.0 - ov)).round().max(1.0) as u32;
    for row in 0..grid {
        for col in 0..grid {
            let x0 = (col * step_x).min(w - tw);
            let y0 = (row * step_y).min(h - th);
            out.push((x0, y0, tw, th));
        }
    }
    out
}

/// 自动扫描单张截图：OCR 识别文本行 → 按配置检测敏感内容 → 对命中行区域脱敏（模糊/色块）。
///
/// 返回命中的区域数；有命中时写回磁盘原路径。
pub fn redact_screenshot_auto(
    path: &std::path::Path,
    config: &RedactionConfig,
    mode: RedactMode,
) -> Result<usize, String> {
    let lines = ocr::recognize_image(path)?;
    if lines.is_empty() {
        return Ok(0);
    }

    let img = image::open(path).map_err(|e| format!("open image failed: {}", e))?;
    let mut rgba = img.to_rgba8();
    let (iw, ih) = (rgba.width(), rgba.height());
    let mut hits = redact_hits_on_lines(&mut rgba, &lines, config, mode);

    // 🔴 零命中二次机会：Windows OCR 对同一内容的识别有波动（实测同一学号列
    // 在相邻两张近乎相同的截图里一张命中一张漏检），漏检的直接后果是
    // 「有的图打码了有的没打」。灰度预处理可消除 ClearType 彩色边缘，
    // 对小字号数字/字母识别更稳。只对零命中图多花一次 OCR，不影响命中图的速度。
    if hits == 0 {
        if let Ok(lines2) =
            ocr::recognize_crop_with(path, None, image::imageops::FilterType::Nearest, ocr::OCR_UPSCALE, 1)
        {
            hits = redact_hits_on_lines(&mut rgba, &lines2, config, mode);
        }
    }

    // 🔴 瓦片复查（第三级）：前两级（整图 / 灰度放大整图）都零命中且图够大时，
    // 切 2×2 带 12% 重叠的瓦片逐片 OCR——小字（手机号/学号等）在瓦片里的
    // 有效字号放大 4 倍，命中率显著提升。仅零命中图多花 4 次 OCR。
    if hits == 0 && long_edge_at_least(iw, ih, TILE_MIN_LONG_EDGE) {
        for (x0, y0, tw, th) in tile_origins(iw, ih, TILE_GRID, TILE_OVERLAP) {
            let Ok(tlines) = ocr::recognize_crop_with(
                path,
                Some((x0, y0, tw, th)),
                image::imageops::FilterType::Nearest,
                ocr::OCR_UPSCALE,
                1,
            ) else {
                continue; // 单瓦片失败跳过，不阻断
            };
            // 瓦片内坐标 → 全图坐标（recognize_crop_with 的坐标相对裁剪原点）
            let shifted: Vec<ocr::OcrLine> = tlines
                .into_iter()
                .map(|l| ocr::OcrLine {
                    text: l.text,
                    x: l.x + x0 as f32,
                    y: l.y + y0 as f32,
                    w: l.w,
                    h: l.h,
                })
                .collect();
            hits += redact_hits_on_lines(&mut rgba, &shifted, config, mode);
        }
    }

    if hits > 0 {
        // B10: 覆盖原图前先备份副本，避免「扫描脱敏」误脱敏后不可恢复。
        // 备份写入 `<temp>/flowio_redacted/_backup/`，文件名带时间戳防冲撞。
        let backup_dir = std::env::temp_dir()
            .join("flowio_redacted")
            .join("_backup");
        std::fs::create_dir_all(&backup_dir)
            .map_err(|e| format!("create redact backup dir failed: {}", e))?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("original.png");
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let backup_path = backup_dir.join(format!("{}_{}", ts, file_name));
        std::fs::copy(path, &backup_path)
            .map_err(|e| format!("backup original screenshot failed: {}", e))?;

        DynamicImage::ImageRgba8(rgba)
            .save(path)
            .map_err(|e| format!("save redacted image failed: {}", e))?;
    }
    Ok(hits)
}

/// 自动扫描单张截图并生成脱敏副本（Phase C：导出前强制扫描）。
///
/// 与原图写回版 `redact_screenshot_auto` 不同，本函数**不修改原图**：
/// OCR 命中敏感区域时，将脱敏结果写入 `<temp>/flowio_redacted/<文件名>` 并返回副本路径，
/// 供导出/分享链路使用（原始数据保持不动，合规）。
/// 未命中或读取失败时返回 `Ok(None)`。
pub fn redact_screenshot_copy(
    path: &std::path::Path,
    config: &RedactionConfig,
    mode: RedactMode,
) -> Result<Option<std::path::PathBuf>, String> {
    let lines = ocr::recognize_image(path)?;
    if lines.is_empty() {
        return Ok(None);
    }

    let img = image::open(path).map_err(|e| format!("open image failed: {}", e))?;
    let mut rgba = img.to_rgba8();
    let mut hits = 0usize;
    for line in &lines {
        let text = normalize_ocr_text(&line.text);
        if text.is_empty() {
            continue;
        }
        if find_sensitive_with_config(&text, config).is_empty() {
            continue;
        }
        let x = (line.x - 4.0).floor().max(0.0) as i32;
        let y = (line.y - 4.0).floor().max(0.0) as i32;
        let w = (line.w + 8.0).ceil() as i32;
        let h = (line.h + 8.0).ceil() as i32;
        match mode {
            RedactMode::Blur => blur_region_rgba(&mut rgba, x, y, w, h),
            RedactMode::Block => fill_block_rgba(&mut rgba, x, y, w, h),
        }
        hits += 1;
    }
    if hits == 0 {
        return Ok(None);
    }

    // 写入隔离目录，避免污染系统 temp 根目录
    let dir = std::env::temp_dir().join("flowio_redacted");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create redacted dir failed: {}", e))?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("redacted.png");
    let out = dir.join(file_name);
    DynamicImage::ImageRgba8(rgba)
        .save(&out)
        .map_err(|e| format!("save redacted copy failed: {}", e))?;
    Ok(Some(out))
}

/// 对 base64 编码的截图做 OCR 脱敏（AI 外发大模型前专用）。
///
/// 将 base64 解码写入临时文件后复用 [`redact_screenshot_copy`] 的 OCR 脱敏逻辑，
/// 再把脱敏副本读回 base64 返回。未命中敏感内容时返回 `Ok(None)`（原图标量相等）。
///
/// 注意：OCR 失败（引擎初始化失败、读图失败）时返回 `Err`，调用方必须**丢弃该截图**
/// 而不是原样外发——此时敏感像素未经遮挡，宁可缺失参考图也不能裸传。
pub fn redact_base64_screenshot(
    b64: &str,
    config: &RedactionConfig,
    mode: RedactMode,
) -> Result<Option<String>, String> {
    if b64.is_empty() {
        return Ok(None);
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("decode base64 failed: {}", e))?;

    // 落临时文件后复用 redact_screenshot_copy（OCR 引擎仅支持文件路径）
    let dir = std::env::temp_dir().join("flowio_ai_redact");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create redact dir failed: {}", e))?;
    let tmp = dir.join(format!(
        "ai_{}_{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("write temp image failed: {}", e))?;

    let result = redact_screenshot_copy(&tmp, config, mode);
    let _ = std::fs::remove_file(&tmp);

    match result? {
        Some(copy_path) => {
            let data = std::fs::read(&copy_path)
                .map_err(|e| format!("read redacted copy failed: {}", e))?;
            let _ = std::fs::remove_file(&copy_path);
            Ok(Some(base64::engine::general_purpose::STANDARD.encode(&data)))
        }
        None => Ok(None),
    }
}

/// 对整个图片做高斯模糊（整图脱敏，区域接口的便捷封装）。
pub fn redact_image_full(img: &DynamicImage) -> DynamicImage {
    let w = img.width() as i32;
    let h = img.height() as i32;
    redact_image_region(img, 0, 0, w, h)
}

/// box blur（一次水平 + 一次垂直 = 一遍）。
fn box_blur(img: &RgbaImage, radius: u32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let mut tmp = RgbaImage::new(w, h);
    let mut out = RgbaImage::new(w, h);
    let r = radius as i64;
    let div = (2 * r + 1) as u32;

    // 水平
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0i64; 4];
            for dx in -r..=r {
                let sx = (x as i64 + dx).clamp(0, w as i64 - 1) as u32;
                let p = img.get_pixel(sx, y);
                acc[0] += p[0] as i64;
                acc[1] += p[1] as i64;
                acc[2] += p[2] as i64;
                acc[3] += p[3] as i64;
            }
            tmp.put_pixel(x, y, image::Rgba([
                (acc[0] / div as i64) as u8,
                (acc[1] / div as i64) as u8,
                (acc[2] / div as i64) as u8,
                (acc[3] / div as i64) as u8,
            ]));
        }
    }
    // 垂直
    for x in 0..w {
        for y in 0..h {
            let mut acc = [0i64; 4];
            for dy in -r..=r {
                let sy = (y as i64 + dy).clamp(0, h as i64 - 1) as u32;
                let p = tmp.get_pixel(x, sy);
                acc[0] += p[0] as i64;
                acc[1] += p[1] as i64;
                acc[2] += p[2] as i64;
                acc[3] += p[3] as i64;
            }
            out.put_pixel(x, y, image::Rgba([
                (acc[0] / div as i64) as u8,
                (acc[1] / div as i64) as u8,
                (acc[2] / div as i64) as u8,
                (acc[3] / div as i64) as u8,
            ]));
        }
    }
    out
}

#[cfg(test)]
mod repro_tests {
    use super::*;

    /// 回归：regex 编译配置不支持 look-around 时曾导致 Regex::new().unwrap() panic 崩溃
    #[test]
    fn repro_redact_edge_inputs() {
        let samples = [
            "手机号13800138000在中间",
            "身份证110101199003071234号",
            "卡号6222020202020202020结束",
            "邮箱zhang@example.com后面",
            "混合：13800138000 和 110101199003071234 和 a@b.cn",
            "连续数字123456789012345678901234567890太长",
            "中文正常描述，点击按钮，输入内容",
            "前后数字13 13800138000 4边界",
            "",
        ];
        for s in samples {
            let spans = find_sensitive(s);
            let _ = redact_text(s);
            let _ = contains_sensitive(s);
            // 校验 span 不越界、有序不重叠
            let mut prev_end = 0usize;
            for sp in &spans {
                assert!(sp.start <= sp.end && sp.end <= s.len(), "bad span {sp:?} in {s:?}");
                assert!(sp.start >= prev_end, "overlap {sp:?} in {s:?}");
                prev_end = sp.end;
            }
        }
    }

    /// 确保身份证/手机号/银行卡/邮箱仍能被识别
    #[test]
    fn repro_redact_kinds() {
        let cases = [
            ("手机号是13800138000", SensitiveKind::Phone),
            ("身份证110101199003071234", SensitiveKind::IdCard),
            ("银行卡6222020202020202020", SensitiveKind::BankCard),
            ("邮箱abc@example.com", SensitiveKind::Email),
        ];
        for (text, kind) in cases {
            let spans = find_sensitive(text);
            assert!(spans.iter().any(|s| s.kind == kind), "miss {kind:?} in {text:?}: {spans:?}");
        }
    }

    /// 新增类别：API Key / 信用代码 / QQ微信 / 密码 / 内网IP / MAC / URL 参数
    #[test]
    fn repro_redact_new_kinds() {
        let cases = [
            ("密钥sk-abcdefghijklmnop123456", SensitiveKind::ApiKey),
            ("Token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef", SensitiveKind::ApiKey),
            ("信用代码91330100MA2ABCDEFG", SensitiveKind::CreditCode),
            ("QQ号123456789", SensitiveKind::QqWechat),
            ("微信号wxid_ab12cd34ef56", SensitiveKind::QqWechat),
            ("密码：P@ssw0rd123", SensitiveKind::PasswordField),
            ("password=abc123xyz", SensitiveKind::PasswordField),
            ("内网IP 192.168.1.100", SensitiveKind::IpMac),
            ("MAC aa:bb:cc:dd:ee:ff", SensitiveKind::IpMac),
            ("链接 https://x.com?a=1&token=secret7890", SensitiveKind::UrlParam),
        ];
        for (text, kind) in cases {
            let spans = find_sensitive(text);
            assert!(spans.iter().any(|s| s.kind == kind), "miss {kind:?} in {text:?}: {spans:?}");
        }
    }

    /// 探针：真实截图 OCR 识别 + 归一化 + 敏感命中（临时，验证后删除）
    #[test]
    fn probe_ocr_real_screenshot() {
        let path = std::path::Path::new(r"C:\Users\taotao\AppData\Roaming\Tencent\Marvis\User\oAN1i2RTK1OdpZWkE6hUeDLr1hbE\workspace\conv_19fbbedbec4_414988788e37\temp\idx5_after.jpg");
        // 正式配置回归：整图 NEAR-3x；参考：IP 区与密码区
        let cases = [
            ("FULL-NEAR-3x", None, image::imageops::FilterType::Nearest, 3u32, 0),
            ("NEAR-3x-ip", Some((180, 480, 380, 50)), image::imageops::FilterType::Nearest, 3u32, 0),
            ("NEAR-3x-pwd", Some((180, 500, 140, 70)), image::imageops::FilterType::Nearest, 3u32, 0),
        ];
        for (name, crop, filter, scale, preprocess) in cases {
            let t0 = std::time::Instant::now();
            let res = super::ocr::recognize_crop_with(path, crop, filter, scale, preprocess);
            match res {
                Ok(lines) => {
                    let texts: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
                    let n = normalize_ocr_text(&texts.join(" "));
                    let hits = find_sensitive_with_config(&n, &RedactionConfig::default_all());
                    let hit_names: Vec<String> = hits.iter().map(|s| format!("{:?}", s.kind)).collect();
                    println!("[{}] {}ms lines={} texts={:?}", name, t0.elapsed().as_millis(), lines.len(), texts);
                    println!("[{}] HITS: {:?}", name, hit_names);
                }
                Err(e) => println!("[{}] ERR: {}", name, e),
            }
        }
    }

    /// 配置开关：关闭某类别后不再命中
    #[test]
    fn repro_redact_config_disable() {
        let cfg = RedactionConfig {
            enabled: vec![SensitiveKind::Phone],
            custom_patterns: vec![],
        };
        let spans = find_sensitive_with_config("手机13800138000 邮箱a@b.cn", &cfg);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, SensitiveKind::Phone);

        // 自定义正则生效
        let cfg2 = RedactionConfig {
            enabled: vec![SensitiveKind::Custom],
            custom_patterns: vec![r"员工号\s*[:：]\s*[A-Z]{2}\d{4}".to_string()],
        };
        let spans2 = find_sensitive_with_config("员工号: AB1234 内容", &cfg2);
        assert_eq!(spans2.len(), 1);
        assert_eq!(spans2[0].kind, SensitiveKind::Custom);
    }

    /// 🔴 回归（2026-09-06）：enabled 为**空数组**时必须原样生效（= 什么都不检测）。
    /// 之前空数组被当成「未配置」静默回落 default_all：设置里全不勾，扫描仍按全量规则跑。
    #[test]
    fn from_json_empty_enabled_means_none() {
        let cfg = RedactionConfig::from_json(&serde_json::json!({
            "enabled": [],
            "custom_patterns": []
        }));
        assert!(cfg.enabled.is_empty(), "空 enabled 必须保持为空");
        // 空配置下任何文本都不命中（手机号也不行）
        assert!(find_sensitive_with_config("手机13800138000", &cfg).is_empty());
    }

    /// 旧配置兼容：**键缺失**（无 enabled 字段）才回落全量默认
    #[test]
    fn from_json_missing_key_defaults_all() {
        let cfg = RedactionConfig::from_json(&serde_json::json!({}));
        assert_eq!(cfg.enabled.len(), SensitiveKind::ALL.len());
        // 只勾一部分 → 原样生效
        let cfg2 = RedactionConfig::from_json(&serde_json::json!({ "enabled": ["phone"] }));
        assert_eq!(cfg2.enabled, vec![SensitiveKind::Phone]);
    }

    /// redact_hits_on_lines：命中行就地脱敏 + 零命中返回 0（纯逻辑）
    #[test]
    fn redact_hits_on_lines_basic() {
        let mut img = image::RgbaImage::new(200, 40);
        for (_, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([240, 240, 240, 255]);
        }
        let lines = vec![
            ocr::OcrLine { text: "联系电话 13800138000".into(), x: 10.0, y: 10.0, w: 120.0, h: 16.0 },
            ocr::OcrLine { text: "普通说明文字".into(), x: 10.0, y: 30.0, w: 80.0, h: 8.0 },
        ];
        let cfg = RedactionConfig { enabled: vec![SensitiveKind::Phone], custom_patterns: vec![] };
        let hits = redact_hits_on_lines(&mut img, &lines, &cfg, RedactMode::Blur);
        assert_eq!(hits, 1);
        // 空配置 → 0 命中
        let empty = RedactionConfig { enabled: vec![], custom_patterns: vec![] };
        let mut img2 = image::RgbaImage::new(200, 40);
        assert_eq!(redact_hits_on_lines(&mut img2, &lines, &empty, RedactMode::Blur), 0);
    }

    /// long_edge_at_least：阈值判断与无效图兜底（纯逻辑）
    #[test]
    fn long_edge_at_least_basic() {
        assert!(long_edge_at_least(1920, 1080, TILE_MIN_LONG_EDGE));
        assert!(long_edge_at_least(800, TILE_MIN_LONG_EDGE, TILE_MIN_LONG_EDGE));
        assert!(!long_edge_at_least(1199, 800, TILE_MIN_LONG_EDGE));
        assert!(!long_edge_at_least(0, 0, 1)); // 无效图不切瓦片
    }

    /// tile_origins：2×2 瓦片覆盖全图（v0.65.97 含边缘）、带重叠、不越界（纯逻辑）
    #[test]
    fn tile_origins_basic() {
        let tiles = tile_origins(1920, 1080, TILE_GRID, TILE_OVERLAP);
        assert_eq!(tiles.len(), 4);
        // ★ v0.65.97: tw = ceil(1920×1.12/2) = 1076（旧实现 960 会漏右侧 115px）
        let tw = ((((1920_f64) * (1.0 + TILE_OVERLAP)) / 2.0).ceil()) as u32;
        let th = ((((1080_f64) * (1.0 + TILE_OVERLAP)) / 2.0).ceil()) as u32;
        // 全部瓦片尺寸一致且不越界
        for &(x0, y0, w, h) in &tiles {
            assert_eq!((w, h), (tw, th));
            assert!(x0 + w <= 1920 && y0 + h <= 1080);
        }
        // ★ 全覆盖：x/y 两轴上 末片终点 ≥ 全图尺寸、首片贴原点、相邻片衔接无缝
        let mut xs: Vec<u32> = tiles.iter().map(|t| t.0).collect();
        xs.sort_unstable();
        xs.dedup();
        assert_eq!(xs[0], 0);
        assert_eq!(xs[0] + tw, 0 + tw); // sanity
        assert!(xs.last().unwrap() + tw >= 1920, "x 轴必须盖满右边缘");
        assert!(xs.windows(2).all(|w| w[1] <= w[0] + tw), "x 轴相邻片衔接无缝");
        let mut ys: Vec<u32> = tiles.iter().map(|t| t.1).collect();
        ys.sort_unstable();
        ys.dedup();
        assert_eq!(ys[0], 0);
        assert!(ys.last().unwrap() + th >= 1080, "y 轴必须盖满下边缘");
        assert!(ys.windows(2).all(|w| w[1] <= w[0] + th));
        // 小图（未达阈值尺寸的极端小图）不除零、不越界且覆盖
        let tiny = tile_origins(10, 10, TILE_GRID, TILE_OVERLAP);
        assert_eq!(tiny.len(), 4);
        for &(x0, y0, w, h) in &tiny {
            assert!(x0 + w <= 10 && y0 + h <= 10);
        }
        assert!(tiny.iter().map(|t| t.0 + t.2).max().unwrap() >= 10);
        // 无效输入返回空
        assert!(tile_origins(0, 0, 2, 0.12).is_empty());
    }
}

/// 脱敏功能系统化测试：文本检测/替换、图片区域脱敏、配置与策略、OCR 文本归一化。
#[cfg(test)]
mod redaction_tests {
    use super::*;

    // ── 文本检测：各类别命中 ─────────────────────────────────

    #[test]
    fn detects_all_kinds() {
        let cases: &[(&str, SensitiveKind)] = &[
            ("手机号是13800138000", SensitiveKind::Phone),
            ("身份证110101199003071234号", SensitiveKind::IdCard),
            ("银行卡6222020202020202020结束", SensitiveKind::BankCard),
            ("邮箱abc@example.com", SensitiveKind::Email),
            ("密钥sk-abcdefghijklmnop123456", SensitiveKind::ApiKey),
            ("Token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef", SensitiveKind::ApiKey),
            ("信用代码91330100MA2ABCDEFG", SensitiveKind::CreditCode),
            ("QQ号123456789", SensitiveKind::QqWechat),
            ("微信号wxid_ab12cd34ef56", SensitiveKind::QqWechat),
            ("密码：P@ssw0rd123", SensitiveKind::PasswordField),
            ("password=abc123xyz", SensitiveKind::PasswordField),
            ("内网IP 192.168.1.100", SensitiveKind::IpMac),
            ("MAC aa:bb:cc:dd:ee:ff", SensitiveKind::IpMac),
            ("链接 https://x.com?a=1&token=secret7890", SensitiveKind::UrlParam),
        ];
        for (text, kind) in cases {
            let spans = find_sensitive(text);
            assert!(spans.iter().any(|s| s.kind == *kind), "miss {:?} in {:?}: {:?}", kind, text, spans);
        }
    }

    #[test]
    fn detects_custom_pattern() {
        let cfg = RedactionConfig {
            enabled: vec![SensitiveKind::Custom],
            custom_patterns: vec![r"工号[:：]\d{4}".to_string()],
        };
        let spans = find_sensitive_with_config("工号:8848", &cfg);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, SensitiveKind::Custom);
        // 非法正则应被忽略，不 panic
        let cfg_bad = RedactionConfig {
            enabled: vec![SensitiveKind::Custom],
            custom_patterns: vec!["(".to_string(), r"正常\d+".to_string()],
        };
        let spans_bad = find_sensitive_with_config("正常123", &cfg_bad);
        assert_eq!(spans_bad.len(), 1);
        assert_eq!(spans_bad[0].kind, SensitiveKind::Custom);
    }

    // ── 文本检测：边界条件 ───────────────────────────────────

    #[test]
    fn digit_boundary_prevents_partial_digits() {
        let cases: &[(&str, usize)] = &[
            ("13800138000", 1),      // 标准 11 位
            ("138001380001", 0),     // 后接数字：不是独立手机号
            ("213800138000", 0),     // 前接数字
            ("x13800138000", 1),     // 前接字母仍命中
            ("13800138000x", 1),     // 后接字母仍命中
        ];
        for (text, n) in cases {
            assert_eq!(find_sensitive(text).len(), *n, "text={:?}", text);
        }
    }

    #[test]
    fn email_boundary_prevents_partial() {
        let cases: &[(&str, usize)] = &[
            ("abc@example.com", 1),
            // 尾部字母会被域名部分贪婪吞入，整体仍是合法邮箱
            ("abc@example.comx", 1),
            // 域名无法吞入的邮箱字符（-/_）触发尾部边界拦截
            ("abc@example.com-", 0),
            ("abc@example.com_", 0),
            // 前/后接非邮箱字符：整体/局部正常命中
            ("xabc@example.com", 1),
            ("联系abc@example.com吧", 1),
        ];
        for (text, n) in cases {
            assert_eq!(find_sensitive(text).len(), *n, "text={:?}", text);
        }
    }

    #[test]
    fn empty_and_noise_text_no_hits() {
        assert!(find_sensitive("").is_empty());
        assert!(find_sensitive("普通中文描述，点击按钮，确认提交").is_empty());
        assert!(find_sensitive("1 2 3 4 5 6 7 8 9 10").is_empty());
    }

    // ── 重叠去重：长区间优先 ─────────────────────────────────

    #[test]
    fn overlapping_spans_keep_longer() {
        // 19 位连续数字：银行卡规则命中且只保留一条
        let text = "卡号6222020202020202020";
        let spans = find_sensitive(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, SensitiveKind::BankCard);

        // 相邻敏感片段各自保留、顺序正确
        let text2 = "手机13800138000邮箱abc@example.com";
        let spans2 = find_sensitive(text2);
        assert_eq!(spans2.len(), 2);
        assert!(spans2[0].start < spans2[1].start);
    }

    // ── 文本替换 ─────────────────────────────────────────────

    #[test]
    fn redact_text_replaces_and_preserves_order() {
        let out = redact_text("联系人：13800138000，邮箱 abc@example.com，请查收");
        assert!(out.contains("[已隐藏]"));
        assert!(!out.contains("13800138000"));
        assert!(!out.contains("abc@example.com"));
        assert!(out.starts_with("联系人："));
        assert!(out.ends_with("，请查收"));
        assert!(out.matches("[已隐藏]").count() == 2);
    }

    #[test]
    fn redact_text_clean_text_unchanged() {
        let text = "普通内容，无敏感信息";
        assert_eq!(redact_text(text), text);
    }

    #[test]
    fn redact_text_respects_config() {
        let cfg = RedactionConfig {
            enabled: vec![SensitiveKind::Phone],
            custom_patterns: vec![],
        };
        let out = redact_text_with_config("手机13800138000 邮箱abc@example.com", &cfg);
        assert!(out.contains("[已隐藏]"));
        assert!(out.contains("abc@example.com")); // 邮箱未启用 → 保留
    }

    #[test]
    fn contains_sensitive_basic() {
        assert!(contains_sensitive("密码：abc123"));
        assert!(!contains_sensitive("没有敏感信息"));
    }

    // ── OCR 文本归一化 ────────────────────────────────────────

    #[test]
    fn normalize_fullwidth_and_whitespace() {
        assert_eq!(normalize_ocr_text("密 码 ： x"), "密码:x");
        assert_eq!(normalize_ocr_text("47 ． 102 ． 154 ． 233"), "47.102.154.233");
        assert_eq!(normalize_ocr_text("password = abc"), "password=abc");
        assert_eq!(normalize_ocr_text("　全角空格　"), "全角空格");
        assert_eq!(normalize_ocr_text(""), "");
    }

    #[test]
    fn normalize_keeps_chinese_digits() {
        // 全角数字不在归一化映射内，保持原样；中文保留
        assert_eq!(normalize_ocr_text("手机１３８"), "手机１３８");
    }

    // ── 图片区域脱敏 ─────────────────────────────────────────

    fn gradient_image(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, image::Rgba([(x * 6) as u8, 100, (y * 8) as u8, 255]));
            }
        }
        img
    }

    #[test]
    fn blur_changes_region_keeps_outside() {
        let mut img = gradient_image(40, 30);
        let before_center = *img.get_pixel(20, 15);
        let before_outside = *img.get_pixel(1, 1);
        blur_region_rgba(&mut img, 10, 10, 20, 10);
        assert_ne!(*img.get_pixel(20, 15), before_center, "区域内应被模糊");
        assert_eq!(*img.get_pixel(1, 1), before_outside, "区域外不应变化");
    }

    #[test]
    fn block_fills_region_keeps_outside() {
        let mut img = gradient_image(20, 20);
        let before_outside = *img.get_pixel(0, 0);
        fill_block_rgba(&mut img, 5, 5, 10, 10);
        for y in 5..15 {
            for x in 5..15 {
                assert_eq!(*img.get_pixel(x, y), image::Rgba([33, 41, 54, 255]));
            }
        }
        assert_eq!(*img.get_pixel(0, 0), before_outside);
        assert_ne!(*img.get_pixel(19, 19), image::Rgba([33, 41, 54, 255]));
    }

    #[test]
    fn region_clip_no_panic() {
        let mut img = gradient_image(10, 10);
        // 越界区域裁剪到图内，不 panic
        fill_block_rgba(&mut img, -5, -5, 100, 100);
        assert_eq!(*img.get_pixel(0, 0), image::Rgba([33, 41, 54, 255]));
        // 过小区域 blur 直接跳过，不 panic
        let mut img2 = gradient_image(10, 10);
        blur_region_rgba(&mut img2, 10, 10, 1, 1);
        blur_region_rgba(&mut img2, 20, 20, 0, 0);
    }

    #[test]
    fn image_region_and_full_keep_dimensions() {
        let img = DynamicImage::ImageRgba8(gradient_image(16, 12));
        let blurred = redact_image_region(&img, 2, 2, 8, 8);
        assert_eq!(blurred.width(), 16);
        assert_eq!(blurred.height(), 12);
        let full = redact_image_full(&img);
        assert_eq!(full.width(), 16);
        assert_eq!(full.height(), 12);
    }

    // ── 配置与策略 ───────────────────────────────────────────

    #[test]
    fn redact_mode_from_key() {
        assert_eq!(RedactMode::from_key(Some("block")), RedactMode::Block);
        assert_eq!(RedactMode::from_key(Some("blur")), RedactMode::Blur);
        assert_eq!(RedactMode::from_key(None), RedactMode::Blur);
        assert_eq!(RedactMode::from_key(Some("unknown")), RedactMode::Blur);
    }

    #[test]
    fn kind_key_roundtrip() {
        for k in SensitiveKind::ALL {
            assert_eq!(SensitiveKind::from_key(k.key()), Some(k), "key={}", k.key());
        }
        assert_eq!(SensitiveKind::from_key("not_a_kind"), None);
        assert_eq!(SensitiveKind::from_key(""), None);
    }

    #[test]
    fn config_json_roundtrip() {
        let cfg = RedactionConfig {
            enabled: vec![SensitiveKind::Phone, SensitiveKind::Email, SensitiveKind::Custom],
            custom_patterns: vec![r"\d{4}".to_string()],
        };
        let cfg2 = RedactionConfig::from_json(&cfg.to_json());
        assert_eq!(cfg2.enabled, cfg.enabled);
        assert_eq!(cfg2.custom_patterns, cfg.custom_patterns);
    }

    #[test]
    fn config_json_empty_enabled_means_none() {
        let cfg = RedactionConfig::from_json(&serde_json::json!({
            "enabled": [],
            "custom_patterns": []
        }));
        // 🔴 2026-09-06 行为变更：空 enabled = 用户全部不勾 = 什么都不检测（原为回落全量）
        assert!(cfg.enabled.is_empty());
        assert!(cfg.custom_patterns.is_empty());
    }

    #[test]
    fn config_json_ignores_unknown_kind() {
        let cfg = RedactionConfig::from_json(&serde_json::json!({
            "enabled": ["phone", "not_a_kind"],
            "custom_patterns": []
        }));
        assert_eq!(cfg.enabled, vec![SensitiveKind::Phone]);
    }

    #[test]
    fn auto_redact_policy_gate() {
        // 初始无策略
        assert!(auto_redact_policy().is_none());
        // 开启后返回策略
        set_auto_redact_policy(true, RedactMode::Block, RedactionConfig::default_all());
        let (mode, cfg) = auto_redact_policy().expect("policy should be set");
        assert_eq!(mode, RedactMode::Block);
        assert_eq!(cfg.enabled.len(), SensitiveKind::ALL.len());
        // 关闭后返回 None
        set_auto_redact_policy(false, RedactMode::Blur, RedactionConfig::default_all());
        assert!(auto_redact_policy().is_none());
        // 复位，避免影响其他测试
        set_auto_redact_policy(true, RedactMode::Blur, RedactionConfig::default_all());
    }


    // ── 边界与压力：补充用例 ──────────────────────────────

    #[test]
    fn redact_very_long_mixed_text_no_panic() {
        let mut text = String::new();
        for _ in 0..5000 {
            text.push_str("手机号13800138000 邮箱abc@example.com 密码:abc123 正常内容");
        }
        let spans = find_sensitive(&text);
        assert!(spans.len() >= 15000, "expected >=15000 hits, got {}", spans.len());
        let redacted = redact_text(&text);
        assert!(!redacted.contains("13800138000"));
        assert!(!redacted.contains("abc@example.com"));
        assert!(!redacted.contains("abc123"));
    }

    #[test]
    fn redact_text_multiple_same_kind() {
        let out = redact_text("手机13800138000 手机13900139000 邮箱a@b.cn");
        assert!(out.matches("[已隐藏]").count() == 3);
    }

    #[test]
    fn spans_stress_sorted_non_overlap() {
        let mut text = String::new();
        for i in 0..300 {
            text.push_str(&format!("第{}项：手机13800138000 邮箱a{}@b.cn 卡号6222020202020202020 ", i, i % 10));
        }
        let spans = find_sensitive(&text);
        assert_eq!(spans.len(), 900);
        let mut prev_end = 0usize;
        for sp in &spans {
            assert!(sp.start <= sp.end && sp.end <= text.len());
            assert!(sp.start >= prev_end);
            prev_end = sp.end;
        }
    }

    #[test]
    fn normalize_ocr_text_then_detect() {
        let n = normalize_ocr_text("138 0013 8000");
        assert_eq!(n, "13800138000");
        assert!(contains_sensitive(&n));
    }

    #[test]
    fn redact_text_with_config_empty_enabled_disables_all() {
        let cfg = RedactionConfig {
            enabled: vec![],
            custom_patterns: vec![],
        };
        let out = redact_text_with_config("手机13800138000", &cfg);
        assert!(!out.contains("[已隐藏]"), "empty enabled means no kinds matched");
    }

    #[test]
    fn redact_text_with_config_enabled_phone_only() {
        let cfg = RedactionConfig {
            enabled: vec![SensitiveKind::from_key("phone").unwrap()],
            custom_patterns: vec![],
        };
        let out = redact_text_with_config("手机13800138000 邮箱abc@example.com", &cfg);
        assert!(out.contains("[已隐藏]"));
        assert!(out.contains("abc@example.com"));
    }

    #[test]
    fn contains_sensitive_long_clean_text_false() {
        let text = "这是一个完全没有敏感信息的普通描述文本，".repeat(200);
        assert!(!contains_sensitive(&text));
    }

    // B8 回归：银行卡正则 13-19 → 16-19，13~15 位纯数字（订单/流水号）不得误伤
    #[test]
    fn bank_card_boundary_16_19_not_13_15() {
        let cases: &[(&str, usize)] = &[
            // 13/14/15 位：不该命中（B8 收紧点）
            ("订单号1234567890123", 0),
            ("流水号12345678901234", 0),
            ("编号123456789012345", 0),
            // 16/19 位：应命中
            ("卡号1234567890123456", 1),
            ("卡号1234567890123456789", 1),
        ];
        for (text, n) in cases {
            let hits = find_sensitive(text)
                .iter()
                .filter(|s| s.kind == SensitiveKind::BankCard)
                .count();
            assert_eq!(hits, *n, "text={:?}", text);
        }
    }

    #[test]
    fn bank_card_20_digits_behavior_pinned() {
        // 记录 20 位连续数字行为：正则 ≤19 位配合数字边界，20 位整段不被命中。
        // 若产品要求覆盖 20 位，需扩展正则并在下一条规则前加长数处理。
        let text = "20位数字12345678901234567890";
        let hits = find_sensitive(text)
            .iter()
            .filter(|s| s.kind == SensitiveKind::BankCard)
            .count();
        assert_eq!(hits, 0);
    }
}
