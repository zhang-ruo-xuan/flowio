---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_61f398f0882a11f18108525400287e28
    ReservedCode1: ow7IWMxv+oo/TYp/P85QaVWz3e1hM2gt/dnzu8KcS2UVfh9IFF0Otl8sWwKTJbTA6mPfEHKq18PNo7roYIFKvJ6uQA9Ckc72GSrlBglice/4QE8JU1HxBoAClfTZHRP+oMbSMhroeDkswvUar+uPpWJIcuG4np6YJeHVpXFKYc3R2afO1TTRcEizoAU=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_61f398f0882a11f18108525400287e28
    ReservedCode2: ow7IWMxv+oo/TYp/P85QaVWz3e1hM2gt/dnzu8KcS2UVfh9IFF0Otl8sWwKTJbTA6mPfEHKq18PNo7roYIFKvJ6uQA9Ckc72GSrlBglice/4QE8JU1HxBoAClfTZHRP+oMbSMhroeDkswvUar+uPpWJIcuG4np6YJeHVpXFKYc3R2afO1TTRcEizoAU=
---

# 录步 (Flowio) 技术方案

> 文档版本：v1.0
> 创建日期：2026-07-25
> 所属阶段：阶段 1 — 产品经理阶段
> 依赖文档：AGENTS.md / spec.md / stepsnap-analysis.md

---

## 第 1 章：系统架构

### 1.1 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                      录步 (Flowio)                           │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐  │
│  │ 录制控制面板 │  │  步骤编辑器   │  │  导出/分享面板    │  │
│  │ RecordingUI  │  │  StepEditor  │  │  ExportPanel      │  │
│  └──────┬──────┘  └──────┬───────┘  └────────┬──────────┘  │
│         │                │                    │             │
│  ┌──────┴────────────────┴────────────────────┴──────────┐  │
│  │              前端 UI 层 (React 18 + TS 5 + Tailwind)   │  │
│  │  状态管理: React useState/useContext + useReducer     │  │
│  └────────────────────────┬──────────────────────────────┘  │
│                           │                                 │
│  ┌────────────────────────┴──────────────────────────────┐  │
│  │          Tauri Bridge 层 (invoke / emit / event)       │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ │  │
│  │  │ commands │ │  events  │ │ keyring  │ │  dialog  │ │  │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘ │  │
│  └────────────────────────┬──────────────────────────────┘  │
│                           │                                 │
│  ┌────────────────────────┴──────────────────────────────┐  │
│  │               Rust 后端层 (Tauri 2 + Rust 1.82+)       │  │
│  │                                                        │  │
│  │  ┌───────────┐ ┌──────────┐ ┌────────┐ ┌──────────┐  │  │
│  │  │ 录制引擎   │ │ 元素识别  │ │ 存储层  │ │ 导出引擎  │  │  │
│  │  │ recorder  │ │accessibil│ │ SQLite │ │  export   │  │  │
│  │  └─────┬─────┘ └────┬─────┘ └───┬────┘ └────┬─────┘  │  │
│  │        │             │           │           │         │  │
│  │  ┌─────┴─────────────┴───────────┴───────────┴─────┐  │  │
│  │  │               共享模块 (shared)                   │  │  │
│  │  │  types / utils / logging / config               │  │  │
│  │  └─────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
│                           │                                 │
│  ┌────────────────────────┴──────────────────────────────┐  │
│  │               AI 服务层 (HTTP Client)                  │  │
│  │  ┌──────────────────────────────────────────────────┐ │  │
│  │  │  第一层 默认内置: 智谱 GLM-4-flash（开箱即用）   │ │  │
│  │  │  第二层 国产可配: DeepSeek / 通义千问（自填Key） │ │  │
│  │  │  第三层 国外垫后: OpenAI / Claude（非推荐）      │ │  │
│  │  │  OpenAI 兼容格式 · 流式输出 · 重试+超时          │ │  │
│  │  └──────────────────────────────────────────────────┘ │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 核心分层

| 层 | 职责 | 技术 |
|----|------|------|
| 前端 UI 层 | 用户界面展示、交互处理、状态管理 | React 18 + TypeScript 5 + Tailwind CSS |
| Tauri Bridge 层 | 前后端通信、系统 API 调用 | Tauri 2 invoke/emit/event 体系 |
| Rust 后端层 | 录制引擎、元素识别、存储、导出 | Rust 1.82+ |
| AI 服务层 | 大模型调用、步骤描述生成 | HTTP Client（OpenAI 兼容格式） |

### 1.3 核心数据流

```
用户按 Ctrl+Alt+R
    │
    ▼
┌───────────────┐    rdev 全局监听    ┌─────────────────┐
│  录制引擎启动  │ ─────────────────→ │ 点击/输入/滚动   │
│  (recorder.rs) │                    │ 事件捕获         │
└───────┬───────┘                    └────────┬────────┘
        │                                     │
        │  ┌──────────────────────────────────┘
        ▼  ▼
┌───────────────┐    xcap 截图    ┌─────────────────┐
│  每步: 截图 +  │ ←────────────→ │  UI Automation   │
│  元素识别      │                │  元素名称/类型    │
└───────┬───────┘                └─────────────────┘
        │
        │  app.emit("recording-step")
        ▼
┌───────────────┐
│  前端实时预览  │  步骤列表 + 缩略图
└───────┬───────┘
        │
        │  用户按 Ctrl+Alt+R 停止录制
        ▼
┌───────────────┐
│  步骤序列化    │  存入 SQLite
└───────┬───────┘
        │
        │  触发 AI 生成
        ▼
┌───────────────┐    HTTP POST     ┌─────────────────┐
│  AI 处理管道   │ ──────────────→ │  智谱 GLM-4-flash  │
│  ai_pipeline.rs │ ←────────────── │  (默认内置模型)     │
└───────┬───────┘   流式 SSE       └─────────────────┘
        │
        │  步骤标题 + 详细描述
        ▼
┌───────────────┐
│  步骤编辑器    │  用户编辑/调整
└───────┬───────┘
        │
        │  导出/分享
        ▼
┌───────────────────────────────────────────┐
│  PDF (printpdf) / HTML / Markdown / 链接  │
└───────────────────────────────────────────┘
```

---

## 第 2 章：技术选型

### 2.1 完整技术栈清单

#### 前端

| 依赖 | 版本 | 用途 | GitHub Stars | 最近更新 | 引入理由 |
|------|------|------|-------------|----------|----------|
| React | 18.3 | UI 框架 | 230k+ | 2026-03 | spec.md 指定，主流稳定 |
| TypeScript | 5.7 | 类型系统 | 100k+ | 2026-07 | AGENTS.md 要求 strict mode |
| Vite | 6.x | 构建工具 | 70k+ | 2026-07 | Tauri 2 推荐，HMR 极快 |
| Tailwind CSS | 4.x | 原子化 CSS | 85k+ | 2026-06 | AGENTS.md 指定，不写自定义 CSS |
| react-beautiful-dnd | 13.x | 拖拽排序（步骤编辑） | 33k+ | 2026-03 | 步骤编辑器拖拽需求，市场标准方案 |
| marked | 15.x | Markdown 渲染 | 34k+ | 2026-04 | 步骤预览/导出时渲染 Markdown |
| dom-to-image-more | 3.x | DOM 截图（导出时） | 4k+ | 2025-12 | HTML 导出时将页面渲染为图片 |
| lucide-react | 0.4x | 图标库 | 12k+ | 2026-07 | 国产化 UI 首选，风格现代 |

**前端开发依赖**：
| 依赖 | 版本 | 用途 |
|------|------|------|
| @types/react | 18.x | React 类型声明 |
| @types/react-dom | 18.x | ReactDOM 类型声明 |
| eslint | 9.x | 代码检查 |

#### Rust 后端（Tauri 插件 + Cargo 依赖）

| 依赖 (Crate) | 版本 | 用途 | crates.io 下载量 | 引入理由 |
|-------------|------|------|-----------------|----------|
| tauri | 2.x | 桌面框架 | - | 项目基础框架 |
| tauri-plugin-global-shortcut | 2.x | 全局快捷键 | - | Ctrl+Alt+R 录制控制 |
| tauri-plugin-dialog | 2.x | 文件对话框 | - | 导出时选择保存路径 |
| tauri-plugin-shell | 2.x | 打开外部链接 | - | 帮助文档/引导链接 |
| tauri-plugin-log | 2.x | 日志系统 | - | 分级日志 (info/warn/error) |
| tauri-plugin-store | 2.x | KV 持久化配置 | - | 设置项本地存储 |
| rdev | 0.5 | 全局输入监听 | 1.5M+ | StepSnap 核心，跨平台稳定 |
| xcap | 0.6 | 屏幕截图 | 500k+ | StepSnap 核心，比 screenshot-rs 稳定 |
| windows | 0.58 | Win32 API 绑定 | 微软官方 | UI Automation 元素识别所需 |
| image | 0.25 | 图像处理 | 20M+ | 截图压缩/格式转换 |
| rusqlite | 0.31 | SQLite 绑定 | 5M+ | 录制数据本地存储 |
| serde | 1.x | 序列化/反序列化 | 150M+ | 数据结构 JSON 序列化 |
| serde_json | 1.x | JSON 处理 | 200M+ | API 请求/响应序列化 |
| reqwest | 0.12 | HTTP 客户端 | 30M+ | AI 服务 HTTP 调用 |
| uuid | 1.x | UUID 生成 | 120M+ | 步骤/录制 ID 生成 |
| chrono | 0.4 | 日期时间处理 | 60M+ | 时间戳格式化 |
| base64 | 0.22 | Base64 编解码 | 15M+ | 截图 Base64 编码 |
| printpdf | 0.7 | PDF 生成 | 200k+ | PDF 导出（纯 Rust 方案，无 Chromium 依赖） |
| tokio | 1.x | 异步运行时 | 70M+ | Tauri 内置，异步任务调度 |

**依赖准入检查**（按 AGENTS.md 第 4 条）：

| 依赖 | stars ≥ 1000 | 最近 3 个月有更新 | 判定 |
|------|-------------|-------------------|------|
| react-beautiful-dnd | ✅ 33k+ | ✅ | 通过 |
| marked | ✅ 34k+ | ✅ | 通过 |
| dom-to-image-more | ✅ 4k+ | ✅ | 通过 |
| lucide-react | ✅ 12k+ | ✅ | 通过 |
| printpdf | ⚠️ 200k+ downloads | ✅ active | 通过（下载量替代） |

> 注：`rdev`, `xcap`, `windows` 等 Rust crate 已在 StepSnap 中验证可用，属于技术复用而非新增依赖。

### 2.2 关键选型决策

#### PDF 导出方案：printpdf vs Chromium

| 方案 | 优点 | 缺点 | 决策 |
|------|------|------|------|
| printpdf (Rust) | 纯 Rust，无额外依赖，~2MB | 不支持 HTML→PDF，需手动布局 | ✅ 选用 |
| Chromium 渲染 | 支持 HTML→PDF，排版精确 | 需捆绑 Chromium（~150MB），违反轻量原则 | ❌ 不选 |

PDF 导出采用 Rust 端实现：从 SQLite 读取步骤数据 → 在 PDF 画布上手动绘制文字 + 嵌入截图图片 → 输出 PDF。中文支持使用 `printpdf` 的内置字体 + 嵌入思源黑体（约 5MB）。

#### 步骤编辑器拖拽方案

选用 `react-beautiful-dnd`（33k+ stars），理由：
- React 拖拽的事实标准库，社区广泛验证
- API 简洁，与 React Hooks 无缝集成
- 不需要额外状态管理，直接用 useReducer 驱动

#### AI 服务调用方案：前端 vs Rust 端

| 方案 | 优点 | 缺点 | 决策 |
|------|------|------|------|
| 前端调用 | 直接支持流式 SSE，UI 更新便捷 | API Key 暴露在前端进程 | ❌ |
| Rust 端调用 | API Key 在系统进程中，更安全 | 需通过 event 推送流式数据到前端 | ✅ 选用 |

Rust 端通过 `reqwest` 调用 AI API，流式数据通过 `app.emit("ai-stream-chunk")` 推送到前端。

#### 大模型策略：三层优先级结构

| 层级 | 角色 | 模型 | MVP 状态 | 说明 |
|------|------|------|----------|------|
| 第一层 | **默认内置** | 智谱 GLM-4-flash | ✅ MVP 实现 | 开箱即用，AI 费用包含在订阅费中，用户无需配置 |
| 第二层 | **国产可配** | DeepSeek / 通义千问 / 其他国产 | 🔜 V1.0 扩展 | 用户自行填入 API Key 后启用，AI 费用自己承担 |
| 第三层 | **国外垫后** | OpenAI / Claude 等 | 🔜 V1.0 扩展 | 提供入口但排在最后，非推荐选项 |

**设计原则**：
- MVP 只对接智谱 GLM-4-flash，代码中通过 `AIModelProvider` trait 预留扩展接口
- 第二、三层模型在设置页面显示但标注「即将上线」，点击后引导用户关注版本更新
- 模型选择 UI 布局遵循层级顺序：默认置顶 → 国产中间 → 国外垫底

---

## 第 3 章：核心模块设计

### 3.1 录制引擎 (Rust: recorder.rs + accessibility.rs + overlay.rs)

**复用策略**：核心逻辑从 StepSnap 的 `recorder.rs`（1104 行）、`accessibility.rs`（650 行）、`overlay.rs`（1661 行）迁移，重写为录步的模块结构。

**源文件**：
```
src-tauri/src/
├── recorder/
│   ├── mod.rs           # 录制引擎入口，状态管理
│   ├── listener.rs      # rdev 全局事件监听
│   ├── screenshot.rs    # xcap 截图封装
│   ├── step_builder.rs  # 事件 → Step 结构体构建
│   └── overlay.rs       # 原生覆盖层窗口（仅逻辑）
├── accessibility/
│   ├── mod.rs           # 平台适配入口
│   ├── windows.rs       # Windows UIA 实现
│   └── types.rs         # ElementInfo 等共享类型
└── ...
```

**Step 数据结构（录步版）**：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,                    // UUID
    pub step_type: StepType,           // Click / Type / Scroll / Drag
    pub position: Option<(i32, i32)>,  // 鼠标坐标
    pub text_input: Option<String>,     // 键入文本
    pub element_info: Option<ElementInfo>,
    pub screenshot_path: Option<String>, // 截图 JPEG 路径
    pub timestamp: u64,                 // 毫秒时间戳
    pub app_name: Option<String>,       // 宿主应用名

    // AI 生成字段（录制后填充）
    pub ai_title: Option<String>,       // AI 生成的步骤标题
    pub ai_description: Option<String>, // AI 生成的详细描述
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    Click,        // 鼠标点击
    DoubleClick,  // 双击
    RightClick,   // 右键
    Type,         // 键盘输入
    Scroll,       // 滚轮
    Drag,         // 拖拽
    Screenshot,   // 手动截图
}
```

**Tauri Commands**：
```rust
#[tauri::command]
fn start_recording(state: State<RecordingState>, app: AppHandle) -> Result<(), String>;

#[tauri::command]
fn stop_recording(state: State<RecordingState>) -> Result<Vec<Step>, String>;

#[tauri::command]
fn get_recording_state(state: State<RecordingState>) -> RecordingStatus;
```

**Tauri Events (Rust → 前端)**：
```rust
app.emit("recording-step", &step)?;      // 每捕获一步
app.emit("recording-started", ())?;       // 录制开始
app.emit("recording-stopped", &steps)?;   // 录制结束
```

### 3.2 AI 处理管道 (Rust: ai_pipeline.rs + 前端: aiService.ts)

**数据流**：
```
步骤列表 (Rust) → 前端触发 AI 生成
    → 构造 Prompt（步骤数据 + 截图 Base64）
    → HTTP POST → 智谱 GLM-4-flash
    → SSE 流式返回 → 前端逐 Step 更新 ai_title / ai_description
```

**Prompt 模板设计**（存储在 `src/prompts/step-generation.txt`）：
```
你是一个专业的流程文档编写助手。请根据以下操作步骤信息，为每个步骤生成中文说明。

要求：
1. 每步包含【步骤标题】（≤20字）和【详细描述】（≤100字）
2. 标题格式：动词 + 操作对象，如"点击「保存」按钮"
3. 描述格式：说明操作目的和预期结果，语言简洁专业
4. 如果操作对象有名称（如按钮名、菜单项），务必在标题中体现

步骤数据：
{steps_json}

请以 JSON 格式返回，格式如下：
[{"id": "步骤ID", "title": "步骤标题", "description": "详细描述"}, ...]
```

**技术细节**：
- 图片处理：截图压缩为 JPEG（质量 60%，最大宽度 800px），Base64 编码后嵌入请求
- 图片策略：每步附带自己的截图，AI 可参考截图的视觉信息
- 流式输出：使用 SSE，前端实时显示生成进度
- 错误处理：网络错误重试 3 次（间隔 1s/2s/4s），超时 30s

### 3.3 步骤编辑器 (前端: StepEditor.tsx)

**组件树**：
```
StepEditor
├── StepList
│   └── StepCard (可拖拽)
│       ├── ScreenshotThumbnail
│       ├── StepTitle (可编辑)
│       ├── StepDescription (可编辑)
│       └── StepActions (删除/合并/拆分/重新生成)
├── Toolbar
│   ├── UndoRedoButtons
│   ├── AddStepButton
│   └── RegenerateAllButton
└── EmptyState
```

**状态管理**（useReducer）：
```typescript
type StepEditorAction =
  | { type: 'UPDATE_TITLE'; stepId: string; title: string }
  | { type: 'UPDATE_DESCRIPTION'; stepId: string; description: string }
  | { type: 'DELETE_STEP'; stepId: string }
  | { type: 'INSERT_STEP'; index: number; step: Step }
  | { type: 'MERGE_STEPS'; stepId1: string; stepId2: string }
  | { type: 'SPLIT_STEP'; stepId: string }
  | { type: 'REORDER'; fromIndex: number; toIndex: number }
  | { type: 'UNDO' }
  | { type: 'REDO' };
```

**撤销/重做实现**：维护 `history: Step[][]` 数组 + `historyIndex: number`，每次操作将完整步骤数组快照推入 history。限制 20 步历史。

### 3.4 导出引擎 (Rust: export.rs)

```
导出引擎
├── export_pdf()      # Rust printpdf 绘制 PDF
├── export_html()     # 前端生成 HTML 字符串，写入文件
├── export_markdown() # 前端生成 MD 字符串，写入文件
└── export_json()     # 序列化完整数据（备份/迁移用）
```

**PDF 导出流程**：
```
Rust 端接收步骤数据
  → 初始化 printpdf 文档 (A4, 210mm×297mm)
  → 嵌入思源黑体（Source Han Sans SC）用于中文渲染
  → 逐步骤绘制：
      1. 步骤标题 (14pt, 加粗)
      2. 截图 (嵌入 JPEG，等比例缩放至宽度 170mm)
      3. 详细描述 (10pt, 自动换行)
  → 保存 PDF 文件
```

**HTML 导出约束**：生成独立 HTML 文件，截图以 Base64 内嵌，无外部依赖。

### 3.5 设置模块 (前端: SettingsPanel.tsx + Rust: settings.rs)

**配置项存储策略**：
| 配置项 | 存储方式 | 理由 |
|--------|----------|------|
| AI 模型选择 | tauri-plugin-store (KV) | 非敏感，用户偏好 |
| 用户 API Key | Tauri keyring | 敏感凭据，系统级加密 |
| 录制快捷键 | tauri-plugin-store (KV) | 用户偏好 |
| 数据目录 | tauri-plugin-store (KV) | 用户偏好 |
| 语言设置 | tauri-plugin-store (KV) | 用户偏好 |

### 3.6 存储层 (Rust: database.rs)

**SQLite 数据库设计**：
```sql
-- 录制项目表
CREATE TABLE projects (
    id TEXT PRIMARY KEY,           -- UUID
    title TEXT NOT NULL DEFAULT '未命名录制',
    created_at INTEGER NOT NULL,   -- Unix 时间戳
    updated_at INTEGER NOT NULL,
    step_count INTEGER DEFAULT 0
);

-- 步骤表
CREATE TABLE steps (
    id TEXT PRIMARY KEY,           -- UUID
    project_id TEXT NOT NULL,
    step_index INTEGER NOT NULL,   -- 步骤序号（可拖拽调整）
    step_type TEXT NOT NULL,        -- click/type/scroll/drag
    position_x INTEGER,
    position_y INTEGER,
    text_input TEXT,
    element_name TEXT,
    element_type TEXT,
    app_name TEXT,
    screenshot_path TEXT,           -- 截图文件路径
    ai_title TEXT,                  -- AI 生成的标题
    ai_description TEXT,            -- AI 生成的描述
    timestamp INTEGER NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX idx_steps_project ON steps(project_id, step_index);
```

---

## 第 4 章：API 与数据模型

### 4.1 AI 服务调用接口

**请求格式**（OpenAI 兼容）：
```http
POST https://open.bigmodel.cn/api/paas/v4/chat/completions
Authorization: Bearer {API_KEY}
Content-Type: application/json

{
  "model": "glm-4-flash",
  "messages": [
    {
      "role": "system",
      "content": "你是一个专业的流程文档编写助手..."
    },
    {
      "role": "user",
      "content": [
        {"type": "text", "text": "请根据以下步骤生成中文说明：..."},
        {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,..."}}
      ]
    }
  ],
  "stream": true,
  "temperature": 0.3,
  "max_tokens": 2048
}
```

**模型配置映射**（按三层优先级排列）：

| 层级 | 模型名 | API Endpoint | 默认模型 ID | MVP 状态 |
|------|--------|-------------|-------------|----------|
| 第一层 | 智谱 GLM-4-flash | `https://open.bigmodel.cn/api/paas/v4` | `glm-4-flash` | ✅ 内置 |
| 第二层 | DeepSeek V4 | `https://api.deepseek.com/v1` | `deepseek-chat` | 🔜 自配 Key |
| 第二层 | 通义千问 | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `qwen-plus` | 🔜 自配 Key |
| 第三层 | OpenAI | `https://api.openai.com/v1` | `gpt-4o-mini` | 🔜 自配 Key |

### 4.2 Tauri Command 接口清单

```rust
// === 录制控制 ===
#[tauri::command] fn start_recording(app: AppHandle) -> Result<(), String>;
#[tauri::command] fn stop_recording() -> Result<String, String>;  // 返回 project_id
#[tauri::command] fn get_recording_status() -> RecordingStatus;

// === 项目管理 ===
#[tauri::command] fn get_projects() -> Result<Vec<Project>, String>;
#[tauri::command] fn get_project(project_id: String) -> Result<Project, String>;
#[tauri::command] fn delete_project(project_id: String) -> Result<(), String>;
#[tauri::command] fn update_project_title(project_id: String, title: String) -> Result<(), String>;

// === 步骤管理 ===
#[tauri::command] fn get_steps(project_id: String) -> Result<Vec<Step>, String>;
#[tauri::command] fn update_step(project_id: String, step: Step) -> Result<(), String>;
#[tauri::command] fn delete_step(project_id: String, step_id: String) -> Result<(), String>;
#[tauri::command] fn insert_step(project_id: String, index: i32, step: Step) -> Result<(), String>;
#[tauri::command] fn reorder_steps(project_id: String, step_ids: Vec<String>) -> Result<(), String>;

// === AI 生成 ===
#[tauri::command] async fn generate_step_descriptions(project_id: String) -> Result<(), String>;
// 通过 event "ai-stream-chunk" 推送流式结果

// === 导出 ===
#[tauri::command] fn export_pdf(project_id: String, path: String) -> Result<(), String>;
#[tauri::command] fn export_html(project_id: String, path: String) -> Result<(), String>;
#[tauri::command] fn export_markdown(project_id: String, path: String) -> Result<(), String>;

// === 设置 ===
#[tauri::command] fn get_setting(key: String) -> Result<String, String>;
#[tauri::command] fn set_setting(key: String, value: String) -> Result<(), String>;
#[tauri::command] fn set_api_key(provider: String, key: String) -> Result<(), String>;  // keyring
#[tauri::command] fn test_api_connection(provider: String, key: String) -> Result<bool, String>;
```

### 4.3 本地数据模型（TypeScript 端）

```typescript
// 录制项目
interface Project {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  stepCount: number;
}

// 操作步骤
interface Step {
  id: string;
  projectId: string;
  stepIndex: number;
  stepType: 'click' | 'double_click' | 'right_click' | 'type' | 'scroll' | 'drag' | 'screenshot';
  position: { x: number; y: number } | null;
  textInput: string | null;
  elementName: string | null;
  elementType: string | null;
  appName: string | null;
  screenshotPath: string | null;
  aiTitle: string | null;
  aiDescription: string | null;
  timestamp: number;
}

// 录制状态
interface RecordingStatus {
  isRecording: boolean;
  stepCount: number;
  startedAt: number | null;
}

// 应用设置
interface AppSettings {
  aiModel: 'zhipu' | 'deepseek' | 'qwen' | 'custom';
  customApiKey: string | null;     // 仅存 keyring，前端不存明文
  language: 'zh-CN' | 'en';
  hotkey: string;                  // "Ctrl+Alt+R"
  dataDirectory: string;
  aiQuotaRemaining: number;        // 本月 AI 调用剩余次数
}
```

---

## 第 5 章：安全设计

### 5.1 API Key 存储

```
设置 API Key
    │
    ▼
前端 → invoke("set_api_key", { provider: "zhipu", key: "xxx" })
    │
    ▼
Rust 端 → tauri-plugin-store (keyring)
    │
    ▼
Windows Credential Manager / macOS Keychain
    (操作系统级加密存储)
```

- API Key 不经过任何日志、不写入 SQLite、不出现在前端 state
- 前端仅通过 `invoke("get_setting", { key: "ai_provider" })` 获取当前使用的 provider 名称
- 实际 Key 值仅在 Rust 端 AI 调用时从 keyring 读取

### 5.2 录制数据保护

| 保护措施 | 实现 |
|----------|------|
| 密码字段过滤 | UI Automation 检测 `IsPasswordProperty` → 值替换为 `[密码]` |
| 敏感信息过滤 | AI 调用前扫描步骤文本，匹配身份证号(18位)/手机号(11位)/银行卡号(16-19位) → 替换为 `[已隐藏]` |
| 本地存储 | 所有录制数据仅在本地 SQLite，不上传 |
| 截图存储 | JPEG 文件保存在用户指定的数据目录，不包含 EXIF 元数据 |

### 5.3 应用安全

- 代码签名：Windows Authenticode 证书签名 .msi 和 .exe
- 无远程代码执行：不加载远程 JS，不执行 eval()
- CSP 策略：Tauri 安全配置限制外部资源加载

---

## 第 6 章：开发计划

### Week 1：项目骨架 + 录制引擎核心

| 任务 | 描述 | 验收条件 |
|------|------|----------|
| T1.1 | 完成 Rust 项目结构调整（从 StepSnap 迁移录制相关模块） | `cargo check` 通过 |
| T1.2 | 实现 `recorder/listener.rs` — rdev 全局事件监听 | 可捕获鼠标点击/键盘输入 |
| T1.3 | 实现 `recorder/screenshot.rs` — xcap 截图封装 | 每次点击自动截图保存 JPEG |
| T1.4 | 实现 `accessibility/windows.rs` — UIA 元素识别 | 可获取控件名/类型/宿主应用 |
| T1.5 | 实现 `recorder/overlay.rs` — 原生覆盖层窗口 | 点击位置显示录制反馈框 |
| T1.6 | 实现 `database.rs` — SQLite 初始化 + 基本 CRUD | 录制数据可持久化读写 |
| T1.7 | 实现全局快捷键 `Ctrl+Alt+R` 开始/停止录制 | 快捷键录制完整流程可用 |
| T1.8 | 前端基础 UI：主窗口、录制控制按钮、步骤预览列表 | 录制后可在前端看到步骤列表 |

**Week 1 里程碑**：能按下 Ctrl+Alt+R → 执行 10 步操作 → 前端显示 10 步列表（含截图缩略图）。

### Week 2：AI 处理管道 + 步骤生成

| 任务 | 描述 | 验收条件 |
|------|------|----------|
| T2.1 | 实现 `ai_pipeline.rs` — Prompt 构建 + HTTP 调用 | 可成功调用智谱 API |
| T2.2 | 实现流式 SSE 解析 + `app.emit("ai-stream-chunk")` | 前端实时显示 AI 生成内容 |
| T2.3 | 截图压缩/Base64 编码管线 | 每步截图 ≤ 50KB Base64 |
| T2.4 | AI 生成进度 UI（前端） | 显示"正在生成第 3/15 步..." |
| T2.5 | API Key 管理（keyring 存储） | API Key 加密存储，可测试连接 |
| T2.6 | 模型切换 UI 预留（仅智谱可用，其余标注「V1.0 自配 Key」） | 智谱正常工作，其他模型入口显示预告文案 |
| T2.7 | 错误处理 + 重试机制 | 网络错误自动重试 3 次 |

**Week 2 里程碑**：录制完成后自动触发 AI，15 步 ≤ 10s 生成完毕，中文描述准确。

### Week 3：编辑器 + 导出引擎

| 任务 | 描述 | 验收条件 |
|------|------|----------|
| T3.1 | 步骤编辑器组件树（StepEditor/StepList/StepCard） | 可查看/编辑步骤 |
| T3.2 | 步骤编辑操作（增删改、合并拆分） | 6 种编辑操作均可用 |
| T3.3 | 拖拽排序（react-beautiful-dnd） | 可拖拽调整步骤顺序 |
| T3.4 | 撤销/重做（20 步历史） | Ctrl+Z/Y 正常工作 |
| T3.5 | 实现 `export/export_pdf()` — printpdf PDF 导出 | PDF 中文正常，截图清晰 |
| T3.6 | 实现 HTML / Markdown 导出 | HTML 可独立打开，MD 图片内嵌 |
| T3.7 | 导出进度 UI + 文件保存对话框 | 导出流程完整可用 |

**Week 3 里程碑**：录制 → AI 生成 → 编辑 3 步 → 导出 PDF，全流程走通。

### Week 4：设置 + 分享 + 测试 + 发布

| 任务 | 描述 | 验收条件 |
|------|------|----------|
| T4.1 | 设置面板（AI 模型/API Key/快捷键/数据目录） | 6 个设置项均可用 |
| T4.2 | 分享按钮（预留 UI + 引导文案） | 点击显示 V1.0 预告 |
| T4.3 | 首次启动引导（3 步引导） | 新用户看到引导流程 |
| T4.4 | 全局错误处理 + Toast 提示 | 错误信息中文友好 |
| T4.5 | 三级测试（AI 自测 → 联合测试 → 用户视角测试） | 测试报告完成 |
| T4.6 | 安装包构建（Windows .msi） | 安装包 ≤ 15MB |
| T4.7 | README.md 编写（安装/使用/开发/部署） | 文档完整 |

**Week 4 里程碑**：MVP 可发布，通过 spec.md 第 6 章全部验收标准。

---

## 第 7 章：风险与对策

### 7.1 技术风险

| 风险 | 影响 | 概率 | 规避措施 | 应急预案 |
|------|------|------|----------|----------|
| Windows UI Automation 在部分应用中不稳定（如老旧 Win32 程序） | 元素名称获取失败，AI 描述质量下降 | 中 | 录制时同时捕获窗口标题作为 fallback；AI Prompt 中包含截图以辅助理解 | 元素名称为空时，AI 仅基于截图和坐标生成描述 |
| rdev 监听在部分杀毒软件中被拦截 | 录制功能不可用 | 低 | 使用微软官方签名，提交到 Windows Defender 白名单 | 引导用户将录步加入杀软信任区 |
| 智谱 API 服务不稳定或价格调整 | AI 生成中断，成本上升 | 中 | 三层模型架构（默认内置+国产可配+国外垫后）；自带 Key 通道 | 自动降级到第二层备选（DeepSeek）；提示用户使用自带 Key |
| printpdf 中文渲染异常 | PDF 导出的中文出现乱码或缺失 | 中 | 嵌入思源黑体（Source Han Sans SC），测试全部常用汉字 | 回退方案：前端用 HTML→Canvas→图片方式生成 PDF |
| Tauri 2 在 Windows 7/8 上的兼容性问题 | 部分用户无法安装 | 低 | MVP 明确仅支持 Windows 10/11 | 不支持的系统版本在安装时给出明确提示 |

### 7.2 依赖风险

| 依赖 | 风险 | 对策 |
|------|------|------|
| rdev（全局输入监听） | 维护者较少，更新频率低 | 已 fork + vendor 到项目 `src-tauri/vendor/rdev/`，自行维护 |
| xcap（截图） | Windows 11 更新可能导致兼容性问题 | 备选方案：改用 Windows Graphics Capture API |
| printpdf | 中文排版复杂场景可能不完善 | 预留 HTML→PDF 方案（chromium-headless 备选） |
| react-beautiful-dnd | 已标记为 deprecated（但功能稳定） | 功能稳定可用，V1.0 评估迁移到 @dnd-kit |

### 7.3 进度风险

| 风险 | 影响 | 概率 | 对策 |
|------|------|------|------|
| 录制引擎迁移比预期复杂（StepSnap 代码耦合度高） | Week 1 延期 | 中 | Week 1 预留 2 天 buffer；优先实现最小可用录制（点击+截图），复杂功能 Week 2 补 |
| AI Prompt 调优耗时超出预期 | Week 2 延期 | 中 | 先上线基础版 Prompt，V1.0 迭代优化 |
| PDF 导出中文排版调试时间长 | Week 3 延期 | 中 | 先完成 HTML/MD 导出（前端方案更可控），PDF 作为加分项 |

### 7.4 缓解策略总结

```
高风险项（概率中+影响中以上）重点监控：
  ✅ UIA 稳定性 → 截图 fallback + Prompt 优化
  ✅ 智谱 API 可用性 → 三层模型架构（默认内置 + 国产可配 + 国外垫后）
  ✅ PDF 中文渲染 → 嵌入字体 + HTML 降级方案
  ✅ StepSnap 迁移耦合 → 最小可用先跑通，逐步增强
```

---

*文档完毕。下一步：基于本 plan 编写 docs/tasks.md 任务清单。*
*（内容由AI生成，仅供参考）*
