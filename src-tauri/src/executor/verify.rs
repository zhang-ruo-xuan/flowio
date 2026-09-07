//! 校验门：执行后截图 vs 录制预期截图做像素差异比对（审计胚胎）。

use image::imageops::FilterType;
use image::DynamicImage;

/// 两张图缩放后取亮度均差，返回 0~1 的差异率（0=完全一致）。
pub fn diff_ratio(a: &DynamicImage, b: &DynamicImage) -> f32 {
    let w = 320u32;
    let h = 180u32;
    let a = a.resize_exact(w, h, FilterType::Nearest).to_luma8();
    let b = b.resize_exact(w, h, FilterType::Nearest).to_luma8();
    let mut sum = 0u64;
    for (p, q) in a.pixels().zip(b.pixels()) {
        let d = (p[0] as i32 - q[0] as i32).abs();
        sum += d as u64;
    }
    sum as f32 / (w * h * 255) as f32
}

/// 校验某步：抓当前全屏，与录制时的 after_screenshot 比对。
///
/// 返回 Some(差异率)；若预期截图缺失/文件打不开/抓屏失败则返回 None（跳过校验）。
pub fn verify_step(after_screenshot: &Option<String>) -> Option<f32> {
    let path = after_screenshot.as_ref()?;
    if path.is_empty() {
        return None;
    }
    let expected = image::open(path).ok()?;
    let actual = crate::recorder::screenshot::capture_fullscreen().ok()?;
    Some(diff_ratio(&expected, &actual))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb};

    fn solid(v: u8) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(
            320, 180, Rgb([v, v, v]),
        ))
    }

    #[test]
    fn test_diff_identical_is_zero() {
        let img = solid(100);
        assert!((diff_ratio(&img, &img)) < 1e-6);
    }

    #[test]
    fn test_diff_black_vs_white_near_one() {
        let a = solid(0);
        let b = solid(255);
        let d = diff_ratio(&a, &b);
        assert!(d > 0.9 && d <= 1.0, "diff={}", d);
    }
}
