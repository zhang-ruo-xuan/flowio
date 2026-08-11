---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_7e9b7ba58d7e11f1b8c1525400826444
    ReservedCode1: GdQGsC74q1cIJy+4AwC+zfyo2ZXhMZ1hDrjjFhKqHNQAWHjjP1EEK0R1BqnOfMUklNhZuvHFLLx5lsViEQhkdDB2u3UChlgFM83p4zchLPFA3I4o56TQQAKK+Dav6IsSTqpOsknNRoNCo+BZk9bhubnYgfmwO69CnYBysgkhuOOgeIoZabWlnHsPJFk=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_7e9b7ba58d7e11f1b8c1525400826444
    ReservedCode2: GdQGsC74q1cIJy+4AwC+zfyo2ZXhMZ1hDrjjFhKqHNQAWHjjP1EEK0R1BqnOfMUklNhZuvHFLLx5lsViEQhkdDB2u3UChlgFM83p4zchLPFA3I4o56TQQAKK+Dav6IsSTqpOsknNRoNCo+BZk9bhubnYgfmwO69CnYBysgkhuOOgeIoZabWlnHsPJFk=
---

# Flowio Desktop 桌面端架构

> 版本：v2.0 · 日期：2026-08-01
> 前置阅读：`docs/architecture.md`（全系统架构）、`docs/scribe-full-product-matrix.md`（产品矩阵）

---

## 一、进程与线程模型

```
┌──────────────────────────────────────────────────────────────────┐
│                        Tauri Main Process                         │
│                                                                   │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐   │
│  │   Rust Backend  │  │   System Tray   │  │  Hotkey Monitor │   │
│  │   (主线程)       │  │   (独立线程)     │  │  (独立线程)      │   │
│  └────────┬────────┘  └─────────────────┘  └─────────────────┘   │
│           │                                                       │
│           │ Commands (IPC)                                        │
│           ▼                                                       │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │                   Tauri Core Runtime                         │ │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │ │
│  │  │  Event Loop  │  │  IPC Bridge  │  │  Asset Proto │      │ │
│  │  └──────────────┘  └──────────────┘  └──────────────┘      │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │                     Background Workers                        │ │
│  │                                                               │ │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐        │ │
│  │  │ Recorder     │  │ AI Pipeline  │  │ Share Server │        │ │
│  │  │ Thread       │  │ Thread       │  │ Thread       │        │ │
│  │  │              │  │              │  │              │        │ │
│  │  │ • Hook 监听  │  │ • 去重       │  │ • HTTP 服务  │        │ │
│  │  │ • 截图捕获   │  │ • 裁剪       │  │ • 查看器渲染 │        │ │
│  │  │ • UIA 查询   │  │ • API 调用   │  │ • 访问控制   │        │ │
│  │  │ • 步骤构建   │  │ • 结果解析   │  │              │        │ │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘        │ │
│  │         │                 │                 │                 │ │
│  │         ▼                 ▼                 ▼                 │ │
│  │  ┌─────────────────────────────────────────────────────────┐ │ │
│  │  │                    SQLite (rusqlite)                     │ │ │
│  │  │  读写锁：WAL 模式，多线程读，单线程写                     │ │ │
│  │  └─────────────────────────────────────────────────────────┘ │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
                                   │
                                   │ IPC Bridge
                                   ▼
┌──────────────────────────────────────────────────────────────────┐
│                     WebView Process (React)                        │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │                    React 18 + TypeScript                     │ │
│  │                                                               │ │
│  │  ┌─────────┐ ┌─────────┐ ┌──────────┐ ┌────────────────┐    │ │
│  │  │  Router │ │  State  │ │  Event   │ │  Component     │    │ │
│  │  │ (React  │ │ (Zustand│ │ Listener │ │  Library       │    │ │
│  │  │  Router)│ │ + SWR)  │ │ (Tauri)  │ │  (Tailwind v4) │    │ │
│  │  └─────────┘ └─────────┘ └──────────┘ └────────────────┘    │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

### 线程职责与通信

```
┌──────────────┐     Arc<Mutex<AppState>>     ┌──────────────┐
│  Main Thread │◄────────────────────────────►│  Recorder    │
│  (Rust)      │                               │  Thread      │
└──────┬───────┘                               └──────┬───────┘
       │                                              │
       │  Tauri Event                                 │  Event Channel
       │  (emit to JS)                                │  (mpmc Sender)
       │                                              │
       ▼                                              ▼
┌──────────────┐     Arc<Mutex<AppState>>     ┌──────────────┐
│  WebView     │                               │  AI Pipeline │
│  (React)     │                               │  Thread      │
└──────┬───────┘                               └──────┬───────┘
       │                                              │
       │  invoke (IPC)                                │  Event Channel
       │                                              │
       ▼                                              ▼
┌──────────────┐     Arc<Mutex<AppState>>     ┌──────────────┐
│  Main Thread │                               │  Share       │
│  (Rust)      │                               │  Server      │
└──────────────┘                               └──────────────┘
```

- **Main Thread**：处理 Tauri Commands、IPC 通信、生命周期管理
- **Recorder Thread**：独立线程运行 Windows Hook 消息泵，不阻塞 UI
- **AI Pipeline Thread**：异步 HTTP 请求 + 流式解析，通过 Tauri Event 推送进度
- **Share Server Thread**：`tiny_http` 监听本地端口，按需启动/停止

---

## 二、Rust 模块详解

### 2.1 模块树

```
src-tauri/src/
│
├── main.rs                         # 入口，配置并启动 Tauri
├── lib.rs                          # Tauri Builder 配置 + 注册所有 Commands
├── types.rs                        # 公共类型定义（共享给所有模块）
├── error.rs                        # 统一错误类型（实现 Serialize 以透传前端）
├── consts.rs                       # 常量（目录路径、文件扩展名等）
│
├── recorder/                       # 录制引擎 ★
│   ├── mod.rs                      # 入口 + Recorder 结构体 + 状态机
│   ├── capture.rs                  # 屏幕截图实现
│   ├── listener.rs                 # 全局鼠标/键盘 Hook
│   ├── uia.rs                      # UIAutomation 元素查询
│   ├── step_builder.rs             # 原始事件 → Step 对象
│   ├── dedup.rs                    # 连续操作去重
│   ├── threshold.rs               # 静止帧检测（像素差阈值）
│   ├── sensitive_check.rs          # 密码框 / 敏感窗口跳过
│   └── screenshot_writer.rs        # 截图文件写入（异步 IO）
│
├── ai/                             # AI 引擎 ★
│   ├── mod.rs                      # AiEngine 结构体 + 流水线入口
│   ├── pipeline.rs                 # 流水线编排逻辑
│   ├── dedup.rs                    # 预处理去重（与 recorder::dedup 规则不同）
│   ├── crop.rs                     # 智能裁剪（检测变化区域）
│   ├── client.rs                   # HTTP 客户端（支持多模型）
│   ├── prompt.rs                   # Prompt 模板系统
│   ├── parser.rs                   # AI 响应解析（JSON + 容错 + 格式修复）
│   ├── chinese_ui.rs               # 中文 UI 术语词表与匹配
│   ├── deidentify.rs               # 脱敏：OCR + 正则 + 模糊
│   └── key_manager.rs              # API Key 加密存储与轮换
│
├── editor/                         # 编辑器后端
│   ├── mod.rs
│   ├── commands.rs                 # 步骤 CRUD Tauri Commands
│   ├── reorder.rs                  # 拖拽重排逻辑
│   ├── merge.rs                    # 步骤合并（截图拼接 + 文本拼接）
│   ├── batch.rs                    # 批量操作（全选删除 / 批量重置 AI）
│   └── history.rs                  # 撤销/重做（VecDeque<Action>）
│
├── export/                         # 导出引擎
│   ├── mod.rs                      # 入口 + 格式分发
│   ├── pdf.rs                      # PDF 模板渲染 → printpdf
│   ├── html.rs                     # HTML 单文件查看器
│   ├── markdown.rs                 # Markdown 导出
│   ├── long_image.rs               # 微信长图（image crate 纵向拼接）
│   └── storage.rs                  # 导出历史记录
│
├── share/                          # 分享服务
│   ├── mod.rs                      # ShareServer 结构体
│   ├── server.rs                   # HTTP Server（tiny_http）
│   ├── viewer.rs                   # 查看器 HTML 生成（含密码校验 JS）
│   └── auth.rs                     # 访问密码哈希 + 验证
│
├── db/                             # 数据库层
│   ├── mod.rs                      # 连接池初始化
│   ├── schema.rs                   # 建表 DDL
│   ├── recordings.rs               # Recording CRUD
│   ├── steps.rs                    # Step CRUD
│   └── migration.rs                # Schema 版本升级
│
├── settings/                       # 设置
│   ├── mod.rs
│   ├── config.rs                   # JSON 配置读写
│   └── defaults.rs                 # 默认值常量
│
├── hotkey/                         # 全局快捷键
│   ├── mod.rs                      # RegisterHotKey / UnregisterHotKey
│   └── actions.rs                  # 快捷键 → 动作映射
│
└── update/                         # 自动更新
    ├── mod.rs                      # 版本检查 + 下载 + 安装
    └── release.rs                  # GitHub Release / 自定义服务器
```

### 2.2 核心结构体

```rust
// types.rs

/// 录制状态机
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RecordingStatus {
    Idle,
    Recording { started_at: DateTime<Local>, step_count: u32 },
    Paused { resumed_from: DateTime<Local> },
    Processing { step_count: u32 },
    Completed { recording_id: String },
}

/// 录制文档
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub title: String,
    pub app_name: String,
    pub status: RecordingStatus,
    pub step_count: u32,
    pub ai_generated: bool,
    pub thumbnail_path: Option<String>,
    pub created_at: DateTime<Local>,
    pub updated_at: DateTime<Local>,
}

/// 单个步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub recording_id: String,
    pub index: u32,
    pub action_type: ActionType,
    pub before_screenshot: String,      // 本地路径
    pub after_screenshot: String,       // 本地路径
    pub element_info: Option<ElementInfo>,
    pub position: Option<ScreenPosition>,
    pub ai_title: Option<String>,
    pub ai_description: Option<String>,
    pub user_title: Option<String>,
    pub user_description: Option<String>,
    pub annotations: Vec<Annotation>,
    pub redactions: Vec<Redaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionType {
    Click,
    DoubleClick,
    RightClick,
    Type { text: String },
    Scroll { direction: ScrollDirection, delta: i32 },
    Drag { from: ScreenPosition, to: ScreenPosition },
    Select { text: String },
    Navigate { url: String },
    Hotkey { keys: Vec<String> },
    Wait { seconds: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementInfo {
    pub control_type: String,          // Button / Edit / ListItem ...
    pub name: Option<String>,
    pub automation_id: Option<String>,
    pub class_name: Option<String>,
    pub bounding_rect: Option<Rect>,
    pub is_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub annotation_type: AnnotationType,
    pub rect: Rect,
    pub color: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnnotationType {
    Highlight,
    Circle,
    Arrow { direction: ArrowDirection },
    Number,
    TextOverlay { text: String, font_size: u8 },
    Blur,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Redaction {
    pub id: String,
    pub rect: Rect,
    pub redaction_type: RedactionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RedactionType {
    Blur { radius: u8 },
    SolidColor { color: String },
    Pixelate { block_size: u8 },
}
```

---

## 三、录制引擎（Recorder）

### 3.1 状态机

```
                    ┌──────────┐
                    │   Idle   │  ← 初始状态 / 取消后
                    └────┬─────┘
                         │ start_recording()
                         ▼
                    ┌──────────┐
        ┌──────────►│Recording │◄──────────┐
        │           └────┬─────┘           │
        │ pause()        │  stop()         │ resume()
        │                │                 │
        │           ┌────▼─────┐           │
        │           │Processing │           │
        │           └────┬─────┘           │
        │                │                 │
        │                │ complete()      │
        │                ▼                 │
        │           ┌──────────┐           │
        │           │Completed │           │
        │           └──────────┘           │
        │                                  │
        └────────── Paused ◄───────────────┘
                    cancel()
                        │
                        ▼
                    ┌──────────┐
                    │   Idle   │  (丢弃本次所有步骤)
                    └──────────┘
```

### 3.2 事件处理流水线

```
Windows Hook (WH_MOUSE_LL + WH_KEYBOARD_LL)
                │
                │  RawEvent { type, timestamp, x, y, key, ... }
                ▼
┌──────────────────────────────────────────────────────┐
│  1. 事件过滤 filter_event()                           │
│     • 丢弃录制窗口自身的事件                           │
│     • 丢弃鼠标移动事件（非点击）                        │
│     • 丢弃系统组合键（Ctrl+Alt+Del 等）                │
│     • 静默期事件合并（< 200ms 连续操作合并）            │
└────────┬─────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  2. 敏感检测 check_sensitive()                        │
│     • 当前焦点是否为密码框（UIA PasswordMask）         │
│     • 当前窗口是否在黑名单中                           │
│     • 是 → 标记 is_sensitive = true，跳过截图         │
└────────┬─────────────────────────────────────────────┘
         │ 非敏感
         ▼
┌──────────────────────────────────────────────────────┐
│  3. 截图 Before & After                               │
│     before: 操作前 100ms 截一帧                        │
│     after:  操作后 500ms 截一帧（等 UI 刷新）           │
│     实现：BitBlt (Win32) 全屏 → 裁剪到当前窗口         │
└────────┬─────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  4. UIA 查询 query_element()                          │
│     • IUIAutomation::ElementFromPoint(x, y)           │
│     • 提取 control_type / name / automation_id         │
│     • 提取 bounding_rect                              │
└────────┬─────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  5. 步骤构建 StepBuilder::build()                     │
│     RawEvent + screenshots + ElementInfo → Step        │
└────────┬─────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  6. 去重检查 dedup::should_append()                   │
│     • 与上一步 ActionType 相同 + 坐标接近 → 合并       │
│     • 两张 After 截图像素差 < threshold → 丢弃         │
└────────┬─────────────────────────────────────────────┘
         │ 通过
         ▼
┌──────────────────────────────────────────────────────┐
│  7. 持久化 storage::append_step()                     │
│     • 截图写入 output_dir/recording_id/steps/N/       │
│     • Step 元数据 INSERT INTO steps                   │
│     • 推 Tauri Event → 前端 StepAdded                 │
└──────────────────────────────────────────────────────┘
```

### 3.3 截图策略

```rust
/// 三种截图模式，按场景自动切换
enum ScreenshotMode {
    /// BitBlt 快速抓取（~5ms），适合录制中高频截图
    Fast,
    /// DXGI Desktop Duplication API（~2ms），零拷贝，适合 GPU 加速场景
    Dxgi,
    /// PrintWindow（~50ms），可捕获被遮挡窗口
    Full,
}

/// 截图优化
struct ScreenshotOptimizer {
    /// 窗口变化检测：只在变化区域的边界框内截图
    change_detection: bool,           // 默认 true
    /// 截图缓存：相同窗口 + 无变化 → 复用上一帧
    frame_cache: Option<Vec<u8>>,
    /// JPEG 压缩质量（磁盘节省 70%）
    jpeg_quality: u8,                 // 默认 75
}
```

---

## 四、AI 引擎（AI Pipeline）

### 4.1 流水线架构

```
Recording (steps[]) 完成
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  Pipeline Stage 0: Preprocess                         │
│  ┌────────────────────────────────────────────────┐  │
│  │ a. 去重合并 (AI 级别)                            │  │
│  │    • 合并连续 Scroll（只保留首尾）                │  │
│  │    • 合并导航系列 (URL 输入 + Enter → 一步)       │  │
│  │    • 丢弃 < 3 步的操作序列                       │  │
│  │ b. 智能裁剪 After 截图                           │  │
│  │    • Before/After diff → 变化区域 bbox            │  │
│  │    • 裁剪到 bbox + padding (32px)                │  │
│  │    • 仅将裁剪后图片发给 AI（节省 token）          │  │
│  │ c. 敏感信息脱敏                                  │  │
│  │    • OCR → 正则 → 模糊                           │  │
│  └────────────────────────────────────────────────┘  │
└────────┬─────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  Pipeline Stage 1: AI Generation (并行批量)           │
│                                                       │
│  ┌─────────────────────────────────────────────────┐ │
│  │ Batch 1 (steps 1-5)                              │ │
│  │  ┌────────┐ ┌────────┐ ┌────────┐               │ │
│  │  │ Step 1 │ │ Step 2 │ │ Step 3 │ ...           │ │
│  │  └───┬────┘ └───┬────┘ └───┬────┘               │ │
│  │      │          │          │                     │ │
│  │      ▼          ▼          ▼                     │ │
│  │  ┌──────────────────────────────────────────┐   │ │
│  │  │          GLM-4-flash API                  │   │ │
│  │  │          (HTTP POST, Base64 截图)         │   │ │
│  │  └──────────────────────────────────────────┘   │ │
│  └─────────────────────────────────────────────────┘ │
│                                                       │
│  Batch 2 (steps 6-10)  ←  上一批完成后再发            │
│  Batch N ...                                          │
│                                                       │
│  并发控制：最多 3 个并发请求，间隔 200ms              │
└────────┬─────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  Pipeline Stage 2: Parse & Validate                   │
│  ┌────────────────────────────────────────────────┐  │
│  │ a. JSON 提取                                    │  │
│  │    • 正则 / 代码块提取 / 首尾 {} 匹配            │  │
│  │    • JSON 修复（补引号 / 补逗号 / 截断修复）     │  │
│  │ b. 结构化校验                                   │  │
│  │    • title: 2-20 字，需包含动词                 │  │
│  │    • description: 10-200 字                     │  │
│  │    • 不合格 → 回退重试（最多 1 次）             │  │
│  │ c. 中文术语校验 (chinese_ui)                    │  │
│  │    • 扫描 title 中的 UI 术语，与词表匹配         │  │
│  │    • "下拉框" → "下拉菜单"，"弹窗" → "对话框"   │  │
│  └────────────────────────────────────────────────┘  │
└────────┬─────────────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│  Pipeline Stage 3: Persist                           │
│  ┌────────────────────────────────────────────────┐  │
│  │ • UPDATE steps SET ai_title = ?, ai_description │  │
│  │ • UPDATE recordings SET ai_generated = true     │  │
│  │ • 每完成一个 Batch → Tauri Event (progress %)   │  │
│  └────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
```

### 4.2 Prompt 模板系统

```rust
/// Prompt 模板结构
struct PromptTemplate {
    /// 系统提示词
    system: String,
    /// 用户消息模板（{screenshots} / {app_name} / {step_number} 为占位符）
    user: String,
    /// 输出格式约束
    output_schema: String,
    /// 上下文窗口剩余（用于判断是否需分拆步骤）
    remaining_tokens: usize,
}

/// 正向引导式中文 Prompt 范例
impl PromptTemplate {
    fn default() -> Self {
        Self {
            system: r#"你是 Scribe 风格的操作指南撰写专家。擅长将软件操作截图转化为简洁、专业的中文分步教程。
能力：
1. 识别 UI 元素（按钮/菜单/对话框/输入框/选项卡/图标），使用规范中文术语
2. 从 After 截图推断用户完成了什么操作，用动词开头撰写标题
3. 描述时强调操作对象和预期结果，不啰嗦
原则：
- 每个步骤标题 2-20 字，必须包含动词（点击/输入/选择/拖拽/勾选/滚动/切换）
- 描述 10-200 字，包含操作对象、位置、预期结果
- 按钮名称用原文，界面文字用原文，其他描述用中文
- 不要猜测截图中不存在的元素
- 不要描述不相关的界面细节（颜色/形状等，除非是操作的关键标识）"#.into(),

            user: "请为以下 {step_number} 个操作步骤生成中文标题和描述。操作发生在「{app_name}」应用中。\n\n每一步包含操作前和操作后的截图，请根据 After 截图判断用户完成了什么操作，据此生成标题和描述。",

            output_schema: r#"严格按以下 JSON 格式输出，不要加任何说明文字：
```json
{
  "steps": [
    {
      "step_number": 1,
      "title": "2-20字中文标题，动词开头",
      "description": "10-200字中文描述"
    }
  ]
}
```"#.into(),

            remaining_tokens: 8192,
        }
    }
}
```

### 4.3 多模型支持

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AiModel {
    ZhipuGlm4Flash,      // 默认，免费/低成本
    ZhipuGlm4,           // 高质量，收费
    DeepSeekV3,          // 备用
    QwenMax,             // 通义千问
    Custom {             // 自定义（支持 OpenAI 兼容 API）
        name: String,
        endpoint: String,
        model: String,
    },
}

struct AiClient {
    model: AiModel,
    api_key: String,           // 解密后的 Key
    http_client: reqwest::Client,
    retry_config: RetryConfig,
}

struct RetryConfig {
    max_retries: u8,           // 3
    base_delay: Duration,      // 1s
    max_delay: Duration,       // 30s
    backoff_multiplier: f32,   // 2.0
}
```

---

## 五、前端架构（React）

### 5.1 路由结构

```
/                              → Dashboard (首页，录制历史列表)
/record                        → Recorder (录制中，浮动控件)
/editor/:recordingId           → Editor (编辑页)
/preview/:recordingId          → Preview (预览/查看器)
/settings                      → Settings (设置页)
/settings/ai                   → AiSettings (AI 设置)
/settings/shortcuts            → ShortcutSettings (快捷键设置)
/settings/account              → AccountSettings (账户设置)
/share/:shareCode              → SharedViewer (接收方查看页)
```

### 5.2 组件树（编辑页为例）

```
<EditorPage>
├── <EditorToolbar>                          # 顶部工具栏
│   ├── <BackButton />                       # 返回
│   ├── <RecordingTitle />                   # 可编辑标题
│   ├── <AppBadge />                         # 应用图标 + 名称
│   ├── <AiGenerateButton />                 # AI 生成按钮 + 进度
│   ├── <UndoRedoButtons />                  # 撤销/重做
│   ├── <ExportMenu />                       # 导出下拉
│   │   ├── Export PDF
│   │   ├── Export HTML
│   │   ├── Export Markdown
│   │   └── Export 微信长图
│   ├── <ShareButton />                      # 分享
│   └── <SettingsButton />                   # 设置
│
├── <EditorBody>                             # 主体：左右分栏
│   ├── <StepList sidebar>                    # 左侧步骤列表 (300px)
│   │   ├── <StepListItem /> × N             # 每个步骤项
│   │   │   ├── <StepNumber />               # 圆角蓝色序号 (Scribe 风格)
│   │   │   ├── <StepTitle />                # AI/用户 标题 (内联编辑)
│   │   │   ├── <StepActions />              # 删除/合并/新增
│   │   │   └── <DragHandle />               # 拖拽手柄
│   │   └── <AddStepButton />                # 底部新增空步骤
│   │
│   └── <StepDetail main>                    # 右侧步骤详情
│       ├── <StepHeader>
│       │   ├── <StepTitleEditor />          # 标题内联编辑
│       │   ├── <StepActionType />           # 操作类型图标 + 文字
│       │   └── <ElementInfoBadge />         # 元素信息标签
│       ├── <ScreenshotViewer>               # 截图查看器
│       │   ├── <BeforeAfterToggle />        # Before/After 切换
│       │   ├── <ScreenshotCanvas>           # 画布（支持缩放+标注）
│       │   │   ├── <AnnotationLayer />      # 标注层 (SVG)
│       │   │   │   ├── <HighlightRect />
│       │   │   │   ├── <CircleAnnotation />
│       │   │   │   ├── <ArrowAnnotation />
│       │   │   │   └── <NumberBadge />
│       │   │   └── <RedactionLayer />       # 脱敏层
│       │   │       ├── <BlurRegion />
│       │   │       └── <SolidColorRegion />
│       │   └── <AnnotationToolbar>
│       │       ├── 高亮
│       │       ├── 圆形标注
│       │       ├── 箭头
│       │       ├── 编号
│       │       ├── 文字叠加
│       │       └── 模糊（脱敏）
│       ├── <StepDescriptionEditor />         # 描述编辑区
│       ├── <TipSection />                    # 提示/警告/备注 (可展开)
│       └── <ActionInfoPanel />               # 操作详情 (折叠)
│           ├── 坐标位置
│           ├── 元素信息
│           ├── 操作时间
│           └── 从录制中重新截图
│
└── <EditorFooter>                            # 底部状态栏
    ├── 步骤总数
    ├── AI 状态指示
    └── 上次保存时间
```

### 5.3 状态管理架构

```
┌─────────────────────────────────────────────┐
│                 Zustand Store                │
│                                              │
│  ┌─────────────┐  ┌──────────────────────┐  │
│  │ useAppStore  │  │ useRecordingStore    │  │
│  │              │  │                      │  │
│  │ • theme      │  │ • currentRecording   │  │
│  │ • locale     │  │ • steps[]            │  │
│  │ • sidebar    │  │   - selectedStepIdx  │  │
│  │   collapsed  │  │   - editingFields    │  │
│  └─────────────┘  │   - unsavedChanges    │  │
│                   │ • undoStack / redoStack│  │
│  ┌─────────────┐  └──────────────────────┘  │
│  │ useAiStore   │                            │
│  │              │  ┌──────────────────────┐  │
│  │ • progress   │  │ useShareStore        │  │
│  │ • status     │  │ • serverUrl          │  │
│  │ • error      │  │ • isActive           │  │
│  └─────────────┘  │ • password            │  │
│                   └──────────────────────┘  │
│                                              │
│  ┌──────────────────────────────────────┐    │
│  │         SWR (useSWR / useMutation)    │    │
│  │                                       │    │
│  │  • useRecordings()   → 录制列表       │    │
│  │  • useExportUrl()    → 导出状态       │    │
│  │  • useSettings()     → 全局设置       │    │
│  └──────────────────────────────────────┘    │
└─────────────────────────────────────────────┘
```

### 5.4 IPC 通信模式

```typescript
// 前端与 Rust 后端的三种通信模式

// 1. 同步命令调用（Request-Response）
const result = await invoke<Recording>('get_recording', {
  recordingId: 'xxx'
});

// 2. 事件监听（Rust → JS 推送）
import { listen } from '@tauri-apps/api/event';

const unlisten = await listen<AiProgress>('ai:progress', (event) => {
  // AI 流水线进度更新
  store.setAiProgress(event.payload);
});

// 3. 状态通道（录制中的高频事件，走独立 Event）
const unlisten = await listen<StepAddedEvent>('recorder:step-added', (e) => {
  store.appendStep(e.payload.step);
});
```

---

## 六、数据库设计（SQLite）

### 6.1 Schema

```sql
-- 录制文档
CREATE TABLE recordings (
    id TEXT PRIMARY KEY,                    -- UUID v4
    title TEXT NOT NULL DEFAULT '未命名录制',
    app_name TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'draft',   -- draft / recording / completed
    step_count INTEGER NOT NULL DEFAULT 0,
    ai_generated INTEGER NOT NULL DEFAULT 0,
    thumbnail_path TEXT,                    -- 缩略图（第一步 After 截图）
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE INDEX idx_recordings_status ON recordings(status);
CREATE INDEX idx_recordings_updated ON recordings(updated_at DESC);

-- 步骤
CREATE TABLE steps (
    id TEXT PRIMARY KEY,                    -- UUID v4
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    step_index INTEGER NOT NULL,            -- 顺序（从 0 开始）
    action_type TEXT NOT NULL,              -- Click / Type / Scroll / ...
    before_screenshot TEXT NOT NULL,        -- 本地相对路径
    after_screenshot TEXT NOT NULL,
    element_name TEXT,                      -- UIA name
    element_type TEXT,                      -- UIA control_type
    position_x REAL,                        -- 归一化坐标 [0,1]
    position_y REAL,
    ai_title TEXT,
    ai_description TEXT,
    user_title TEXT,
    user_description TEXT,
    annotations TEXT DEFAULT '[]',          -- JSON Array<Annotation>
    redactions TEXT DEFAULT '[]',           -- JSON Array<Redaction>
    is_sensitive INTEGER DEFAULT 0,         -- 敏感步骤（无截图）
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE INDEX idx_steps_recording ON steps(recording_id, step_index);

-- 导出记录
CREATE TABLE export_history (
    id TEXT PRIMARY KEY,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    format TEXT NOT NULL,                   -- pdf / html / md / long_image
    output_path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

-- 设置（KV 存储）
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- API Key 加密存储
CREATE TABLE api_keys (
    model TEXT PRIMARY KEY,                 -- glm4_flash / deepseek / qwen
    encrypted_key TEXT NOT NULL,            -- AES-256-GCM 加密
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);
```

### 6.2 文件系统布局

```
%APPDATA%/com.flowio.app/
│
├── flowio.db                         # SQLite 数据库
│
├── recordings/                       # 录制数据根目录
│   └── {recording_id}/               # 每个录制一个文件夹
│       ├── meta.json                 # 录制元数据（备份用）
│       ├── thumbnail.jpg             # 缩略图
│       └── steps/
│           ├── 000/                  # 第 0 步
│           │   ├── before.png        # 操作前截图（原始全屏）
│           │   ├── after.png         # 操作后截图（原始全屏）
│           │   ├── after_cropped.png # AI 裁剪版
│           │   └── annotated.png     # 标注后的最终截图（缓存）
│           ├── 001/
│           └── ...
│
├── exports/                          # 导出输出目录
│   └── {recording_title}_{timestamp}/ # 每次导出一个文件夹
│       └── ...
│
├── settings.json                     # 全局设置
└── logs/                             # 日志
    ├── app.log
    └── recorder.log
```

---

## 七、导出引擎

### 7.1 PDF 模板

```
┌──────────────────────────────────────────┐
│  Flowio │ 标题                    第 1/5 页 │
├──────────────────────────────────────────┤
│                                          │
│  ┌──────────────────────────────────┐   │
│  │                                  │   │
│  │          After 截图               │   │
│  │                                  │   │
│  └──────────────────────────────────┘   │
│                                          │
│  ① 点击文件菜单                          │
│  在窗口左上角找到并点击「文件」菜单按钮。  │
│                                          │
├──────────────────────────────────────────┤
│  Flowio 录步 · 2026-08-01 15:30         │
└──────────────────────────────────────────┘
```

### 7.2 HTML 查看器（离线单文件）

```html
<!DOCTYPE html>
<html>
<head>
  <meta charset="UTF-8">
  <title>录制标题 - Flowio 录步</title>
  <!-- 内联 CSS + JS -->
</head>
<body>
  <div class="viewer">
    <aside class="step-list"><!-- 左侧步骤导航 --></aside>
    <main class="step-detail"><!-- 右侧截图 + 描述 --></main>
  </div>
</body>
</html>
```

### 7.3 微信长图

```
步骤 1 After 截图
───────────────── (分隔线)
步骤 1 标题 + 描述
─────────────────
步骤 2 After 截图
─────────────────
步骤 2 标题 + 描述
─────────────────
       ...
─────────────────
页脚：Flowio 录步 · 日期 · 二维码
```

---

## 八、分享服务

### 8.1 本地 HTTP 分享

```
┌──────────────────────────────────────────────┐
│            同一局域网                          │
│                                              │
│  ┌──────────┐                                │
│  │ Desktop A │  ShareServer::start()          │
│  │ (发起方)  │  → tiny_http on 0.0.0.0:随机端口│
│  └────┬─────┘                                │
│       │                                       │
│       │  http://192.168.1.5:12345/share/abc   │
│       │                                       │
│  ┌────▼─────┐        ┌──────────┐            │
│  │ Desktop B│        │ 手机浏览器 │            │
│  │ (接收方)  │        │ (接收方)  │            │
│  └──────────┘        └──────────┘            │
│                                              │
└──────────────────────────────────────────────┘
```

### 8.2 查看器页面

```
┌──────────────────────────────────────────────────┐
│  Flowio 录步 — 查看器                             │
│  ┌────────────────────────────────────────────┐  │
│  │  [输入密码]  (如有设置)                      │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
│  ┌───────┬────────────────────────────────────┐  │
│  │       │                                    │  │
│  │ 步骤  │         After 截图                  │  │
│  │ 列表  │                                    │  │
│  │ (左)  │                                    │  │
│  │       │  ① 点击文件菜单                     │  │
│  │       │  在窗口左上角找到并点击「文件」菜单。  │  │
│  │       │                                    │  │
│  └───────┴────────────────────────────────────┘  │
│                                                  │
│  ── Flowio 录步 · 仅供查看 ──                     │
└──────────────────────────────────────────────────┘
```

---

## 九、性能目标

| 指标 | 目标值 | 测量方法 |
|------|--------|---------|
| 应用冷启动 | < 2s | Tauri main() → WebView 首屏渲染 |
| 录制帧率 | ≥ 5 fps (截图) | 录制中平均截图间隔 |
| 录制延迟 | Hook → 截图 < 200ms | 事件时间戳差 |
| 录制内存 | < 200MB（30分钟录制） | 任务管理器 Private Bytes |
| AI 单步耗时 | < 3s | API 请求 → 解析完成 |
| AI 20步总耗时 | < 30s (3 并发) | AI Pipeline 总 Wall Time |
| 编辑器渲染 | 100 步 < 200ms (First Paint) | React DevTools Profiler |
| PDF 导出 | 20 步 < 5s | printpdf 渲染 + 文件写入 |
| 安装包大小 | < 15MB | NSIS 安装包 |
| 磁盘占用 | 录制 30min < 100MB | JPEG q=75 |

---

> 本文档定义了 Flowio Desktop 桌面端的完整技术架构，覆盖进程模型、Rust 模块、录制引擎、AI 流水线、React 前端组件树、SQLite 数据模型、导出引擎和分享服务。可直接作为桌面端开发的基准蓝图。
*（内容由AI生成，仅供参考）*
