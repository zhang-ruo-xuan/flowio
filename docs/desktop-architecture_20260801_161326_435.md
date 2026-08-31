---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_f82c58ad8d8011f1b82d525400287e28
    ReservedCode1: uNbjSXGlDiAc7h6DYqqAp0Q93Rbqq0CGxgLNNn9Pc+VlaaVJsRHLf0dkw1TMNN+JPsJF6/5XZl7QcSPQ6IQwl1BUO2CJ18Mzn41RDw/yACDDr80+QfcYkchAqjOFMgWXn3iVym/oX5PyZ4H2z7FMyufMkGm3d0z9ka6J9Nv885V73XRDEANEJhYlqUY=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_f82c58ad8d8011f1b82d525400287e28
    ReservedCode2: uNbjSXGlDiAc7h6DYqqAp0Q93Rbqq0CGxgLNNn9Pc+VlaaVJsRHLf0dkw1TMNN+JPsJF6/5XZl7QcSPQ6IQwl1BUO2CJ18Mzn41RDw/yACDDr80+QfcYkchAqjOFMgWXn3iVym/oX5PyZ4H2z7FMyufMkGm3d0z9ka6J9Nv885V73XRDEANEJhYlqUY=
---

# Flowio Desktop 工具版架构

> 版本：v3.0 · 日期：2026-08-01
> 定位：**纯本地桌面工具** — 录制 → AI 生成 → 编辑 → 导出，打磨好再扩展插件和生态

---

## 一、整体边界

```
┌──────────────────────────────────────────────────────────────┐
│                     Flowio Desktop (工具版)                     │
│                                                               │
│  输入：用户在 Windows 桌面上的任意软件操作                       │
│  输出：PDF / HTML / Markdown / 微信长图（本地文件）              │
│                                                               │
│  ┌──────────┐   ┌──────────┐   ┌──────────┐   ┌──────────┐  │
│  │  录制    │ → │  AI 生成  │ → │  编辑    │ → │  导出    │  │
│  │  Recorder │   │ Pipeline │   │  Editor  │   │  Export  │  │
│  └──────────┘   └──────────┘   └──────────┘   └──────────┘  │
│                                                               │
│  存储：本地 SQLite + 文件系统                                  │
│  网络：仅 AI 调用时联网，其余全部离线                           │
│  用户：单机单用户，无账户/无登录/无团队                         │
└──────────────────────────────────────────────────────────────┘
```

### 不做的事情（留给后续版本）

| 不做 | 原因 |
|------|------|
| 分享链接 / 局域网 HTTP 服务 | 工具版专注"生产"，分享等生态阶段 |
| 云端同步 / 团队协作 | 工具版单机单用户 |
| 浏览器扩展 | 先打磨桌面端核心体验 |
| 飞书/钉钉/企微集成 | 生态阶段再做 |
| 用户账户 / 登录 | 纯本地，无账户体系 |
| 小程序查看端 | 生态阶段再做 |
| 品牌定制 | Pro 阶段 |
| 评论系统 | 团队版 |
| Live Link 同步 | 需要云端，团队版 |

---

## 二、进程与线程模型

```
┌───────────────────────────────────────────────────────────────┐
│                      Tauri Main Process                        │
│                                                                │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐         │
│  │ Rust Backend │  │ System Tray  │  │Hotkey Monitor│         │
│  │  (主线程)     │  │  (独立线程)   │  │  (独立线程)   │         │
│  └──────┬───────┘  └──────────────┘  └──────────────┘         │
│         │                                                      │
│         │ Tauri Commands (IPC)                                  │
│         ▼                                                      │
│  ┌───────────────────────────────────────────────────────────┐ │
│  │                  Background Workers                         │ │
│  │                                                            │ │
│  │  ┌─────────────────┐          ┌─────────────────┐          │ │
│  │  │ Recorder Thread │          │ AI Pipeline     │          │ │
│  │  │                 │          │ Thread          │          │ │
│  │  │ • Hook 监听     │          │ • 去重+裁剪     │          │ │
│  │  │ • 截图捕获      │          │ • API 调用      │          │ │
│  │  │ • UIA 查询      │          │ • 结果解析      │          │ │
│  │  │ • 步骤构建      │          │ • 流式推送进度  │          │ │
│  │  └────────┬────────┘          └────────┬────────┘          │ │
│  │           │                            │                   │ │
│  │           ▼                            ▼                   │ │
│  │  ┌───────────────────────────────────────────────────────┐ │ │
│  │  │                SQLite (rusqlite, WAL)                  │ │ │
│  │  └───────────────────────────────────────────────────────┘ │ │
│  └───────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────┘
                                │
                                │ IPC Bridge
                                ▼
┌───────────────────────────────────────────────────────────────┐
│                    WebView Process (React)                      │
│                                                                │
│  ┌──────────┐  ┌──────────┐  ┌────────────┐  ┌─────────────┐ │
│  │  Router  │  │  Zustand │  │Tauri Event │  │ Tailwind v4 │ │
│  │ (React)  │  │  + SWR   │  │  Listener  │  │             │ │
│  └──────────┘  └──────────┘  └────────────┘  └─────────────┘ │
└───────────────────────────────────────────────────────────────┘
```

**只有两个后台线程**：Recorder（录制）和 AI Pipeline（AI 生成）。没有 Share Server，没有 WebSocket。

---

## 三、Rust 模块树（精简版）

```
src-tauri/src/
│
├── main.rs                     # 入口
├── lib.rs                      # 注册 Tauri Commands
├── types.rs                    # 共享类型
├── error.rs                    # 统一错误
├── consts.rs                   # 常量
│
├── recorder/                   # 录制引擎
│   ├── mod.rs                  # 状态机 Idle→Recording→Paused→Processing→Completed
│   ├── capture.rs              # 截图：BitBlt / DXGI / PrintWindow
│   ├── listener.rs             # 全局 Hook：WH_MOUSE_LL + WH_KEYBOARD_LL
│   ├── uia.rs                  # UIAutomation 元素查询
│   ├── step_builder.rs         # 原始事件 → Step
│   ├── dedup.rs                # 去重（连续点击合并 / 静止帧丢弃）
│   ├── threshold.rs            # 像素差阈值检测
│   ├── sensitive_check.rs      # 密码框 / 敏感窗口跳过
│   └── screenshot_writer.rs    # 截图异步写入
│
├── ai/                         # AI 引擎
│   ├── mod.rs                  # AiEngine 入口
│   ├── pipeline.rs             # 流水线编排
│   ├── dedup.rs                # AI 级去重
│   ├── crop.rs                 # 智能裁剪（Before/After diff → bbox）
│   ├── client.rs               # HTTP 客户端（多模型）
│   ├── prompt.rs               # 中文 Prompt 模板
│   ├── parser.rs               # AI 响应解析（JSON 容错）
│   ├── chinese_ui.rs           # 中文 UI 术语词表
│   ├── deidentify.rs           # 脱敏（OCR + 正则 + 模糊）
│   └── key_manager.rs          # API Key AES-GCM 加密存储
│
├── editor/                     # 编辑器后端
│   ├── mod.rs
│   ├── commands.rs             # 步骤 CRUD
│   ├── reorder.rs              # 拖拽重排
│   ├── merge.rs                # 步骤合并
│   ├── batch.rs                # 批量操作
│   └── history.rs              # 撤销/重做
│
├── export/                     # 导出引擎
│   ├── mod.rs
│   ├── pdf.rs                  # PDF (printpdf)
│   ├── html.rs                 # HTML 单文件查看器
│   ├── markdown.rs             # Markdown
│   ├── long_image.rs           # 微信长图 (image crate 拼接)
│   └── storage.rs              # 导出历史
│
├── db/                         # 数据库
│   ├── mod.rs                  # 连接池
│   ├── schema.rs               # DDL
│   ├── recordings.rs           # Recording CRUD
│   ├── steps.rs                # Step CRUD
│   └── migration.rs            # Schema 升级
│
├── settings/                   # 设置
│   ├── mod.rs
│   ├── config.rs               # JSON 配置读写
│   └── defaults.rs             # 默认值
│
├── hotkey/                     # 全局快捷键
│   ├── mod.rs                  # RegisterHotKey / UnregisterHotKey
│   └── actions.rs              # 快捷键 → 动作映射
│
└── update/                     # 自动更新
    ├── mod.rs                  # 版本检查 + 下载 + 安装
    └── release.rs              # GitHub Release
```

**砍掉的模块**：`share/` 整个目录删除。

---

## 四、核心结构体

```rust
// types.rs

/// 录制状态机
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RecordingStatus {
    Idle,
    Recording { started_at: DateTime<Local>, step_count: u32 },
    Paused,
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

/// 操作类型
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

/// 单步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub recording_id: String,
    pub index: u32,
    pub action_type: ActionType,
    pub before_screenshot: String,       // 本地路径
    pub after_screenshot: String,
    pub element_info: Option<ElementInfo>,
    pub position: Option<ScreenPosition>,
    pub ai_title: Option<String>,
    pub ai_description: Option<String>,
    pub user_title: Option<String>,
    pub user_description: Option<String>,
    pub annotations: Vec<Annotation>,
    pub redactions: Vec<Redaction>,
}

/// UIA 元素信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementInfo {
    pub control_type: String,
    pub name: Option<String>,
    pub automation_id: Option<String>,
    pub class_name: Option<String>,
    pub bounding_rect: Option<Rect>,
    pub is_password: bool,
}

/// 标注
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub annotation_type: AnnotationType,
    pub rect: Rect,
    pub color: String,                 // 默认 #F97316 (Scribe 橙)
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AnnotationType {
    Highlight,
    Circle,
    Arrow { direction: ArrowDirection },
    Number,
    TextOverlay { text: String },
}

/// 脱敏
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

**砍掉的字段**：`is_sensitive` 不再需要持久化在 Step 中——敏感步骤录制时当场跳过截图，不需要事后标记。

---

## 五、录制引擎

### 5.1 状态机

```
              ┌──────────┐
              │   Idle   │  ← 初始 / 取消后
              └────┬─────┘
                   │ start_recording()
                   ▼
              ┌──────────┐
    ┌────────►│Recording │◄────────┐
    │         └────┬─────┘         │
    │ pause()     │  stop()        │ resume()
    │              │                │
    │         ┌────▼─────┐          │
    │         │Processing│          │
    │         └────┬─────┘          │
    │              │ complete()     │
    │              ▼                │
    │         ┌──────────┐          │
    │         │Completed │          │
    │         └──────────┘          │
    │                               │
    └─────── Paused ◄───────────────┘
              │ cancel()
              ▼
         ┌──────────┐
         │   Idle   │  (丢弃本次所有步骤)
         └──────────┘
```

### 5.2 事件处理流水线

```
Windows Hook (WH_MOUSE_LL + WH_KEYBOARD_LL)
                │
                │  RawEvent
                ▼
┌─────────────────────────────────────────────────┐
│  1. 过滤                                        │
│     • 丢弃录制窗口自身事件                        │
│     • 丢弃纯鼠标移动（非点击）                     │
│     • 丢弃系统组合键                              │
│     • <200ms 连续操作合并                         │
└────────┬────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────┐
│  2. 敏感检测                                     │
│     • UIA PasswordMask → 跳过截图                │
│     • 黑名单窗口 → 跳过                          │
└────────┬────────────────────────────────────────┘
         │ 通过
         ▼
┌─────────────────────────────────────────────────┐
│  3. Before/After 截图                            │
│     Before: 操作前 100ms                         │
│     After:  操作后 500ms（等 UI 刷新）            │
│     实现：BitBlt 全屏 → 裁剪到活动窗口            │
└────────┬────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────┐
│  4. UIA 查询                                     │
│     ElementFromPoint(x, y)                       │
│     → control_type / name / automation_id        │
│     → bounding_rect                              │
└────────┬────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────┐
│  5. 构建 Step                                    │
│     RawEvent + Screenshots + ElementInfo → Step  │
└────────┬────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────┐
│  6. 去重                                         │
│     • After 截图像素差 < threshold → 丢弃        │
│     • 同类型 + 同位置连续操作 → 合并              │
└────────┬────────────────────────────────────────┘
         │ 通过
         ▼
┌─────────────────────────────────────────────────┐
│  7. 持久化                                       │
│     • 截图写入 recordings/{id}/steps/{N}/        │
│     • INSERT INTO steps                          │
│     • Tauri Event → 前端 StepAdded               │
└─────────────────────────────────────────────────┘
```

### 5.3 录制浮窗控件

对标 Scribe 桌面端录制体验：屏幕右下角浮动控件。

```
┌────────────────────────────────┐
│                         ┌────┐ │
│                         │ ⏺ │ │  红点闪烁 = 录制中
│                         └────┘ │
│                  点击红点展开：  │
│                  ┌──────────┐  │
│                  │ ⏸  暂停   │  │
│                  │ ✓  完成   │  │
│                  │ ✕  取消   │  │
│                  └──────────┘  │
│                                │
│   Windows 桌面                  │
└────────────────────────────────┘
```

**实现方式**：Tauri 的无边框透明窗口（`always_on_top: true`），非侵入式覆盖在桌面。录制时显示闪烁红点，鼠标悬停显示步骤计数，点击展开操作菜单。录制完成后自动关闭。

---

## 六、AI 引擎

### 6.1 流水线

```
Recording (steps[]) 完成
         │
         ▼
┌─────────────────────────────────────────────────┐
│  Stage 0: Preprocess                             │
│  ┌───────────────────────────────────────────┐  │
│  │ a. AI 级去重                               │  │
│  │    • 合并连续 Scroll（首尾）               │  │
│  │    • 合并导航系列（URL 输入 + Enter）       │  │
│  │    • 丢弃 < 3 步的录制                     │  │
│  │ b. 智能裁剪 After 截图                     │  │
│  │    • Before/After diff → 变化区域 bbox      │  │
│  │    • 裁剪 bbox + 32px padding              │  │
│  │ c. 脱敏                                    │  │
│  │    • OCR → 正则匹配 6 类敏感信息 → 模糊     │  │
│  └───────────────────────────────────────────┘  │
└────────┬────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────┐
│  Stage 1: AI 生成（并行批量）                    │
│                                                  │
│  每批 5 步，最多 3 并发，间隔 200ms               │
│  模型：GLM-4-flash（默认）/ DeepSeek-V3 / 通义千问 │
│                                                  │
│  每步发送：Before 裁剪截图 + After 裁剪截图        │
│  Prompt：中文正向引导 + 中文 UI 术语              │
│  期望输出：{ title: "…", description: "…" }       │
└────────┬────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────┐
│  Stage 2: Parse & Validate                       │
│  ┌───────────────────────────────────────────┐  │
│  │ a. JSON 提取（正则/代码块/{}匹配）          │  │
│  │ b. 格式修复（补引号/补逗号/截断修复）        │  │
│  │ c. 校验：title 2-20字动词开头               │  │
│  │          description 10-200字               │  │
│  │    不合格 → 回退重试（最多 1 次）            │  │
│  │ d. 中文术语校验（chinese_ui 词表）           │  │
│  └───────────────────────────────────────────┘  │
└────────┬────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────┐
│  Stage 3: Persist                                │
│  ┌───────────────────────────────────────────┐  │
│  │ • UPDATE steps SET ai_title/ai_description  │  │
│  │ • UPDATE recordings SET ai_generated=true   │  │
│  │ • 每 Batch 完成 → Tauri Event (progress%)   │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

### 6.2 模型支持

| 模型 | 多模态 | 中文效果 | 默认 |
|------|:---:|:---:|:---:|
| 智谱 GLM-4-flash | ✅ | ⭐⭐⭐⭐⭐ | 是 |
| 通义千问 VL | ✅ | ⭐⭐⭐⭐☆ | 否 |
| DeepSeek-V3 | ❌ 无视觉 | ⭐⭐⭐⭐⭐ | 否（仅文字润色） |
| 自定义（OpenAI 兼容） | — | — | 否 |

### 6.3 Prompt 模板

```rust
struct PromptTemplate {
    system: String,
    user: String,
    output_schema: String,
}
```

- **system**：Scribe 风格中文操作指南专家角色，要求识别中文 UI 元素、动词开头标题、使用规范术语
- **user**：传入「{app_name}」和 {N} 张 Before+After 截图，要求根据 After 截图推断操作
- **output_schema**：严格 JSON，`{ "steps": [{ "step_number": 1, "title": "…", "description": "…" }] }`

### 6.4 脱敏覆盖

| 类型 | 正则 | 示例 |
|------|------|------|
| 身份证号 | `\d{17}[\dXx]` | 110101199001011234 |
| 银行卡号 | `\d{16,19}` | 6222021234567890123 |
| 手机号 | `1[3-9]\d{9}` | 13800138000 |
| 邮箱 | `[\w.-]+@[\w.-]+\.\w+` | user@example.com |
| 统一社会信用代码 | `[0-9A-HJ-NPQRTUWXY]{18}` | 91110000123456789A |
| 车牌号 | `[京津沪渝冀豫云辽黑湘皖鲁新苏浙赣鄂桂甘晋蒙陕吉闽贵粤川青藏琼宁][A-Z][A-HJ-NP-Z0-9]{4,5}[A-HJ-NP-Z0-9挂学警港澳]` | 京A12345 |

---

## 七、编辑器

### 7.1 路由

```
/                              → Dashboard (录制历史列表)
/record                        → Recorder (录制中 + 浮窗控件)
/editor/:recordingId           → Editor (编辑页，核心)
/settings                      → Settings
/settings/ai                   → AI 设置（模型选择 / API Key）
/settings/shortcuts            → 快捷键设置
/settings/appearance           → 外观设置（主题/语言）
```

### 7.2 编辑器布局（对标 Scribe 桌面版）

```
┌──────────────────────────────────────────────────────────────────┐
│  [← 返回]  [文档标题_____________________]  [导出▼]  [···设置]  │
├──────────────┬───────────────────────────────────────────────────┤
│              │                                                    │
│  ① 点击登录   │  ┌────────────────────────────────────────────┐  │
│  ┌──────┐    │  │          After 截图（全宽）                  │  │
│  │ 缩略图│    │  │                                            │  │
│  └──────┘    │  │      ┌──────────┐  ← 橙色圆环标注          │  │
│  ② 输入账号   │  │      │   登录    │                         │  │
│  ┌──────┐    │  │      └──────────┘                          │  │
│  │ 缩略图│    │  └────────────────────────────────────────────┘  │
│  └──────┘    │                                                    │
│  ③ 点击登录   │  ① 点击登录按钮                                   │
│  ┌──────┐    │  在窗口右上角找到「登录」按钮并点击。               │
│  │ 缩略图│    │                                                    │
│  └──────┘    │  [Before/After 切换]                                │
│              │                                                    │
│              │  ── 标注工具栏 ──                                   │
│              │  [高亮] [圆形] [箭头] [编号] [文字] [模糊]          │
│              │                                                    │
│  [+ 添加步骤] │                                                    │
│              │                                                    │
└──────────────┴───────────────────────────────────────────────────┘
  左侧 300px                       右侧（自适应）
```

### 7.3 组件树

```
<EditorPage>
├── <EditorToolbar>
│   ├── <BackButton />
│   ├── <RecordingTitle />                # 可编辑标题
│   ├── <AppBadge />                      # 应用图标+名称
│   ├── <AiGenerateButton />              # AI 生成 + 进度条
│   ├── <UndoRedoButtons />
│   ├── <ExportMenu />                    # PDF / HTML / MD / 微信长图
│   └── <SettingsButton />
│
├── <EditorBody>
│   ├── <StepList sidebar>                # 左侧 300px
│   │   ├── <StepListItem /> × N
│   │   │   ├── <StepNumber />            # 蓝色圆形序号
│   │   │   ├── <Thumbnail />             # After 缩略图
│   │   │   ├── <StepTitle preview />     # 标题摘要
│   │   │   ├── <StepActions />           # 删除/合并
│   │   │   └── <DragHandle />
│   │   └── <AddStepButton />
│   │
│   └── <StepDetail main>
│       ├── <StepHeader>
│       │   ├── <StepTitleEditor />       # 内联编辑
│       │   └── <ActionTypeBadge />       # 点击/输入/滚动
│       ├── <ScreenshotViewer>
│       │   ├── <BeforeAfterToggle />
│       │   ├── <ScreenshotCanvas>
│       │   │   ├── <AnnotationLayer />   # SVG 标注层
│       │   │   │   ├── <HighlightRect />
│       │   │   │   ├── <CircleAnnotation />
│       │   │   │   ├── <ArrowAnnotation />
│       │   │   │   └── <NumberBadge />
│       │   │   └── <RedactionLayer />    # 脱敏层
│       │   │       ├── <BlurRegion />
│       │   │       └── <SolidColorRegion />
│       │   └── <AnnotationToolbar />
│       ├── <StepDescriptionEditor />
│       ├── <TipSection />               # 可折叠提示区
│       └── <ActionInfoPanel />          # 坐标/元素信息/时间
│
└── <EditorFooter>                       # 步骤计数 + 保存状态
```

### 7.4 编辑交互

| 操作 | 触发 | 效果 |
|------|------|------|
| 选中步骤 | 点击左侧步骤 | 右侧切换为该步骤详情 |
| 编辑标题 | 点击标题文字 | 内联编辑，Enter 确认 |
| 编辑描述 | 点击描述区域 | 展开文本编辑区 |
| 拖拽排序 | 长按步骤条目拖动 | 改变步骤顺序 |
| 合并步骤 | 拖到相邻步骤上 | 合并截图+文字 |
| 删除步骤 | 悬停 × 按钮 | 移除该步骤 |
| 添加步骤 | 底部 [+] 按钮 / 两步之间的 [+] | 在末尾/指定位置插入空步骤 |
| Before/After 切换 | 点击切换按钮 | 截图区域显示 Before 或 After |
| 添加标注 | 选中工具 → 在截图上绘制 | 叠加标注元素 |
| 添加脱敏 | 选中模糊工具 → 涂抹区域 | 叠加高斯模糊/纯色块 |
| 添加提示 | 步骤下方「添加提示」 | 插入可折叠提示文字块 |
| 撤销/重做 | Ctrl+Z / Ctrl+Y | 回退/前进编辑操作 |

---

## 八、导出引擎

| 格式 | 实现 | 说明 |
|------|------|------|
| **PDF** | `printpdf` crate | A4 排版，每页一步骤，Before+After 截图并排 |
| **HTML** | 模板渲染 | 单文件离线查看器，内联 CSS+JS，左右分栏只读 |
| **Markdown** | 模板拼接 | 纯文本 + 本地图片引用，适合 Git 仓库 |
| **微信长图** | `image` crate 纵向拼接 | 每步 After 截图 + 标题 + 描述，末尾 QR 码 + 水印 |

---

## 九、数据库（SQLite）

### 9.1 Schema

```sql
CREATE TABLE recordings (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT '未命名录制',
    app_name TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'draft',
    step_count INTEGER NOT NULL DEFAULT 0,
    ai_generated INTEGER NOT NULL DEFAULT 0,
    thumbnail_path TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE steps (
    id TEXT PRIMARY KEY,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    step_index INTEGER NOT NULL,
    action_type TEXT NOT NULL,
    before_screenshot TEXT NOT NULL,
    after_screenshot TEXT NOT NULL,
    element_name TEXT,
    element_type TEXT,
    position_x REAL,
    position_y REAL,
    ai_title TEXT,
    ai_description TEXT,
    user_title TEXT,
    user_description TEXT,
    annotations TEXT DEFAULT '[]',    -- JSON
    redactions TEXT DEFAULT '[]',     -- JSON
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE export_history (
    id TEXT PRIMARY KEY,
    recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
    format TEXT NOT NULL,
    output_path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE api_keys (
    model TEXT PRIMARY KEY,
    encrypted_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);
```

### 9.2 文件系统布局

```
%APPDATA%/com.flowio.app/
│
├── flowio.db
│
├── recordings/
│   └── {recording_id}/
│       ├── thumbnail.jpg
│       └── steps/
│           ├── 000/
│           │   ├── before.png
│           │   ├── after.png
│           │   └── after_cropped.png
│           ├── 001/
│           └── ...
│
├── exports/
│   └── {title}_{timestamp}/
│
├── settings.json
└── logs/
    ├── app.log
    └── recorder.log
```

---

## 十、前端状态管理

```
┌──────────────────────────────────────────────┐
│               Zustand Stores                  │
│                                               │
│  ┌───────────────┐  ┌──────────────────────┐ │
│  │  useAppStore   │  │  useRecordingStore   │ │
│  │               │  │                      │ │
│  │ • theme       │  │ • currentRecording   │ │
│  │ • locale      │  │ • steps[]            │ │
│  │               │  │ • selectedStepIdx    │ │
│  └───────────────┘  │ • unsavedChanges     │ │
│                     │ • undoStack          │ │
│  ┌───────────────┐  │ • redoStack          │ │
│  │  useAiStore   │  └──────────────────────┘ │
│  │               │                            │
│  │ • progress    │                            │
│  │ • status      │                            │
│  │ • error       │                            │
│  └───────────────┘                            │
│                                               │
│  ┌──────────────────────────────────────────┐ │
│  │           SWR (数据获取)                   │ │
│  │  • useRecordings() → 录制列表             │ │
│  │  • useSettings()   → 全局设置             │ │
│  │  • useExportUrl()  → 导出状态             │ │
│  └──────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘
```

---

## 十一、Tauri Command 清单

```rust
// === 录制 ===
fn start_recording(app_name: String) -> Result<(), String>
fn pause_recording() -> Result<(), String>
fn resume_recording() -> Result<(), String>
fn stop_recording(title: String) -> Result<Recording, String>
fn cancel_recording() -> Result<(), String>
fn get_recording_status() -> Result<RecordingStatus, String>

// === AI ===
fn generate_ai(recording_id: String) -> Result<(), String>
fn get_ai_progress() -> Result<AiProgress, String>
fn cancel_ai_generation() -> Result<(), String>

// === 编辑器 ===
fn get_recording(recording_id: String) -> Result<Recording, String>
fn update_step_title(step_id: String, title: String) -> Result<(), String>
fn update_step_description(step_id: String, desc: String) -> Result<(), String>
fn delete_step(step_id: String) -> Result<(), String>
fn reorder_steps(recording_id: String, from: usize, to: usize) -> Result<(), String>
fn merge_steps(step_a: String, step_b: String) -> Result<Step, String>
fn add_step(recording_id: String, after_index: usize) -> Result<Step, String>
fn recapture_screenshot(step_id: String) -> Result<String, String>
fn add_annotation(step_id: String, annotation: Annotation) -> Result<(), String>
fn remove_annotation(step_id: String, annotation_id: String) -> Result<(), String>
fn add_redaction(step_id: String, redaction: Redaction) -> Result<(), String>

// === 导出 ===
fn export_pdf(recording_id: String, output_path: Option<String>) -> Result<String, String>
fn export_html(recording_id: String, output_path: Option<String>) -> Result<String, String>
fn export_markdown(recording_id: String, output_path: Option<String>) -> Result<String, String>
fn export_long_image(recording_id: String, output_path: Option<String>) -> Result<String, String>

// === 管理 ===
fn list_recordings(filter: Option<String>) -> Result<Vec<RecordingSummary>, String>
fn delete_recording(recording_id: String) -> Result<(), String>
fn duplicate_recording(recording_id: String) -> Result<Recording, String>

// === 设置 ===
fn get_settings() -> Result<Settings, String>
fn update_settings(settings: Settings) -> Result<(), String>
fn validate_api_key(model: String, key: String) -> Result<bool, String>
```

---

## 十二、性能目标

| 指标 | 目标 |
|------|------|
| 冷启动 | < 2s |
| 录制内存 | < 200MB（30 分钟录制） |
| Hook → 截图延迟 | < 200ms |
| AI 单步耗时 | < 3s |
| AI 20 步总耗时 | < 30s（3 并发） |
| 编辑器 100 步渲染 | < 200ms（First Paint） |
| PDF 导出 20 步 | < 5s |
| 安装包大小 | < 15MB |
| 磁盘占用（30min 录制） | < 100MB (JPEG q=75) |

---

> 工具版架构只做四件事：**录得好、生成得准、编辑得顺手、导出得漂亮**。插件和生态是下一阶段的命题。
*（内容由AI生成，仅供参考）*
