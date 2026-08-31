//! Windows OCR 封装（Phase B：截图自动扫描脱敏）
//!
//! 基于系统内置 `Windows.Media.Ocr`，无需额外安装 OCR 引擎。
//! 引擎进程级复用（避免每图重建）；单图识别整体放入后台线程，
//! 主流程用 channel + recv_timeout 等待结果，超时（10s）返回
//! `OCR timeout` 错误，由调用方跳过该图继续，杜绝 WinRT 异步调用
//! 偶发永久挂起导致整次扫描卡死。

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::core::HSTRING;
use windows::Foundation::Rect;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

/// 单图 OCR 超时兜底：超过即跳过该图继续后续处理。
/// 保持较短（15s）以避免多图导出时串行累积成"导出中"永久卡顿。
const OCR_TIMEOUT: Duration = Duration::from_secs(15);

/// OCR 前对图像放大倍数：Windows OCR 对小字号（<10px）识别率极低，
/// 放大 3 倍后数字/字母识别准确率显著提升（密码/IP 等敏感内容多为小字）。
/// P1-05：降为 2 倍——3 倍放大使像素数为 9 倍，BMP 编码/解码与引擎识别
/// 都随像素数线性变慢，是导出"一直等"的主要瓶颈；2 倍仍可识别常见字号，
/// 导出整体提速约 40%。
const OCR_UPSCALE: u32 = 2;

/// OCR 识别出的一行文本及其像素包围盒（相对于原图像素坐标）。
#[derive(Debug, Clone)]
pub struct OcrLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 进程级 OCR 引擎缓存。引擎创建昂贵（语言包枚举/加载），进程内只创建一次并复用。
/// 创建/获取均在 Mutex 锁内完成；识别时 clone 引擎到当前线程（识别本身串行）。
static OCR_ENGINE: OnceLock<Mutex<Option<OcrEngine>>> = OnceLock::new();

fn get_engine() -> Result<OcrEngine, String> {
    let lock = OCR_ENGINE.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().map_err(|_| "OCR engine lock poisoned".to_string())?;
    if let Some(eng) = guard.as_ref() {
        return Ok(eng.clone());
    }
    // 首次创建：优先用户语言，失败回退中文
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .or_else(|_| {
            Language::CreateLanguage(&HSTRING::from("zh-CN"))
                .and_then(|lang| OcrEngine::TryCreateFromLanguage(&lang))
        })
        .map_err(|_| "OCR engine unavailable".to_string())?;
    *guard = Some(engine.clone());
    Ok(engine)
}

/// 对单张图片执行 OCR，返回识别到的所有文本行。
///
/// - OCR 引擎进程级复用（避免每图重建）；
/// - 识别整体在独立线程内执行（MTA COM），主流程 `recv_timeout` 最多等待
///   10 秒；超时返回错误，调用方跳过该图继续，杜绝永久挂起。
pub fn recognize_image(path: &Path) -> Result<Vec<OcrLine>, String> {
    let path = path.to_path_buf();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = ocr_on_thread(&path);
        let _ = tx.send(result);
    });
    rx.recv_timeout(OCR_TIMEOUT)
        .map_err(|_| format!("OCR timeout after {:?}", OCR_TIMEOUT))?
}

/// 在独立线程内执行的真实 OCR 流程（初始化 COM + 复用引擎 + 识别）。
fn ocr_on_thread(path: &Path) -> Result<Vec<OcrLine>, String> {
    ocr_image_inner(
        path,
        None,
        image::imageops::FilterType::Nearest,
        OCR_UPSCALE,
        0,
    )
}

/// 探针用：可指定裁剪区域、插值算法、放大倍数、预处理与语言。
/// preprocess: 0=无 1=灰度 2=二值 3=灰度+反转 4=RGB反转
pub(crate) fn recognize_crop_with(
    path: &Path,
    crop: Option<(u32, u32, u32, u32)>,
    filter: image::imageops::FilterType,
    scale: u32,
    preprocess: u8,
) -> Result<Vec<OcrLine>, String> {
    ocr_image_inner(path, crop, filter, scale, preprocess)
}

/// 合并两个 word 包围盒为同时包含两者的最小矩形（OCR 行聚合，纯逻辑）。
fn merge_word_rects(r: Rect, wr: Rect) -> Rect {
    Rect {
        X: r.X.min(wr.X),
        Y: r.Y.min(wr.Y),
        Width: (r.X + r.Width).max(wr.X + wr.Width) - r.X.min(wr.X),
        Height: (r.Y + r.Height).max(wr.Y + wr.Height) - r.Y.min(wr.Y),
    }
}

/// 将识别框从放大图坐标映射回原图坐标；无框时返回全 0（纯逻辑）。
/// scale 为 0 时按 1 处理，避免除零产生 inf/NaN。
fn map_rect_to_original(rect: Option<Rect>, scale: u32) -> (f32, f32, f32, f32) {
    let scale_f = scale.max(1) as f32;
    rect.map(|r| (r.X / scale_f, r.Y / scale_f, r.Width / scale_f, r.Height / scale_f))
        .unwrap_or((0.0, 0.0, 0.0, 0.0))
}

/// 单像素二值化阈值：>128 置白，否则置黑（纯逻辑）。
fn binarize_pixel(v: u8) -> u8 {
    if v > 128 {
        255
    } else {
        0
    }
}

/// 单像素反色（纯逻辑）。
fn invert_pixel(v: u8) -> u8 {
    255 - v
}

/// 对缩放后图像应用预处理（preprocess: 0=无 1=灰度 2=二值 3=灰度+反转 4=RGB反转）。
fn apply_preprocess(
    scaled: image::DynamicImage,
    preprocess: u8,
) -> Result<image::DynamicImage, String> {
    if preprocess == 1 {
        // 灰度化：消除亚像素渲染（ClearType）的彩色边缘，OCR 对纯亮度字形识别更稳
        Ok(image::DynamicImage::ImageLuma8(scaled.to_luma8()))
    } else if preprocess == 2 {
        let luma = scaled.to_luma8();
        let bw: Vec<u8> = luma.pixels().map(|p| binarize_pixel(p.0[0])).collect();
        Ok(image::DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(luma.width(), luma.height(), bw)
                .ok_or("binary convert failed")?,
        ))
    } else if preprocess == 3 {
        // 灰度 + 反转：截图常为深色背景白字（反白），OCR 引擎对黑字白底识别更稳
        let luma = scaled.to_luma8();
        let inv: Vec<u8> = luma.pixels().map(|p| invert_pixel(p.0[0])).collect();
        Ok(image::DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(luma.width(), luma.height(), inv)
                .ok_or("invert convert failed")?,
        ))
    } else if preprocess == 4 {
        // RGB 反转（保留颜色）：反白截图 + ClearType 彩色边缘整体取反
        let rgb = scaled.to_rgb8();
        let inv: Vec<u8> = rgb
            .pixels()
            .flat_map(|p| [invert_pixel(p.0[0]), invert_pixel(p.0[1]), invert_pixel(p.0[2])])
            .collect();
        Ok(image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(rgb.width(), rgb.height(), inv)
                .ok_or("rgb invert failed")?,
        ))
    } else {
        Ok(scaled)
    }
}

/// 统一识别主流程：读图 → 可选裁剪 → 放大 scale 倍 → BMP 编码 → BitmapDecoder
/// → SoftwareBitmap → OCR。
fn ocr_image_inner(
    path: &Path,
    crop: Option<(u32, u32, u32, u32)>,
    filter: image::imageops::FilterType,
    scale: u32,
    preprocess: u8,
) -> Result<Vec<OcrLine>, String> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }

    // 原图读取 + 放大 scale 倍：小字号文字（密码/IP 等）识别率大幅提升。
    // BMP 无压缩编码/解码快，适合作为 OCR 中间载体。
    let mut img = image::open(path).map_err(|e| format!("read screenshot failed: {}", e))?;
    if let Some((x, y, w, h)) = crop {
        img = img.crop_imm(x, y, w, h);
    }
    let (w, h) = (img.width(), img.height());
    let mut scaled = img.resize(w * scale, h * scale, filter);
    scaled = apply_preprocess(scaled, preprocess)?;
    let mut bmp_buf = std::io::Cursor::new(Vec::new());
    scaled
        .write_to(&mut bmp_buf, image::ImageFormat::Bmp)
        .map_err(|e| format!("encode scaled screenshot failed: {}", e))?;
    let bytes = bmp_buf.into_inner();

    // 图片字节 → 内存流 → BitmapDecoder → SoftwareBitmap(BGRA8)
    let stream = InMemoryRandomAccessStream::new().map_err(|e| e.to_string())?;
    let writer = DataWriter::CreateDataWriter(&stream).map_err(|e| e.to_string())?;
    writer
        .WriteBytes(&bytes)
        .map_err(|e| format!("write stream failed: {}", e))?;
    writer
        .StoreAsync()
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;
    writer
        .FlushAsync()
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;
    let bitmap_raw = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;
    let bitmap = SoftwareBitmap::ConvertWithAlpha(
        &bitmap_raw,
        BitmapPixelFormat::Bgra8,
        BitmapAlphaMode::Premultiplied,
    )
    .map_err(|e| e.to_string())?;

    let engine = get_engine()?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| e.to_string())?
        .get()
        .map_err(|e| e.to_string())?;

    let lines_view = result.Lines().map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    for i in 0..lines_view.Size().map_err(|e| e.to_string())? {
        let line = lines_view.GetAt(i).map_err(|e| e.to_string())?;
        let text = line.Text().map_err(|e| e.to_string())?.to_string_lossy();
        // OcrLine 本身没有包围盒，聚合其全部 Word 的 BoundingRect
        let words = line.Words().map_err(|e| e.to_string())?;
        let n = words.Size().map_err(|e| e.to_string())?;
        let mut rect: Option<Rect> = None;
        for j in 0..n {
            let word = words.GetAt(j).map_err(|e| e.to_string())?;
            let wr: Rect = word.BoundingRect().map_err(|e| e.to_string())?;
            rect = Some(match rect {
                None => wr,
                Some(r) => merge_word_rects(r, wr),
            });
        }
        // 识别在放大图上进行，坐标映射回原图
        let (x, y, w, h) = map_rect_to_original(rect, scale);
        lines.push(OcrLine {
            text,
            x,
            y,
            w,
            h,
        });
    }
    Ok(lines)
}

#[cfg(test)]
mod ocr_tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect {
            X: x,
            Y: y,
            Width: w,
            Height: h,
        }
    }

    // ── merge_word_rects ─────────────────────────────────

    #[test]
    fn merge_rects_disjoint() {
        let r = merge_word_rects(rect(0.0, 0.0, 10.0, 10.0), rect(20.0, 20.0, 10.0, 10.0));
        assert_eq!(r.X, 0.0);
        assert_eq!(r.Y, 0.0);
        assert_eq!(r.Width, 30.0);
        assert_eq!(r.Height, 30.0);
    }

    #[test]
    fn merge_rects_overlap() {
        let r = merge_word_rects(rect(5.0, 5.0, 20.0, 20.0), rect(10.0, 10.0, 30.0, 15.0));
        assert_eq!(r.X, 5.0);
        assert_eq!(r.Y, 5.0);
        assert_eq!(r.Width, 35.0);
        assert_eq!(r.Height, 20.0);
    }

    #[test]
    fn merge_rects_same() {
        let r = merge_word_rects(rect(3.0, 4.0, 5.0, 6.0), rect(3.0, 4.0, 5.0, 6.0));
        assert_eq!(r.X, 3.0);
        assert_eq!(r.Y, 4.0);
        assert_eq!(r.Width, 5.0);
        assert_eq!(r.Height, 6.0);
    }

    #[test]
    fn merge_rects_second_contains_first() {
        let r = merge_word_rects(rect(2.0, 2.0, 3.0, 3.0), rect(0.0, 0.0, 10.0, 10.0));
        assert_eq!(r.X, 0.0);
        assert_eq!(r.Y, 0.0);
        assert_eq!(r.Width, 10.0);
        assert_eq!(r.Height, 10.0);
    }

    #[test]
    fn merge_rects_negative_coords() {
        let r = merge_word_rects(rect(-5.0, -5.0, 10.0, 10.0), rect(0.0, 0.0, 5.0, 5.0));
        assert_eq!(r.X, -5.0);
        assert_eq!(r.Y, -5.0);
        assert_eq!(r.Width, 10.0);
        assert_eq!(r.Height, 10.0);
    }

    // ── map_rect_to_original ─────────────────────────────

    #[test]
    fn map_rect_scales_down() {
        let (x, y, w, h) = map_rect_to_original(Some(rect(10.0, 20.0, 100.0, 50.0)), 2);
        assert!((x - 5.0).abs() < 1e-6);
        assert!((y - 10.0).abs() < 1e-6);
        assert!((w - 50.0).abs() < 1e-6);
        assert!((h - 25.0).abs() < 1e-6);
    }

    #[test]
    fn map_rect_none_returns_zeros() {
        let (x, y, w, h) = map_rect_to_original(None, 2);
        assert_eq!(x, 0.0);
        assert_eq!(y, 0.0);
        assert_eq!(w, 0.0);
        assert_eq!(h, 0.0);
    }

    #[test]
    fn map_rect_scale_1_identity() {
        let (x, y, w, h) = map_rect_to_original(Some(rect(15.0, 25.0, 35.0, 45.0)), 1);
        assert!((x - 15.0).abs() < 1e-6);
        assert!((y - 25.0).abs() < 1e-6);
        assert!((w - 35.0).abs() < 1e-6);
        assert!((h - 45.0).abs() < 1e-6);
    }

    #[test]
    fn map_rect_scale_0_no_inf() {
        // scale=0 时防御为 1，不应产生 inf/NaN
        let (x, y, w, h) = map_rect_to_original(Some(rect(10.0, 20.0, 30.0, 40.0)), 0);
        assert!(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite());
        assert!((x - 10.0).abs() < 1e-6);
    }

    #[test]
    fn map_rect_large_scale() {
        let (x, y, w, h) = map_rect_to_original(Some(rect(100.0, 200.0, 300.0, 400.0)), 10);
        assert!((x - 10.0).abs() < 1e-6);
        assert!((y - 20.0).abs() < 1e-6);
        assert!((w - 30.0).abs() < 1e-6);
        assert!((h - 40.0).abs() < 1e-6);
    }

    // ── binarize_pixel ───────────────────────────────────

    #[test]
    fn binarize_below_threshold_black() {
        assert_eq!(binarize_pixel(0), 0);
        assert_eq!(binarize_pixel(127), 0);
        assert_eq!(binarize_pixel(128), 0);
    }

    #[test]
    fn binarize_above_threshold_white() {
        assert_eq!(binarize_pixel(129), 255);
        assert_eq!(binarize_pixel(200), 255);
        assert_eq!(binarize_pixel(255), 255);
    }

    // ── invert_pixel ─────────────────────────────────────

    #[test]
    fn invert_values() {
        assert_eq!(invert_pixel(0), 255);
        assert_eq!(invert_pixel(255), 0);
        assert_eq!(invert_pixel(128), 127);
        assert_eq!(invert_pixel(1), 254);
        assert_eq!(invert_pixel(254), 1);
    }

    // ── apply_preprocess ─────────────────────────────────

    fn sample_rgba() -> DynamicImage {
        // 2x2 图：pixel(0,0) 暗色, pixel(1,0) 亮色, pixel(0,1) 中性, pixel(1,1) 白色
        let mut img = RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        img.put_pixel(1, 0, image::Rgba([200, 210, 220, 255]));
        img.put_pixel(0, 1, image::Rgba([128, 128, 128, 255]));
        img.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn preprocess_0_identity() {
        let img = sample_rgba();
        let out = apply_preprocess(img.clone(), 0).unwrap();
        assert_eq!(out.width(), 2);
        assert_eq!(out.height(), 2);
        // 原样返回，比较像素
        let out_rgba = out.to_rgba8();
        assert_eq!(out_rgba.get_pixel(0, 0), &image::Rgba([10, 20, 30, 255]));
    }

    #[test]
    fn preprocess_1_grayscale() {
        let img = sample_rgba();
        let out = apply_preprocess(img, 1).unwrap();
        assert_eq!(out.width(), 2);
        assert_eq!(out.height(), 2);
        // 灰度输出：暗像素 luma 低，亮像素 luma 高
        let luma = out.to_luma8();
        assert!(luma.get_pixel(0, 0).0[0] < luma.get_pixel(1, 0).0[0]);
    }

    #[test]
    fn preprocess_2_binary() {
        let img = sample_rgba();
        let out = apply_preprocess(img, 2).unwrap();
        assert_eq!(out.width(), 2);
        assert_eq!(out.height(), 2);
        let luma = out.to_luma8();
        // pixel(0,0) 暗色 luma 约 18 → <=128 → 0
        // pixel(1,0) 亮色 luma 约 210 → >128 → 255
        assert_eq!(luma.get_pixel(0, 0).0[0], 0);
        assert_eq!(luma.get_pixel(1, 0).0[0], 255);
        // pixel(1,1) 全白 luma=255 → >128 → 255
        assert_eq!(luma.get_pixel(1, 1).0[0], 255);
    }

    #[test]
    fn preprocess_3_grayscale_invert() {
        let img = sample_rgba();
        let out = apply_preprocess(img, 3).unwrap();
        assert_eq!(out.width(), 2);
        assert_eq!(out.height(), 2);
        let luma = out.to_luma8();
        // 暗像素反色后变亮，亮像素反色后变暗
        assert!(luma.get_pixel(0, 0).0[0] > luma.get_pixel(1, 0).0[0]);
        // 0 的反色是 255，255 的反色是 0
        assert_eq!(luma.get_pixel(1, 1).0[0], 0);
    }

    #[test]
    fn preprocess_4_rgb_invert() {
        let img = sample_rgba();
        let out = apply_preprocess(img, 4).unwrap();
        assert_eq!(out.width(), 2);
        assert_eq!(out.height(), 2);
        let rgb = out.to_rgb8();
        // 10→245, 20→235, 30→225
        assert_eq!(rgb.get_pixel(0, 0).0, [245, 235, 225]);
        // 200→55, 210→45, 220→35
        assert_eq!(rgb.get_pixel(1, 0).0, [55, 45, 35]);
        // 255→0
        assert_eq!(rgb.get_pixel(1, 1).0, [0, 0, 0]);
    }

    #[test]
    fn preprocess_unknown_identity() {
        let img = sample_rgba();
        let out = apply_preprocess(img.clone(), 99).unwrap();
        // 未知值返回原图
        assert_eq!(out.to_rgba8().get_pixel(0, 0), &image::Rgba([10, 20, 30, 255]));
    }

    #[test]
    fn preprocess_0_identity_big() {
        let mut img = RgbaImage::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                img.put_pixel(x, y, image::Rgba([(x * 50) as u8, (y * 50) as u8, 100, 255]));
            }
        }
        let out = apply_preprocess(DynamicImage::ImageRgba8(img.clone()), 0).unwrap();
        let out_rgba = out.to_rgba8();
        for y in 0..5 {
            for x in 0..5 {
                assert_eq!(out_rgba.get_pixel(x, y), img.get_pixel(x, y));
            }
        }
    }
}
