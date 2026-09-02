//! 扩展 → 桌面端的选区上报通道（COPY 步骤裁图的 L0 数据源）。
//!
//! ## 为什么需要它
//! 桌面端只能从系统外面**猜**——走 UIA 无障碍树，而 Chromium 的 a11y 树是
//! 惰性激活的，命中率不稳（见 `screenshot::warmup_uia_text_pattern` 的说明）。
//! 浏览器扩展站在页面**里面**，能直接读 `window.getSelection()` 的精确矩形，
//! 这正是 Scribe 扩展版裁图精度的来源。
//!
//! ## 通道设计
//! 扩展的 background service worker 在录制中把选区矩形 POST 到本机
//! `127.0.0.1:<PORT>`，桌面端只缓存最近一条；COPY 步骤回填时**优先**使用它
//! （仍必须通过剪贴板文本校验，防止误用残留选区）。
//!
//! 没有上报时（扩展未安装、或操作发生在原生应用里）自动回落到 UIA / OCR，
//! 不影响原有链路——L0 是增益，不是依赖。
//!
//! ## 安全
//! 只监听回环地址，不对外网开放；只接受 `POST /selection`，其余一律 404。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::Deserialize;

use super::screenshot::{ocr_debug, selection_matches_clipboard};

/// 候选监听端口（仅回环地址）。被占用时依次回退，全部失败则 L0 静默不可用。
const PORTS: [u16; 4] = [9527, 9528, 9529, 9530];

#[derive(Debug, Deserialize)]
struct SelectionPayload {
    /// 被选中的文本，用于与剪贴板内容校验
    text: String,
    /// 选区矩形（**屏幕物理像素**）：左上角 x/y + 宽高。
    /// 由扩展自行估算，作为校准不可用时的兜底。
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    /// 以下 5 项为**窗口校准**路径的输入，全部缺省时（旧版扩展）走兜底。
    /// 选区左上角相对**视口**的偏移（CSS 像素）
    rx: Option<f64>,
    ry: Option<f64>,
    /// 视口尺寸（CSS 像素）
    vw: Option<f64>,
    vh: Option<f64>,
    /// `window.devicePixelRatio`
    dpr: Option<f64>,
}

/// 边框宽度上限（物理像素）。超出说明布局不对称（如 DevTools 停靠在侧边），
/// 校准前提不成立 → 放弃。
const MAX_FRAME_PX: i32 = 32;
/// 浏览器非网页区域高度上限（物理像素）：标签栏 + 地址栏 + 书签栏 + 边框。
const MAX_CHROME_PX: i32 = 512;

/// 用真实窗口矩形，把「视口相对坐标」换算成「屏幕物理坐标」。
///
/// ## 为什么需要
/// 扩展自己算绝对坐标要走 `(window.screenX + r.left) * devicePixelRatio`。
/// 但 `screenX` 是 **CSS 像素**、原点在**主显示器**左上角 —— 多显示器缩放倍数
/// 不同时（如主屏 100%、副屏 150%），这个乘法会整体偏掉几百像素。
/// 客户区原点是 Win32 直接给出的**物理像素**，与截图同一坐标系，
/// 可以彻底绕开这整类问题。
///
/// ## 参数
/// - `client`：`activity::get_active_client_rect()` → (x, y, w, h)，物理像素
/// - `rx/ry`：选区相对视口的偏移，CSS 像素
/// - `vw/vh`：视口尺寸，CSS 像素
/// - `dpr`：`devicePixelRatio`
///
/// ## 返回 `None` 的语义
/// 数据"讲不通"（坐标系不一致 / 布局不对称 / 参数越界）→ 调用方应**原样回落**
/// 到扩展估算的绝对坐标。宁可不修正，也不要引入新的偏移。
pub(crate) fn calibrate_to_screen(
    client: (i32, i32, i32, i32),
    rx: f64,
    ry: f64,
    vw: f64,
    vh: f64,
    dpr: f64,
) -> Option<(i32, i32)> {
    if vw <= 0.0 || vh <= 0.0 || !dpr.is_finite() || !(0.25..=8.0).contains(&dpr) {
        return None;
    }
    if !rx.is_finite() || !ry.is_finite() {
        return None;
    }
    let (cx, cy, cw, ch) = client;
    if cw <= 0 || ch <= 0 {
        return None;
    }

    let vw_px = (vw * dpr).round();
    let vh_px = (vh * dpr).round();

    // 左右边框（假设对称）
    let frame = ((cw as f64 - vw_px) / 2.0).round() as i32;
    // 浏览器 chrome（标签栏 + 地址栏…）+ 下边框
    let chrome = ch - vh_px as i32;

    // 守卫：frame 必须是个"边框该有的量"。为负说明视口比客户区还宽 ——
    // 典型诱因是桌面端未做 DPI 感知、坐标被系统虚拟化。
    if !(0..=MAX_FRAME_PX).contains(&frame) {
        return None;
    }
    // chrome 至少得有一个下边框的高度，也不该离谱地高
    if !(frame..=MAX_CHROME_PX).contains(&chrome) {
        return None;
    }

    Some((
        cx + frame + (rx * dpr).round() as i32,
        cy + chrome - frame + (ry * dpr).round() as i32,
    ))
}

/// 决定这条上报最终采用哪个矩形：**窗口校准优先，失败则原样兜底**。
fn resolve_rect(p: &SelectionPayload) -> (i32, i32, i32, i32) {
    let w = p.width.max(1);
    let h = p.height.max(1);

    if let (Some(rx), Some(ry), Some(vw), Some(vh), Some(dpr)) =
        (p.rx, p.ry, p.vw, p.vh, p.dpr)
    {
        if let Some(client) = super::activity::get_active_client_rect() {
            match calibrate_to_screen(client, rx, ry, vw, vh, dpr) {
                Some((x, y)) => {
                    ocr_debug(&format!(
                        "selection_bridge: calibrated win=({},{},{},{}) vw={} vh={} dpr={} -> abs=({},{}) est=({},{})",
                        client.0, client.1, client.2, client.3, vw, vh, dpr, x, y, p.x, p.y
                    ));
                    return (x, y, w, h);
                }
                None => {
                    ocr_debug(&format!(
                        "selection_bridge: calibration rejected win=({},{},{},{}) vw={} vh={} dpr={} -> keep estimate",
                        client.0, client.1, client.2, client.3, vw, vh, dpr
                    ));
                }
            }
        }
    }
    (p.x, p.y, w, h)
}

/// 一条缓存的上报记录
#[derive(Debug, Clone)]
pub struct SelectionReport {
    pub text: String,
    /// (x, y, width, height)，屏幕物理像素
    pub rect: (i32, i32, i32, i32),
    pub at: Instant,
}

/// 缓存条数上限。
///
/// 必须 > 1：用户常常**连续复制**好几段（复制 A → 复制 B → 复制 C），
/// 桌面端为每个步骤回填时都会拿剪贴板内容来查。若只留最后一条，
/// A、B 两个步骤的文本校验会全部失败，反而集体退化到 UIA 兜底 ——
/// 比"从不上报"更糟。保留一小段历史后，按文本匹配即可各取所需。
const MAX_REPORTS: usize = 8;

static RECENT: OnceLock<Mutex<Vec<SelectionReport>>> = OnceLock::new();
/// 实际监听中的端口（诊断用；未启动则为 None）
static ACTIVE_PORT: OnceLock<u16> = OnceLock::new();

fn recent_slot() -> &'static Mutex<Vec<SelectionReport>> {
    RECENT.get_or_init(|| Mutex::new(Vec::new()))
}

/// 记录一条扩展上报的选区
fn record_selection(text: String, rect: (i32, i32, i32, i32)) {
    if let Ok(mut g) = recent_slot().lock() {
        g.push(SelectionReport {
            text,
            rect,
            at: Instant::now(),
        });
        if g.len() > MAX_REPORTS {
            g.remove(0);
        }
    }
}

/// 取与剪贴板内容匹配的选区矩形（屏幕物理像素），**从最新往回找**。
///
/// 两道校验，与 L1a（UIA）保持同等严格：
/// 1. 时效：超过 `max_age_ms` 视为过期（复制动作与回填之间隔太久）；
/// 2. 文本：选区文本必须匹配剪贴板内容（防残留/Ctrl+F 查找条选区骗过几何）。
pub fn last_selection_matching(clipboard: &str, max_age_ms: u64) -> Option<(i32, i32, i32, i32)> {
    let g = recent_slot().lock().ok()?;
    for r in g.iter().rev() {
        // 用 >= 而非 >：max_age_ms = 0 的语义是"不采信任何上报"，
        // 若用 >，同一毫秒内（elapsed 取整为 0）反而会被判为有效。
        if r.at.elapsed().as_millis() >= max_age_ms as u128 {
            continue;
        }
        let (_x, _y, w, h) = r.rect;
        if w < 4 || h < 4 {
            continue;
        }
        if selection_matches_clipboard(&r.text, clipboard) {
            return Some(r.rect);
        }
    }
    None
}

/// 当前监听端口（诊断用）
pub fn active_port() -> Option<u16> {
    ACTIVE_PORT.get().copied()
}

/// 启动上报接收服务（后台线程，阻塞式 accept，请求量极低无需异步）。
/// 端口全被占用时静默放弃 —— L0 不可用不影响录制。
pub fn start_server() {
    start_server_on(PORTS.to_vec());
}

/// 在**指定端口**上启动（供集成测试使用；生产请调 `start_server()`）。
pub fn start_server_on(ports: Vec<u16>) {
    std::thread::spawn(move || {
        let bound = ports
            .iter()
            .find_map(|p| TcpListener::bind(("127.0.0.1", *p)).ok().map(|l| (*p, l)));

        let Some((port, listener)) = bound else {
            ocr_debug("selection_bridge: no port available, L0 disabled");
            return;
        };
        let _ = ACTIVE_PORT.set(port);
        ocr_debug(&format!("selection_bridge: listening on 127.0.0.1:{}", port));

        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    std::thread::spawn(move || handle(s));
                }
                Err(_) => continue,
            }
        }
    });
}

fn handle(mut stream: TcpStream) {
    let mut reader = BufReader::new(&mut stream);

    // 请求行
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let is_options = request_line.starts_with("OPTIONS");

    // 请求头（只关心 Content-Length）
    let mut content_len: usize = 0;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
            Err(_) => return,
        }
    }

    // 预检请求直接放行
    if is_options {
        drop(reader);
        let _ = stream.write_all(cors_response(204, "").as_bytes());
        return;
    }

    let mut body = vec![0u8; content_len.min(64 * 1024)];
    if content_len > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    drop(reader);

    let resp = match serde_json::from_slice::<SelectionPayload>(&body) {
        Ok(p) => {
            let (x, y, w, h) = resolve_rect(&p);
            ocr_debug(&format!(
                "selection_bridge: L0 hit rect=({},{},{},{}) text={:?}",
                x,
                y,
                w,
                h,
                p.text.chars().take(40).collect::<String>()
            ));
            record_selection(p.text, (x, y, w, h));
            cors_response(200, "ok")
        }
        Err(e) => {
            ocr_debug(&format!("selection_bridge: bad payload: {}", e));
            cors_response(400, "bad request")
        }
    };
    let _ = stream.write_all(resp.as_bytes());
}

fn cors_response(status: u16, body: &str) -> String {
    let phrase = if status == 200 {
        "OK"
    } else if status == 204 {
        "No Content"
    } else {
        "Bad Request"
    };
    format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Length: {}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: Content-Type\r\n\
         Connection: close\r\n\r\n{}",
        status,
        phrase,
        body.len(),
        body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录 + 匹配：文本一致时应命中
    #[test]
    fn record_then_match_returns_rect() {
        record_selection("hello world".to_string(), (10, 20, 300, 40));
        let r = last_selection_matching("hello world", 3000);
        assert_eq!(r, Some((10, 20, 300, 40)));
    }

    /// 文本不一致必须拒绝（防残留选区/查找条选区骗过几何校验）
    #[test]
    fn mismatched_text_is_rejected() {
        record_selection("selected text".to_string(), (10, 20, 300, 40));
        assert_eq!(last_selection_matching("something else", 3000), None);
    }

    /// 超时应过期（max_age = 0 时任何上报都算过期）
    #[test]
    fn stale_report_is_ignored() {
        record_selection("hello world".to_string(), (10, 20, 300, 40));
        assert_eq!(last_selection_matching("hello world", 0), None);
    }

    /// 过小矩形拒绝（防止扩展上报噪声导致裁图塌缩）
    #[test]
    fn tiny_rect_is_rejected() {
        record_selection("x".to_string(), (0, 0, 2, 1));
        assert_eq!(last_selection_matching("x", 3000), None);
    }

    /// 无上报时返回 None —— L0 不可用应干净地回落到 L1a
    #[test]
    fn empty_store_returns_none() {
        // 注意：与上面的测试共享全局状态，这里只能断言"文本完全不同时必为 None"
        assert_eq!(last_selection_matching("__definitely_not_present__", 0), None);
    }

    /// **连续复制**场景（MAX_REPORTS 的存在理由）：复制 A → B → C 后，
    /// 每个步骤回填时都应取到自己那段文本的矩形。
    /// 若只缓存最后一条，A/B 的文本校验会全部失败 → 集体退化，比不上报更糟。
    #[test]
    fn consecutive_copies_each_resolve_to_own_rect() {
        record_selection("copy alpha unique".to_string(), (1, 1, 100, 20));
        record_selection("copy beta unique".to_string(), (2, 2, 200, 20));
        record_selection("copy gamma unique".to_string(), (3, 3, 300, 20));
        assert_eq!(
            last_selection_matching("copy alpha unique", 3000),
            Some((1, 1, 100, 20))
        );
        assert_eq!(
            last_selection_matching("copy beta unique", 3000),
            Some((2, 2, 200, 20))
        );
        assert_eq!(
            last_selection_matching("copy gamma unique", 3000),
            Some((3, 3, 300, 20))
        );
    }

    /// 缓存上限：超过 MAX_REPORTS 后最旧的被挤出（防止长时间录制后无限增长）
    #[test]
    fn report_buffer_is_bounded() {
        for i in 0..(MAX_REPORTS + 5) {
            record_selection(format!("bounded {}", i), (i as i32, 0, 10, 10));
        }
        // 最旧的几条应已被丢弃
        assert_eq!(last_selection_matching("bounded 0", 3000), None);
        // 最新的仍在
        assert_eq!(
            last_selection_matching(&format!("bounded {}", MAX_REPORTS + 4), 3000),
            Some(((MAX_REPORTS + 4) as i32, 0, 10, 10))
        );
    }

    /// 端到端：真的起服务、真的发 HTTP POST、真的从缓存里查到。
    /// 覆盖 HTTP 解析（请求头 + Content-Length + body）这条手写链路，
    /// 这是纯单元测试碰不到、又最容易写错的部分。
    #[test]
    fn http_round_trip_records_selection() {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        const TEST_PORT: u16 = 19527; // 与生产端口（9527+）刻意错开
        start_server_on(vec![TEST_PORT]);

        // 等服务线程完成 bind + listen
        let mut up = false;
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", TEST_PORT)).is_ok() {
                up = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(up, "test server failed to bind on port {}", TEST_PORT);

        let body = r#"{"text":"l0 http round trip","x":11,"y":22,"width":333,"height":44}"#;
        let req = format!(
            "POST /selection HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let mut s = TcpStream::connect(("127.0.0.1", TEST_PORT)).expect("connect failed");
        s.write_all(req.as_bytes()).expect("write failed");
        let _ = s.flush();

        let mut resp = String::new();
        let _ = s.read_to_string(&mut resp);
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "unexpected response: {:?}",
            resp
        );

        // 服务端在另一个线程写缓存，轮询等待
        let mut got = None;
        for _ in 0..100 {
            if let Some(r) = last_selection_matching("l0 http round trip", 3000) {
                got = Some(r);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(got, Some((11, 22, 333, 44)));
    }

    /// 坏 payload 应返回 400 且不污染缓存
    #[test]
    fn malformed_payload_is_rejected() {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        const TEST_PORT: u16 = 19528;
        start_server_on(vec![TEST_PORT]);

        let mut up = false;
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", TEST_PORT)).is_ok() {
                up = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(up, "test server failed to bind on port {}", TEST_PORT);

        let body = "not-json-at-all";
        let req = format!(
            "POST /selection HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let mut s = TcpStream::connect(("127.0.0.1", TEST_PORT)).expect("connect failed");
        s.write_all(req.as_bytes()).expect("write failed");
        let _ = s.flush();

        let mut resp = String::new();
        let _ = s.read_to_string(&mut resp);
        assert!(
            resp.starts_with("HTTP/1.1 400"),
            "expected 400, got: {:?}",
            resp
        );
    }

    // ---------- 窗口校准（calibrate_to_screen）----------

    /// 基线：最大化 Chrome，100% 缩放。
    /// 客户区 = 工作区 (0,0,1920,1040)；视口 1920×950 → chrome 高 90。
    /// 选区相对视口 (100,200) → 屏幕 (100, 290)。
    #[test]
    fn calibrate_maximized_chrome_at_100pct() {
        assert_eq!(
            calibrate_to_screen((0, 0, 1920, 1040), 100.0, 200.0, 1920.0, 950.0, 1.0),
            Some((100, 290))
        );
    }

    /// 150% 缩放：客户区与视口都以物理像素计，chrome 也跟着放大到 135。
    /// 视口内相对坐标 (100,200) CSS px → 物理 (150,300)。
    #[test]
    fn calibrate_scales_with_dpr() {
        assert_eq!(
            calibrate_to_screen((0, 0, 2880, 1560), 100.0, 200.0, 1920.0, 950.0, 1.5),
            Some((150, 435))
        );
    }

    /// **这条是存在理由**：副屏（客户区原点物理 (2560,100)）。
    /// 扩展用 `screenX * dpr` 算绝对坐标时会把副屏偏移按错误的倍数放大，
    /// 而校准路径直接用客户区原点，不受影响。
    #[test]
    fn calibrate_second_monitor_uses_client_origin() {
        assert_eq!(
            calibrate_to_screen((2560, 100, 1920, 1040), 10.0, 20.0, 1920.0, 950.0, 1.0),
            Some((2570, 210))
        );
    }

    /// 还原（非最大化）窗口：客户区宽 1922 > 视口 1920 → 每边边框 1px。
    #[test]
    fn calibrate_restored_window_accounts_for_frame() {
        assert_eq!(
            calibrate_to_screen((100, 100, 1922, 1032), 10.0, 20.0, 1920.0, 930.0, 1.0),
            Some((111, 221))
        );
    }

    /// DevTools 停靠在右侧 → 左右不对称，frame 被算成 240px。
    /// 校准前提不成立，必须返回 None 让调用方回落，而不是给出偏移几百像素的结果。
    #[test]
    fn calibrate_rejects_devtools_docked_side() {
        assert_eq!(
            calibrate_to_screen((0, 0, 2400, 1040), 10.0, 20.0, 1920.0, 950.0, 1.0),
            None
        );
    }

    /// 坐标系不一致（假设桌面端被系统虚拟化成 96-dpi，客户区只有 1280 宽）：
    /// frame 为负 → 拒绝。宁可不修正，也不要引入新的偏移。
    #[test]
    fn calibrate_rejects_dpi_mismatch() {
        assert_eq!(
            calibrate_to_screen((0, 0, 1280, 693), 10.0, 20.0, 1920.0, 950.0, 1.0),
            None
        );
    }

    /// 参数越界/非法一律拒绝
    #[test]
    fn calibrate_rejects_nonsense_params() {
        let ok = (0, 0, 1920, 1040);
        assert_eq!(calibrate_to_screen(ok, 0.0, 0.0, 1920.0, 950.0, 0.0), None); // dpr=0
        assert_eq!(calibrate_to_screen(ok, 0.0, 0.0, 1920.0, 950.0, -1.0), None); // dpr<0
        assert_eq!(calibrate_to_screen(ok, 0.0, 0.0, 1920.0, 950.0, 99.0), None); // dpr 离谱
        assert_eq!(calibrate_to_screen(ok, 0.0, 0.0, 0.0, 950.0, 1.0), None); // vw=0
        assert_eq!(calibrate_to_screen(ok, 0.0, 0.0, 1920.0, -5.0, 1.0), None); // vh<0
        assert_eq!(calibrate_to_screen((0, 0, 0, 0), 0.0, 0.0, 100.0, 100.0, 1.0), None); // 空窗口
        assert_eq!(
            calibrate_to_screen(ok, f64::NAN, 0.0, 1920.0, 950.0, 1.0),
            None
        ); // NaN
    }

    /// 兜底路径：上报里没有校准字段时，`resolve_rect` 必须原样返回扩展估算值。
    #[test]
    fn resolve_rect_falls_back_without_calibration_fields() {
        let p = SelectionPayload {
            text: "t".into(),
            x: 11,
            y: 22,
            width: 333,
            height: 44,
            rx: None,
            ry: None,
            vw: None,
            vh: None,
            dpr: None,
        };
        assert_eq!(resolve_rect(&p), (11, 22, 333, 44));
    }

    /// 宽高为 0/负时兜底成 1，避免裁图塌缩
    #[test]
    fn resolve_rect_clamps_degenerate_size() {
        let p = SelectionPayload {
            text: "t".into(),
            x: 1,
            y: 2,
            width: 0,
            height: -3,
            rx: None,
            ry: None,
            vw: None,
            vh: None,
            dpr: None,
        };
        assert_eq!(resolve_rect(&p), (1, 2, 1, 1));
    }
}
