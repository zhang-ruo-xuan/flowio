use std::time::Instant;
use xcap::Monitor;

fn main() {
    println!("=== monitor enum ===");
    let monitors = match Monitor::all() {
        Ok(m) => m,
        Err(e) => {
            println!("Monitor::all() FAILED: {}", e);
            return;
        }
    };
    println!("monitor count: {}", monitors.len());
    for m in &monitors {
        println!(
            "  x={} y={} w={} h={} name={:?}",
            m.x(),
            m.y(),
            m.width(),
            m.height(),
            m.name()
        );
    }
    let min_x = monitors.iter().map(|m| m.x()).min().unwrap_or(0);
    let min_y = monitors.iter().map(|m| m.y()).min().unwrap_or(0);
    let max_x = monitors.iter().map(|m| m.x() + m.width() as i32).max().unwrap_or(0);
    let max_y = monitors.iter().map(|m| m.y() + m.height() as i32).max().unwrap_or(0);
    let vw = (max_x - min_x) as u32;
    let vh = (max_y - min_y) as u32;
    println!("virtual desktop: min=({}, {}) max=({}, {}) size={}x{}", min_x, min_y, max_x, max_y, vw, vh);

    // per-monitor capture timing
    for m in &monitors {
        let t = Instant::now();
        match m.capture_image() {
            Ok(img) => println!(
                "  capture monitor ({},{}) -> {}x{} in {:?}",
                m.x(),
                m.y(),
                img.width(),
                img.height(),
                t.elapsed()
            ),
            Err(e) => println!("  capture monitor ({},{}) FAILED: {}", m.x(), m.y(), e),
        }
    }

    // full virtual desktop canvas (same as capture_virtual_desktop)
    let t = Instant::now();
    let mut canvas = image::DynamicImage::new_rgba8(vw, vh);
    for m in &monitors {
        let img = match m.capture_image() {
            Ok(i) => i,
            Err(e) => {
                println!("  canvas: monitor ({},{}) capture FAILED: {}", m.x(), m.y(), e);
                continue;
            }
        };
        let ox = (m.x() - min_x) as i64;
        let oy = (m.y() - min_y) as i64;
        image::imageops::overlay(&mut canvas, &image::DynamicImage::ImageRgba8(img), ox, oy);
    }
    println!("virtual desktop canvas done in {:?}", t.elapsed());

    // JPEG encode timing (like save)
    let t = Instant::now();
    let mut buf = std::io::Cursor::new(Vec::new());
    let out = image::DynamicImage::ImageRgba8(canvas.into_rgba8())
        .write_to(&mut buf, image::ImageFormat::Jpeg);
    match out {
        Ok(()) => println!("jpeg encode done in {:?}, bytes={}", t.elapsed(), buf.get_ref().len()),
        Err(e) => println!("jpeg encode FAILED: {}", e),
    }
    println!("DONE");
}
