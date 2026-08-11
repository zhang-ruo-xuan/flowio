# StepSnap 源码技术分析报告

> 分析日期：2026-07-25
> 源码仓库：https://github.com/pimzino/stepsnap.git
> ⚠️ 只分析技术逻辑，不参考任何 UI 组件/样式/布局/配色

---

## 一、项目概览

StepSnap 是基于 Tauri 2 的桌面录屏文档工具。技术栈：Rust（后端）+ React + TypeScript（前端）。

### 目录结构

```
src-tauri/src/
├── main.rs          # 应用入口
├── lib.rs           # Tauri 命令注册、状态管理、前后端通信枢纽（3209行）
├── recorder.rs      # 录制引擎核心（1104行）
├── overlay.rs       # 原生覆盖层窗口（1661行）
├── accessibility.rs # 无障碍 API 封装（650行）
├── ocr.rs           # OCR 引擎（ONNX 模型）
├── database.rs      # SQLite 数据持久化
├── display.rs       # Linux 显示服务器检测
└── logging.rs       # 日志系统

src/
├── features/recorder/RecorderOverlay.tsx  # 录制覆盖层 UI
├── store/recorderStore.ts                  # 录制状态管理（Zustand）
├── lib/aiService.ts                       # AI 服务调用
└── lib/stepMapper.ts                      # 步骤数据映射
```

---

## 二、DOM 事件捕获逻辑

### 2.1 核心技术栈

| 组件 | 技术 | 作用 |
|------|------|------|
| 全局输入监听 | `rdev` crate | 跨平台键盘/鼠标事件捕获 |
| 屏幕截图 | `xcap::Monitor` | 全屏/区域截图 |
| OCR | ONNX Runtime + PaddleOCR 模型 | 文本识别 |
| 图像处理 | `image` + `imageproc` crate | 截图标注（圆圈、高亮） |

### 2.2 事件监听机制

```rust
// recorder.rs 核心流程
rdev::listen(move |event| {
    match event.event_type {
        EventType::ButtonPress(button) => { /* 鼠标点击 → 截图+记录 */ }
        EventType::KeyPress(key) => { /* 按键 → 累积文本 */ }
        EventType::ButtonRelease(_) => { /* 鼠标释放 */ }
        _ => {}
    }
})
```

- `rdev::listen` 是阻塞式全局事件监听，在独立线程中运行
- 通过 `mpsc::channel` 与主线程通信
- 每捕获一个事件 → 截图 → 生成 Step → 通过 `app.emit()` 发送到前端

### 2.3 Step 数据结构

```rust
struct Step {
    id: String,                // UUID，关联 OCR 结果
    type_: String,             // "click" | "type" | "capture"
    x: Option<i32>,            // 鼠标 X 坐标
    y: Option<i32>,            // 鼠标 Y 坐标
    text: Option<String>,      // 键入文本
    timestamp: u64,            // 毫秒时间戳
    screenshot: Option<String>, // 截图文件路径
    element_name: Option<String>,   // UI 元素名称（来自无障碍API）
    element_type: Option<String>,   // UI 元素类型
    element_value: Option<String>,  // UI 元素当前值
    app_name: Option<String>,       // 宿主应用名
    input_source: Option<String>,   // 文本来源标记
}
```

### 2.4 录制状态管理

```rust
pub struct RecordingState {
    is_recording: Arc<Mutex<bool>>,
    is_picker_open: Arc<Mutex<bool>>,
    ocr_enabled: Arc<Mutex<bool>>,
    state_diff_enabled: Arc<Mutex<bool>>,    // after-frame 截图
    after_frame_max_wait_ms: Arc<Mutex<u64>>, // 稳定等待上限
    video_clips_enabled: Arc<Mutex<bool>>,    // 短视频片段
    start_hotkey: Arc<Mutex<HotkeyBinding>>,
    stop_hotkey: Arc<Mutex<HotkeyBinding>>,
    capture_hotkey: Arc<Mutex<HotkeyBinding>>,
}
```

### 2.5 关键设计决策

1. **After-frame 截图**：每次事件后等待 UI 稳定（最多 2 秒），再截一张"操作后"的图，提供状态变化对比
2. **文本捕获双通道**：键盘事件流（实时）+ 无障碍 API（最终值），后者更可靠（处理自动补全/粘贴/IME 输入）
3. **密码字段保护**：检测到 secure 字段时，值设为 `"[password]"` 哨兵，不记录真实内容
4. **自过滤**：`is_stepsnap_app()` 检测并忽略 StepSnap 自身窗口的事件

---

## 三、截图机制

### 3.1 截图流程

```
事件触发 → xcap::Monitor::capture() → 图像处理 → 保存 JPEG
                                              ↓
                            OCR (ONNX) → 文本识别结果
```

### 3.2 截图方式

- **全屏截图**：`Monitor::from_point(x, y)?.capture_image()?`
- **区域截图**：指定坐标和尺寸
- **保存格式**：JPEG（默认质量 85），GIF（视频片段模式）

### 3.3 控件高亮（Overlay）

不需要在截图中画高亮——使用**独立的原生覆盖层窗口**实现的实时视觉反馈：

**Windows 实现**（`overlay.rs :: windows_impl`）：
```rust
// 创建透明覆盖层窗口
CreateWindowExW(
    WS_EX_LAYERED        // 分层窗口
    | WS_EX_TRANSPARENT  // 鼠标穿透
    | WS_EX_TOPMOST      // 置顶
    | WS_EX_TOOLWINDOW   // 不显示在任务栏
    | WS_EX_NOACTIVATE,  // 不抢焦点
    "StepSnapOverlay",
    ...,
    WS_POPUP | WS_VISIBLE,
    x, y, width, height,
)
```

- 边框宽度：4px
- 边框颜色：绿色 `#22C55E`（`0x005EC722` BGR）
- 通过 `SetWindowPos(HWND_TOPMOST, ...)` 移动/更新位置
- 无窗口装饰、鼠标穿透、永不激活

### 3.4 截图标注

在截图图像上直接绘制（`imageproc` crate）：
- `draw_filled_circle_mut()` — 点击位置实心圆
- `draw_hollow_circle_mut()` — 点击位置空心圆

---

## 四、UI 元素识别算法

### 4.1 架构

```
坐标(x,y) → Windows UI Automation → ElementInfo
                                    ├── name (控件名称)
                                    ├── element_type (控件类型)
                                    ├── value (当前值)
                                    └── app_name (宿主应用)
```

### 4.2 Windows 实现

```rust
// 初始化 COM
CoInitializeEx(None, COINIT_MULTITHREADED);

// 创建 UI Automation 实例
let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, ...);

// 根据坐标获取元素
let element = automation.ElementFromPoint(POINT { x, y });

// 读取属性
element.CurrentName()                  // 控件名称
element.CurrentLocalizedControlType()  // 本地化控件类型（中文）
```

### 4.3 获取宿主应用名

通过 `ControlViewWalker` 向上遍历父节点（最多 10 层），获取顶层窗口标题作为应用名。

### 4.4 聚焦字段值读取

```rust
pub struct FocusedFieldValue {
    value: String,
    source: &'static str,  // "ax_value" | "ax_text" | "ax_legacy"
    is_password: bool,
}
```

- 通过 `GetFocusedElement()` 获取当前焦点元素
- 尝试多种方式读取值：`ValuePattern` → `TextPattern` → Legacy IAccessible
- 密码字段检测：`IsPasswordProperty` → 返回 `"[password]"` 哨兵
- 值截断：最大 2000 字符，保留首尾各半

### 4.5 跨平台

- Windows：UI Automation (UIA)
- macOS：Accessibility API（类似封装）
- 统一 `ElementInfo` 结构体，平台实现通过 `#[cfg(target_os)]` 条件编译

---

## 五、Tauri 桌面框架集成

### 5.1 前后端通信

**Rust → 前端（事件推送）**：
```rust
// 通过 Tauri Emitter 推送事件
app.emit("recording-step", &step)?;
app.emit("recording-state", &state)?;
app.emit("startup-progress", &status)?;
```

**前端 → Rust（命令调用）**：
```rust
#[tauri::command]
fn start_recording(state: State<RecordingState>, app: AppHandle) { }

#[tauri::command]
fn stop_recording(state: State<RecordingState>) { }

#[tauri::command]
async fn get_recordings(db: State<DatabaseState>) -> Vec<Recording> { }
```

### 5.2 全局状态管理

```rust
// Tauri State 自动注入
pub struct DatabaseState(pub Mutex<Database>);
pub struct RecordingState { ... }
pub struct StartupState(pub Arc<Mutex<StartupStatus>>);

// 在 main.rs 中注册
tauri::Builder::default()
    .manage(DatabaseState(Mutex::new(db)))
    .manage(RecordingState::new())
    .manage(StartupState::new())
```

### 5.3 全局快捷键

```rust
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, Modifiers, Code};

// 注册快捷键
app.plugin(tauri_plugin_global_shortcut::Builder::new().build())?;

// 监听快捷键
app.handle().plugin(
    global_shortcut.on_shortcut(move |_app, shortcut, event| {
        if event.state == ShortcutState::Pressed {
            // Ctrl+Alt+R 开始/停止录制，Ctrl+Alt+C 截图
        }
    })
);
```

### 5.4 插件生态

| 插件 | 用途 |
|------|------|
| `tauri_plugin_global_shortcut` | 全局快捷键 |
| `tauri_plugin_dialog` | 文件对话框 |
| `tauri_plugin_shell` | 打开外部应用/链接 |
| `tauri_plugin_updater` | 自动更新 |

---

## 六、可复用技术逻辑总结

以下模块可直接复用（纯技术逻辑，无 UI 依赖）：

| 模块 | 文件 | 复用价值 | 注意事项 |
|------|------|----------|----------|
| 全局输入监听 | recorder.rs | ⭐⭐⭐⭐⭐ 核心 | `rdev` 跨平台稳定 |
| 屏幕截图 | recorder.rs (xcap) | ⭐⭐⭐⭐⭐ 核心 | xcap 比 screenshot-rs 更稳定 |
| UI 元素识别 | accessibility.rs | ⭐⭐⭐⭐ 高 | 仅复用 Windows UIA 部分 |
| 原生覆盖层 | overlay.rs | ⭐⭐⭐⭐ 高 | 仅复用 Windows 原生窗口逻辑，不参考配色/样式 |
| Tauri 集成模式 | lib.rs | ⭐⭐⭐⭐ 高 | State 管理 + emit 通信模式 |
| OCR 引擎 | ocr.rs | ⭐⭐⭐ 中 | ONNX 模型需额外部署 |
| 显示服务器检测 | display.rs | ⭐⭐ 低 | 仅 Linux 需要 |

---

## 七、不复用清单

- ❌ 任何 React 组件（App.tsx 及所有 components/）
- ❌ CSS 样式（index.css）
- ❌ 页面布局/路由结构
- ❌ 图标/配色方案（overlay.rs 中的颜色常量仅作技术参考）
- ❌ 导出功能 UI（ExportDropdown 等）
- ❌ 设置面板 UI（SettingsPanel 等）
- ❌ Toast/通知 UI 组件
- ❌ 任何与 StepSnap 品牌相关的视觉元素

---

*报告完毕。*
