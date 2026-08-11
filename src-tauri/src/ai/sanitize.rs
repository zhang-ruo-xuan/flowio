use regex::Regex;
use std::sync::LazyLock;

// ── Pre-compiled regex patterns ───────────────────────────────

static PHONE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"1[3-9]\d{9}").unwrap());

static ID_CARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{17}[\dXx]").unwrap());

static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\w.\-]+@[\w.\-]+\.\w+").unwrap());

static BANK_CARD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{16,19}").unwrap());

static CREDIT_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9A-HJ-NPQRTUWXY]{18}").unwrap());

static PLATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[京津沪渝冀豫云辽黑湘皖鲁新苏浙赣鄂桂甘晋蒙陕吉闽贵粤川青藏琼宁][A-Z][A-HJ-NP-Z0-9]{4,5}[A-HJ-NP-Z0-9挂学警港澳]",
    )
    .unwrap()
});

// ── Public API ────────────────────────────────────────────────

/// Replace sensitive personal information (phone, ID card, email,
/// bank card, credit code, license plate) with `***` placeholders.
pub fn sanitize_text(text: &str) -> String {
    let mut result = text.to_string();

    // Order matters: ID card (18 digits) before bank card (16-19) to avoid
    // partial match; also, bank card should NOT match inside an ID card.
    // We process ID card first, then bank card on the already-sanitized output.
    result = ID_CARD_RE.replace_all(&result, "***").to_string();
    result = CREDIT_CODE_RE.replace_all(&result, "***").to_string();
    result = PHONE_RE.replace_all(&result, "***").to_string();
    result = EMAIL_RE.replace_all(&result, "***").to_string();
    result = BANK_CARD_RE.replace_all(&result, "***").to_string();
    result = PLATE_RE.replace_all(&result, "***").to_string();

    result
}

/// Sanitize text **before** sending it to the AI model.
///
/// This is a convenience alias for `sanitize_text`. In future
/// it may incorporate OCR-based or fuzzy matching logic.
pub fn sanitize_for_ai(text: &str) -> String {
    sanitize_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phone_sanitize() {
        assert_eq!(sanitize_text("电话：13800138000。"), "电话：***。");
    }

    #[test]
    fn test_email_sanitize() {
        assert_eq!(
            sanitize_text("邮箱 user@example.com 联系"),
            "邮箱 *** 联系"
        );
    }

    #[test]
    fn test_id_card_sanitize() {
        assert_eq!(
            sanitize_text("身份证 110101199001011234"),
            "身份证 ***"
        );
    }

    #[test]
    fn test_bank_card_sanitize() {
        assert_eq!(
            sanitize_text("卡号 6222021234567890123"),
            "卡号 ***"
        );
    }

    #[test]
    fn test_no_sensitive() {
        let input = "点击右上角的设置按钮";
        assert_eq!(sanitize_text(input), input);
    }
}
