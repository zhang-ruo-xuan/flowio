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
    LazyLock::new(|| Regex::new(r"\d{19}").unwrap());

static BANK_CARD_RE_16_18: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{16,18}").unwrap());

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
/// bank card, credit code, license plate) with `[已隐藏]` placeholders.
pub fn sanitize_text(text: &str) -> String {
    let mut result = text.to_string();

    // Order matters (regex crate has no look-around, use staged rules):
    // 1) 19-digit bank card first, so it is not partially eaten by the
    //    18-digit ID-card rule (which would leak the last digit);
    // 2) ID card (18 digits, may end with X) also covers 18-digit bank cards;
    // 3) remaining 16-18 digit bank cards are caught last.
    // Overlapping matches all become `[已隐藏]`, so the result is equivalent.
    result = BANK_CARD_RE.replace_all(&result, "[已隐藏]").to_string();
    result = ID_CARD_RE.replace_all(&result, "[已隐藏]").to_string();
    result = BANK_CARD_RE_16_18.replace_all(&result, "[已隐藏]").to_string();
    result = CREDIT_CODE_RE.replace_all(&result, "[已隐藏]").to_string();
    result = PHONE_RE.replace_all(&result, "[已隐藏]").to_string();
    result = EMAIL_RE.replace_all(&result, "[已隐藏]").to_string();
    result = PLATE_RE.replace_all(&result, "[已隐藏]").to_string();

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
        assert_eq!(sanitize_text("电话：13800138000。"), "电话：[已隐藏]。");
    }

    #[test]
    fn test_email_sanitize() {
        assert_eq!(
            sanitize_text("邮箱 user@example.com 联系"),
            "邮箱 [已隐藏] 联系"
        );
    }

    #[test]
    fn test_id_card_sanitize() {
        assert_eq!(
            sanitize_text("身份证 110101199001011234"),
            "身份证 [已隐藏]"
        );
    }

    #[test]
    fn test_bank_card_sanitize() {
        assert_eq!(
            sanitize_text("卡号 6222021234567890123"),
            "卡号 [已隐藏]"
        );
    }

    #[test]
    fn test_bank_card_16_digits() {
        assert_eq!(sanitize_text("卡号 6222021234567890"), "卡号 [已隐藏]");
    }

    #[test]
    fn test_bank_card_17_digits() {
        assert_eq!(sanitize_text("卡号 62220212345678901"), "卡号 [已隐藏]");
    }

    #[test]
    fn test_bank_card_18_digits() {
        assert_eq!(sanitize_text("卡号 622202123456789012"), "卡号 [已隐藏]");
    }

    #[test]
    fn test_id_card_with_x() {
        assert_eq!(
            sanitize_text("身份证 11010119900101123X"),
            "身份证 [已隐藏]"
        );
        assert_eq!(
            sanitize_text("身份证 11010119900101123x"),
            "身份证 [已隐藏]"
        );
    }

    #[test]
    fn test_bank_card_and_id_card_together() {
        let input = "身份证 110101199001011234，卡号 6222021234567890123";
        assert_eq!(sanitize_text(input), "身份证 [已隐藏]，卡号 [已隐藏]");
    }

    #[test]
    fn test_bank_card_not_eaten_by_id_card() {
        // 19-digit bank card must not leave a trailing digit after ID-card pass
        let input = "借记卡：6222021234567890123";
        assert_eq!(sanitize_text(input), "借记卡：[已隐藏]");
    }

    #[test]
    fn test_credit_code_sanitize() {
        assert_eq!(
            sanitize_text("统一社会信用代码 91310000MA1FL1XW2A"),
            "统一社会信用代码 [已隐藏]"
        );
    }

    #[test]
    fn test_plate_sanitize() {
        assert_eq!(sanitize_text("车牌 京A12345 已记录"), "车牌 [已隐藏] 已记录");
    }

    #[test]
    fn test_no_sensitive() {
        let input = "点击右上角的设置按钮";
        assert_eq!(sanitize_text(input), input);
    }
}
