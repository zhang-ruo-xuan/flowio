//! 页脚二维码（对齐插件 `src/lib/qr.ts` + `export.ts::footerQrHtml`）。
//!
//! 插件用 `qrcode-generator`（JS）在「有公开地址且未关二维码」时，往 HTML 页脚
//! 左侧插一枚矢量 SVG（扫码看线上版）。桌面端没有发布链路，改造目标是把**能力**
//! 补齐：导出时传入公开地址（`publicUrl`），页脚就出同样的二维码。
//!
//! 对齐要点（插件 `qr.ts` 顶部注释里的设计约束）：
//!   ① 纠错等级 M（可恢复约 15% 的遮挡/磨损）；
//!   ② 静区固定 4 模块 —— 少于 4 时扫码器常常定位不到；
//!   ③ 非 ASCII 一律 encodeURI —— 保证进编码器的字节流是 ASCII，中文 URL 扫码后仍是同一地址；
//!   ④ 用矢量 SVG 而非位图 —— 打印分辨率下不糊。

use qrcode::Color;
use qrcode::EcLevel;
use qrcode::QrCode;

/// 纠错等级：M 约可恢复 15% 的遮挡/磨损（对齐插件 `QR_EC_LEVEL = "M"`）。
const QR_EC_LEVEL: EcLevel = EcLevel::M;

/// 静区（quiet zone）模块数。标准要求 ≥4，少了扫码器定位不到（对齐插件 `QR_QUIET_ZONE = 4`）。
const QR_QUIET_ZONE: u32 = 4;

/// 规范化为二维码承载的字符串（对齐插件 `qrPayload` 的 `encodeURI` 语义）。
///
/// 保留 URL 保留字符，非 ASCII（中文等）转成 %XX —— 库按字节编码时中文会生成
/// 「能看不能扫」的图，percent-encode 后进编码器的字节流一定是 ASCII，扫码后仍是同一地址。
pub fn encode_uri(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            // encodeURI 不编码的 ASCII 字符：字母数字 + 保留/标记字符
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b';'
            | b','
            | b'/'
            | b'?'
            | b':'
            | b'@'
            | b'&'
            | b'='
            | b'+'
            | b'$'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')'
            | b'#' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 生成二维码 SVG（矢量，打印不糊；对齐插件 `qrSvg(url, cellSize)`）。
///
/// 返回的 SVG 自带 4 模块静区（viewBox 已含静区）。内容为空 / 生成失败时返回 Err，
/// 调用方只丢二维码、不能带崩整份导出（对齐插件 `footerQrHtml` 的 try/catch 语义）。
pub fn qr_svg(url: &str, cell_size: u32) -> Result<String, String> {
    if url.trim().is_empty() {
        return Err("二维码内容为空：没有可编码的链接".to_string());
    }
    let payload = encode_uri(url);
    if payload.trim().is_empty() {
        return Err("二维码内容为空：没有可编码的链接".to_string());
    }
    let code = QrCode::with_error_correction_level(payload.as_bytes(), QR_EC_LEVEL)
        .map_err(|e| format!("二维码生成失败（内容 {} 字节）：{}", payload.len(), e))?;

    let modules = code.width() as u32;
    let quiet = QR_QUIET_ZONE;
    let cell = cell_size.max(1);
    let side = (modules + quiet * 2) * cell;

    // path：每个黑模块画一个矩形（M x y h w v h h -w z），静区偏移 (quiet*cell)。
    let mut d = String::new();
    for r in 0..modules {
        for c in 0..modules {
            if code[(r as usize, c as usize)] == Color::Dark {
                let x = (c + quiet) * cell;
                let y = (r + quiet) * cell;
                d.push_str(&format!("M{} {}h{}v{}h-{}z", x, y, cell, cell, cell));
            }
        }
    }

    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\" \
         shape-rendering=\"crispEdges\"><path fill=\"#000\" d=\"{d}\"/></svg>",
        side = side,
        d = d,
    ))
}

/// 生成二维码 PNG 位图（DOCX 页脚用；Word 不认 SVG，必须落位图）。
/// 与 `qr_svg` 同一套 encode_uri / 纠错等级 M / 4 模块静区语义，仅输出形态不同。
pub fn qr_png_bytes(url: &str, scale: u32) -> Result<Vec<u8>, String> {
    if url.trim().is_empty() {
        return Err("二维码内容为空：没有可编码的链接".to_string());
    }
    let payload = encode_uri(url);
    if payload.trim().is_empty() {
        return Err("二维码内容为空：没有可编码的链接".to_string());
    }
    let code = QrCode::with_error_correction_level(payload.as_bytes(), QR_EC_LEVEL)
        .map_err(|e| format!("二维码生成失败（内容 {} 字节）：{}", payload.len(), e))?;

    let modules = code.width() as u32;
    let quiet = QR_QUIET_ZONE;
    let cell = scale.max(1);
    let side = (modules + quiet * 2) * cell;

    // 白底黑码（Luma8：255=白，0=黑）
    let mut buf = vec![255u8; (side * side) as usize];
    for r in 0..modules {
        for c in 0..modules {
            if code[(r as usize, c as usize)] == Color::Dark {
                let x0 = (c + quiet) * cell;
                let y0 = (r + quiet) * cell;
                for dy in 0..cell {
                    let row = ((y0 + dy) * side + x0) as usize;
                    for dx in 0..cell {
                        buf[row + dx as usize] = 0;
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    {
        use image::ImageEncoder;
        let mut enc = image::codecs::png::PngEncoder::new(&mut out);
        enc.write_image(&buf, side, side, image::ExtendedColorType::L8)
            .map_err(|e| format!("二维码 PNG 编码失败：{}", e))?;
    }
    Ok(out)
}

#[cfg(test)]
mod qr_tests {
    use super::*;

    #[test]
    fn encode_uri_keeps_reserved_ascii() {
        assert_eq!(encode_uri("https://flowio.cn/g/abc-123?x=1&y=2"), "https://flowio.cn/g/abc-123?x=1&y=2");
    }

    #[test]
    fn encode_uri_percent_encodes_non_ascii() {
        assert_eq!(encode_uri("https://flowio.cn/g/教程"), "https://flowio.cn/g/%E6%95%99%E7%A8%8B");
    }

    #[test]
    fn qr_svg_empty_url_errors() {
        assert!(qr_svg("", 3).is_err());
        assert!(qr_svg("   ", 3).is_err());
    }

    #[test]
    fn qr_svg_emits_svg_with_path() {
        let svg = qr_svg("https://flowio.cn/g/demo", 3).expect("qr svg should generate");
        assert!(svg.starts_with("<svg"), "got: {}", &svg[..svg.len().min(30)]);
        assert!(svg.contains("<path"), "svg should contain path");
        assert!(svg.contains("viewBox=\"0 0"), "svg should carry viewBox");
    }

    #[test]
    fn qr_svg_same_input_same_output() {
        let a = qr_svg("https://flowio.cn/g/demo", 3).unwrap();
        let b = qr_svg("https://flowio.cn/g/demo", 3).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn qr_svg_cell_size_scales_side() {
        let small = qr_svg("https://flowio.cn/g/demo", 1).unwrap();
        let big = qr_svg("https://flowio.cn/g/demo", 4).unwrap();
        assert_ne!(small, big);
        // 大 cell 的 viewBox 边长必须更大
        let side_of = |svg: &str| {
            let head = svg.split("viewBox=\"0 0 ").nth(1).unwrap();
            head.split(' ').next().unwrap().parse::<u32>().unwrap()
        };
        assert!(side_of(&big) > side_of(&small));
    }
}
