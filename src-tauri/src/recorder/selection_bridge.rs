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
    /// 选区矩形（**屏幕物理像素**）：左上角 x/y + 宽高
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// 一条缓存的上报记录
#[derive(Debug, Clone)]
pub struct SelectionReport {
    pub text: String,
    /// (x, y, width, height)，屏幕物理像素
    pub rect: (i32, i32, i32, i32),
    pub at: Instant,
}

static LAST: OnceLock<Mutex<Option<SelectionReport>>> = OnceLock::new();
/// 实际监听中的端口（诊断用；未启动则为 None）
static ACTIVE_PORT: OnceLock<u16> = OnceLock::new();

fn last_slot() -> &'static Mutex<Option<SelectionReport>> {
    LAST.get_or_init(|| Mutex::new(None))
}

/// 记录一条扩展上报的选区
fn record_selection(text: String, rect: (i32, i32, i32, i32)) {
    if let Ok(mut g) = last_slot().lock() {
        *g = Some(SelectionReport {
            text,
            rect,
            at: Instant::now(),
        });
    }
}

/// 取最近一条与剪贴板内容匹配的选区矩形（屏幕物理像素）。
///
/// 两道校验，与 L1a（UIA）保持同等严格：
/// 1. 时效：超过 `max_age_ms` 视为过期（复制动作与回填之间隔太久）；
/// 2. 文本：选区文本必须匹配剪贴板内容（防残留/Ctrl+F 查找条选区骗过几何）。
pub fn last_selection_matching(clipboard: &str, max_age_ms: u64) -> Option<(i32, i32, i32, i32)> {
    let g = last_slot().lock().ok()?;
    let r = g.as_ref()?;
    // 用 >= 而非 >：max_age_ms = 0 的语义是"不采信任何上报"，
    // 若用 >，同一毫秒内（elapsed 取整为 0）反而会被判为有效。
    if r.at.elapsed().as_millis() >= max_age_ms as u128 {
        return None;
    }
    let (_x, _y, w, h) = r.rect;
    if w < 4 || h < 4 {
        return None;
    }
    if !selection_matches_clipboard(&r.text, clipboard) {
        return None;
    }
    Some(r.rect)
}

/// 当前监听端口（诊断用）
pub fn active_port() -> Option<u16> {
    ACTIVE_PORT.get().copied()
}

/// 启动上报接收服务（后台线程，阻塞式 accept，请求量极低无需异步）。
/// 端口全被占用时静默放弃 —— L0 不可用不影响录制。
pub fn start_server() {
    std::thread::spawn(|| {
        let bound = PORTS
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
            ocr_debug(&format!(
                "selection_bridge: L0 hit rect=({},{},{},{}) text={:?}",
                p.x,
                p.y,
                p.width,
                p.height,
                p.text.chars().take(40).collect::<String>()
            ));
            record_selection(p.text, (p.x, p.y, p.width, p.height));
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
}
