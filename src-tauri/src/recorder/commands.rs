use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const _MARKER: &str = "[FLOWIO_CMDS_MARKER_20260825]";

/// Monotonic counter bumped once per recording session.
///
/// Background "settle" threads snapshot it when they are spawned and discard
/// their result if the value moved on. Without this, a settle pass fired just
/// before the user stopped recording would wake up afterwards, capture the
/// desktop — now showing Flowio's own window instead of the target app — and
/// overwrite a good step with a meaningless frame.
static SETTLE_EPOCH: AtomicU64 = AtomicU64::new(0);

fn current_settle_epoch() -> u64 {
    SETTLE_EPOCH.load(Ordering::SeqCst)
}

fn bump_settle_epoch() -> u64 {
    SETTLE_EPOCH.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
}
use image::DynamicImage;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;
use uuid::Uuid;

use crate::recorder::activity;
use crate::recorder::capture::{self, FrameRing};
use crate::recorder::hook;
use crate::recorder::ime;
use crate::recorder::keyboard::{self, KeyEvent};
use crate::recorder::mouse::{CapturedClick, ClickPhase};
use crate::recorder::recorder;
use crate::recorder::screenshot;
use crate::recorder::selection_bridge;
use crate::recorder::state::RecordingState;
use crate::ai::domain::AppDomain;
use crate::recorder::StepData;
use crate::RecordingManager;

// ── Flush helpers ───────────────────────────────────────────────────────

/// Resolve the description text for a typed step.
/// Priority: pre-captured UIA text → current focused-element text → keystroke buffer.
fn resolve_step_description(
    pre_captured: Option<&str>,
    current: Option<&str>,
    typed: &str,
) -> String {
    pre_captured
        .filter(|t| !t.is_empty())
        .or_else(|| current.filter(|t| !t.is_empty()))
        .unwrap_or(typed)
        .to_string()
}

/// Build the title for a typed step.
/// 浏览器域自动剥离「 - 浏览器」后缀只保留网页名，其余领域沿用窗口标题原样。
fn resolve_typed_title(window_title: Option<&str>, domain: AppDomain) -> String {
    match window_title {
        Some(t) if !t.is_empty() => {
            let display = match domain {
                AppDomain::Browser => crate::ai::domain::browser_page_name(t),
                _ => t.to_string(),
            };
            let display = crate::recorder::activity::shorten_window_label(
                &display,
                crate::recorder::activity::TITLE_LABEL_MAX_CHARS,
            );
            format!("在「{}」中输入", display)
        }
        _ => "在「」中输入".to_string(),
    }
}

/// 录制打字步骤时，在「flush 时刻」取前台窗口标题，而非首键时刻。
///
/// 首键时刻常处在对话框弹出 / 焦点切换的过渡期，标题会抓错（如抓到桌面
/// `Program Manager`），导致重放时聚焦错窗口、键盘落到错误位置、文件永远
/// 建不出来。flush 时刻前台已稳定（用户正在输入的那一个对话框），此时抓取
/// 最可靠。这与快捷键步骤（flush_shortcut_step 内部直接取
/// `get_active_window_title()`）的语义保持一致。
fn flush_time_window_title() -> String {
    activity::get_active_window_title().unwrap_or_default()
}

/// Re-index drained captured steps onto the recorder's step list:
/// `order_index` starts at `base_idx`, `step_number` = order_index + 1.
fn reindex_steps(captured: Vec<StepData>, base_idx: i32) -> Vec<StepData> {
    captured
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            s.order_index = base_idx + i as i32;
            s.step_number = s.order_index + 1;
            s
        })
        .collect()
}

/// 组装键盘输入步骤字段（flush_key_step 的 StepData 构造，便于单测）。
fn build_typed_step(
    window_title: Option<String>,
    description: String,
    screenshot_path: String,
    before_screenshot_path: String,
    timestamp: i64,
    is_sensitive: bool,
    crop_x: Option<i32>,
    crop_y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
) -> StepData {
    let domain = window_title
        .as_deref()
        .map(crate::ai::domain::detect_domain)
        .unwrap_or(AppDomain::Other);
    StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: "type".to_string(),
        title: resolve_typed_title(window_title.as_deref(), domain),
        description,
        tip: None,
        screenshot_path,
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path,
        x: crop_x,
        y: crop_y,
        crop_w,
        crop_h,
        after_screenshot_path: None,
        timestamp,
        window_title: window_title.unwrap_or_default(),
        hotkey_combo: String::new(),
        uia_anchor: String::new(),
        mouse_button: String::new(),
        is_sensitive,
    }
}

/// Keyboard step flush (StepSnap-style):
/// - Capture fullscreen → save as both before_screenshot and screenshot (no annotation, no crop).
/// - Text priority: uia_text (captured while focus was on input) → get_focused_element_text() → keystroke buffer.
/// - 密码字段（UIA IsPassword）：明文不入录，描述统一替换为 `[密码已隐藏]`，步骤标记 is_sensitive。
///
/// P1 降噪（`allow_merge`）：仅 idle 停顿触发的 flush 允许把新一轮输入并进
/// 上一步（同一窗口同一输入框因停顿 >1.5s 被拆成多步的历史问题）。
/// 点击 / 快捷键 / 导航键 / 切窗触发的 flush 一律 `allow_merge=false`。
fn flush_key_step(
    typed: &str,
    window_title: Option<String>,
    captured: &Arc<Mutex<Vec<StepData>>>,
    uia_text: Option<(String, bool)>,
    before_path: Option<String>,
    ring: &FrameRing,
    key_ts: Instant,
    allow_merge: bool,
) {
    if typed.is_empty() {
        return;
    }

    // Use the frame closest to (last keystroke + render lag). Taking the frame
    // exactly at keypress time shows the field *before* the app repainted the
    // new character, so the thumbnail is missing the last typed glyph. 250 ms
    // is enough for any app to render while still being inside the ring window.
    let settle_ts = key_ts + Duration::from_millis(250);
    let frame = ring
        .closest_to(settle_ts)
        .or_else(|| ring.closest_to(key_ts))
        .map(|f| (f.image.clone(), f.origin_x, f.origin_y))
        .or_else(|| {
            eprintln!("[flush_key] ring empty, direct capture");
            screenshot::capture_fullscreen_with_origin().ok()
        });
    let Some((frame_img, offset_x, offset_y)) = frame else { return };

    // 全屏原图入册：编辑器的「全图」视图必须就是整个屏幕，不裁窗口。
    let mut clean_img = frame_img;
    // UIA 坐标换算要基于"未裁水印"的图，所以先量尺寸。
    let (pre_w, pre_h) = (clean_img.width() as i32, clean_img.height() as i32);

    // Crop Flowio's own recording status bar from the top; the returned cut
    // height must be subtracted from any crop coordinates that were computed
    // in the pre-crop image space (they would otherwise drift downwards).
    let watermark_cut = screenshot::crop_watermark(&mut clean_img) as i32;

    // Save fullscreen JPEG (both before and screenshot use the same image — no annotation)
    let ss_path = screenshot::next_screenshot_path("key");
    let ss_str = match screenshot::capture_frame_jpeg(&clean_img, &ss_path) {
        Ok(()) => ss_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[flush_key] JPEG write failed: {}", e);
            String::new()
        }
    };

    // Text priority: pre-captured UIA text → current UIA → keystroke fallback
    let (current_uia, current_sensitive) = screenshot::get_focused_element_text()
        .map(|(t, s)| (Some(t), s))
        .unwrap_or((None, false));
    let sensitive = current_sensitive
        || uia_text.as_ref().map(|(_, s)| *s).unwrap_or(false);
    // 密码框：明文绝不入录，统一替换为占位符
    let description = if sensitive {
        "[密码已隐藏]".to_string()
    } else {
        resolve_step_description(
            uia_text.as_ref().map(|(t, _)| t.as_str()),
            current_uia.as_deref(),
            typed,
        )
    };
    // P1 降噪：本轮描述是否来自 UIA（读到的是整个输入框的最终内容，
    // 合并时可直接覆盖旧描述；否则只能按按键流拼接）
    let desc_from_uia = !sensitive
        && (uia_text.as_ref().map(|(t, _)| !t.is_empty()).unwrap_or(false)
            || current_uia.as_ref().map(|t| !t.is_empty()).unwrap_or(false));

    let now_ms = current_time_millis();

    // Scribe-style: anchor the crop to the focused input area so the step's
    // cropped view shows the interaction region instead of the full screen.
    // Uses window-space coordinates (see get_focused_element_rect_in_window):
    // mixing UIA screen coords with window image size used to reject valid
    // rects for non-maximised windows and produce "blank" crops.
    let (win_w, win_h) = (clean_img.width() as i32, clean_img.height() as i32);
    let (crop_x, crop_y, crop_w, crop_h) =
        match screenshot::get_focused_element_rect_in_window(offset_x, offset_y, pre_w, pre_h) {
            Some((rx, ry, w, h)) => {
                let (cx, cy, bw, bh) = screenshot::centered_crop_box(
                    rx + w / 2,
                    ry + h / 2 - watermark_cut,
                    w + CROP_PAD * 2,
                    h + CROP_PAD * 2,
                    win_w,
                    win_h,
                    CROP_MIN_W,
                    CROP_MIN_H,
                );
                (Some(cx), Some(cy), Some(bw), Some(bh))
            }
            None => (
                Some(win_w / 2),
                Some(win_h / 2),
                Some(win_w),
                Some(win_h),
            ),
        };

    let step = build_typed_step(
        window_title,
        description,
        ss_str.clone(),
        before_path.unwrap_or_else(|| ss_str.clone()),
        now_ms,
        sensitive,
        crop_x,
        crop_y,
        crop_w,
        crop_h,
    );

    if let Ok(mut steps) = captured.lock() {
        // P1 降噪：连续输入合并 —— idle 停顿把同一次输入拆成多步时并回上一步
        let can_merge = allow_merge
            && steps
                .last()
                .map(|prev| should_merge_type_steps(prev, &step.window_title, step.is_sensitive))
                .unwrap_or(false);
        if can_merge {
            let prev = steps.last_mut().unwrap();
            merge_typed_step(prev, &step, desc_from_uia, typed);
            eprintln!(
                "[BG] type merge: merged into step {} (desc {} chars)",
                prev.id,
                prev.description.chars().count()
            );
        } else if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
            // B5: 200 步上限（spec.md:160）；超出后停止累积，避免内存与导出体积失控
            steps.push(step);
        } else {
            eprintln!(
                "[flush_key] 录制步骤数已达上限 {}，本次步骤已丢弃",
                crate::recorder::state::MAX_RECORDING_STEPS
            );
        }
    }
}

/// P1 降噪：判断新一轮输入步骤能否并进上一步（纯函数，便于单测）。
/// 条件：上一步也是输入步骤、双方都非敏感（密码输入绝不合并）、同一窗口。
fn should_merge_type_steps(prev: &StepData, new_window_title: &str, new_sensitive: bool) -> bool {
    prev.action_type == "type"
        && !prev.is_sensitive
        && !new_sensitive
        && !prev.window_title.is_empty()
        && prev.window_title == new_window_title
}

/// P1 降噪：把新一轮输入并进上一步（纯函数，便于单测）。
/// - 描述：UIA 可用时直接用本轮文本（读的是整个输入框的最终内容，天然
///   包含前几轮输入）；UIA 不可用时退化为「旧描述 + 本轮按键」拼接。
/// - 截图：主图换成本轮（最终状态）；操作前图保留上一步的（第一次输入前）。
/// - 裁剪框取本轮焦点区域；timestamp / title / window_title 保留上一步。
fn merge_typed_step(prev: &mut StepData, new: &StepData, desc_from_uia: bool, new_typed: &str) {
    prev.description = if desc_from_uia {
        new.description.clone()
    } else {
        format!("{}{}", prev.description, new_typed)
    };
    if !new.screenshot_path.is_empty() {
        prev.screenshot_path = new.screenshot_path.clone();
    }
    prev.x = new.x;
    prev.y = new.y;
    prev.crop_w = new.crop_w;
    prev.crop_h = new.crop_h;
}

/// P1 降噪：两次物理点击是否判为「双击」（纯函数，便于单测）。
/// `dx`/`dy` 为两次点击坐标差，`gap_ms` 为间隔，`dbl_ms` 为系统双击速度
/// （GetDoubleClickTime()，通常 500ms）。距离阈值 8px（略宽于系统 4px，
/// 触屏/高 DPI 下更稳）。
fn is_double_click(dx: f64, dy: f64, gap_ms: u64, dbl_ms: u64) -> bool {
    gap_ms <= dbl_ms && dx.abs() <= 8.0 && dy.abs() <= 8.0
}

/// 系统双击速度（ms）；读不到时用 500ms 默认值。
fn double_click_time_ms() -> u64 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
    let t = unsafe { GetDoubleClickTime() };
    if t == 0 { 500 } else { t as u64 }
}

/// Human-readable label for a shortcut key (KeyC → C, Comma → ,, Space → Space).
fn display_shortcut_key(key: &str) -> String {
    match key {
        "KeySpace" => "Space".to_string(),
        "Return" | "KeyEnter" => "Enter".to_string(),
        "KeyTab" => "Tab".to_string(),
        "Backspace" | "Back" => "Backspace".to_string(),
        "Escape" => "Esc".to_string(),
        "ShiftLeft" | "ShiftRight" => "Shift".to_string(),
        "ControlLeft" | "ControlRight" => "Ctrl".to_string(),
        "Alt" => "Alt".to_string(),
        "MetaLeft" | "MetaRight" => "Win".to_string(),
        "CapsLock" => "CapsLock".to_string(),
        "Comma" => ",".to_string(),
        "Period" => ".".to_string(),
        "SemiColon" => ";".to_string(),
        "Slash" => "/".to_string(),
        "BackSlash" => "\\".to_string(),
        "Minus" => "-".to_string(),
        "Equal" => "=".to_string(),
        "LeftBracket" => "[".to_string(),
        "RightBracket" => "]".to_string(),
        "Quote" => "'".to_string(),
        "Apostrophe" => "'".to_string(),
        "Grave" => "`".to_string(),
        "Num0" | "Num1" | "Num2" | "Num3" | "Num4" | "Num5" | "Num6" | "Num7"
        | "Num8" | "Num9" => key.trim_start_matches("Num").to_string(),
        s if s.starts_with("Key") => s.strip_prefix("Key").unwrap_or(s).to_uppercase(),
        s if s.starts_with("F") => s.to_string(),
        s if s.starts_with("Arrow") || s.ends_with("Arrow") => s.to_string(),
        s if s.starts_with("Unknown(") => s
            .trim_start_matches("Unknown(")
            .trim_end_matches(')')
            .to_string(),
        other => other.to_string(),
    }
}

/// 由修饰键状态 + 主键名构造机器可读组合键串（重放端据此下发）。
///
/// 例：`combo_string(true,false,false,false,"KeyC")` → `"Ctrl+C"`；
/// `combo_string(false,true,false,false,"KeyTab")` → `"Alt+Tab"`；
/// `combo_string(false,false,true,false,"KeyD")` → `"Win+D"`；
/// `combo_string(false,false,false,false,"KeyF5")` → `"F5"`。
///
/// 顺序固定为 Ctrl / Alt / Win / Shift + 主键，便于重放端分词解析。
fn combo_string(ctrl: bool, alt: bool, meta: bool, shift: bool, key_name: &str) -> String {
    let key = display_shortcut_key(key_name);
    let mut parts: Vec<&str> = Vec::with_capacity(4);
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if meta {
        parts.push("Win");
    }
    if shift {
        parts.push("Shift");
    }
    parts.push(key.as_str());
    parts.join("+")
}

/// Windows 键（Meta）组合键 → 中文步骤标题 + 截图 settle。
///
/// 覆盖常用 Win 快捷键；`ctrl`/`shift` 透传用于区分带修饰键的变体
/// （如 Win+Shift+S 区域截图、Win+Ctrl+D 新建虚拟桌面）。
/// 返回 None 表示不识别，交由调用方兜底成「快捷键 Win+X」。
fn win_shortcut(shift: bool, ctrl: bool, key: &str) -> Option<(String, ShortcutSettle)> {
    use ShortcutSettle as S;
    let title = match key {
        "KeyD" => match (ctrl, shift) {
            (true, _) => "Win+Ctrl+D 新建虚拟桌面".to_string(),
            _ => "Win+D 显示桌面".to_string(),
        },
        "KeyE" => "Win+E 打开文件资源管理器".to_string(),
        "KeyR" => "Win+R 打开运行".to_string(),
        "KeyL" => "Win+L 锁定计算机".to_string(),
        "KeyV" => "Win+V 打开剪贴板历史".to_string(),
        "KeyM" => if shift {
            "Win+Shift+M 还原最小化的窗口".to_string()
        } else {
            "Win+M 最小化所有窗口".to_string()
        },
        "KeyComma" => "Win+, 临时查看桌面".to_string(),
        "KeyTab" => "Win+Tab 打开任务视图".to_string(),
        "KeyS" => match (ctrl, shift) {
            (_, true) => "Win+Shift+S 区域截图".to_string(),
            (true, _) => "Win+Ctrl+S 保存全部".to_string(),
            _ => "Win+S 打开搜索".to_string(),
        },
        "KeyI" => "Win+I 打开设置".to_string(),
        "KeyX" => "Win+X 打开快捷菜单".to_string(),
        "KeyA" => "Win+A 打开快速设置".to_string(),
        "KeyN" => "Win+N 打开通知中心".to_string(),
        "KeyB" => "Win+B 焦点移到托盘通知区".to_string(),
        "KeyT" => "Win+T 循环切换任务栏程序".to_string(),
        "KeyU" => "Win+U 打开轻松使用/设置".to_string(),
        "KeyP" => "Win+P 打开投影菜单".to_string(),
        "KeyK" => "Win+K 打开无线显示".to_string(),
        "KeyZ" => "Win+Z 打开窗口贴靠布局".to_string(),
        "KeySpace" => "Win+空格 切换输入法".to_string(),
        "Period" | "SemiColon" => "Win+. 打开表情符号面板".to_string(),
        "PrintScreen" => "Win+PrintScreen 截图并保存".to_string(),
        "KeyF" => "Win+F 打开反馈中心".to_string(),
        "KeyH" => "Win+H 语音输入".to_string(),
        "KeyG" => "Win+G 打开 Xbox 游戏栏".to_string(),
        "KeyC" => "Win+C 打开 Copilot".to_string(),
        "KeyLeftArrow" => match (ctrl, shift) {
            (true, _) => "Win+Ctrl+← 切换到上一个虚拟桌面".to_string(),
            (_, true) => "Win+Shift+← 移动窗口到左侧显示器".to_string(),
            _ => "Win+← 贴靠窗口到左侧".to_string(),
        },
        "KeyRightArrow" => match (ctrl, shift) {
            (true, _) => "Win+Ctrl+→ 切换到下一个虚拟桌面".to_string(),
            (_, true) => "Win+Shift+→ 移动窗口到右侧显示器".to_string(),
            _ => "Win+→ 贴靠窗口到右侧".to_string(),
        },
        "KeyUpArrow" => if shift {
            "Win+Shift+↑ 最大化窗口高度".to_string()
        } else {
            "Win+↑ 最大化窗口".to_string()
        },
        "KeyDownArrow" => if shift {
            "Win+Shift+↓ 还原窗口高度".to_string()
        } else {
            "Win+↓ 还原/最小化窗口".to_string()
        },
        "Key1" | "Key2" | "Key3" | "Key4" | "Key5" | "Key6" | "Key7" | "Key8" | "Key9" => {
            let n = key.chars().last().unwrap().to_digit(10).unwrap();
            if shift {
                format!("Win+Shift+{} 打开任务栏第{}个程序的新实例", n, n)
            } else if ctrl {
                format!("Win+Ctrl+{} 固定/取消固定任务栏程序", n)
            } else {
                format!("Win+{} 打开/激活任务栏第{}个程序", n, n)
            }
        }
        _ => return None,
    };
    Some((title, S::visual(450)))
}

/// Ctrl/Alt 组合键的通用中文标签（覆盖常见办公/浏览器快捷键）。
/// 返回 None 时调用方兜底成「快捷键 Ctrl+X」/「快捷键 Alt+X」。
fn common_combo_label(ctrl: bool, alt: bool, shift: bool, key: &str) -> Option<String> {
    let k = if ctrl { "Ctrl" } else { "Alt" };
    Some(match key {
        "KeyW" => format!("{}+W 关闭窗口/标签页", k),
        "KeyT" => if shift {
            format!("{}+Shift+T 恢复关闭的标签页", k)
        } else {
            format!("{}+T 新建标签页", k)
        },
        "KeyN" => if shift {
            format!("{}+Shift+N 新建隐身窗口", k)
        } else {
            format!("{}+N 新建", k)
        },
        "KeyO" => format!("{}+O 打开", k),
        "KeyP" => if shift {
            format!("{}+Shift+P 命令面板", k)
        } else {
            format!("{}+P 打印", k)
        },
        "KeyS" => if shift {
            format!("{}+Shift+S 另存为", k)
        } else {
            format!("{}+S 保存", k)
        },
        "KeyB" => format!("{}+B 加粗", k),
        "KeyI" => format!("{}+I 斜体", k),
        "KeyU" => format!("{}+U 下划线", k),
        "PrintScreen" => if alt {
            // Alt+PrintScreen = 截取当前活动窗口（录屏场景高频操作）
            "Alt+PrintScreen 截取当前窗口".to_string()
        } else {
            format!("{}+PrintScreen", k)
        },
        "KeyF4" => if ctrl {
            "Ctrl+F4 关闭标签页".to_string()
        } else {
            "Alt+F4 关闭窗口".to_string()
        },
        // 系统级组合键：Ctrl+Alt+Del / Ctrl+Shift+Esc 等此前被记成泛化
        // 「快捷键 Ctrl+Delete」，这里给出正确的中文语义。
        "Delete" => if ctrl && alt {
            "Ctrl+Alt+Del 打开安全选项".to_string()
        } else if shift {
            format!("{}+Shift+Delete 永久删除", k)
        } else {
            format!("{}+Delete 删除", k)
        },
        "Escape" => if ctrl && shift {
            "Ctrl+Shift+Esc 打开任务管理器".to_string()
        } else {
            format!("{}+Esc 取消/关闭", k)
        },
        "F10" => if shift {
            format!("{}+Shift+F10 打开右键菜单", k)
        } else {
            "F10 菜单栏".to_string()
        },
        "KeyTab" => if ctrl && alt {
            "Ctrl+Alt+Tab 打开窗口切换器".to_string()
        } else if shift {
            format!("{}+Shift+Tab 上一个标签页", k)
        } else {
            format!("{}+Tab 下一个标签页", k)
        },
        "KeyEnter" => format!("{}+Enter 提交", k),
        "PageUp" => format!("{}+PageUp 上一个标签页", k),
        "PageDown" => format!("{}+PageDown 下一个标签页", k),
        s if (ctrl && s.starts_with("Key"))
            && matches!(
                s, "Key1" | "Key2" | "Key3" | "Key4" | "Key5" | "Key6" | "Key7" | "Key8" | "Key9"
            ) =>
        {
            let n = s.chars().last().unwrap().to_digit(10).unwrap();
            format!("Ctrl+{} 切换到第{}个标签页", n, n)
        }
        _ => return None,
    })
}

/// 功能键（F1–F12）无修饰键时，此前会被当成空白字符直接丢弃。
/// 这里补成快捷键步骤，常见键带语义标签。
fn function_key_label(key: &str) -> Option<String> {
    Some(
        match key {
            "F1" => "F1 帮助",
            "F2" => "F2 重命名",
            "F3" => "F3 查找下一个",
            "F4" => "F4 地址栏/重复",
            "F5" => "F5 刷新",
            "F6" => "F6 切换焦点区域",
            "F7" => "F7 拼写检查",
            "F8" => "F8 扩展选择",
            "F9" => "F9 发送/接收",
            "F10" => "F10 菜单栏",
            "F11" => "F11 全屏",
            "F12" => "F12 开发者工具",
            _ => return None,
        }
        .to_string(),
    )
}

/// 无修饰键的「动作键」中，此前会被当成空白字符静默丢弃、但明显是一步操作的
/// 键。目前只覆盖截图键（PrintScreen）——它是录屏场景最该被记录的键之一。
fn standalone_key_label(key: &str) -> Option<String> {
    match key {
        "PrintScreen" => Some("PrintScreen 截图保存".to_string()),
        _ => None,
    }
}

/// 无修饰键的「编辑/导航键」中，此前会被当成空白字符静默丢弃、但明显是一步
/// 操作的键。覆盖保存对话框里常用的方向键（在文件列表上下选择）、Backspace/
/// Delete（改文件名）、Home/End/PageUp/PageDown（跳转）——这些键在重放时若丢失，
/// 会导致「在对话框里选错文件 / 文件名删错字符」等，间接造成目标文件没生成。
/// 统一补成快捷键步骤，replay 端 press_combo 已能映射出对应虚拟键。
fn action_key_label(key: &str) -> Option<String> {
    let label = match key {
        "ArrowUp" => "↑ 上移",
        "ArrowDown" => "↓ 下移",
        "ArrowLeft" => "← 左移",
        "ArrowRight" => "→ 右移",
        "Backspace" => "Backspace 删除",
        "Delete" => "Delete 删除",
        "Home" => "Home 行首",
        "End" => "End 行尾",
        "PageUp" => "PageUp 上翻页",
        "PageDown" => "PageDown 下翻页",
        "Insert" => "Insert 切换插入",
        _ => return None,
    };
    Some(label.to_string())
}

/// 裁剪框最小尺寸（px）。比这更小的元素矩形会被撑大，否则裁出来是一条
/// 看不清内容的窄带。
const CROP_MIN_W: i32 = 420;
const CROP_MIN_H: i32 = 280;
/// 元素矩形外扩 padding（px）——让被复制/被输入的文本不贴边。
///
/// ⚠️ 这是**生成落库值**的一端，和仓库里另外几处同名 `CROP_PAD` **语义不同、
/// 取值也故意不同**，改之前务必看清自己改的是哪一端：
///   - 这里（录制端，`recorder/commands.rs`）：PAD=24，**会被 bake 进落库值**；
///   - 消费端前端 `src/lib/cropAnchor.ts` 与导出端 `export/commands.rs`：PAD=8，
///     拿到落库值后再各外扩一次；
///   - 消费端 AI `ai/pipeline.rs`：PAD=24（另有 MIN=280 与 90% 屏幕缩放）。
/// 因此**元素四周的实际留白 = 24（这里）+ 消费端各自的 PAD**，不是任何单处的
/// 常量值。想改"裁图松紧"必须同时看两端，只改一处会得到意料之外的总留白。
/// 消费端之间的几何一致性由 `both_consumers_must_contain_the_visible_element_rect`
/// （ai/pipeline.rs）与 `npm run check:crop` 第 6 段兜底。
const CROP_PAD: i32 = 24;

/// When to re-capture the screen after a shortcut keystroke.
///
/// A flat "wait 450 ms" is wrong for every case: too late for select-all (the
/// user has already pressed Ctrl+C/Ctrl+V and the step shows the *next*
/// action), and far too early for a window switch (the Alt+Tab selector is
/// still on screen).
#[derive(Copy, Clone)]
enum SettleWait {
    /// Sleep a fixed duration, then capture.
    Fixed(u64),
    /// Poll every `poll` ms and capture the **first** frame that actually
    /// differs from the keystroke frame — i.e. the frame where the effect
    /// appeared. Gives up after `max` ms and keeps the last frame.
    UntilChange { max: u64, poll: u64 },
    /// No settle pass — the caller schedules its own (Alt+Tab waits for the
    /// Alt key to be released before it can see the target window).
    Skip,
}

/// Where the step's crop rectangle comes from.
#[derive(Copy, Clone, PartialEq, Eq)]
enum CropSource {
    /// Recompute at settle time from what the shortcut changed on screen.
    Changed,
    /// Read the anchor **at keystroke time** and never touch it again.
    ///
    /// Copy / paste barely change the screen, so there is nothing to diff;
    /// and reading the focused element later is actively wrong: Ctrl+A → C → V
    /// arrive back to back, and by the time the copy step settles the focus has
    /// moved to the paste target — which is how copy steps ended up cropped to
    /// the find-in-page bar.
    FocusAtKeystroke,
    /// No crop at all: show the whole screen (window switch).
    Full,
}

/// How a shortcut step should be "settled" after the keystroke.
#[derive(Copy, Clone)]
struct ShortcutSettle {
    wait: SettleWait,
    /// Where to read the step description from, if anywhere.
    source: Option<BackfillSource>,
    crop: CropSource,
}

impl ShortcutSettle {
    /// Re-capture after the keystroke so the step's screenshot reflects what
    /// the screen looks like *now* (the find-in-page bar after Ctrl+F, the
    /// inserted text after Ctrl+V, ...).
    const fn visual(ms: u64) -> Self {
        Self { wait: SettleWait::Fixed(ms), source: None, crop: CropSource::Changed }
    }
    /// Copy / cut: clip-board-driven step. The screen does NOT change when the
    /// user copies, so re-shooting at settle time only captures the *next*
    /// action the user has already performed (typically Ctrl+V) and overwrites
    /// the original frame. The crop anchor is read at keystroke time
    /// (selection rect or cursor box) and never touched again.
    const fn copy() -> Self {
        Self {
            wait: SettleWait::Skip,
            source: Some(BackfillSource::Clipboard),
            crop: CropSource::FocusAtKeystroke,
        }
    }
    /// Paste: screen DOES change (inserted text). Re-shoot at settle, anchor
    /// on the focused element then (the paste target is the focus).
    const fn paste() -> Self {
        Self {
            wait: SettleWait::Fixed(450),
            source: Some(BackfillSource::Clipboard),
            crop: CropSource::Changed,
        }
    }
    /// Select-all: capture the frame where the highlight appeared, not the
    /// frame half a second later.
    const fn select_all(max: u64, source: BackfillSource) -> Self {
        Self {
            wait: SettleWait::UntilChange { max, poll: 50 },
            source: Some(source),
            crop: CropSource::Changed,
        }
    }
    /// Window switch (Alt+Tab): caller drives the settle pass.
    const fn switch() -> Self {
        Self { wait: SettleWait::Skip, source: None, crop: CropSource::Full }
    }
}

/// Build and push a combo-key step (Scribe-style copy/paste/hotkey semantics).
///
/// The step is created immediately with the frame closest to the keystroke so
/// it never blocks the input thread; `settle` then schedules a background
/// pass that re-captures the screen once the app has finished reacting and
/// rewrites the screenshot / crop / description with the settled state.
///
/// Returns the pushed step's index (used to merge repeat presses such as
/// Alt+Tab into a single step), or `None` when nothing was recorded.
fn flush_shortcut_step(
    action_type: &str,
    title: String,
    description: String,
    captured: &Arc<Mutex<Vec<StepData>>>,
    settle: ShortcutSettle,
    ring: &FrameRing,
    key_ts: Instant,
    // 机器可读组合键串（"Ctrl+C" / "Alt+Tab" / "Win+D" 等）；
    // 重放端据此下发组合键，而不是解析 title 文本（title 含原始键码、格式不统一）。
    combo: &str,
) -> Option<usize> {
    // Frame at keystroke time = the "before" view. Kept around so the settle
    // pass can diff against it and locate exactly what the shortcut changed.
    let before = ring
        .closest_to(key_ts)
        .map(|f| (f.image.clone(), f.origin_x, f.origin_y))
        .or_else(|| {
            eprintln!("[flush_shortcut] ring empty, direct capture");
            screenshot::capture_fullscreen_with_origin().ok()
        });
    let Some((before_full, ox, oy)) = before else { return None; };

    // 全屏原图直接入册：编辑器的「全图」视图必须就是整个屏幕。以前这里会按
    // 活动窗口裁剪，多显示器或非最大化窗口下就裁出一条窄带 —— 用户报的那张
    // 坏图就是这么来的。
    let mut clean_img = before_full.clone();
    // UIA 坐标换算基于"未裁水印"的图，所以先量尺寸。
    let (pre_w, pre_h) = (clean_img.width() as i32, clean_img.height() as i32);
    // Crop Flowio's own recording status bar from the top; crop coordinates
    // computed in the pre-crop space must be shifted up by this amount.
    let watermark_cut = screenshot::crop_watermark(&mut clean_img) as i32;

    let ss_path = screenshot::next_screenshot_path("key");
    let ss_str = match screenshot::capture_frame_jpeg(&clean_img, &ss_path) {
        Ok(()) => ss_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[flush_shortcut] JPEG write failed: {}", e);
            String::new()
        }
    };

    let (win_w, win_h) = (clean_img.width() as i32, clean_img.height() as i32);

    // 复制/粘贴：锚点必须现在读。这类操作画面几乎不变（没法靠像素差定位），
    // 而等到 settle 线程再去问 UIA「焦点在哪」时，Ctrl+A→C→V 早已把焦点带到
    // 粘贴目标上了 —— 复制步骤于是裁到了查找框。
    //
    // 锚点优先级（从可信到兜底）：
    //   1. UIA 选中文本矩形 —— 唯一能直接回答"复制发生在哪"的信息；可惜
    //      chromium 不暴露 text range 给 MSAA 桥接，常常返回 None。
    //   2. 鼠标位置周围一个窗口 —— 用户复制时鼠标通常悬停在被复制的元素
    //      上（链接/单元格/输入框），比 UIA 焦点可靠得多。
    //   3. 屏幕中心 60% 区域 —— 兜底，至少能看到核心内容；不裁是错全屏，
    //      裁成"贴一个角"也是错，居中是错得最少的默认。
    //
    // 旧的 fallback `get_focused_element_rect` 在 chromium 下返回整渲染树根
    // 的 bbox，screen_rect_in_image 算出来 = (0, 0, image_w, image_h)——但
    // 当 chromium 窗口没占满屏幕时（侧边栏/任务栏可见），裁出来的图偏向窗口
    // 所在的位置，**和"复制的文本"毫无关系**。删掉。
    let anchor = if settle.crop == CropSource::FocusAtKeystroke {
        let cursor_box = screenshot::get_cursor_pos()
            .map(|(cx, cy)| (cx - 400, cy - 130, 800, 260));
        let sel = screenshot::get_selected_text_rect();
        let raw = sel
            .or(cursor_box);
        raw.and_then(|r| screenshot::screen_rect_in_image(r, ox, oy, pre_w, pre_h))
            .or_else(|| {
                // 兜底：屏幕中心 60% 区域，居中显示
                let (bx, by) = (pre_w / 5, pre_h / 5);
                let (bw, bh) = (pre_w * 3 / 5, pre_h * 3 / 5);
                Some((bx, by, bw, bh))
            })
    } else {
        None
    };

    // Provisional crop = whole screen; the precise crop is computed later by
    // the settle pass (or already pinned by the keystroke-time anchor above).
    let (crop_x, crop_y, crop_w, crop_h) = match anchor {
        Some((rx, ry, w, h)) => {
            let (cx, cy, bw, bh) = screenshot::centered_crop_box(
                rx + w / 2,
                ry + h / 2 - watermark_cut,
                w + CROP_PAD * 2,
                h + CROP_PAD * 2,
                win_w,
                win_h,
                CROP_MIN_W,
                CROP_MIN_H,
            );
            (Some(cx), Some(cy), Some(bw), Some(bh))
        }
        None => (Some(win_w / 2), Some(win_h / 2), Some(win_w), Some(win_h)),
    };

    let window_title = activity::get_active_window_title();
    let step = StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: action_type.to_string(),
        title,
        description,
        tip: None,
        screenshot_path: ss_str.clone(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: ss_str.clone(),
        x: crop_x,
        y: crop_y,
        crop_w,
        crop_h,
        after_screenshot_path: None,
        timestamp: current_time_millis(),
        window_title: window_title.unwrap_or_default(),
        is_sensitive: false,
        hotkey_combo: combo.to_string(),
        uia_anchor: String::new(),
        mouse_button: String::new(),
    };

    let mut pushed: Option<usize> = None;
    if let Ok(mut steps) = captured.lock() {
        if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
            // ★ Dedup: skip consecutive identical hotkey combos within 150 ms.
            // A stuck Alt flag or physical key repeat can produce "Alt+A", "Alt+A",
            // "Alt+A" in rapid succession — the user only intended one action.
            let is_dup = steps.last().map_or(false, |last| {
                last.action_type == action_type
                    && last.hotkey_combo == combo
                    && (current_time_millis() as i64 - last.timestamp as i64) < 150
            });
            if is_dup {
                eprintln!(
                    "[flush_shortcut] dedup: skipping duplicate combo '{}' ({}ms since last)",
                    combo,
                    current_time_millis() as i64 - steps.last().unwrap().timestamp as i64
                );
            } else {
                let idx = steps.len();
                steps.push(step);
                pushed = Some(idx);
            }
        } else {
            eprintln!(
                "[flush_shortcut] 录制步骤数已达上限 {}，本次步骤已丢弃",
                crate::recorder::state::MAX_RECORDING_STEPS
            );
        }
    }
    let Some(idx) = pushed else { return None; };

    if !matches!(settle.wait, SettleWait::Skip) {
        settle_shortcut_step(
            idx,
            captured,
            settle,
            before_full,
            FrameGeom { origin_x: ox, origin_y: oy, pre_w, pre_h, watermark_cut },
        );
    } else if let Some(source) = settle.source {
        // Wait=Skip 但仍要回填 description（剪贴板 / 焦点选中文本）。
        // 重截图会捕获用户已经按下的下一步操作，所以只起一个轻量线程读 text。
        let epoch = current_settle_epoch();
        let cs = Arc::clone(captured);
        let ss_path = PathBuf::from(ss_str.clone());
        let win_w_i32 = win_w;
        let win_h_i32 = win_h;
        let ox_i32 = ox;
        let oy_i32 = oy;
        // 截图时刻的鼠标位置——用户复制时鼠标通常就在被复制的文字附近，
        // 这是最可靠的"大概在哪"信号。比 200ms 后再读（可能已移开）、
        // 比活动窗口中心（整个浏览器）、比屏幕中心都靠谱得多。
        let cursor_at_capture = screenshot::get_cursor_pos();
        // 截图时刻的 UIA 查询——仅作 a11y warmup（促使 Chromium 提前激活无障碍树，
        // 让后台线程的带校验查询更快命中）。其返回值不采信：此处剪贴板还没读，
        // 无法做「选区文本 == 复制内容」校验，且查找条残留选区会污染结果。
        let _ = screenshot::get_selected_text_rect();
        std::thread::spawn(move || {
            // 剪贴板 / 焦点选区可能尚未刷新，等一小段时间再读。
            std::thread::sleep(std::time::Duration::from_millis(200));
            if current_settle_epoch() != epoch {
                return;
            }
            let text = match source {
                BackfillSource::Clipboard => {
                    screenshot::get_clipboard_text().unwrap_or_default()
                }
                BackfillSource::FocusedSelection => {
                    screenshot::get_focused_selected_text().unwrap_or_default()
                }
            };
            if text.is_empty() {
                return;
            }

            // 1) description 必须写回（之前已经做）
            if let Ok(mut st) = cs.lock() {
                if let Some(s) = st.get_mut(idx) {
                    if s.description.is_empty() {
                        s.description = text.clone();
                    }
                }
            }

            // 2) 重写裁剪锚点——按操作类型分策略：
            //
            //   COPY 类型（Ctrl+C / Ctrl+X）：
            //     L1: OCR 文字定位  → 剪贴板内容直接对应屏幕文字，OCR 最可靠
            //     L2: 选区高亮检测  → 蓝色选中背景（如有）
            //     L3: 活动窗口中心  → 50% 高度（内容区中位，比 35% 更稳）
            //     L4: 屏幕中心      → 最终兜底
            //
            //   其他类型（CLICK/HOTKEY 等）：
            //     L1: 选区高亮检测  → 点击/操作位置的高亮最可靠
            //     L2: OCR 文字定位  → 焦点选区文字
            //     L3: 鼠标位置      → 用户刚操作的位置
            //     L4: 活动窗口      → 窗口中心
            //
            //  Scribe 的核心思路：对 COPY 类操作，OCR 是主力而非备选。
            //  鼠标位置对 COPY 完全不可靠（用户可能用键盘选中后 Ctrl+C，
            //  鼠标在任意位置），所以 COPY 必须以 OCR 为 L1。

            let is_copy_op = matches!(source, BackfillSource::Clipboard);

            // ── COPY L1: UIA 选区矩形（被复制文本的精确几何）──
            // sel_rect_at_capture（事件时刻抓的）只作 a11y warmup——事件时刻剪贴板还没读，
            // 无法做文本校验，不可直接采信（会拿到查找条残留选区）。
            // 统一在后台线程读剪贴板后带校验查询：选区在复制后通常还在，
            // 重试窗口约 2s（递增退避，前段密集抢选区还在的窗口，后段等 a11y 树激活）。
            // 递增退避不影响录制主流程；配合录制启动时的 a11y 预热，
            // 稳态下应该第 1 次就命中。
            // ── COPY L0（最高优先级）：浏览器扩展上报的 DOM selection 矩形 ──
            // 扩展站在页面**里面**，直接读 window.getSelection() 的精确矩形，
            // 不受 UIA a11y 惰性激活影响（浏览器场景下等价于 Scribe 的精度）。
            // 没有上报时（原生应用 / 扩展未安装 / 上报已过期）自动回落 L1a，
            // L0 是增益而非依赖。文本校验由 last_selection_matching 内部完成。
            let sel_rect = if is_copy_op {
                selection_bridge::last_selection_matching(&text, 3000)
                    .or_else(|| screenshot::get_selected_text_rect_with_retry(10, 100, &text))
            } else {
                None
            };
            let mut anchor_found = false;
            if is_copy_op {
                if let Some((sl, st, sw, sh)) = sel_rect {
                    // sel_rect 已经过「选区文本 == 剪贴板内容」校验（get_selected_text_rect_matching），
                    // 几何就是被复制文本的位置，直接采信。
                    let rx = (sl - ox_i32).clamp(0, win_w_i32);
                    let ry = (st - oy_i32).clamp(0, win_h_i32);
                    let rw = sw.clamp(0, win_w_i32 - rx);
                    let rh = sh.clamp(0, win_h_i32 - ry);
                    if rw > 4 && rh > 4 {
                        // 以选区中心为裁图中心、外扩 40px 留边 → 文本居中且完整显示
                        let pad = 40i32;
                        let desired_w = (rw + pad * 2).max(280).min(win_w_i32);
                        let desired_h = (rh + pad * 2).max(120).min(win_h_i32);
                        let cx = rx + rw / 2;
                        let cy = ry + rh / 2;
                        let (bx, by, bw, bh) = screenshot::centered_crop_box(
                            cx, cy, desired_w, desired_h,
                            win_w_i32, win_h_i32, desired_w, desired_h,
                        );
                        if let Ok(mut st) = cs.lock() {
                            if let Some(s) = st.get_mut(idx) {
                                s.x = Some(bx);
                                s.y = Some(by);
                                s.crop_w = Some(bw);
                                s.crop_h = Some(bh);
                            }
                        }
                        screenshot::ocr_debug(&format!(
                            "crop: COPY-L1 sel_rect HIT (text-verified) center ({},{}) sel {}x{} box {}x{}",
                            cx, cy, rw, rh, bw, bh
                        ));
                        anchor_found = true;
                    }
                }
            }

            if !anchor_found {
                anchor_found = {
                if is_copy_op {
                    // ═══════════════════════════════════════════
                    //  COPY L1: 选区高亮检测（用户用鼠标选中文字后按 Ctrl+C，
                    //  截图里必有蓝色/系统高亮色背景——这是最可靠的定位信号）
                    // ═══════════════════════════════════════════
                    let mut copy_anchor = None;
                    if let Ok(img_bytes) = std::fs::read(&ss_path) {
                        if let Ok(img) = image::load_from_memory_with_format(
                            &img_bytes,
                            image::ImageFormat::Jpeg,
                        ) {
                            if let Some((hx, hy, hw, hh)) = screenshot::find_selection_highlight(&img) {
                                screenshot::ocr_debug(&format!(
                                    "crop: COPY-L1 sel_highlight HIT at ({},{}) {}x{}",
                                    hx, hy, hw, hh
                                ));
                                let (cx, cy, cw, ch) = screenshot::centered_crop_box(
                                    hx + hw / 2, hy + hh / 2,
                                    (hw as f64 * 1.5).max(400.0) as i32,
                                    (hh as f64 * 2.5).max(160.0) as i32,
                                    win_w_i32, win_h_i32,
                                    400, 160,
                                );
                                if let Ok(mut st) = cs.lock() {
                                    if let Some(s) = st.get_mut(idx) {
                                        s.x = Some(cx);
                                        s.y = Some(cy);
                                        s.crop_w = Some(cw);
                                        s.crop_h = Some(ch);
                                    }
                                }
                                copy_anchor = Some(true);
                            } else {
                                screenshot::ocr_debug("crop: COPY-L1 sel_highlight None, trying L2 OCR");
                            }
                        } else {
                            screenshot::ocr_debug("crop: COPY-L1 image load failed, trying L2 OCR");
                        }
                    } else {
                        screenshot::ocr_debug("crop: COPY-L1 fs::read failed, trying L2 OCR");
                    }

                    if copy_anchor.is_some() {
                        true
                    } else {
                    // ── COPY L2: OCR（对短文本可靠，长 URL 易失败）──
                    match screenshot::locate_text_in_image(&ss_path, &text) {
                        Some((raw_x, raw_y, raw_w, raw_h)) => {
                            screenshot::ocr_debug(&format!(
                                "crop: COPY-L2 OCR HIT at ({},{}) {}x{} for '{}'",
                                raw_x, raw_y, raw_w, raw_h,
                                text.chars().take(30).collect::<String>()
                            ));
                            let text_len = text.chars().filter(|c| !c.is_whitespace()).count();
                            let by_ocr = (raw_w as f64 * 1.3) as i32;
                            let desired_w = by_ocr.max(text_len as i32 * 9 + 80).max(800).min(win_w_i32);
                            let desired_h = ((raw_h as f64 * 2.2) as i32).max(160).min(win_h_i32);
                            let (cx, cy, cw, ch) = screenshot::centered_crop_box(
                                raw_x + raw_w / 2, raw_y + raw_h / 2,
                                desired_w, desired_h,
                                win_w_i32, win_h_i32,
                                desired_w, desired_h,
                            );
                            if let Ok(mut st) = cs.lock() {
                                if let Some(s) = st.get_mut(idx) {
                                    s.x = Some(cx);
                                    s.y = Some(cy);
                                    s.crop_w = Some(cw);
                                    s.crop_h = Some(ch);
                                }
                            }
                            true
                        }
                        None => {
                            screenshot::ocr_debug("crop: COPY-L2 OCR None, trying L3 screen_center_90pct");
                            false
                        }
                    }
                    } // end else (copy_anchor not found → tried OCR)
                } else {
                    // ── 非 COPY L1: 选区高亮检测 ──
                    if let Ok(img_bytes) = std::fs::read(&ss_path) {
                        if let Ok(img) = image::load_from_memory_with_format(
                            &img_bytes,
                            image::ImageFormat::Jpeg,
                        ) {
                            if let Some((hx, hy, hw, hh)) = screenshot::find_selection_highlight(&img) {
                                screenshot::ocr_debug(&format!(
                                    "crop: L1 sel_highlight ({},{}) {}x{}",
                                    hx, hy, hw, hh
                                ));
                                let (cx, cy, cw, ch) = screenshot::centered_crop_box(
                                    hx + hw / 2, hy + hh / 2,
                                    (hw as f64 * 1.5).max(400.0) as i32,
                                    (hh as f64 * 2.5).max(160.0) as i32,
                                    win_w_i32, win_h_i32,
                                    400, 160,
                                );
                                if let Ok(mut st) = cs.lock() {
                                    if let Some(s) = st.get_mut(idx) {
                                        s.x = Some(cx);
                                        s.y = Some(cy);
                                        s.crop_w = Some(cw);
                                        s.crop_h = Some(ch);
                                    }
                                }
                                true
                            } else {
                                screenshot::ocr_debug("crop: L1 sel_highlight returned None");
                                false
                            }
                        } else {
                            screenshot::ocr_debug("crop: L1 image load_from_memory failed");
                            false
                        }
                    } else {
                        screenshot::ocr_debug("crop: L1 fs::read jpeg failed");
                        false
                    }
                }
            }
            };

            if !anchor_found {
                if is_copy_op {
                    // ═══════════════════════════════════════════
                    //  COPY L3: 截图几何中心 + 超大框（用户明确要求：
                    //  "不要求截图大小，只要求文本在内且居中"）
                    // ═══════════════════════════════════════════
                    //
                    //  到这里说明：UIA 选区矩形=无、OCR=失败、高亮检测=无。
                    //  所有"猜文本位置"的方案在浏览器长 URL 场景都不可靠。
                    //
                    //  唯一保证"文本在内且居中"的方法：以截图本身为中心，
                    //  用一个接近全图的大框（90%×80%），无论文本在屏幕哪都能包住。
                    let anchor_x = win_w_i32 / 2;
                    let anchor_y = win_h_i32 / 2;
                    let box_w = (win_w_i32 as f64 * 0.9) as i32;
                    let box_h = (win_h_i32 as f64 * 0.8) as i32;
                    let (bx, by, bw, bh) = screenshot::centered_crop_box(
                        anchor_x, anchor_y, box_w, box_h,
                        win_w_i32, win_h_i32, box_w, box_h,
                    );
                    if let Ok(mut st) = cs.lock() {
                        if let Some(s) = st.get_mut(idx) {
                            s.x = Some(bx);
                            s.y = Some(by);
                            s.crop_w = Some(bw);
                            s.crop_h = Some(bh);
                        }
                    }
                    screenshot::ocr_debug(&format!(
                        "crop: COPY-L3 screen_center_90pct ({},{}) box {}x{}",
                        anchor_x, anchor_y, bw, bh
                    ));
                } else {
                    // ── 非 COPY L2: OCR 文字定位 ──
                    match screenshot::locate_text_in_image(&ss_path, &text) {
                        Some((raw_x, raw_y, raw_w, raw_h)) => {
                            screenshot::ocr_debug(&format!(
                                "crop: L2 OCR HIT at ({},{}) {}x{} for '{}'",
                                raw_x, raw_y, raw_w, raw_h,
                                text.chars().take(30).collect::<String>()
                            ));
                            if let Ok(mut st) = cs.lock() {
                                if let Some(s) = st.get_mut(idx) {
                                    let text_len = text.chars().filter(|c| !c.is_whitespace()).count();
                                    let by_text = text_len as i32 * 9 + 80;
                                    let by_ocr = (raw_w as f64 * 1.3) as i32;
                                    let desired_w = by_ocr.max(by_text).max(800).min(win_w_i32);
                                    let desired_h = ((raw_h as f64 * 2.2) as i32).max(160).min(win_h_i32);
                                    let (cx, cy, cw, ch) = screenshot::centered_crop_box(
                                        raw_x + raw_w / 2,
                                        raw_y + raw_h / 2,
                                        desired_w, desired_h,
                                        win_w_i32, win_h_i32,
                                        desired_w, desired_h,
                                    );
                                    s.x = Some(cx);
                                    s.y = Some(cy);
                                    s.crop_w = Some(cw);
                                    s.crop_h = Some(ch);
                                }
                            }
                        }
                        None => {
                            // ── 非 COPY L3: 鼠标位置 ──
                            let mut fallback_used = false;
                            if let Some((mx, my)) = cursor_at_capture {
                                let rx = (mx - ox_i32).clamp(0, win_w_i32);
                                let ry = (my - oy_i32).clamp(0, win_h_i32);
                                let text_len = text.chars().filter(|c| !c.is_whitespace()).count();
                                let desired_w = (text_len as i32 * 9 + 120).max(800).min(win_w_i32);
                                let desired_h = 260.min(win_h_i32);
                                let (cx, cy, cw, ch) = screenshot::centered_crop_box(
                                    rx, ry, desired_w, desired_h,
                                    win_w_i32, win_h_i32, desired_w, desired_h,
                                );
                                if let Ok(mut st) = cs.lock() {
                                    if let Some(s) = st.get_mut(idx) {
                                        s.x = Some(cx);
                                        s.y = Some(cy);
                                        s.crop_w = Some(cw);
                                        s.crop_h = Some(ch);
                                    }
                                }
                                screenshot::ocr_debug(&format!(
                                    "crop: L3 cursor_at_capture ({},{}) box {}x{}",
                                    mx, my, desired_w, desired_h
                                ));
                                fallback_used = true;
                            }

                            if !fallback_used {
                                // ── 非 COPY L4: 活动窗口（精确裁剪到窗口边界）──
                                // get_active_window_rect() 返回 GetWindowRect（含标题栏/边框），
                                // screen_rect_in_image 转成图坐标后直接用作裁图区域，
                                // 不再打 8 折/7 折（旧逻辑留 20%/30% 缝隙导致「全图」模式
                                // 裁剪框比实际应用窗口小一圈）。
                                if let Some((wx, wy, ww, wh)) = crate::recorder::activity::get_active_window_rect()
                                    .and_then(|r| screenshot::screen_rect_in_image(r, ox_i32, oy_i32, win_w_i32, win_h_i32))
                                {
                                    // 窗口矩形已由 screen_rect_in_image clamp 到图内，直接使用
                                    let (cx, cy, cw, ch) = (wx, wy, ww, wh);
                                    if let Ok(mut st) = cs.lock() {
                                        if let Some(s) = st.get_mut(idx) {
                                            s.x = Some(cx);
                                            s.y = Some(cy);
                                            s.crop_w = Some(cw);
                                            s.crop_h = Some(ch);
                                        }
                                    }
                                } else {
                                    // ── 非 COPY L5: 屏幕中心 ──
                                    screenshot::ocr_debug("crop: L5 screen_center final fallback");
                                    let (cx, cy, cw, ch) = screenshot::centered_crop_box(
                                        win_w_i32 / 2, win_h_i32 / 2,
                                        (win_w_i32 as f64 * 0.6) as i32,
                                        (win_h_i32 as f64 * 0.5) as i32,
                                        win_w_i32, win_h_i32, 600, 300,
                                    );
                                    if let Ok(mut st) = cs.lock() {
                                        if let Some(s) = st.get_mut(idx) {
                                            s.x = Some(cx);
                                            s.y = Some(cy);
                                            s.crop_w = Some(cw);
                                            s.crop_h = Some(ch);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }
    Some(idx)
}

/// Geometry of the provisional step image, used by the settle pass to align
/// its re-capture with the keystroke frame.
#[derive(Copy, Clone, PartialEq, Eq)]
struct FrameGeom {
    /// Captured monitor's top-left corner in virtual-screen coordinates.
    origin_x: i32,
    origin_y: i32,
    /// Image size **before** the watermark was cropped off.
    pre_w: i32,
    pre_h: i32,
    /// Rows removed from the top by `crop_watermark`.
    watermark_cut: i32,
}

/// Block until the Alt key is released, or `max_ms` elapses.
///
/// Alt+Tab keeps the window switcher on screen for as long as Alt is held, so
/// anything captured before the release shows the switcher — never the window
/// the user actually switched to.
fn wait_alt_release(max_ms: u64) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    loop {
        let down = unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } < 0;
        if !down || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Capture the **first** frame that visibly differs from `before`.
///
/// "Wait 450 ms, then shoot" is too coarse for select-all: Ctrl+A → C → V
/// often arrive within that window, so the step ends up showing the pasted
/// result instead of the selection highlight. Polling from ~50 ms and stopping
/// at the first real change records the moment the user actually selected.
///
/// Returns `None` if recording stopped meanwhile or nothing could be captured.
fn capture_when_changed(
    before: &DynamicImage,
    geom: FrameGeom,
    max_ms: u64,
    poll_ms: u64,
    epoch: u64,
) -> Option<(DynamicImage, i32, i32)> {
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    let mut last: Option<(DynamicImage, i32, i32)> = None;
    loop {
        std::thread::sleep(Duration::from_millis(poll_ms));
        if current_settle_epoch() != epoch {
            return None;
        }
        if let Ok((img, ox, oy)) = screenshot::capture_fullscreen_with_origin() {
            // 只有同一显示器、同尺寸的帧才能和按键帧做像素比对
            let aligned = ox == geom.origin_x
                && oy == geom.origin_y
                && img.width() == before.width()
                && img.height() == before.height();
            if aligned && screenshot::changed_region(before, &img, 28, 6).is_some() {
                return Some((img, ox, oy));
            }
            last = Some((img, ox, oy));
        }
        if Instant::now() >= deadline {
            return last;
        }
    }
}

/// Background "settle" pass for a shortcut step.
///
/// Waits according to `settle.wait`, then in one shot:
///   1. reads the description text while focus is still on the target
///      (`Clipboard` for copy/cut/paste, `FocusedSelection` for select-all);
///   2. re-captures the screen — this is the frame the reader actually wants
///      to see, e.g. the find-in-page bar that Ctrl+F opened;
///   3. recomputes the crop rectangle from the settled view, preferring the
///      UIA focused element and falling back to a before/after pixel diff;
///   4. writes all of the above back into the step.
fn settle_shortcut_step(
    idx: usize,
    captured: &Arc<Mutex<Vec<StepData>>>,
    settle: ShortcutSettle,
    before_full: DynamicImage,
    geom: FrameGeom,
) {
    let epoch = current_settle_epoch();
    let cs = Arc::clone(captured);
    std::thread::spawn(move || {
        // 1) 等应用反应完 —— 不同快捷键等待策略不同。
        let after = match settle.wait {
            SettleWait::Fixed(ms) => {
                std::thread::sleep(Duration::from_millis(ms));
                screenshot::capture_fullscreen_with_origin().ok()
            }
            SettleWait::UntilChange { max, poll } => {
                capture_when_changed(&before_full, geom, max, poll, epoch)
            }
            SettleWait::Skip => None,
        };

        // Recording already stopped — the screen now shows Flowio itself, so
        // whatever we captured would be wrong. Keep the provisional frame.
        if current_settle_epoch() != epoch {
            return;
        }

        // 2) Text first — the focused control still owns the selection.
        let text = match settle.source {
            Some(BackfillSource::Clipboard) => {
                screenshot::get_clipboard_text().unwrap_or_default()
            }
            Some(BackfillSource::FocusedSelection) => {
                screenshot::get_focused_selected_text().unwrap_or_default()
            }
            None => String::new(),
        };

        // 3) Re-capture the settled screen.
        let mut new_path: Option<String> = None;
        let mut new_crop: Option<(i32, i32, i32, i32)> = None; // (cx, cy, w, h)
        let mut new_win_title: Option<String> = None;

        if let Some((after_full, ox, oy)) = after {
            let mut after_img = after_full;
            // UIA 坐标要换算到"未裁水印"的图空间，所以先量尺寸再裁。
            let (pre_w, pre_h) = (after_img.width() as i32, after_img.height() as i32);
            let cut = screenshot::crop_watermark(&mut after_img) as i32;
            let (img_w, img_h) = (after_img.width() as i32, after_img.height() as i32);

            if settle.crop == CropSource::Changed {
                // ① UIA focused element (semantically exact: the input, the
                //    find box, the text field being edited).
                let uia = screenshot::get_focused_element_rect_in_window(ox, oy, pre_w, pre_h)
                    .map(|(rx, ry, w, h)| (rx, ry - cut, w, h));

                // ② Pixel diff against the keystroke frame. Chromium reports
                //    document-virtual coords for in-page content, so UIA often
                //    fails there — the diff still finds the highlight/bar.
                let diff = {
                    let mut before_img = before_full;
                    let bcut = screenshot::crop_watermark(&mut before_img) as i32;
                    let aligned = geom
                        == FrameGeom {
                            origin_x: ox,
                            origin_y: oy,
                            pre_w,
                            pre_h,
                            watermark_cut: bcut,
                        }
                        && img_w > 0
                        && img_h > 0;
                    if aligned {
                        screenshot::changed_region(&before_img, &after_img, 28, 6)
                            .map(|(dx, dy, w, h)| (dx, dy, w, h))
                    } else {
                        None
                    }
                };

                // 居中 + 完整落在图内：以前只把左上角 clamp 到边界，元素就被
                // 挤到一边甚至被切掉一半。
                new_crop = uia.or(diff).map(|(rx, ry, w, h)| {
                    screenshot::centered_crop_box(
                        rx + w / 2,
                        ry + h / 2,
                        w + CROP_PAD * 2,
                        h + CROP_PAD * 2,
                        img_w,
                        img_h,
                        CROP_MIN_W,
                        CROP_MIN_H,
                    )
                });
            }

            let np = screenshot::next_screenshot_path("key_after");
            if screenshot::capture_frame_jpeg(&after_img, &np).is_ok() {
                new_path = Some(np.to_string_lossy().to_string());
            }
            new_win_title = activity::get_active_window_title();
        }

        // 4) Commit everything in a single lock so the editor never sees a
        //    half-updated step (new crop on the old image, etc.).
        if current_settle_epoch() != epoch {
            return;
        }
        if let Ok(mut st) = cs.lock() {
            if let Some(s) = st.get_mut(idx) {
                if !text.is_empty() && s.description.is_empty() {
                    s.description = text;
                }
                match settle.crop {
                    CropSource::Changed => match new_crop {
                        Some((cx, cy, w, h)) => {
                            s.x = Some(cx);
                            s.y = Some(cy);
                            s.crop_w = Some(w);
                            s.crop_h = Some(h);
                        }
                        // 没定位到变化区域：整屏展示，不做半吊子裁剪
                        None => {
                            s.x = None;
                            s.y = None;
                            s.crop_w = None;
                            s.crop_h = None;
                        }
                    },
                    CropSource::FocusAtKeystroke => {
                        // 保留按键时刻读到的锚点：此刻的焦点已经不是复制发生的地方了
                    }
                    CropSource::Full => {
                        s.x = None;
                        s.y = None;
                        s.crop_w = None;
                        s.crop_h = None;
                    }
                }
                // before_screenshot_path intentionally keeps the pre-action
                // frame, so the editor shows a real before/after pair.
                if let Some(p) = new_path {
                    s.screenshot_path = p;
                }
                if let Some(t) = new_win_title.filter(|t| !t.is_empty()) {
                    s.window_title = t;
                }
            }
        }
    });
}

#[derive(Copy, Clone)]
enum BackfillSource {
    Clipboard,
    FocusedSelection,
}

/// True when the key is a modifier pressed on its own.
///
/// The hook sets the Ctrl/Alt/Shift state flags *before* forwarding the press,
/// so `Alt` arrives as `Press { key: "Alt", alt: true }` and used to fall into
/// the combo branch — producing a bogus "快捷键 Alt+Alt" step right before the
/// real "快捷键 Alt+Tab". Modifiers never carry an action of their own.
fn is_pure_modifier(key: &str) -> bool {
    matches!(
        key,
        "Control" | "ControlLeft" | "ControlRight"
            | "Shift" | "ShiftLeft" | "ShiftRight"
            | "Alt" | "AltLeft" | "AltRight" | "AltGr"
            | "Meta" | "MetaLeft" | "MetaRight"
            | "CapsLock" | "NumLock" | "ScrollLock"
    )
}

/// Rewrite an Alt+Tab step's title once the target window has actually been
/// activated, e.g. "快捷键 Alt+Tab · 切换到 无标题 - Notepad", and replaces
/// the screenshot with the window that was landed on.
///
/// Called again on every repeat Tab press, so both the title and the image
/// always show the window the user finally landed on.
///
/// The wait is driven by the **Alt key release**, not a fixed delay: while Alt
/// is held the screen shows the window switcher, so any screenshot taken
/// before the release shows the switcher instead of the target window.
fn schedule_alt_tab_settle(idx: usize, captured: &Arc<Mutex<Vec<StepData>>>) {
    let epoch = current_settle_epoch();
    let cs = Arc::clone(captured);
    std::thread::spawn(move || {
        wait_alt_release(3000);
        std::thread::sleep(Duration::from_millis(300));
        if current_settle_epoch() != epoch {
            return;
        }
        let title = activity::get_active_window_title().filter(|t| !t.is_empty());
        let shot = screenshot::capture_fullscreen_with_origin().ok().and_then(
            |(mut img, _, _)| {
                screenshot::crop_watermark(&mut img);
                let p = screenshot::next_screenshot_path("key_after");
                screenshot::capture_frame_jpeg(&img, &p)
                    .ok()
                    .map(|_| p.to_string_lossy().to_string())
            },
        );
        if current_settle_epoch() != epoch {
            return;
        }
        if let Ok(mut st) = cs.lock() {
            if let Some(s) = st.get_mut(idx) {
                if let Some(t) = title {
                    s.title = format!(
                        "快捷键 Alt+Tab · 切换到 {}",
                        crate::recorder::activity::shorten_window_label(
                            &t,
                            crate::recorder::activity::TITLE_LABEL_MAX_CHARS,
                        )
                    );
                    s.window_title = t;
                }
                // 切窗口：整屏展示新窗口，不做裁剪
                s.x = None;
                s.y = None;
                s.crop_w = None;
                s.crop_h = None;
                if let Some(p) = shot {
                    s.screenshot_path = p;
                }
            }
        }
    });
}

/// Build a click step: fullscreen + annotated with a red element box and
/// a numbered step badge (UI spec: 红框 + 右上角序号圆圈).
/// Both before_screenshot and screenshot use the same fullscreen annotated image — no cropping.
fn build_click_step(
    x: f64,
    y: f64,
    click_ts: Instant,
    ring: &FrameRing,
    step_no: i32,
    is_double: bool,
    button: &str,
) -> Option<StepData> {
    let frame = ring
        .closest_to(click_ts)
        .map(|f| (f.image.clone(), f.origin_x, f.origin_y))
        .or_else(|| {
            eprintln!("[BG] click: ring empty, direct capture");
            screenshot::capture_fullscreen_with_origin().ok()
        })?;
    let (frame_img, origin_x, origin_y) = frame;

    // 全屏原图入册：编辑器的「全图」视图必须就是整个屏幕，不裁窗口。
    // 点击坐标是全局虚拟屏幕坐标，减去显示器原点换算到图像空间。
    let (cx, cy) = (x.round() as i32 - origin_x, y.round() as i32 - origin_y);

    // Red element box + step number badge drawn on the fullscreen image
    let mut annotated = frame_img.clone();
    screenshot::draw_click_marker(&mut annotated, cx, cy, step_no);

    // Crop Flowio's own recording status bar from both images; the stored
    // click coordinates must be shifted up by the cropped height so the editor
    // anchors the thumbnail at the real click point.
    let mut clean_before = frame_img;
    let watermark_cut = screenshot::crop_watermark(&mut clean_before) as i32;
    screenshot::crop_watermark(&mut annotated);
    let (win_cx, win_cy) = (cx, cy - watermark_cut);

    // Save clean before screenshot (no annotation)
    let before_path = screenshot::next_screenshot_path("click_before");
    let before_str = match screenshot::capture_frame_jpeg(&clean_before, &before_path) {
        Ok(()) => before_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[BG] click before screenshot write failed: {}", e);
            return None;
        }
    };

    // Save annotated screenshot
    let ss_path = screenshot::next_screenshot_path("click");
    let ss_str = match screenshot::capture_frame_jpeg(&annotated, &ss_path) {
        Ok(()) => ss_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[BG] click screenshot write failed: {}", e);
            return None;
        }
    };

    let now_ms = current_time_millis();

    // Stage 2b：抓取点击处的控件锚点（控件类名 + 归一化点击比例），用于分辨率无关重放。
    // 任何失败返回空串，重放端回退到录制坐标。
    let uia_anchor = crate::anchor::capture_anchor(x as i32, y as i32)
        .map(|a| serde_json::to_string(&a).unwrap_or_default())
        .unwrap_or_default();

    let window_title = activity::get_active_window_title();
    let click_domain = window_title
        .as_deref()
        .map(crate::ai::domain::detect_domain)
        .unwrap_or(AppDomain::Other);
    let title = activity::make_click_title_for_domain(window_title.as_deref(), click_domain);
    // P1 降噪：双击合并——标题改为「双击」，action_type 保持 "click"
    // （前端/导出/AI 均按 click 渲染，不引入未知类型）。
    let title = if is_double { title.replacen("点击", "双击", 1) } else { title };
    // 右键点击：标题标「右键」让用户可见；action_type 保持 "click"，
    // 按键信息落 mouse_button 列，重放端据此下发右键（桌面右键菜单链路依赖）。
    let title = if button.eq_ignore_ascii_case("right") { title.replacen("点击", "右键点击", 1) } else { title };

    Some(StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: "click".to_string(),
        title,
        description: String::new(),
        tip: None,
        screenshot_path: ss_str.clone(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: before_str,
        x: Some(win_cx),
            y: Some(win_cy),
            crop_w: None,
            crop_h: None,
            after_screenshot_path: None,
            timestamp: now_ms,
            window_title: window_title.unwrap_or_default(),
            hotkey_combo: String::new(),
            uia_anchor,
            is_sensitive: false,
            // 按键 × 双击编码进 mouse_button：left/right/left-double/right-double。
            // 双击识别当前只对左键触发（dbl_prev_id 要求 left），右键分支保持对称。
            mouse_button: encode_mouse_button(button, is_double),
    })
}

/// 按键 × 双击 → mouse_button 列编码（纯函数，便于单测锁契约）。
/// ⚠️ 必须先归一化小写：hook 历史上曾发 "Right"/"Left"（首字母大写），
/// 大小写不匹配会让右键/双击全部静默退化为 left（2026-09-08 事故根因）。
fn encode_mouse_button(button: &str, is_double: bool) -> String {
    match (button.to_lowercase().as_str(), is_double) {
        ("right", true) => "right-double".to_string(),
        ("right", false) => "right".to_string(),
        (_, true) => "left-double".to_string(),
        (_, false) => "left".to_string(),
    }
}

/// Build a drag step: the fullscreen frame captured at *release* time (so the
/// selection is fully drawn), annotated with a red selection rectangle spanning
/// the dragged region, and cropped to the bounding box of the press→release
/// points (+ a small margin) rather than centering on a single click point.
///
/// This is what fixes "drag steps crop to an empty area": the old path only
/// recorded the button-press coordinate as a plain click, so the crop landed on
/// the drag's start point and missed the selected region entirely.
/// drag 步骤裁图框几何（纯函数，便于单测）。
///
/// 输入：选区包围盒 (bx0, by0, bw, bh) 与图尺寸；输出：裁图矩形 (left, top, w, h)。
/// 规则：
/// - 选区各向 100% 外扩（至少 DRAG_PAD_MIN）——拖拽选区多为扁长矩形（如表格选几行/列），
///   50% 外扩对单行选区（~25px）只加 12px（被 min 吃掉），用户看不出在拖哪里；
///   100% 外扩保证「选区 + 等量上下文」，扁条也能带出周围行/列。
/// - 最小视野 DRAG_MIN_CROP_W/H：确保即使点按式拖拽（bw/bh≈0）也有足够上下文辨认位置；
/// - 饱和到图尺寸，中心尽量对齐选区中心；贴边时整体平移进图内（不缩框）。
fn drag_crop_box(
    bx0: i32,
    by0: i32,
    bw: i32,
    bh: i32,
    img_w: i32,
    img_h: i32,
) -> (i32, i32, i32, i32) {
    const DRAG_PAD_MIN: i32 = 40;
    const DRAG_MIN_CROP_W: i32 = 560;
    const DRAG_MIN_CROP_H: i32 = 520;
    let img_w = img_w.max(1);
    let img_h = img_h.max(1);
    let pad_x = bw.max(DRAG_PAD_MIN);
    let pad_y = bh.max(DRAG_PAD_MIN);
    let crop_w = (bw + pad_x * 2).max(DRAG_MIN_CROP_W).min(img_w);
    let crop_h = (bh + pad_y * 2).max(DRAG_MIN_CROP_H).min(img_h);
    let cx0 = bx0 + bw / 2;
    let cy0 = by0 + bh / 2;
    let left = (cx0 - crop_w / 2).clamp(0, img_w - crop_w);
    let top = (cy0 - crop_h / 2).clamp(0, img_h - crop_h);
    (left, top, crop_w, crop_h)
}

fn build_drag_step(
    dx: f64,
    dy: f64,
    ux: f64,
    uy: f64,
    release_ts: Instant,
    ring: &FrameRing,
    step_no: i32,
) -> Option<StepData> {
    let frame = ring
        .closest_to(release_ts)
        .map(|f| (f.image.clone(), f.origin_x, f.origin_y))
        .or_else(|| {
            eprintln!("[BG] drag: ring empty, direct capture");
            screenshot::capture_fullscreen_with_origin().ok()
        })?;
    let (frame_img, origin_x, origin_y) = frame;

    // Press/release points in image coordinates (subtract monitor origin).
    let (dx_i, dy_i) = (dx.round() as i32 - origin_x, dy.round() as i32 - origin_y);
    let (ux_i, uy_i) = (ux.round() as i32 - origin_x, uy.round() as i32 - origin_y);

    // Red selection rectangle drawn on the fullscreen image.
    let mut annotated = frame_img.clone();
    screenshot::draw_drag_marker(&mut annotated, dx_i, dy_i, ux_i, uy_i);

    // Crop Flowio's own recording status bar off the top of both images; the
    // stored coordinates must shift up by the cropped height.
    let mut clean_before = frame_img;
    let watermark_cut = screenshot::crop_watermark(&mut clean_before) as i32;
    screenshot::crop_watermark(&mut annotated);

    // Bounding box of the drag in origin-shifted image coords, then shift up by
    // the watermark cut so it lines up with the cropped screenshot space.
    let bx0 = (dx_i.min(ux_i)) - watermark_cut;
    let by0 = (dy_i.min(uy_i)) - watermark_cut;
    let bw = (dx_i - ux_i).abs();
    let bh = (dy_i - uy_i).abs();

    let (img_w, img_h) = (annotated.width() as i32, annotated.height() as i32);
    let (left, top, crop_w, crop_h) = drag_crop_box(bx0, by0, bw, bh, img_w, img_h);
    let (win_cx, win_cy) = (left + crop_w / 2, top + crop_h / 2);

    // Save clean before screenshot (no annotation)
    let before_path = screenshot::next_screenshot_path("drag_before");
    let before_str = match screenshot::capture_frame_jpeg(&clean_before, &before_path) {
        Ok(()) => before_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[BG] drag before screenshot write failed: {}", e);
            return None;
        }
    };

    // Save annotated screenshot
    let ss_path = screenshot::next_screenshot_path("drag");
    let ss_str = match screenshot::capture_frame_jpeg(&annotated, &ss_path) {
        Ok(()) => ss_path.to_string_lossy().to_string(),
        Err(e) => {
            eprintln!("[BG] drag screenshot write failed: {}", e);
            return None;
        }
    };

    let now_ms = current_time_millis();
    let window_title = activity::get_active_window_title();

    // Stage 2b：拖拽起点（dx, dy）处抓取控件锚点，用于分辨率无关重放。
    let uia_anchor = crate::anchor::capture_anchor(dx as i32, dy as i32)
        .map(|a| serde_json::to_string(&a).unwrap_or_default())
        .unwrap_or_default();

    Some(StepData {
        id: Uuid::new_v4().to_string(),
        order_index: 0,
        step_number: 0,
        action_type: "drag".to_string(),
        title: "拖拽选中".to_string(),
        description: String::new(),
        tip: None,
        screenshot_path: ss_str.clone(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: before_str,
        x: Some(win_cx),
        y: Some(win_cy),
            crop_w: Some(crop_w),
            crop_h: Some(crop_h),
            after_screenshot_path: None,
            timestamp: now_ms,
            window_title: window_title.unwrap_or_default(),
            hotkey_combo: String::new(),
            uia_anchor,
            is_sensitive: false,
            mouse_button: String::new(),
    })
}

// ── Spawn event-processing thread (extracted for reuse in start & resume) ──

/// 当前 Unix 时间戳（毫秒）。
fn current_time_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// 按键对累积输入缓冲区的效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferEffect {
    /// 产生了实际输入字符（需要刷新 idle 计时）
    Input,
    /// 退格删除字符（需要刷新 UIA 文本）
    Backspace,
    /// 无效键（修饰键 / 导航键 / 未知键），无副作用
    Ignored,
}

/// 将按键应用到累积输入缓冲区，返回该键的效果。
/// `char_str` 是已经用 ToUnicodeEx 解析出的「真实输入字符」（符号/大小写/布局
/// 都已正确）；`key_name` 仅用于退格识别。
fn apply_key_to_buffer(buffer: &mut String, key_name: &str, char_str: &str) -> BufferEffect {
    if key_name == "Backspace" || key_name == "Back" {
        if !buffer.is_empty() {
            buffer.pop();
        }
        BufferEffect::Backspace
    } else if !char_str.is_empty() && !char_str.starts_with('[') {
        buffer.push_str(char_str);
        BufferEffect::Input
    } else {
        BufferEffect::Ignored
    }
}

/// 尚未开始输入时，初始化一次 typing step：记录前台窗口标题并截「按键前」图。
/// 用于首键与 IME 提交中文的时刻。
fn ensure_typing_started(
    accumulated_keys: &mut String,
    typing_window_title: &mut Option<String>,
    keyboard_before_path: &mut Option<String>,
) {
    if accumulated_keys.is_empty() {
        *typing_window_title = activity::get_active_window_title();
        if let Ok(img) = screenshot::capture_fullscreen() {
            let mut win_img = img;
            screenshot::crop_watermark(&mut win_img);
            let bp = screenshot::next_screenshot_path("key_before");
            if screenshot::capture_frame_jpeg(&win_img, &bp).is_ok() {
                *keyboard_before_path = Some(bp.to_string_lossy().to_string());
            }
        }
    }
}

/// 按键是否属于导航键（Enter/Tab/Escape，触发立即 flush）。
///
/// ⚠️ 必须同时接受 hook 原始名与规范化名：`hook::vk_to_key_name` 对 VK_TAB
/// 返回 **"KeyTab"**（VK_RETURN 返回 "Return"、VK_ESCAPE 返回 "Escape"），
/// 而这里早先只认 "Tab" → Tab 键既不匹配导航键、也不被
/// function/standalone/action_key_label 覆盖、ToUnicodeEx 又无字符输出，
/// 于是被静默丢弃：录制里完全看不到 Tab 步骤，重放自然不会切字段
/// （用户报「tab 切换键没有执行」）。
fn is_nav_key(key_name: &str) -> bool {
    matches!(
        key_name,
        "Enter" | "Return" | "KeyEnter" | "Tab" | "KeyTab" | "Escape"
    )
}

/// 前台窗口变化时是否触发 flush：两侧均有标题、标题不同、且距上次按键超过 500ms。
fn should_flush_on_window_change(
    last: Option<&str>,
    current: Option<&str>,
    since_last_key: Duration,
) -> bool {
    matches!((last, current), (Some(l), Some(c)) if l != c)
        && since_last_key > Duration::from_millis(500)
}

fn spawn_event_thread(
    running: Arc<AtomicBool>,
    mouse_rx: mpsc::Receiver<CapturedClick>,
    key_rx: mpsc::Receiver<KeyEvent>,
    frame_rx: mpsc::Receiver<capture::CapturedFrame>,
    captured_steps: Arc<Mutex<Vec<StepData>>>,
    record_self: bool,
    target_window: Option<String>,
) -> std::thread::JoinHandle<()> {
    let running_clone = Arc::clone(&running);
    let cs_for_thread = captured_steps;

    std::thread::spawn(move || {
        eprintln!("[BG] event thread started");

        // Ring buffer: 2 seconds of history at 4 FPS = 8 frames, use 16 for margin
        let mut ring = FrameRing::new(16);

        let mut accumulated_keys = String::new();
        let mut last_key_time = Instant::now();
        let mut typing_window_title: Option<String> = None;
        let mut last_window_title: Option<String> = None;
        let mut last_uia_text: Option<(String, bool)> = None;
        let mut keyboard_before_path: Option<String> = None;
        // 按住 Alt 连按 Tab 时，把多次切换合并进同一个步骤：
        // (最后一次 Tab 的时刻, 该步骤在 captured 中的下标)
        let mut last_alt_tab: Option<(Instant, usize)> = None;
        // P1 降噪：上一次通过过滤的左键点击 (x, y, 时刻, 步骤id, fg_pid)
        let mut last_click: Option<(f64, f64, Instant, String, u32)> = None;
        // 拖拽配对：记录最近一次按下（及为其创建的点击步骤 id），
        // 待松开时若两点相距超过阈值则改为拖拽步骤。
        let mut pending_down: Option<(CapturedClick, Option<String>)> = None;
        // P1 降噪：上一步骤是否为「可续并」的输入步骤（仅 idle flush 置 true，
        // 点击/快捷键/导航键/切窗 flush 置 false）
        let mut can_merge_type = false;
        let flush_interval = Duration::from_millis(1500);
        // IME 组合状态跟踪：用于检测「组合结束」的瞬间，提交中文最终文本。
        let mut prev_composing = false;

        loop {
            if !running_clone.load(Ordering::Relaxed) {
                eprintln!("[BG] running flag cleared, exiting");
                if !accumulated_keys.is_empty() {
                    let typed = std::mem::take(&mut accumulated_keys);
                    flush_key_step(&typed, None, &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time, false);
                }
                break;
            }

            // ── IME 组合结束 → 提交中文最终文本 ─────────────
            // 在每个循环 tick 检测：一旦组合从「进行中」变为「结束」，立刻读取
            // GCS_RESULTSTR 拿到最终中文（如「你好」），并入输入缓冲，效果等同
            // 输入了一串真实字符。放在循环顶部（而非仅按键时）是因为中文可能
            // 通过点击候选词提交、不再有后续按键，必须靠轮询捕捉状态翻转。
            {
                let composing_now = ime::is_ime_composing();
                if prev_composing && !composing_now {
                    if let Some(result) = ime::get_composition_result() {
                        if !result.is_empty() {
                            ensure_typing_started(
                                &mut accumulated_keys,
                                &mut typing_window_title,
                                &mut keyboard_before_path,
                            );
                            accumulated_keys.push_str(&result);
                            if let Some((text, sensitive)) =
                                screenshot::get_focused_element_text()
                            {
                                if !text.is_empty() {
                                    last_uia_text = Some((text, sensitive));
                                }
                            }
                            last_key_time = Instant::now();
                            eprintln!("[BG] IME committed: '{}'", result);
                        }
                    }
                }
                prev_composing = composing_now;
            }

            // ── Drain capture ring frames ──────────────────
            while let Ok(frame) = frame_rx.try_recv() {
                ring.push(frame);
            }

            // ── Mouse events ───────────────────────────────
            match mouse_rx.try_recv() {
                Ok(click) => {
                    let is_down = click.phase == ClickPhase::Down;
                    eprintln!(
                        "[BG] mouse {}: {} at ({}, {}), ts={:?}, fg_pid={}",
                        if is_down { "down" } else { "up" },
                        click.button, click.x, click.y, click.timestamp, click.foreground_pid
                    );

                    // Self-filter using PID captured at hook time (not current foreground)
                    if !record_self && recorder::pid_is_flowio(click.foreground_pid) {
                        eprintln!("[BG] self-filter: skipping click in flowio window (pid={})", click.foreground_pid);
                        continue;
                    }

                    // Target-app filter using PID captured at hook time (录制指定软件)
                    if let Some(ref target) = target_window {
                        if !recorder::pid_is_target(click.foreground_pid, target) {
                            eprintln!(
                                "[BG] target-filter: skipping click (fg_pid={} != {})",
                                click.foreground_pid, target
                            );
                            continue;
                        }
                    }

                    if is_down {
                        // Flush pending keystrokes before the click step
                        if !accumulated_keys.is_empty() {
                            let typed = std::mem::take(&mut accumulated_keys);
                            let win_title = flush_time_window_title();
                            flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time, false);
                        }
                        last_key_time = Instant::now();
                        can_merge_type = false; // 点击打断输入连续性

                        // 拖拽配对：若上一轮按下还没收到松开（极少见，如鼠标移出窗口），
                        // 先把那次按下当成点击落成步骤，避免漏步。
                        if let Some((stale_down, _)) = pending_down.take() {
                            let step_no = { let s = cs_for_thread.lock().unwrap(); s.len() as i32 + 1 };
                            if let Some(step) = build_click_step(stale_down.x, stale_down.y, stale_down.timestamp, &ring, step_no, false, &stale_down.button) {
                                let mut steps = cs_for_thread.lock().unwrap();
                                if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
                                    steps.push(step);
                                }
                            }
                        }

                        // ── P1 降噪：双击识别与合并 ──
                        // 与上一次「通过过滤的左键点击」间隔 < 系统双击速度、
                        // 位置 ≤8px、同前台窗口 → 撤掉上一步，用第二次点击重建
                        // 为「双击」步骤（如 WPS 单元格「点两下才进编辑」）。
                        let dbl_prev_id = match last_click {
                            Some((lx, ly, lts, ref lid, lpid))
                                if click.button.eq_ignore_ascii_case("left")
                                    && lpid == click.foreground_pid
                                    && is_double_click(
                                        click.x - lx,
                                        click.y - ly,
                                        lts.elapsed().as_millis() as u64,
                                        double_click_time_ms(),
                                    ) =>
                            {
                                Some(lid.clone())
                            }
                            _ => None,
                        };
                        let mut new_click_id: Option<String> = None;
                        {
                            let mut steps = cs_for_thread.lock().unwrap();
                            if let Some(ref prev_id) = dbl_prev_id {
                                if let Some(pos) = steps
                                    .iter()
                                    .position(|s| &s.id == prev_id && s.action_type == "click")
                                {
                                    steps.remove(pos);
                                    eprintln!("[BG] double-click: removed single-click step {}", prev_id);
                                }
                            }
                            let click_step_no = steps.len() as i32 + 1;
                            if let Some(step) = build_click_step(
                                click.x, click.y, click.timestamp, &ring, click_step_no,
                                dbl_prev_id.is_some(), &click.button,
                            ) {
                                if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
                                    new_click_id = Some(step.id.clone());
                                    steps.push(step);
                                } else {
                                    eprintln!(
                                        "[BG] 录制步骤数已达上限 {}，本次点击步骤已丢弃",
                                        crate::recorder::state::MAX_RECORDING_STEPS
                                    );
                                }
                            }
                        }
                        // 记录本次按下，供松开阶段判定拖拽
                        pending_down = Some((click.clone(), new_click_id.clone()));
                        last_click = match (dbl_prev_id, new_click_id) {
                            // 双击合并后清空：三连击不再继续链（第三次点击按新点击算）
                            (Some(_), _) => None,
                            (None, Some(id)) => Some((
                                click.x,
                                click.y,
                                click.timestamp,
                                id,
                                click.foreground_pid,
                            )),
                            _ => last_click,
                        };
                    } else {
                        // ── 松开阶段：判定拖拽 ──
                        const DRAG_THRESHOLD_PX: f64 = 8.0; // 超过即视为拖拽
                        if let Some((down, down_step_id)) = pending_down.take() {
                            let dist_x = click.x - down.x;
                            let dist_y = click.y - down.y;
                            let dist = (dist_x * dist_x + dist_y * dist_y).sqrt();
                            if click.button == down.button && dist > DRAG_THRESHOLD_PX {
                                // 拖拽：用按下→松开包围盒裁图，并覆盖掉刚才落的点击步骤
                                let step_no = { let s = cs_for_thread.lock().unwrap(); s.len() as i32 + 1 };
                                if let Some(drag_step) = build_drag_step(
                                    down.x, down.y, click.x, click.y, click.timestamp, &ring, step_no,
                                ) {
                                    let mut steps = cs_for_thread.lock().unwrap();
                                    if let Some(pos) = down_step_id
                                        .as_ref()
                                        .and_then(|id| steps.iter().position(|s| &s.id == id))
                                    {
                                        steps.remove(pos);
                                        eprintln!(
                                            "[BG] drag: removed click step {} (converted to drag, dist={:.1}px)",
                                            down_step_id.as_ref().unwrap(), dist
                                        );
                                    }
                                    if steps.len() < crate::recorder::state::MAX_RECORDING_STEPS {
                                        steps.push(drag_step);
                                    }
                                }
                                last_click = None;
                            }
                            // 否则保持原点击步骤，不做任何改动
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("[BG] mouse channel disconnected");
                    break;
                }
            }

            // ── Key events ─────────────────────────────────
            match key_rx.try_recv() {
                Ok(KeyEvent::Press {
                    key: key_name,
                    vk,
                    scan,
                    hkl,
                    ctrl,
                    shift,
                    alt,
                    meta,
                    ts: key_instant,
                    foreground_pid: fg_pid,
                }) => {
                    // ★ 修饰键状态校准：钩子的原子标志（CTRL_DOWN/ALT_DOWN 等）
                    // 可能因 keyup 被系统吞掉而卡在 true，导致后续所有普通键被
                    // 错误地当成组合键（如用户只按了 "A" 却录成 "Alt+A"）。
                    // 每次消费 KeyEvent 时用 GetAsyncKeyState 查询真实硬件状态，
                    // 覆盖钩子快照中可能过时的修饰位。
                    // 修饰键状态校准：钩子的原子标志（CTRL_DOWN/ALT_DOWN 等）可能因
                    // keyup 被系统吞掉而卡在 true，导致后续所有普通键被错误当成组合键
                    // （如只按了 "A" 却录成 "Ctrl+A"）。每次消费 KeyEvent 用
                    // GetAsyncKeyState 查询真实硬件状态覆盖钩子快照。仅允许 down→up
                    // 方向校准（解除卡死），绝不把物理按住的组合键误判为松开
                    // （那样会破坏「按住 Ctrl 再按 C」这类真实组合）。
                    let (cal_ctrl, cal_shift, cal_alt, cal_meta) = {
                        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN};
                        let real_ctrl = (unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as i16) < 0;
                        let real_shift = (unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) } as i16) < 0;
                        let real_alt = (unsafe { GetAsyncKeyState(VK_MENU.0 as i32) } as i16) < 0;
                        let real_meta = (unsafe { GetAsyncKeyState(VK_LWIN.0 as i32) } as i16) < 0
                            || (unsafe { GetAsyncKeyState(0x5C /* VK_RWIN */) } as i16) < 0;
                        if ctrl && !real_ctrl {
                            eprintln!("[BG] modifier calibration: forcing ctrl=false (was stuck)");
                        }
                        if alt && !real_alt {
                            eprintln!("[BG] modifier calibration: forcing alt=false (was stuck)");
                        }
                        (ctrl && real_ctrl, shift && real_shift, alt && real_alt, meta && real_meta)
                    };
                    let ctrl = cal_ctrl;
                    let shift = cal_shift;
                    let alt = cal_alt;
                    let meta = cal_meta;

                    // Self-filter using PID captured at hook time: 键盘分支此前
                    // 未过滤 Flowio 自身窗口的输入，在 Flowio 内编辑标题/备注时
                    // 会生成多余 type 步骤。录制自身模式下不过滤。
                    if !record_self && recorder::pid_is_flowio(fg_pid) {
                        eprintln!("[BG] self-filter: skipping keystroke in flowio window (pid={})", fg_pid);
                        continue;
                    }

                    // Target-app filter using PID captured at hook time (录制指定软件)
                    if let Some(ref target) = target_window {
                        if !recorder::pid_is_target(fg_pid, target) {
                            eprintln!(
                                "[BG] target-filter: skipping keystroke (fg_pid={} != {})",
                                fg_pid, target
                            );
                            continue;
                        }
                    }

                    // 修饰键自身不是"操作"。此前按下 Alt 会先生成一个
                    // "快捷键 Alt+Alt" 步骤，再按 Tab 又生成一个
                    // "快捷键 Alt+Tab" —— 用户看到的就是两个步骤。
                    if is_pure_modifier(&key_name) {
                        continue;
                    }

                    // Scribe-style combo-key semantics: Ctrl/Alt/Win combinations are
                    // never typed characters. They become copy/paste/hotkey steps,
                    // and their letter keys never pollute the type buffer.
                    // ⚠️ meta（Win 键）优先于 ctrl/alt 判定：这样 Win+Ctrl+Arrow
                    // 这类带 Win 的组合能走 Win 分支拿到正确标签，而不是被当成
                    // 普通 Ctrl+Arrow。
                    if ctrl || alt || meta {
                        // 使用真实按键时刻（钩子触发瞬间），不是 channel 消费时刻。
                        // 关键：连按 Ctrl+A→C→V 时三个事件一起被处理，若用消费时刻
                        // 三者 combo_ts 几乎相同，closest_to 都拿最"新"的帧（即
                        // Ctrl+V 之后的画面），全选步骤就会拍到粘贴后的内容。
                        let combo_ts = key_instant;
                        // 结构化组合键串：重放端据此下发，避免去解析不可靠的 title 文本。
                        let combo = combo_string(ctrl, alt, meta, shift, &key_name);
                        // Flush any pending keystrokes first so the combo is not
                        // merged into the preceding type step.
                        if !accumulated_keys.is_empty() {
                            let typed = std::mem::take(&mut accumulated_keys);
                            let win_title = flush_time_window_title();
                            flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time, false);
                        }
                        typing_window_title = None;
                        keyboard_before_path = None;
                        last_uia_text = None;
                        can_merge_type = false; // 快捷键打断输入连续性

                        // ★ 过滤 Flowio 自身控制热键：Alt+Shift+S（录制/停止）、
                        // Alt+Shift+P（暂停）、Alt+Shift+E（停止）。
                        // 这些是录步自己的全局热键，不应被录进教程里。
                        // combo_string 格式："Alt+Shift+KeyS" 等。
                        if (alt && shift) && matches!(key_name.as_str(), "KeyS" | "KeyP" | "KeyE") {
                            eprintln!(
                                "[BG] control-hotkey filter: skipping Flowio own hotkey {} ({})",
                                combo, key_name
                            );
                            continue;
                        }

                        let _pushed: Option<usize> = if meta {
                            // Windows 键组合键（Win+D / Win+E / …）：之前因缺少
                            // meta 追踪，Win+D 的 "D" 会漏进 type buffer 被记成字符
                            // "d"。现在统一识别成有意义的快捷键步骤。
                            match win_shortcut(shift, ctrl, &key_name) {
                                Some((title, settle)) => flush_shortcut_step(
                                    "hotkey",
                                    title,
                                    String::new(),
                                    &cs_for_thread,
                                    settle,
                                    &ring,
                                    combo_ts,
                                    &combo,
                                ),
                                None => {
                                    let title = format!("快捷键 Win+{}", display_shortcut_key(&key_name));
                                    flush_shortcut_step(
                                        "hotkey",
                                        title,
                                        String::new(),
                                        &cs_for_thread,
                                        ShortcutSettle::visual(450),
                                        &ring,
                                        combo_ts,
                                        &combo,
                                    )
                                }
                            }
                        } else if ctrl {
                            match key_name.as_str() {
                                // 全选：剪贴板不变，必须读焦点控件选中文本。
                                // 截图不等固定 450ms —— 那时用户可能已经按了
                                // Ctrl+C/Ctrl+V，全选步骤就变成了粘贴后的画面。
                                // 改为轮询"画面一变就拍"，拍到的正是高亮选区。
                                "KeyA" => flush_shortcut_step(
                                    "hotkey",
                                    "Ctrl+A 全选".to_string(),
                                    String::new(),
                                    &cs_for_thread,
                                    ShortcutSettle::select_all(
                                        600,
                                        BackfillSource::FocusedSelection,
                                    ),
                                    &ring,
                                    combo_ts,
                                    &combo,
                                ),
                                // 复制：剪贴板更新需要时间，但屏幕不会变。
                                // settle 阶段**不再重截**（否则 450ms 后截到的是
                                // 用户已按下的 Ctrl+V 的目标区域），只读剪贴板
                                // 文字填 description；截图保留按键时刻那一帧。
                                "KeyC" => flush_shortcut_step(
                                    "copy",
                                    "Ctrl+C 复制".to_string(),
                                    String::new(),
                                    &cs_for_thread,
                                    ShortcutSettle::copy(),
                                    &ring,
                                    combo_ts,
                                    &combo,
                                ),
                                // 粘贴：屏幕会变（粘进去的文字），需要 settle
                                // 重截。锚点由 settle 时的焦点元素决定（焦点还
                                // 在粘贴目标上）。
                                "KeyV" => flush_shortcut_step(
                                    "paste",
                                    "Ctrl+V 粘贴".to_string(),
                                    String::new(),
                                    &cs_for_thread,
                                    ShortcutSettle::paste(),
                                    &ring,
                                    combo_ts,
                                    &combo,
                                ),
                                // 剪切：屏幕不会变（剪贴板变了），同 Ctrl+C 逻辑。
                                "KeyX" => flush_shortcut_step(
                                    "hotkey",
                                    "Ctrl+X 剪切".to_string(),
                                    String::new(),
                                    &cs_for_thread,
                                    ShortcutSettle::copy(),
                                    &ring,
                                    combo_ts,
                                    &combo,
                                ),
                                // 查找：浏览器搜索框是滑入动画，500ms 后重拍才拍得到。
                                "KeyF" => flush_shortcut_step(
                                    "hotkey",
                                    "Ctrl+F 查找".to_string(),
                                    String::new(),
                                    &cs_for_thread,
                                    ShortcutSettle::visual(500),
                                    &ring,
                                    combo_ts,
                                    &combo,
                                ),
                                "KeyZ" => flush_shortcut_step("hotkey", "Ctrl+Z 撤销".to_string(), String::new(), &cs_for_thread, ShortcutSettle::visual(450), &ring, combo_ts, &combo),
                                "KeyY" => flush_shortcut_step("hotkey", "Ctrl+Y 重做".to_string(), String::new(), &cs_for_thread, ShortcutSettle::visual(450), &ring, combo_ts, &combo),
                                "KeyS" => flush_shortcut_step("hotkey", "Ctrl+S 保存".to_string(), String::new(), &cs_for_thread, ShortcutSettle::visual(450), &ring, combo_ts, &combo),
                                _ => {
                                    let title = common_combo_label(ctrl, alt, shift, &key_name)
                                        .unwrap_or_else(|| format!("快捷键 Ctrl+{}", display_shortcut_key(&key_name)));
                                    flush_shortcut_step("hotkey", title, String::new(), &cs_for_thread, ShortcutSettle::visual(450), &ring, combo_ts, &combo)
                                }
                            }
                        } else if alt {
                            if key_name == "KeyTab" {
                                // Alt+Tab 会被按住 Alt 连按多次（每按一次切一个
                                // 窗口），这里合并成一步：每次按 Tab 都重排一次
                                // 「Alt 抬起后」的标题 + 截图更新，最终停留的
                                // 就是用户真正切到的那个窗口。
                                let now = Instant::now();
                                let merged = match last_alt_tab {
                                    Some((last_ts, idx)) if now.duration_since(last_ts) < Duration::from_millis(2500) => {
                                        Some(idx)
                                    }
                                    _ => None,
                                };
                                match merged {
                                    Some(idx) => {
                                        last_alt_tab = Some((now, idx));
                                        schedule_alt_tab_settle(idx, &cs_for_thread);
                                        Some(idx)
                                    }
                                    None => {
                                        let idx = flush_shortcut_step(
                                            "hotkey",
                                            "快捷键 Alt+Tab".to_string(),
                                            String::new(),
                                            &cs_for_thread,
                                            ShortcutSettle::switch(),
                                            &ring,
                                            combo_ts,
                                            &combo,
                                        );
                                        if let Some(i) = idx {
                                            last_alt_tab = Some((now, i));
                                            schedule_alt_tab_settle(i, &cs_for_thread);
                                        }
                                        idx
                                    }
                                }
                            } else {
                                let title = common_combo_label(ctrl, alt, shift, &key_name)
                                    .unwrap_or_else(|| format!("快捷键 Alt+{}", display_shortcut_key(&key_name)));
                                flush_shortcut_step("hotkey", title, String::new(), &cs_for_thread, ShortcutSettle::visual(450), &ring, combo_ts, &combo)
                            }
                        } else {
                            None
                        };
                        last_key_time = Instant::now();
                        continue;
                    }

                    // IME 组合中：此刻的按键是拼音/选词，不是最终字符。
                    // 真正的中文在「组合结束」那一刻由循环顶部的 IME 检测提交，
                    // 这里只刷新时间避免误触发 idle flush，然后跳过。
                    if ime::is_ime_composing() {
                        last_key_time = Instant::now();
                        continue;
                    }

                    ensure_typing_started(
                        &mut accumulated_keys,
                        &mut typing_window_title,
                        &mut keyboard_before_path,
                    );

                    // Navigation keys (Enter/Tab/Escape): flush pending keystrokes
                    // immediately and do NOT enter the type buffer (an Enter would
                    // otherwise be modelled as a stray "\n" input character).
                    if is_nav_key(&key_name) {
                        if !accumulated_keys.is_empty() {
                            let final_uia = screenshot::get_focused_element_text()
                                .filter(|(t, _)| !t.is_empty())
                                .or(last_uia_text.take());
                            eprintln!(
                                "[BG] nav-key flush: buffer='{}' uia='{}' key='{}'",
                                accumulated_keys,
                                final_uia
                                    .as_ref()
                                    .map(|(t, _)| t.as_str())
                                    .unwrap_or("(none)"),
                                key_name
                            );
                            let typed = std::mem::take(&mut accumulated_keys);
                            let win_title = flush_time_window_title();
                            flush_key_step(&typed, Some(win_title), &cs_for_thread, final_uia, keyboard_before_path.take(), &ring, last_key_time, false);
                        }
                        // ★ 修复：导航键本身必须有可重放步骤，否则「回车确认保存 /
                        // Tab 切字段 / Esc 取消」这类动作在重放时整段丢失——典型表现
                        // 就是输完文件名却不回车 → 文件永远建不出来（用户报的「文件未生成」）。
                        let nav_label = match key_name.as_str() {
                            "Enter" | "Return" | "KeyEnter" => "Enter 确认".to_string(),
                            "Tab" | "KeyTab" => "Tab 切换焦点".to_string(),
                            "Escape" => "Esc 取消".to_string(),
                            other => display_shortcut_key(other),
                        };
                        let nav_combo = display_shortcut_key(&key_name); // "Enter"/"Tab"/"Esc"，供 replay 端 press_combo
                        flush_shortcut_step(
                            "hotkey",
                            nav_label,
                            String::new(),
                            &cs_for_thread,
                            ShortcutSettle::visual(300),
                            &ring,
                            key_instant,
                            &nav_combo,
                        );
                        typing_window_title = None;
                        keyboard_before_path = None;
                        last_uia_text = None;
                        can_merge_type = false; // Enter/Tab/Esc 切换了输入上下文
                        last_key_time = Instant::now();
                        continue;
                    }

                    // 功能键（F1–F12）/ 截图键 无修饰键时，ToUnicodeEx 不产出
                    // 字符，旧逻辑把它们当空白字符丢弃 → 录制里看不到 F5 刷新、
                    // PrintScreen 截图这类操作。这里补成快捷键步骤（先 flush）。
                    if let Some(label) = function_key_label(&key_name)
                        .or_else(|| standalone_key_label(&key_name))
                        .or_else(|| action_key_label(&key_name))
                    {
                        if !accumulated_keys.is_empty() {
                            let typed = std::mem::take(&mut accumulated_keys);
                            let win_title = flush_time_window_title();
                            flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time, false);
                        }
                        typing_window_title = None;
                        keyboard_before_path = None;
                        last_uia_text = None;
                        can_merge_type = false;
                        eprintln!("[BG] function-key step: {}", label);
                        flush_shortcut_step(
                            "hotkey",
                            label,
                            String::new(),
                            &cs_for_thread,
                            ShortcutSettle::visual(450),
                            &ring,
                            key_instant,
                            &display_shortcut_key(&key_name),
                        );
                        last_key_time = Instant::now();
                        continue;
                    }

                    // 用 ToUnicodeEx 解析「真实输入字符」：符号/大小写/布局都正确。
                    // 若 ToUnicodeEx 无结果（某些 OEM/区域键），回退到旧的 key_to_char。
                    let char_str: String = {
                        let s = keyboard::to_unicode_char(vk, scan, shift, ctrl, alt, hkl);
                        if s.is_empty() {
                            let fb = keyboard::key_to_char(&key_name);
                            if fb.starts_with('[') {
                                String::new()
                            } else {
                                fb
                            }
                        } else {
                            s
                        }
                    };
                    match apply_key_to_buffer(&mut accumulated_keys, &key_name, &char_str) {
                        BufferEffect::Input => {
                            last_key_time = Instant::now();
                            // Cache UIA text while focus is still on the input
                            if let Some((text, sensitive)) = screenshot::get_focused_element_text() {
                                if !text.is_empty() {
                                    last_uia_text = Some((text, sensitive));
                                }
                            }
                        }
                        BufferEffect::Backspace => {
                            // Refresh UIA text (input field value after backspace)
                            last_uia_text = screenshot::get_focused_element_text()
                                .filter(|(t, _)| !t.is_empty());
                        }
                        BufferEffect::Ignored => {}
                    }
                }
                Ok(KeyEvent::Release) => {}
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    eprintln!("[BG] keyboard channel disconnected");
                    break;
                }
            }

            // ── Idle flush (1500ms) ────────────────────────
            if !accumulated_keys.is_empty() && last_key_time.elapsed() > flush_interval {
                eprintln!(
                    "[BG] idle flush: buffer='{}' uia='{}' after {:?}",
                    accumulated_keys,
                    last_uia_text
                        .as_ref()
                        .map(|(t, _)| t.as_str())
                        .unwrap_or("(none)"),
                    last_key_time.elapsed()
                );
                let typed = std::mem::take(&mut accumulated_keys);
                let win_title = flush_time_window_title();
                flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time, can_merge_type);
                // P1 降噪：本步允许下一轮输入继续并进来（连续停顿不拆步）
                can_merge_type = true;
            }

            // ── Foreground window change flush ─────────────
            let current_title = activity::get_active_window_title();
            if !accumulated_keys.is_empty() {
                if let (Some(ref last), Some(ref curr)) = (&last_window_title, &current_title) {
                    let since_last_key = last_key_time.elapsed();
                    if should_flush_on_window_change(Some(last), Some(curr), since_last_key) {
                        eprintln!(
                            "[BG] window change flush: buffer='{}' uia='{}' ({} → {})",
                            accumulated_keys,
                            last_uia_text
                                .as_ref()
                                .map(|(t, _)| t.as_str())
                                .unwrap_or("(none)"),
                            last, curr
                        );
                        let typed = std::mem::take(&mut accumulated_keys);
                        let win_title = flush_time_window_title();
                        flush_key_step(&typed, Some(win_title), &cs_for_thread, last_uia_text.take(), keyboard_before_path.take(), &ring, last_key_time, false);
                        can_merge_type = false; // 切窗打断输入连续性
                    } else if last != curr {
                        // Guard: don't flush on window-change within 500ms of last keystroke.
                        // Chrome auto-completes can change the title immediately after typing;
                        // waiting gives more keystrokes time to accumulate.
                        eprintln!(
                            "[BG] window change suppressed ({}ms since last key): buffer='{}' ({} → {})",
                            since_last_key.as_millis(),
                            accumulated_keys,
                            last, curr
                        );
                    }
                }
            }
            last_window_title = current_title;

            std::thread::sleep(Duration::from_millis(50));
        }

        eprintln!("[BG] event thread exiting");
    })
}

// ── Tauri commands ─────────────────────────────────────────────────────

/// 组装手动标记截图步骤（mark_screenshot_step 的 StepData 构造，便于单测）。
fn build_mark_step(
    title: String,
    x: Option<i32>,
    y: Option<i32>,
    action_type: Option<String>,
    text: Option<String>,
    screenshot_path: Option<String>,
    after_screenshot_path: Option<String>,
    step_index: i32,
    timestamp: i64,
) -> StepData {
    StepData {
        id: Uuid::new_v4().to_string(),
        order_index: step_index,
        step_number: step_index + 1,
        action_type: action_type.unwrap_or_else(|| "screenshot".to_string()),
        title,
        description: text.unwrap_or_default(),
        tip: None,
        screenshot_path: screenshot_path.unwrap_or_default(),
        screenshot_base64: None,
        annotated_path: String::new(),
        before_screenshot_path: String::new(),
        x,
        y,
        crop_w: None,
        crop_h: None,
        after_screenshot_path,
        timestamp,
        window_title: String::new(),
        hotkey_combo: String::new(),
        uia_anchor: String::new(),
        mouse_button: String::new(),
        is_sensitive: false,
    }
}

/// StepData 列表 → 数据库 StepSave 列表（finish_recording 持久化映射）。
fn steps_to_step_saves(steps: &[StepData]) -> Vec<crate::db::StepSave> {
    steps
        .iter()
        .map(|s| crate::db::StepSave {
            order_index: s.order_index,
            step_number: s.step_number,
            title: s.title.clone(),
            description: s.description.clone(),
            action_type: s.action_type.clone(),
            tip: s.tip.clone().unwrap_or_default(),
            screenshot_path: s.screenshot_path.clone(),
            x: s.x,
            y: s.y,
            crop_w: s.crop_w,
            crop_h: s.crop_h,
            before_screenshot_path: s.before_screenshot_path.clone(),
            after_screenshot_path: s.after_screenshot_path.clone(),
            timestamp: s.timestamp,
            window_title: s.window_title.clone(),
            hotkey_combo: s.hotkey_combo.clone(),
            uia_anchor: s.uia_anchor.clone(),
            mouse_button: s.mouse_button.clone(),
        })
        .collect()
}

/// 空标题回退为当前时间格式的录制标题。
fn recording_title_or_default(title: &str) -> String {
    if title.is_empty() {
        chrono::Local::now()
            .format("录制 %Y-%m-%d %H:%M:%S")
            .to_string()
    } else {
        title.to_string()
    }
}

/// 组装 RecordingSave 并生成描述性 rec_id。
fn build_recording_save(
    id: String,
    title: &str,
    steps: &[StepData],
    target_app: Option<String>,
) -> (crate::db::RecordingSave, String) {
    let saves = steps_to_step_saves(steps);
    let step_count = saves.len();
    let save = crate::db::RecordingSave {
        id: id.clone(),
        title: recording_title_or_default(title),
        app_name: String::new(),
        target_app,
        steps: saves,
    };
    (save, format!("{} ({} steps)", id, step_count))
}

/// 是否为活动录制状态（Recording 或 Paused）。
fn is_active_recording_state(state: &RecordingState) -> bool {
    *state == RecordingState::Recording || *state == RecordingState::Paused
}

/// Enumerate visible top-level windows of running apps (录制指定软件 picker).
#[tauri::command]
pub fn list_target_windows() -> Vec<recorder::TargetWindow> {
    recorder::list_target_windows()
}

#[tauri::command]
pub fn start_recording(
    recording_manager: State<RecordingManager>,
    record_self: Option<bool>,
    target_window: Option<String>,
) -> Result<String, String> {
    let record_self = record_self.unwrap_or(false);
    // Normalize empty target to None (录制指定软件).
    //
    // ⚠️ 不再自动识别前台进程作为 target：target_window 同时控制**截图范围**
    // 和**事件过滤**（spawn_event_thread 会丢弃非目标进程的点击/键盘事件）。
    // 自动把「录制开始时恰好在前台的那个进程」设为 target，会导致后续跨软件
    // 操作（浏览器下载 → 安装程序 → 系统设置）的窗口和事件双双丢失。
    // 真实事故：Python 安装教程的安装向导窗口全部没录到。
    //
    // 现在：只有用户在设置里手动选择「录制指定软件」才进 target 模式
    // （localStorage 持久化，选一次后续录制都生效）；否则一律全屏录制。
    let target_window = target_window.filter(|t| !t.trim().is_empty());

    // --- 防重入：已有活跃录制时直接返回它的 id ---
    // 不拦的话，重复调用会再装一次 native hook、再起一个截图线程和一个事件线程。
    // 两个 WH_MOUSE_LL 并存时每次点击会被投递两遍，最终每个操作记成两步。
    let existing_active: Option<String> = {
        let manager = recording_manager
            .0
            .lock()
            .map_err(|e| e.to_string())?;
        manager
            .iter()
            .find(|(_, r)| is_active_recording_state(&r.state))
            .map(|(id, _)| id.clone())
    };
    if let Some(active_id) = existing_active {
        eprintln!(
            "[start] a recording is already active ({}); returning its id instead of starting a new one",
            active_id
        );
        return Ok(active_id);
    }

    let id = Uuid::new_v4().to_string();

    // --- Set up hook channels ---
    let (mouse_tx, mouse_rx) = mpsc::channel::<CapturedClick>();
    let (key_tx, key_rx) = mpsc::channel::<KeyEvent>();

    // Shared running flag
    let running = Arc::new(AtomicBool::new(true));
    let running_for_hook = Arc::clone(&running);

    // Native Windows hooks (WH_KEYBOARD_LL + WH_MOUSE_LL)
    // Reset modifier tracking flags so a stuck Alt/Ctrl from a previous session
    // does not turn every keystroke into a fake combo (e.g. "Alt+A" when the
    // user only pressed "A").
    hook::reset_modifier_flags();
    hook::init_hooks(key_tx, mouse_tx, running_for_hook);

    // --- Start continuous capture thread (4 FPS) ---
    let capture_running = Arc::clone(&running);
    let (frame_tx, frame_rx) = mpsc::channel::<capture::CapturedFrame>();
    capture::start_capture_thread(
        capture_running,
        frame_tx,
        Duration::from_millis(250),
        target_window.clone(),
    );

    // --- Create recorder ---
    let mut recorder = crate::recorder::Recorder::new(id.clone(), String::new());
    recorder.record_self = record_self;
    recorder.target_window = target_window.clone();
    recorder.start()?;

    // --- Shared captured-steps buffer ---
    let captured_steps: Arc<Mutex<Vec<StepData>>> = Arc::new(Mutex::new(Vec::new()));
    let cs_for_thread = Arc::clone(&captured_steps);

    // Invalidate anything left over from the previous session's settle threads.
    bump_settle_epoch();

    let running_clone = Arc::clone(&running);

    let handle = spawn_event_thread(
        running_clone,
        mouse_rx,
        key_rx,
        frame_rx,
        cs_for_thread,
        record_self,
        target_window,
    );

    recorder.event_thread = Some(handle);
    recorder.running_flag = Some(running);
    recorder.captured_steps = Some(captured_steps);

    // ── a11y 预热（后台线程）──────────────────────────────
    // Chromium 的无障碍树是惰性激活的：录制刚开始时树往往还没建好，
    // 会导致**第一次** Ctrl+C 拿不到精确选区矩形（退化到 OCR/屏幕中心兜底）。
    // 录制一开始就后台探测，等用户真正复制时树已就绪，首查即命中。
    // 激活是进程级且持久的，命中后线程立即退出，不做无谓轮询（纯只读查询，无副作用）。
    std::thread::spawn(move || {
        screenshot::warmup_uia_text_pattern(40, 500);
    });

    // --- Store in manager ---
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        manager.insert(id.clone(), recorder);
    }

    Ok(id)
}

#[tauri::command]
pub fn pause_recording(
    recording_manager: State<RecordingManager>,
    id: String,
) -> Result<(), String> {
    let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    let recorder = manager
        .get_mut(&id)
        .ok_or_else(|| format!("Recording {} not found", id))?;
    recorder.pause()
}

#[tauri::command]
pub fn resume_recording(
    recording_manager: State<RecordingManager>,
    id: String,
) -> Result<(), String> {
    // Step 1: Change state to Recording
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        let recorder = manager
            .get_mut(&id)
            .ok_or_else(|| format!("Recording {} not found", id))?;
        recorder.resume()?;
    }

    // Re-read the target window (录制指定软件) so the restarted capture/event
    // threads keep cropping & filtering to the same app.
    let target_window = {
        let manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        manager
            .get(&id)
            .and_then(|r| r.target_window.clone())
            .filter(|t| !t.trim().is_empty())
    };

    // Step 2: Restart background threads (outside the lock)
    let (mouse_tx, mouse_rx) = mpsc::channel::<CapturedClick>();
    let (key_tx, key_rx) = mpsc::channel::<KeyEvent>();
    let running = Arc::new(AtomicBool::new(true));
    let running_for_hook = Arc::clone(&running);

    hook::init_hooks(key_tx, mouse_tx, running_for_hook);

    let capture_running = Arc::clone(&running);
    let (frame_tx, frame_rx) = mpsc::channel::<capture::CapturedFrame>();
    capture::start_capture_thread(
        capture_running,
        frame_tx,
        Duration::from_millis(250),
        target_window.clone(),
    );

    let running_clone = Arc::clone(&running);

    // Step 3: Update recorder with new thread handles
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        let recorder = manager
            .get_mut(&id)
            .ok_or_else(|| format!("Recording {} not found", id))?;

        // Step 3a: 先把暂停前已捕获的步骤排入 recorder.steps，再新建空 captured_steps
        // 避免 finish_recording 时旧步骤被再次 extend 进 recorder.steps（成对重复）。
        if let Some(ref cs) = recorder.captured_steps {
            if let Ok(mut buf) = cs.lock() {
                let base_idx = recorder.steps.len() as i32;
                let drained: Vec<StepData> = buf.drain(..).collect();
                let drained_count = drained.len();
                let reindexed = reindex_steps(drained, base_idx);
                let reindexed_count = reindexed.len();
                recorder.steps.extend(reindexed);
                eprintln!(
                    "[resume] drained {} steps from previous session into recorder.steps, total now {}",
                    reindexed_count,
                    recorder.steps.len()
                );
            }
        }
        let captured_steps = Arc::new(Mutex::new(Vec::new()));
        let cs_for_thread = Arc::clone(&captured_steps);

        let handle = spawn_event_thread(
            running_clone,
            mouse_rx,
            key_rx,
            frame_rx,
            cs_for_thread,
            recorder.record_self,
            target_window,
        );

        recorder.running_flag = Some(running);
        recorder.event_thread = Some(handle);
        recorder.captured_steps = Some(captured_steps);
    }

    Ok(())
}

#[tauri::command]
pub fn finish_recording(
    recording_manager: State<RecordingManager>,
    db: State<crate::DbState>,
    id: String,
) -> Result<(), String> {
    eprintln!("[finish] step0: finish_recording called for id={}", id);

    // --- Block 0: 幂等保护 ---
    // 该 id 已不在 manager 中，说明本次录制已经被 finish / cancel 处理过。
    // 重复调用应当视为成功（幂等），而不是报错——否则用户会看到
    // "停止录制失败: Recording xxx not found" 却不知道录制其实已经保存好了。
    {
        let manager = recording_manager
            .0
            .lock()
            .map_err(|e| e.to_string())?;
        if !manager.contains_key(&id) {
            eprintln!(
                "[finish] id={} not in manager; treating as already finished (idempotent Ok)",
                id
            );
            return Ok(());
        }
    }

    // --- Block 1: Stop event thread + join ---
    eprintln!("[finish] step1: stopping event thread...");
    {
        let mut manager = recording_manager
            .0
            .lock()
            .map_err(|e| {
                eprintln!("[finish] step1 FAIL: lock manager: {}", e);
                e.to_string()
            })?;
        let recorder = manager.get_mut(&id).ok_or_else(|| {
            let msg = format!("Recording {} not found", id);
            eprintln!("[finish] step1 FAIL: {}", msg);
            msg
        })?;

        if let Some(ref flag) = recorder.running_flag {
            flag.store(false, Ordering::Relaxed);
        }

        let thread_handle = recorder.event_thread.take();
        recorder.finish().map_err(|e| {
            eprintln!("[finish] step1 FAIL: recorder.finish(): {}", e);
            e
        })?;
        drop(manager);

        if let Some(handle) = thread_handle {
            eprintln!("[BG] waiting for event thread to finish...");
            let _ = handle.join();
            eprintln!("[BG] event thread joined");
        }
    }
    eprintln!("[finish] step1 OK: event thread stopped");

    // --- Block 2: Drain captured_steps (AFTER event thread fully exited) ---
    eprintln!("[finish] step2: draining captured_steps...");
    {
        let mut manager = recording_manager
            .0
            .lock()
            .map_err(|e| {
                eprintln!("[finish] step2 FAIL: lock manager: {}", e);
                e.to_string()
            })?;
        let recorder = manager.get_mut(&id).ok_or_else(|| {
            let msg = format!("Recording {} not found after join", id);
            eprintln!("[finish] step2 FAIL: {}", msg);
            msg
        })?;

        if let Some(cs) = recorder.captured_steps.take() {
            let mut buf = cs.lock().map_err(|e| {
                eprintln!("[finish] step2 FAIL: lock captured_steps: {}", e);
                e.to_string()
            })?;
            let drained_count = buf.len();
            let base_idx = recorder.steps.len() as i32;
            let reindexed = reindex_steps(buf.drain(..).collect(), base_idx);
            recorder.steps.extend(reindexed);
            eprintln!(
                "[finish] step2 OK: drained {} steps, total now {} steps",
                drained_count,
                recorder.steps.len()
            );
        } else {
            eprintln!("[finish] step2 OK: no captured_steps to drain");
        }
    }

    // --- Block 3: Build RecordingSave → save to DB → remove from manager ---
    eprintln!("[finish] step3: building RecordingSave and saving to DB...");
    let (recording_save, rec_id) = {
        let manager = recording_manager.0.lock().map_err(|e| {
            eprintln!("[finish] step3 FAIL: lock manager: {}", e);
            e.to_string()
        })?;
        let recorder = manager.get(&id).ok_or_else(|| {
            let msg = format!("Recording {} not found after join", id);
            eprintln!("[finish] step3 FAIL: {}", msg);
            msg
        })?;

        build_recording_save(
            recorder.id.clone(),
            &recorder.title,
            &recorder.steps,
            recorder.target_window.clone(),
        )
    };

    let conn = db.0.lock().map_err(|e| {
        eprintln!("[finish] step3 FAIL: lock db: {}", e);
        e.to_string()
    })?;
    crate::db::save_recording_to_db(&conn, &recording_save).map_err(|e| {
        eprintln!("[finish] step3 FAIL: save_recording_to_db: {}", e);
        e
    })?;
    eprintln!("[finish] step3 OK: saved to DB (rec_id: {})", rec_id);

    // --- Block 3.5: 规则降噪（自动合并，2026-09-03）---
    // 用户要求「不要手动合并，录完自动就是干净的」。规则层是纯本地计算
    // （不联网、不调 AI），几十步 <1ms，失败也不影响录制已落库，因此
    // 只记日志不上抛。弹框里那一串点击要靠编辑器里的「AI 精简」按钮。
    // 精简前压全局撤销栈：进编辑器按 Ctrl+Z 就能回到精简前的原始步骤。
    let _ = crate::editor::commands::push_edit_history(&conn, &rec_id);
    match crate::editor::commands::auto_merge_steps_conn(&conn, &rec_id) {
        Ok(r) if r.reduced > 0 => eprintln!(
            "[finish] step3.5 OK: 自动精简 {} → {} 步（省 {} 步）",
            r.before, r.after, r.reduced
        ),
        Ok(_) => eprintln!("[finish] step3.5 OK: 无需自动精简"),
        Err(e) => eprintln!("[finish] step3.5 WARN: 自动精简失败（录制已保存）: {}", e),
    }

    // Remove from manager
    {
        let mut manager = recording_manager.0.lock().map_err(|e| {
            eprintln!("[finish] cleanup FAIL: lock manager: {}", e);
            e.to_string()
        })?;
        manager.remove(&id);
    }
    eprintln!("[finish] all OK: recording {} removed from manager", id);

    Ok(())
}

#[tauri::command]
pub fn cancel_recording(
    recording_manager: State<RecordingManager>,
    id: String,
) -> Result<(), String> {
    {
        let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
        let recorder = manager
            .get_mut(&id)
            .ok_or_else(|| format!("Recording {} not found", id))?;

        if let Some(ref flag) = recorder.running_flag {
            flag.store(false, Ordering::Relaxed);
        }

        let thread_handle = recorder.event_thread.take();
        let _ = recorder.captured_steps.take();
        recorder.cancel()?;
        recorder.steps.clear();

        drop(manager);

        if let Some(handle) = thread_handle {
            let _ = handle.join();
        }
    }

    Ok(())
}

#[tauri::command]
pub fn mark_screenshot_step(
    recording_manager: State<RecordingManager>,
    id: String,
    title: String,
    x: Option<i32>,
    y: Option<i32>,
    action_type: Option<String>,
    text: Option<String>,
    screenshot_path: Option<String>,
    after_screenshot_path: Option<String>,
) -> Result<(), String> {
    let now_ms = current_time_millis();

    let mut manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    let recorder = manager
        .get_mut(&id)
        .ok_or_else(|| format!("Recording {} not found", id))?;

    let step_index = recorder.steps.len() as i32;
    let step = build_mark_step(
        title,
        x,
        y,
        action_type,
        text,
        screenshot_path,
        after_screenshot_path,
        step_index,
        now_ms,
    );

    recorder.push_step(step)
}

#[tauri::command]
pub fn get_active_recording(
    recording_manager: State<RecordingManager>,
) -> Result<Option<serde_json::Value>, String> {
    let manager = recording_manager.0.lock().map_err(|e| e.to_string())?;
    for (id, recorder) in manager.iter() {
        if is_active_recording_state(&recorder.state) {
            return Ok(Some(serde_json::json!({
                "id": id,
                "is_paused": recorder.state == RecordingState::Paused
            })));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod commands_tests {
    use super::*;

    // ── drag_crop_box（drag 裁图上下文外扩） ────────────────────

    #[test]
    fn encode_mouse_button_handles_hook_casing() {
        // 2026-09-08 事故回归：hook 曾发 "Right"/"Left"（首字母大写），
        // 编码端只认小写 → 右键/双击全部静默退化为 left。
        // 契约：无论钩子发什么大小写，编码必须正确。
        assert_eq!(encode_mouse_button("Right", false), "right");
        assert_eq!(encode_mouse_button("RIGHT", false), "right");
        assert_eq!(encode_mouse_button("right", false), "right");
        assert_eq!(encode_mouse_button("Left", false), "left");
        assert_eq!(encode_mouse_button("left", false), "left");
        assert_eq!(encode_mouse_button("Left", true), "left-double");
        assert_eq!(encode_mouse_button("Right", true), "right-double");
        // 未知按键（如中键）按左键处理，绝不产生 "middle"
        assert_eq!(encode_mouse_button("Middle", false), "left");
    }

    #[test]
    fn drag_crop_box_wide_selection_gets_vertical_context() {
        // 用户实测场景：表格里横向拖选 ~1000×100 的列标条。
        // 100% 外扩（pad=100/侧）→ 宽 1200 饱和到全图 1920，
        // 高度走最小视野 520 → 能带出多行数据上下文（旧值 240 只够表头）。
        let (l, t, w, h) = drag_crop_box(400, 100, 1000, 100, 1920, 1080);
        assert_eq!((l, t, w, h), (0, 0, 1920, 520));
        // 选区中心仍应在框内
        assert!(t <= 150 && 150 <= t + h);
    }

    #[test]
    fn drag_crop_box_small_selection_gets_min_context() {
        // 小选区（100×50）：100% 外扩不够，最小视野 560×520 兜底
        let (l, t, w, h) = drag_crop_box(500, 300, 100, 50, 1920, 1080);
        assert_eq!((w, h), (560, 520));
        // 中心对齐选区中心 (550, 325)
        assert_eq!((l, t), (550 - 280, 325 - 260));
    }

    #[test]
    fn drag_crop_box_zero_selection_still_crops() {
        // 按下即松开（bw=bh=0）：pad 回落 40，最小视野兜底，不产生 0 尺寸
        let (l, t, w, h) = drag_crop_box(960, 540, 0, 0, 1920, 1080);
        assert_eq!((w, h), (560, 520));
        assert_eq!((l, t), (960 - 280, 540 - 260));
    }

    #[test]
    fn drag_crop_box_clamps_into_image_and_saturates() {
        // 选区贴左上角：框平移进图内，不出界
        let (l, t, w, h) = drag_crop_box(0, 0, 50, 30, 1920, 1080);
        assert_eq!((l, t), (0, 0));
        assert!(w > 0 && h > 0);
        // 选区比图还大：饱和到全图
        let (l2, t2, w2, h2) = drag_crop_box(0, 0, 5000, 3000, 1920, 1080);
        assert_eq!((l2, t2, w2, h2), (0, 0, 1920, 1080));
    }

    #[test]
    fn drag_crop_box_never_panics_on_degenerate_image() {
        // 退化图尺寸不 panic
        let _ = drag_crop_box(10, 10, 5, 5, 0, 0);
        let _ = drag_crop_box(10, 10, 5, 5, 1, 1);
    }

    fn make_step(order_index: i32) -> StepData {
        StepData {
            id: format!("s{}", order_index),
            order_index,
            step_number: order_index + 1,
            action_type: "click".into(),
            title: "t".into(),
            description: String::new(),
            tip: None,
            screenshot_path: String::new(),
            screenshot_base64: None,
            annotated_path: String::new(),
            before_screenshot_path: String::new(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            after_screenshot_path: None,
            timestamp: 0,
            window_title: String::new(),
            is_sensitive: false,
            hotkey_combo: String::new(),
            uia_anchor: String::new(),
            mouse_button: String::new(),
        }
    }

    // ── resolve_step_description（文本优先级）────────────────────────

    #[test]
    fn description_prefers_pre_captured_uia() {
        let d = resolve_step_description(Some("搜索框内容"), Some("当前焦点"), "typed");
        assert_eq!(d, "搜索框内容");
    }

    #[test]
    fn description_filters_empty_pre_captured() {
        let d = resolve_step_description(Some(""), Some("当前焦点"), "typed");
        assert_eq!(d, "当前焦点");
    }

    #[test]
    fn description_falls_back_to_keystroke_when_both_uia_missing() {
        assert_eq!(resolve_step_description(None, None, "hello"), "hello");
        assert_eq!(resolve_step_description(Some(""), None, "hello"), "hello");
        assert_eq!(resolve_step_description(None, Some(""), "hello"), "hello");
    }

    #[test]
    fn description_prefers_current_over_keystroke() {
        let d = resolve_step_description(None, Some("地址栏"), "baidu.com");
        assert_eq!(d, "地址栏");
    }

    // ── resolve_typed_title ─────────────────────────────────────────

    #[test]
    fn typed_title_with_window() {
        assert_eq!(
            resolve_typed_title(Some("Chrome"), AppDomain::Other),
            "在「Chrome」中输入"
        );
        // 浏览器域自动剥离后缀
        assert_eq!(
            resolve_typed_title(Some("百度 - Google Chrome"), AppDomain::Browser),
            "在「百度」中输入"
        );
    }

    #[test]
    fn typed_title_without_window() {
        assert_eq!(resolve_typed_title(None, AppDomain::Other), "在「」中输入");
    }

    // ── reindex_steps（finish 排水重排）─────────────────────────────

    #[test]
    fn reindex_empty_returns_empty() {
        assert!(reindex_steps(Vec::new(), 0).is_empty());
    }

    #[test]
    fn reindex_from_zero_numbers_sequentially() {
        let out = reindex_steps(vec![make_step(0), make_step(1)], 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].order_index, 0);
        assert_eq!(out[0].step_number, 1);
        assert_eq!(out[1].order_index, 1);
        assert_eq!(out[1].step_number, 2);
    }

    #[test]
    fn reindex_appends_after_existing_steps() {
        // 模拟 finish_recording：recorder.steps 已有 3 步，captured 2 步续接
        let out = reindex_steps(vec![make_step(0), make_step(1)], 3);
        assert_eq!(out[0].order_index, 3);
        assert_eq!(out[0].step_number, 4);
        assert_eq!(out[1].order_index, 4);
        assert_eq!(out[1].step_number, 5);
    }

    #[test]
    fn reindex_overwrites_stale_indices() {
        // 捕获阶段的 order_index 恒为 0，重排后必须被覆盖
        let mut stale = make_step(0);
        stale.step_number = 999;
        let out = reindex_steps(vec![stale], 7);
        assert_eq!(out[0].order_index, 7);
        assert_eq!(out[0].step_number, 8);
    }

    #[test]
    fn reindex_keeps_other_fields() {
        let mut s = make_step(0);
        s.action_type = "type".into();
        s.title = "在「Chrome」中输入".into();
        let out = reindex_steps(vec![s], 1);
        assert_eq!(out[0].action_type, "type");
        assert_eq!(out[0].title, "在「Chrome」中输入");
    }

    // ── current_time_millis（时间戳）───────────────────────────────

    #[test]
    fn current_time_millis_is_non_negative_and_recent() {
        let t = current_time_millis();
        assert!(t > 0);
        // 距 now 不超过 5 秒
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!((now - t).abs() < 5_000);
    }

    #[test]
    fn current_time_millis_monotonic_increasing() {
        let a = current_time_millis();
        let b = current_time_millis();
        assert!(b >= a);
    }

    // ── build_typed_step（键盘步骤组装）────────────────────────────

    #[test]
    fn build_typed_step_basic_fields() {
        let s = build_typed_step(
            Some("Chrome".into()),
            "搜索框内容".into(),
            "ss.jpg".into(),
            "before.jpg".into(),
            12345,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.action_type, "type");
        assert_eq!(s.title, "在「Chrome」中输入");
        assert_eq!(s.description, "搜索框内容");
        assert_eq!(s.screenshot_path, "ss.jpg");
        assert_eq!(s.before_screenshot_path, "before.jpg");
        assert_eq!(s.timestamp, 12345);
        assert_eq!(s.window_title, "Chrome");
        assert_eq!(s.x, None);
        assert_eq!(s.y, None);
        assert_eq!(s.after_screenshot_path, None);
        assert_eq!(s.tip, None);
        assert_eq!(s.is_sensitive, false);
        assert!(!s.id.is_empty());
    }

    #[test]
    fn build_typed_step_sensitive_marks_flag() {
        // B2: 密码框步骤应标记 is_sensitive（描述由调用方替换为 [密码已隐藏]）
        let s = build_typed_step(
            Some("登录".into()),
            "[密码已隐藏]".into(),
            "ss.jpg".into(),
            "before.jpg".into(),
            10,
            true,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.is_sensitive, true);
        assert_eq!(s.description, "[密码已隐藏]");
        assert_eq!(s.window_title, "登录");
    }

    #[test]
    fn build_typed_step_window_title_missing() {
        let s = build_typed_step(
            None,
            "d".into(),
            "ss".into(),
            "before".into(),
            1,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.title, "在「」中输入");
        assert_eq!(s.window_title, "");
    }

    #[test]
    fn build_typed_step_before_falls_back_to_screenshot_when_none() {
        // flush_key_step 调用处：before_path.unwrap_or_else(|| ss_str.clone())
        // 此处模拟 before_path 为空字符串（已 unwrap 后传入）
        let s = build_typed_step(
            None,
            "d".into(),
            "ss.jpg".into(),
            String::new(),
            1,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.before_screenshot_path, "");
    }

    #[test]
    fn build_typed_step_empty_description_and_typed() {
        let s = build_typed_step(
            None,
            String::new(),
            "ss".into(),
            "before".into(),
            0,
            false,
            None,
            None,
            None,
            None,
        );
        assert_eq!(s.description, "");
        assert_eq!(s.action_type, "type");
    }

    // ── build_mark_step（手动标记截图步骤）────────────────────────

    #[test]
    fn build_mark_step_defaults() {
        let s = build_mark_step(
            "我的截图".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            999,
        );
        assert_eq!(s.action_type, "screenshot");
        assert_eq!(s.title, "我的截图");
        assert_eq!(s.description, "");
        assert_eq!(s.screenshot_path, "");
        assert_eq!(s.before_screenshot_path, "");
        assert_eq!(s.after_screenshot_path, None);
        assert_eq!(s.x, None);
        assert_eq!(s.y, None);
        assert_eq!(s.window_title, "");
        assert_eq!(s.order_index, 0);
        assert_eq!(s.step_number, 1);
        assert_eq!(s.timestamp, 999);
    }

    #[test]
    fn build_mark_step_index_and_number() {
        let s = build_mark_step(
            "t".into(),
            Some(10),
            Some(20),
            Some("click".into()),
            Some("desc".into()),
            Some("p.png".into()),
            Some("after.png".into()),
            4,
            1000,
        );
        assert_eq!(s.order_index, 4);
        assert_eq!(s.step_number, 5);
        assert_eq!(s.action_type, "click");
        assert_eq!(s.description, "desc");
        assert_eq!(s.screenshot_path, "p.png");
        assert_eq!(s.after_screenshot_path, Some("after.png".into()));
        assert_eq!(s.x, Some(10));
        assert_eq!(s.y, Some(20));
    }

    #[test]
    fn build_mark_step_empty_title_and_text() {
        let s = build_mark_step(
            String::new(),
            None,
            None,
            None,
            Some(String::new()),
            Some(String::new()),
            None,
            0,
            0,
        );
        assert_eq!(s.title, "");
        assert_eq!(s.description, "");
        assert_eq!(s.screenshot_path, "");
    }

    #[test]
    fn build_mark_step_preserves_after_path_none() {
        let s = build_mark_step(
            "t".into(),
            None,
            None,
            None,
            None,
            None,
            None,
            0,
            0,
        );
        assert_eq!(s.after_screenshot_path, None);
    }

    // ── steps_to_step_saves / recording_title_or_default / build_recording_save ──

    #[test]
    fn steps_to_step_saves_maps_all_fields() {
        let steps = vec![make_step(0), make_step(1)];
        let saves = steps_to_step_saves(&steps);
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[0].order_index, 0);
        assert_eq!(saves[0].step_number, 1);
        assert_eq!(saves[0].title, "t");
        assert_eq!(saves[0].action_type, "click");
        assert_eq!(saves[0].tip, "");
        assert_eq!(saves[0].x, None);
        assert_eq!(saves[0].y, None);
        assert_eq!(saves[0].before_screenshot_path, "");
        assert_eq!(saves[0].after_screenshot_path, None);
        assert_eq!(saves[0].timestamp, 0);
        assert_eq!(saves[0].window_title, "");
    }

    #[test]
    fn steps_to_step_saves_tip_none_becomes_empty() {
        let mut s = make_step(0);
        s.tip = None;
        let saves = steps_to_step_saves(&[s]);
        assert_eq!(saves[0].tip, "");
    }

    #[test]
    fn steps_to_step_saves_empty_input() {
        assert!(steps_to_step_saves(&[]).is_empty());
    }

    #[test]
    fn recording_title_or_default_non_empty_kept() {
        assert_eq!(recording_title_or_default("我的录制"), "我的录制");
    }

    #[test]
    fn recording_title_or_default_empty_uses_timestamp_format() {
        let t = recording_title_or_default("");
        assert!(t.starts_with("录制 "));
        // 格式：录制 YYYY-MM-DD HH:MM:SS
        assert!(regex_like(&t));
    }

    #[test]
    fn build_recording_save_basic() {
        let steps = vec![make_step(0), make_step(1), make_step(2)];
        let (save, rec_id) = build_recording_save("rec-1".into(), "标题", &steps, None);
        assert_eq!(save.id, "rec-1");
        assert_eq!(save.title, "标题");
        assert_eq!(save.app_name, "");
        assert_eq!(save.steps.len(), 3);
        assert_eq!(rec_id, "rec-1 (3 steps)");
    }

    #[test]
    fn build_recording_save_empty_title_fallback() {
        let steps = vec![make_step(0)];
        let (save, rec_id) = build_recording_save("rec-2".into(), "", &steps, None);
        assert!(save.title.starts_with("录制 "));
        assert_eq!(save.app_name, "");
        assert_eq!(rec_id, "rec-2 (1 steps)");
    }

    #[test]
    fn build_recording_save_zero_steps() {
        let (save, rec_id) = build_recording_save("rec-0".into(), "空", &[], None);
        assert_eq!(save.steps.len(), 0);
        assert_eq!(rec_id, "rec-0 (0 steps)");
    }

    // ── is_active_recording_state ─────────────────────────────────

    #[test]
    fn active_state_recording_and_paused() {
        assert!(is_active_recording_state(&RecordingState::Recording));
        assert!(is_active_recording_state(&RecordingState::Paused));
    }

    #[test]
    fn inactive_state_idle_completed_cancelled() {
        assert!(!is_active_recording_state(&RecordingState::Idle));
        assert!(!is_active_recording_state(&RecordingState::Completed));
        assert!(!is_active_recording_state(&RecordingState::Cancelled));
    }

    // ── apply_key_to_buffer / is_nav_key / should_flush_on_window_change ──

    #[test]
    fn apply_key_input_char_appends() {
        let mut buf = String::new();
        // 第三个参数是 ToUnicodeEx 解析出的「真实字符」
        assert_eq!(apply_key_to_buffer(&mut buf, "KeyA", "a"), BufferEffect::Input);
        assert_eq!(buf, "a");
        assert_eq!(apply_key_to_buffer(&mut buf, "KeyB", "b"), BufferEffect::Input);
        assert_eq!(buf, "ab");
    }

    #[test]
    fn apply_key_sentinel_and_unknown_keys_ignored() {
        let mut buf = String::new();
        // 已解析字符为空（未知键 / 修饰键无字符）视为无效键
        assert_eq!(apply_key_to_buffer(&mut buf, "你", ""), BufferEffect::Ignored);
        assert_eq!(buf, "");
        assert_eq!(apply_key_to_buffer(&mut buf, "ShiftLeft", ""), BufferEffect::Ignored);
        assert_eq!(buf, "");
    }

    #[test]
    fn apply_key_space_and_special_chars_input() {
        let mut buf = String::new();
        assert_eq!(apply_key_to_buffer(&mut buf, "KeySpace", " "), BufferEffect::Input);
        assert_eq!(buf, " ");
        assert_eq!(apply_key_to_buffer(&mut buf, "Comma", ","), BufferEffect::Input);
        assert_eq!(buf, " ,");
    }

    #[test]
    fn apply_key_bracket_prefix_ignored() {
        let mut buf = String::new();
        // 功能键的已解析字符以 [ 开头（如 "[Shift]"），不以字符形式输入
        assert_eq!(apply_key_to_buffer(&mut buf, "[Shift]", "[Shift]"), BufferEffect::Ignored);
        assert_eq!(buf, "");
    }

    #[test]
    fn apply_key_backspace_pops() {
        let mut buf = String::from("abc");
        assert_eq!(apply_key_to_buffer(&mut buf, "Backspace", ""), BufferEffect::Backspace);
        assert_eq!(buf, "ab");
        assert_eq!(apply_key_to_buffer(&mut buf, "Back", ""), BufferEffect::Backspace);
        assert_eq!(buf, "a");
    }

    #[test]
    fn apply_key_backspace_on_empty_is_safe() {
        let mut buf = String::new();
        assert_eq!(apply_key_to_buffer(&mut buf, "Backspace", ""), BufferEffect::Backspace);
        assert_eq!(buf, "");
    }

    #[test]
    fn apply_key_enter_is_ignored_as_input() {
        // Enter 是导航键，调用前已解析出空字符（不进入缓冲区）
        let mut buf = String::from("abc");
        assert_eq!(apply_key_to_buffer(&mut buf, "Enter", ""), BufferEffect::Ignored);
        assert_eq!(buf, "abc");
    }

    #[test]
    fn is_nav_key_variants() {
        assert!(is_nav_key("Enter"));
        assert!(is_nav_key("Tab"));
        // ★ 回归：hook 的 vk_to_key_name(VK_TAB) 产出 "KeyTab"，漏了它 Tab 步骤
        // 会整段消失（重放时不切字段）。
        assert!(is_nav_key("KeyTab"));
        assert!(is_nav_key("Return"));
        assert!(is_nav_key("KeyEnter"));
        assert!(is_nav_key("Escape"));
        assert!(!is_nav_key("a"));
        assert!(!is_nav_key("Backspace"));
        assert!(!is_nav_key("Shift"));
    }

    #[test]
    fn should_flush_window_change_true_when_diff_and_slow() {
        assert!(should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(600)
        ));
    }

    #[test]
    fn should_flush_window_change_false_when_same_title() {
        assert!(!should_flush_on_window_change(
            Some("A"),
            Some("A"),
            Duration::from_millis(600)
        ));
    }

    #[test]
    fn should_flush_window_change_false_when_within_500ms() {
        assert!(!should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(300)
        ));
    }

    #[test]
    fn should_flush_window_change_false_when_missing_title() {
        assert!(!should_flush_on_window_change(
            None,
            Some("B"),
            Duration::from_millis(600)
        ));
        assert!(!should_flush_on_window_change(
            Some("A"),
            None,
            Duration::from_millis(600)
        ));
        assert!(!should_flush_on_window_change(
            None,
            None,
            Duration::from_millis(600)
        ));
    }

    #[test]
    fn should_flush_window_change_boundary_500ms() {
        // 恰好 500ms 不算超过（> 而非 >=）
        assert!(!should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(500)
        ));
        assert!(should_flush_on_window_change(
            Some("A"),
            Some("B"),
            Duration::from_millis(501)
        ));
    }

    // 轻量格式校验（避免引入 regex 依赖）
    fn regex_like(s: &str) -> bool {
        // 仅支持本测试用到的简单模式：^录制 \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$
        let expect = "录制 0000-00-00 00:00:00";
        if !s.starts_with("录制 ") {
            return false;
        }
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.len() == 14 && s.len() == expect.len()
    }

    // ── P1 降噪：连续输入合并 ──

    fn typed_step(id: &str, window_title: &str, desc: &str) -> StepData {
        let mut s = make_step(0);
        s.id = id.to_string();
        s.action_type = "type".into();
        s.window_title = window_title.to_string();
        s.description = desc.to_string();
        s.screenshot_path = format!("{}.jpg", id);
        s.before_screenshot_path = format!("{}.before.jpg", id);
        s
    }

    #[test]
    fn should_merge_type_steps_same_window_merges() {
        let prev = typed_step("a", "WPS 表格", "XLOOKUP(");
        assert!(should_merge_type_steps(&prev, "WPS 表格", false));
    }

    #[test]
    fn should_merge_type_steps_rejects_sensitive_or_other_window_or_type() {
        // 敏感步骤（旧/新任一）绝不合并
        let mut prev = typed_step("a", "WPS 表格", "pw");
        prev.is_sensitive = true;
        assert!(!should_merge_type_steps(&prev, "WPS 表格", false));
        let prev = typed_step("a", "WPS 表格", "x");
        assert!(!should_merge_type_steps(&prev, "WPS 表格", true));
        // 窗口不同（换了应用）不合并
        let prev = typed_step("a", "WPS 表格", "x");
        assert!(!should_merge_type_steps(&prev, "Chrome - 百度", false));
        // 上一步不是输入步骤不合并
        let prev = make_step(0); // click
        assert!(!should_merge_type_steps(&prev, "WPS 表格", false));
        // 旧步骤窗口标题为空（兜底构造）不合并
        let prev = typed_step("a", "", "x");
        assert!(!should_merge_type_steps(&prev, "", false));
    }

    #[test]
    fn merge_typed_step_uia_overrides_desc_and_keeps_before() {
        let mut prev = typed_step("a", "WPS 表格", "XLOOKUP(");
        prev.x = Some(10);
        prev.timestamp = 111;
        let mut new = typed_step("b", "WPS 表格", "XLOOKUP(*,C2,*)");
        new.screenshot_path = "final.jpg".to_string();
        new.x = Some(20);
        new.timestamp = 999;

        merge_typed_step(&mut prev, &new, true, "*,C2,*)");
        assert_eq!(prev.description, "XLOOKUP(*,C2,*)"); // UIA 最终内容直接覆盖
        assert_eq!(prev.screenshot_path, "final.jpg"); // 主图 = 最终状态
        assert_eq!(prev.before_screenshot_path, "a.before.jpg"); // 操作前图保留
        assert_eq!(prev.x, Some(20)); // 裁剪框取本轮焦点
        assert_eq!(prev.timestamp, 111); // 时间戳保留步骤起点
        assert_eq!(prev.id, "a");
    }

    #[test]
    fn merge_typed_step_fallback_concatenates_keystrokes() {
        let mut prev = typed_step("a", "WPS 表格", "XLOOKUP(");
        let new = typed_step("b", "WPS 表格", "*,C2,*)");
        merge_typed_step(&mut prev, &new, false, "*,C2,*)");
        assert_eq!(prev.description, "XLOOKUP(*,C2,*)"); // 无 UIA：按键流拼接
    }

    // ── P1 降噪：双击合并 ──

    #[test]
    fn double_click_detection_time_and_distance() {
        let dbl = 500u64;
        // 同点 + 快速 → 双击
        assert!(is_double_click(0.0, 0.0, 180, dbl));
        assert!(is_double_click(3.0, -4.0, 499, dbl));
        // 间隔超时 → 不是
        assert!(!is_double_click(0.0, 0.0, 501, dbl));
        // 距离超阈 → 不是
        assert!(!is_double_click(9.0, 0.0, 100, dbl));
        assert!(!is_double_click(0.0, 8.1, 100, dbl));
    }
}

#[cfg(test)]
mod shortcut_label_tests {
    use super::*;

    #[test]
    fn win_combos_have_chinese_titles() {
        // 用户原 bug：Win+D 显示桌面，不能退化成字符 "d"
        let (t, _) = win_shortcut(false, false, "KeyD").unwrap();
        assert_eq!(t, "Win+D 显示桌面");
        let (t, _) = win_shortcut(false, false, "KeyE").unwrap();
        assert_eq!(t, "Win+E 打开文件资源管理器");
        let (t, _) = win_shortcut(false, false, "KeyR").unwrap();
        assert_eq!(t, "Win+R 打开运行");
        // 带修饰键变体
        let (t, _) = win_shortcut(true, false, "KeyS").unwrap();
        assert_eq!(t, "Win+Shift+S 区域截图");
        let (t, _) = win_shortcut(false, true, "KeyD").unwrap();
        assert_eq!(t, "Win+Ctrl+D 新建虚拟桌面");
        let (t, _) = win_shortcut(false, false, "Key1").unwrap();
        assert_eq!(t, "Win+1 打开/激活任务栏第1个程序");
    }

    #[test]
    fn win_unknown_falls_back() {
        // 不识别的 Win 组合键返回 None，交由调用方兜底成「快捷键 Win+X」
        assert!(win_shortcut(false, false, "KeyQ").is_none());
    }

    #[test]
    fn win_tray_taskbar_keys() {
        let (t, _) = win_shortcut(false, false, "KeyB").unwrap();
        assert_eq!(t, "Win+B 焦点移到托盘通知区");
        let (t, _) = win_shortcut(false, false, "KeyT").unwrap();
        assert_eq!(t, "Win+T 循环切换任务栏程序");
        let (t, _) = win_shortcut(false, false, "KeyU").unwrap();
        assert_eq!(t, "Win+U 打开轻松使用/设置");
    }

    #[test]
    fn alt_printscreen_and_shift_delete() {
        // Alt+PrintScreen 截取当前窗口；Shift+Delete 永久删除
        let t = common_combo_label(false, true, false, "PrintScreen").unwrap();
        assert_eq!(t, "Alt+PrintScreen 截取当前窗口");
        let t = common_combo_label(true, false, true, "Delete").unwrap();
        assert_eq!(t, "Ctrl+Shift+Delete 永久删除");
        let t = common_combo_label(false, true, true, "Delete").unwrap();
        assert_eq!(t, "Alt+Shift+Delete 永久删除");
        // Ctrl+Shift+S 另存为
        let t = common_combo_label(true, false, true, "KeyS").unwrap();
        assert_eq!(t, "Ctrl+Shift+S 另存为");
        // Shift+F10 右键菜单（运行时仅 ctrl/alt 组合会进入此函数，shift 单独按走 function_key_label）
        let t = common_combo_label(false, true, true, "F10").unwrap();
        assert_eq!(t, "Alt+Shift+F10 打开右键菜单");
    }

    #[test]
    fn common_ctrl_alt_labels() {
        let t = common_combo_label(true, false, false, "KeyW").unwrap();
        assert_eq!(t, "Ctrl+W 关闭窗口/标签页");
        let t = common_combo_label(false, true, false, "KeyF4").unwrap();
        assert_eq!(t, "Alt+F4 关闭窗口");
        let t = common_combo_label(true, false, false, "Key1").unwrap();
        assert_eq!(t, "Ctrl+1 切换到第1个标签页");
        let t = common_combo_label(true, false, true, "KeyT").unwrap();
        assert_eq!(t, "Ctrl+Shift+T 恢复关闭的标签页");
        // 没有语义标签的键返回 None → 兜底成「快捷键 Ctrl+X」
        assert!(common_combo_label(true, false, false, "KeyQ").is_none());
    }

    #[test]
    fn function_keys_labelled() {
        // 此前 F5 刷新会被当成空白字符丢弃；现在应出步骤
        assert_eq!(function_key_label("F5").unwrap(), "F5 刷新");
        assert_eq!(function_key_label("F11").unwrap(), "F11 全屏");
        assert_eq!(function_key_label("F12").unwrap(), "F12 开发者工具");
        // 非功能键返回 None
        assert!(function_key_label("KeyD").is_none());
        assert!(function_key_label("F13").is_none());
    }

    #[test]
    fn system_combos_labelled() {
        // Ctrl+Alt+Del / Ctrl+Shift+Esc / Ctrl+Alt+Tab 此前被记成泛化标签
        let t = common_combo_label(true, true, false, "Delete").unwrap();
        assert_eq!(t, "Ctrl+Alt+Del 打开安全选项");
        let t = common_combo_label(true, false, true, "Escape").unwrap();
        assert_eq!(t, "Ctrl+Shift+Esc 打开任务管理器");
        let t = common_combo_label(true, true, false, "KeyTab").unwrap();
        assert_eq!(t, "Ctrl+Alt+Tab 打开窗口切换器");
        // 普通 Ctrl+Delete 仍是删除
        let t = common_combo_label(true, false, false, "Delete").unwrap();
        assert_eq!(t, "Ctrl+Delete 删除");
    }

    #[test]
    fn standalone_keys_labelled() {
        // PrintScreen 单独按此前静默丢弃，现在应出步骤
        assert_eq!(standalone_key_label("PrintScreen").unwrap(), "PrintScreen 截图保存");
        assert!(standalone_key_label("KeyD").is_none());
    }
}
