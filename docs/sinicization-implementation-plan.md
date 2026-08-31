---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_e4c2c9018d7911f196d8525400f8a581
    ReservedCode1: 9VG0ROR2s0CKQh6lzDKOtRZ9PKv9htRA0T6gSuvXTtTLCh4VPfvHjQwdXdc86Xc1VR3e0TKwdPZprCPs+EYzKKecn7EH5PweBKdDrrBE9L6MzoRwYv3fX0HAzgn2YHnkFffiuDEx6R3rlA38H55jlb0bi2ScX/Qx8E9yaTqhBXvSjC+RgLA/ketYJF0=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_e4c2c9018d7911f196d8525400f8a581
    ReservedCode2: 9VG0ROR2s0CKQh6lzDKOtRZ9PKv9htRA0T6gSuvXTtTLCh4VPfvHjQwdXdc86Xc1VR3e0TKwdPZprCPs+EYzKKecn7EH5PweBKdDrrBE9L6MzoRwYv3fX0HAzgn2YHnkFffiuDEx6R3rlA38H55jlb0bi2ScX/Qx8E9yaTqhBXvSjC+RgLA/ketYJF0=
---

# Scribe 全面国产化落地技术方案

> 版本：v1.0.0 · 日期：2026-08-01
> 前置知识库：`docs/scribe-deep-analysis.md`（Scribe 竞品全维度剖析）、`docs/sinicization-roadmap.md`（四阶段路线图）
> 旧版归档：`releases/archive/录步_20260801_151936.zip`

---

## 一、技术蓝图总览

```
┌────────────────────────────────────────────────────────┐
│                    Flowio v2.0                          │
│           "Scribe 全面国产化" 桌面端操作录制工具           │
├────────────────────────────────────────────────────────┤
│                                                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────┐ │
│  │  录制引擎  │  │  AI 引擎  │  │  编辑器   │  │  分享   │ │
│  │ (Rust)   │  │ (Rust)   │  │ (React)  │  │ (Rust) │ │
│  │          │  │          │  │          │  │        │ │
│  │Win32 Hook│  │GLM-4/    │  │Scribe 风格│  │HTTP    │ │
│  │UIA捕获   │  │DeepSeek  │  │左右分栏   │  │本地服务 │ │
│  │帧差兜底   │  │中文优化  │  │Before→   │  │局域网   │ │
│  │智能截图   │  │三层模型   │  │After纵向  │  │分享链接 │ │
│  └──────────┘  └──────────┘  └──────────┘  └────────┘ │
│                                                         │
│  数据层：本地 SQLite → 可选云端同步 → 阿里云 OSS          │
│  分发层：官网直下 .exe/.msi + 自动更新 + 代码签名          │
│                                                         │
└────────────────────────────────────────────────────────┘
```

---

## 二、项目结构定义

```
flowio/
├── src/                          # 前端（React 19 + TypeScript + Tailwind v4）
│   ├── main.tsx                  # 入口
│   ├── App.tsx                   # 路由 + 全局布局
│   ├── index.css                 # Tailwind + 全局样式（system-ui）
│   │
│   ├── pages/
│   │   ├── Home.tsx              # 首页：录制列表 + 启动录制入口
│   │   ├── Editor.tsx            # 编辑器（核心页面，Scribe 左右分栏布局）
│   │   └── Recording.tsx         # 录制中页面（最小化浮窗控件）
│   │
│   ├── components/
│   │   ├── layout/
│   │   │   ├── TitleBar.tsx      # 标题栏（40px，拖拽区域）
│   │   │   └── Sidebar.tsx       # 侧边导航栏（220px）
│   │   ├── home/
│   │   │   ├── RecordingCard.tsx  # 录制列表卡片
│   │   │   ├── RecordingGrid.tsx  # 网格/列表视图切换
│   │   │   └── SearchBar.tsx      # 搜索 + 筛选
│   │   ├── editor/
│   │   │   ├── EditorLayout.tsx   # 编辑器主布局（左右分栏容器）
│   │   │   ├── StepSidebar.tsx    # 左侧步骤列表（可拖拽排序）
│   │   │   ├── StepCard.tsx       # 步骤卡片（序号+缩略图+标题）
│   │   │   ├── StepDetail.tsx     # 右侧步骤详情（Before→After 截图+描述）
│   │   │   ├── StepEditForm.tsx   # 内联编辑表单（标题+描述）
│   │   │   ├── ScreenshotViewer.tsx # 截图查看器（红圈标注 + 缩放）
│   │   │   ├── StepContextMenu.tsx  # 右键菜单（删除/复制/合并）
│   │   │   └── ExportMenu.tsx     # 导出下拉菜单（PDF/HTML/MD/长图）
│   │   ├── recording/
│   │   │   ├── RecordingWidget.tsx  # 录制浮窗控件（屏幕角红点）
│   │   │   └── RecordingControls.tsx # 暂停/完成/删除按钮
│   │   ├── share/
│   │   │   ├── ShareDialog.tsx    # 分享弹窗（链接/嵌入/导出）
│   │   │   ├── ShareLink.tsx      # 分享链接展示 + 复制
│   │   │   └── ShareQRCode.tsx    # QR 码（手机扫码查看）
│   │   ├── settings/
│   │   │   ├── SettingsPanel.tsx  # 设置面板容器
│   │   │   ├── GeneralTab.tsx     # 通用设置
│   │   │   ├── AiModelTab.tsx     # AI 模型配置
│   │   │   ├── ShortcutTab.tsx    # 快捷键设置
│   │   │   ├── ApiKeyInput.tsx    # API Key 输入组件
│   │   │   └── AboutTab.tsx       # 关于页
│   │   └── common/
│   │       ├── Button.tsx         # 通用按钮（Primary/Secondary/Danger）
│   │       ├── Modal.tsx          # 通用弹窗
│   │       ├── Toast.tsx          # Toast 通知
│   │       ├── EmptyState.tsx     # 空状态占位
│   │       ├── ErrorBoundary.tsx  # 错误边界
│   │       ├── LoadingSkeleton.tsx # 加载骨架屏
│   │       └── ConfirmDialog.tsx  # 确认对话框
│   │
│   ├── hooks/
│   │   ├── useEditor.ts           # 编辑器状态管理（步骤增删改查）
│   │   ├── useEditorHistory.ts    # 撤销/重做
│   │   ├── useStepReorder.ts      # 拖拽排序
│   │   ├── useKeyboardShortcuts.ts # 快捷键注册
│   │   ├── useShareServer.ts      # 分享服务状态
│   │   ├── useRecording.ts        # 录制状态管理
│   │   ├── useDarkMode.ts         # 深色模式
│   │   └── useSpeechRecognition.ts # 语音输入
│   │
│   ├── context/
│   │   ├── AppContext.tsx          # 全局状态（当前录制/用户设置）
│   │   └── ToastContext.tsx        # Toast 上下文
│   │
│   ├── types/
│   │   ├── recording.ts           # Recording / Step / ElementInfo 类型
│   │   ├── editor.ts              # 编辑器状态类型
│   │   ├── export.ts              # 导出选项类型
│   │   └── settings.ts            # 设置类型
│   │
│   ├── utils/
│   │   ├── export.ts              # 导出逻辑（PDF/HTML/MD/长图）
│   │   ├── format.ts              # 时间格式化 / 文件大小格式化
│   │   ├── clipboard.ts           # 剪贴板操作
│   │   └── screenshot.ts          # 截图处理（红圈标注 / 裁剪）
│   │
│   └── assets/
│       ├── logo.svg
│       └── icons/                 # SVG 图标（无 emoji）
│
├── src-tauri/                     # Rust 后端
│   ├── Cargo.toml                  # 依赖声明
│   ├── build.rs                    # Tauri 构建脚本
│   ├── tauri.conf.json             # Tauri 配置
│   ├── capabilities/
│   │   └── default.json            # 权限声明
│   ├── icons/                      # 应用图标
│   ├── fonts/
│   │   └── NotoSansSC-*.ttf        # 中文字体
│   └── src/
│       ├── main.rs                 # 入口
│       ├── lib.rs                  # Tauri 命令注册
│       ├── types.rs                # 跨模块类型定义
│       │
│       ├── recorder/               # 录制引擎
│       │   ├── mod.rs              # 模块入口 + 录制状态机
│       │   ├── capture.rs          # 截图采集（Win32 + DirectX）
│       │   ├── listener.rs         # 全局事件监听（Win32 Hook）
│       │   ├── uia.rs              # UI Automation 元素识别
│       │   ├── step_builder.rs     # 步骤构建器（去重 + 合并）
│       │   ├── dedup.rs            # 智能去重算法
│       │   └── storage.rs          # SQLite 持久化
│       │
│       ├── ai/                     # AI 引擎
│       │   ├── mod.rs              # 模块入口
│       │   ├── pipeline.rs         # AI 流水线编排
│       │   ├── client.rs           # HTTP 客户端（GLM-4 / DeepSeek / 通义千问）
│       │   ├── prompt.rs           # Prompt 模板系统（正向引导式）
│       │   ├── parser.rs           # AI 响应解析器
│       │   ├── chinese_ui.rs       # 中文 UI 术语词表 + 匹配
│       │   └── key_manager.rs      # API Key 加密存储
│       │
│       ├── editor/                 # 编辑器后端
│       │   ├── mod.rs              # 模块入口
│       │   ├── commands.rs         # Tauri 命令（CRUD 步骤）
│       │   └── history.rs          # 撤销/重做（Rust 侧）
│       │
│       ├── export/                 # 导出引擎
│       │   ├── mod.rs              # 模块入口
│       │   ├── pdf.rs              # PDF 导出
│       │   ├── html.rs             # HTML 查看器
│       │   ├── markdown.rs         # Markdown 导出
│       │   └── image.rs            # 微信长图导出
│       │
│       ├── share/                  # 分享服务
│       │   ├── mod.rs              # 模块入口
│       │   ├── server.rs           # HTTP 分享服务器（tiny_http）
│       │   ├── templates.rs         # HTML 模板（只读查看器）
│       │   └── auth.rs             # 分享鉴权（密码保护）
│       │
│       └── settings/               # 设置管理
│           ├── mod.rs              # 模块入口
│           └── config.rs           # 配置文件读写
│
├── docs/                           # 项目文档（保留全部）
│   ├── scribe-deep-analysis.md     # Scribe 竞品全维度分析
│   ├── sinicization-roadmap.md     # 四阶段国产化路线图
│   ├── design-system.md            # 设计系统规范
│   ├── plan.md / spec.md           # 原始计划/规格
│   ├── market-research.md          # 市场调研
│   ├── test-checklist.md           # 测试清单
│   ├── decisions/                  # 架构决策记录（5 篇 ADR）
│   ├── terms.md / privacy.md       # 术语 / 隐私
│   └── context.md                  # 上下文
│
├── releases/
│   └── archive/
│       └── 录步_20260801_151936.zip  # v1 旧版归档
│
├── package.json
├── pnpm-lock.yaml
├── vite.config.ts
├── tsconfig.json
├── index.html
├── AGENTS.md
├── CONTEXT.md
└── CHANGELOG.md
```

---

## 三、核心数据模型

### 3.1 Recording（录制）

```typescript
// src/types/recording.ts
interface Recording {
  id: string;                    // UUID
  title: string;                 // 用户自定义标题，兜底用窗口标题
  app_name: string;              // 录制的目标应用名
  window_title: string;          // 录制时的窗口标题
  steps: Step[];                 // 步骤列表
  created_at: string;            // ISO 时间戳
  updated_at: string;
  duration_ms: number;           // 录制时长（毫秒）
  status: 'draft' | 'completed' | 'archived';
  tags: string[];                // 用户标签
  share_link?: string;           // 分享链接 ID
  view_count: number;            // 查看次数
}
```

### 3.2 Step（步骤）

```typescript
// src/types/recording.ts
interface Step {
  id: string;
  recording_id: string;
  index: number;                 // 步骤序号（从 0 开始，支持拖拽后重排）

  // 截图
  screenshot_path: string;       // 点击前截图（Before）
  after_screenshot_path: string; // 操作后截图（After）

  // 操作信息
  action_type: 'click' | 'input' | 'select' | 'scroll' | 'drag' | 'keyboard' | 'navigate';
  position?: [number, number];    // 点击坐标（用于红圈标注）
  input_text?: string;            // 输入内容（如果 action_type === 'input'）
  scroll_delta?: number;          // 滚动量

  // UI 元素
  element_info?: {
    name: string;                 // 按钮/输入框/Aria label
    control_type: string;         // Button/Edit/ComboBox...
    class_name: string;           // Win32 类名
    automation_id: string;        // UIA AutomationId
  };

  // AI 生成
  ai_title: string;               // AI 生成的步骤标题（≤15字）
  ai_description: string;         // AI 生成的描述
  ai_generated_at?: string;       // AI 生成时间
  ai_model?: string;              // 使用的模型

  // 用户编辑
  user_title?: string;            // 用户修改的标题
  user_description?: string;      // 用户修改的描述
  user_edited: boolean;           // 是否被手动编辑

  // 标注与脱敏
  annotations: Annotation[];      // 截图标注（箭头/矩形/文字）
  redactions: Redaction[];        // 脱敏区域
  tip?: string;                   // 用户添加的提示
  tip_image_path?: string;        // 提示配图/GIF
}

interface Annotation {
  type: 'arrow' | 'rectangle' | 'circle' | 'text';
  points: [number, number][];     // 多边形顶点
  color: string;                  // 标注颜色（默认红色 #DC2626）
  text?: string;                  // type=text 时的文字
}

interface Redaction {
  x: number; y: number;
  width: number; height: number;
  type: 'blur' | 'solid';
}
```

### 3.3 Rust 侧对应结构体

```rust
// src-tauri/src/types.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub title: String,
    pub app_name: String,
    pub window_title: String,
    pub steps: Vec<Step>,
    pub created_at: String,
    pub updated_at: String,
    pub duration_ms: u64,
    pub status: RecordingStatus,
    pub tags: Vec<String>,
    pub share_link: Option<String>,
    pub view_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub recording_id: String,
    pub index: usize,
    pub screenshot_path: String,         // Before
    pub after_screenshot_path: String,   // After
    pub action_type: ActionType,
    pub position: Option<(i32, i32)>,
    pub input_text: Option<String>,
    pub element_info: Option<ElementInfo>,
    pub ai_title: String,
    pub ai_description: String,
    pub user_title: Option<String>,
    pub user_description: Option<String>,
    pub user_edited: bool,
    pub annotations: Vec<Annotation>,
    pub redactions: Vec<Redaction>,
    pub tip: Option<String>,
}
```

---

## 四、编辑器架构（核心模块）

### 4.1 布局契约

编辑器的最高标准：**逐像素对标 Scribe 桌面版**。

```
┌──────────────────────────────────────────────────────────┐
│  TitleBar (48px)                                          │
│  [← 返回]  [文档标题_______________]  [分享] [导出▾] [AI优化] │
├────────────────┬─────────────────────────────────────────┤
│  StepSidebar   │  StepDetail                              │
│  (280px)       │                                          │
│                │  ┌────────────────────────────────────┐  │
│  ❶ 点击登录     │  │  Before 截图（操作前）              │  │
│  ┌──────────┐ │  │  全宽展示，保持原始比例               │  │
│  │  缩略图   │ │  └────────────────────────────────────┘  │
│  │  16:9    │ │              ↓ 箭头指示                    │
│  └──────────┘ │  ┌────────────────────────────────────┐  │
│                │  │  After 截图（操作后）               │  │
│  ❷ 输入账号     │  │  红色圆圈标注点击位置               │  │
│  ┌──────────┐ │  │  全宽展示，保持原始比例               │  │
│  │  缩略图   │ │  └────────────────────────────────────┘  │
│  └──────────┘ │                                          │
│                │  ┌────────────────────────────────────┐  │
│  ❸ 点击确认     │  │  步骤标题：ai_title or user_title    │  │
│  ┌──────────┐ │  │  步骤描述：ai_description             │  │
│  │  缩略图   │ │  │                                      │  │
│  └──────────┘ │  │  [编辑文字] [重新截图] [添加快照]      │  │
│                │  └────────────────────────────────────┘  │
│                │                                          │
│  [+ 添加步骤]  │                                          │
└────────────────┴─────────────────────────────────────────┘
```

### 4.2 组件树 & 数据流

```
Editor.tsx
├── useEditor(recordingId)          ← hooks/useEditor.ts
│   ├── steps: Step[]               ← Rust command: get_recording
│   ├── selectedIndex: number
│   ├── selectStep(index)
│   ├── addStep()
│   ├── deleteStep(id)
│   ├── updateStep(id, patch)
│   ├── reorderSteps(from, to)
│   └── generateAI()               ← Rust command: run_ai_pipeline
│
├── useEditorHistory(steps)         ← hooks/useEditorHistory.ts
│   ├── undo()
│   ├── redo()
│   └── canUndo / canRedo
│
├── useStepReorder(steps, reorder)  ← hooks/useStepReorder.ts
│   └── onDragEnd(result)          ← @hello-pangea/dnd
│
├── useKeyboardShortcuts({...})    ← hooks/useKeyboardShortcuts.ts
│
├── <TitleBar>                      ← 顶部工具栏
│   ├── onBack
│   ├── title + onTitleChange
│   ├── onShare → <ShareDialog>
│   ├── <ExportMenu>
│   └── onGenerateAI
│
├── <EditorLayout>                  ← 左右分栏容器
│   ├── <StepSidebar>              ← 左侧 280px
│   │   ├── <StepCard> * N         ← 可拖拽
│   │   │   ├── 序号圆圈（24px 蓝色圆形）
│   │   │   ├── After 缩略图（16:9）
│   │   │   ├── 标题文字（单行截断）
│   │   │   └── onClick → selectStep
│   │   └── <AddStepButton>
│   │
│   └── <StepDetail>               ← 右侧 flex-1
│       ├── <ScreenshotViewer>      ← Before 截图
│       │   └── zoom / pan
│       ├── ↓ 箭头分隔
│       ├── <ScreenshotViewer>      ← After 截图 + 红圈标注
│       │   ├── 红圈标注（position 坐标）
│       │   └── zoom / pan
│       ├── 步骤标题（可点击编辑）
│       ├── 步骤描述（可点击编辑）
│       ├── Tip 区域
│       └── 操作栏：[编辑文字] [重新截图] [添加快照]
│
└── <ShareDialog>                   ← 条件渲染
    ├── <ShareLink>
    ├── <ShareQRCode>
    └── 导出选项
```

### 4.3 关键状态管理（useEditor）

```typescript
// hooks/useEditor.ts
interface EditorState {
  recording: Recording | null;
  isLoading: boolean;
  error: string | null;

  // 选中
  selectedStepIndex: number;

  // 编辑中
  editingStepId: string | null;       // 正在内联编辑的步骤 ID
  editingField: 'title' | 'description' | null;

  // AI
  isAiGenerating: boolean;
  aiProgress: { current: number; total: number } | null;  // 流式进度

  // 分享
  shareServerUrl: string | null;
}
```

---

## 五、AI 引擎架构

### 5.1 AI Pipeline 流程图

```
录制完成 steps[]
      │
      ▼
┌─────────────────────┐
│  Pipeline::run()     │
│  (src-tauri/ai/      │
│   pipeline.rs)       │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│  Phase 1: 去重合并    │  ← dedup.rs（连续重复操作合并）
│  连续相同操作合并      │
│  静态停留截图丢弃      │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│  Phase 2: 截图裁剪    │  ← capture.rs（智能聚焦区域）
│  检测变化区域          │
│  Auto-crop           │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│  Phase 3: AI 标题+描述 │  ← prompt.rs + client.rs
│  组装 Prompt           │
│  调用 GLM-4-flash     │
│  解析响应              │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│  Phase 4: 中文校验    │  ← chinese_ui.rs
│  术语一致性检查        │
│  按钮名 OCR 校验      │
└─────────┬───────────┘
          │
          ▼
   保存到 SQLite + 返回前端
```

### 5.2 Prompt 模板系统

#### 系统提示词（正向引导，非禁则列表）

```rust
// src-tauri/src/ai/prompt.rs

pub const SYSTEM_PROMPT: &str = r#"你是一位中文操作指南撰写专家，擅长将屏幕操作转化为简洁易懂的步骤说明。

你的写作风格：
1. 使用动宾短语开头（"点击"、"输入"、"选择"、"拖拽"、"勾选"）
2. 步骤标题不超过 15 个字，描述不超过 30 个字
3. 引用的按钮/菜单名称用原文加中文引号，如「提交申请」
4. 不使用坐标、像素、DOM 元素、技术 ID
5. 面向非技术用户，让完全没操作过该软件的人也能看懂

输出格式（严格遵守）：
{
  "steps": [
    {
      "title": "点击登录按钮",
      "description": "在页面右上角点击「登录」按钮进入登录页面"
    }
  ]
}

优秀示例：
- title: "输入工号"
  description: "在「用户名」输入框中输入您的员工工号"
- title: "选择发票类型"
  description: "从下拉菜单中选择「增值税专用发票」"
- title: "勾选同意协议"
  description: "勾选页面底部的「我已阅读并同意用户协议」"
"#;
```

#### 用户提示词构建

```rust
pub fn build_user_prompt(steps: &[Step], app_name: &str, window_title: &str) -> String {
    let mut prompt = format!(
        "以下是在「{}」软件（窗口标题：{}）中录制的操作步骤，请为每一步生成中文标题和描述：\n\n",
        app_name, window_title
    );

    for (i, step) in steps.iter().enumerate() {
        prompt.push_str(&format!("步骤 {}：\n", i + 1));
        prompt.push_str(&format!("- 操作类型：{}\n", step.action_type.cn_name()));

        if let Some(ref info) = step.element_info {
            prompt.push_str(&format!("- UI 元素：{}（{}）\n", info.name, info.control_type));
        }

        if let Some(ref text) = step.input_text {
            prompt.push_str(&format!("- 输入内容：{}\n", text));
        }

        prompt.push_str(&format!("- 截图文件：{}（请结合截图内容生成描述）\n", step.after_screenshot_path));
        prompt.push('\n');
    }

    prompt
}
```

### 5.3 模型适配层

```rust
// src-tauri/src/ai/client.rs

pub enum AiModel {
    Glm4Flash,       // 智谱 GLM-4-flash（默认，中文最优）
    DeepSeekV4,      // DeepSeek-V4（纯文字强，无视觉）
    QwenVL,          // 通义千问 VL（视觉强，备选）
}

impl AiModel {
    pub fn endpoint(&self) -> &str {
        match self {
            Self::Glm4Flash => "https://open.bigmodel.cn/api/paas/v4/chat/completions",
            Self::DeepSeekV4 => "https://api.deepseek.com/v1/chat/completions",
            Self::QwenVL => "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        }
    }

    pub fn model_name(&self) -> &str {
        match self {
            Self::Glm4Flash => "glm-4-flash",
            Self::DeepSeekV4 => "deepseek-chat",
            Self::QwenVL => "qwen-vl-max",
        }
    }
}
```

---

## 六、分享服务架构

### 6.1 服务设计

```
┌───────────────────────────────────────────┐
│              Flowio 桌面端                  │
│                                           │
│  用户点击「分享」                            │
│       │                                   │
│       ▼                                   │
│  ShareServer::start(recording_id)          │
│       │                                   │
│       ├── 1. 生成 share_id（UUID）         │
│       ├── 2. 复制截图到临时目录             │
│       ├── 3. 生成只读 HTML 查看器           │
│       ├── 4. 启动 tiny_http 监听随机端口    │
│       └── 5. 返回 http://局域网IP:端口/s/{id}│
│                                           │
└──────────────────┬────────────────────────┘
                   │
    ┌──────────────┼──────────────┐
    ▼              ▼              ▼
  同机器浏览器    局域网手机       局域网其他PC
  localhost:端口  扫码访问        直接打开链接
```

### 6.2 Rust 实现骨架

```rust
// src-tauri/src/share/server.rs

use std::sync::{Arc, Mutex};
use tiny_http::{Server, Response, Header};
use uuid::Uuid;

pub struct ShareServer {
    server: Option<Arc<Mutex<Server>>>,
    port: u16,
    share_id: String,
    base_url: String,
    running: bool,
}

impl ShareServer {
    pub fn new() -> Self { /* ... */ }

    pub fn start(&mut self, recording: &Recording, screenshots_dir: &Path) -> Result<String, String> {
        // 1. 查找可用端口
        let port = find_available_port()?;
        self.port = port;

        // 2. 生成分享 ID
        self.share_id = Uuid::new_v4().to_string();

        // 3. 准备截图副本到临时目录
        let share_dir = prepare_share_dir(&self.share_id, recording, screenshots_dir)?;

        // 4. 生成 HTML 查看器
        let html = build_viewer_html(recording, &self.share_id);

        // 5. 启动 HTTP 服务
        let server = Server::http(format!("0.0.0.0:{}", port)).map_err(|e| e.to_string())?;
        let html_clone = html.clone();
        let share_dir_clone = share_dir.clone();

        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let url = request.url().to_string();
                match url.as_str() {
                    "/" | "" => {
                        let response = Response::from_string(&html_clone)
                            .with_header(Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap());
                        let _ = request.respond(response);
                    }
                    path if path.starts_with("/screenshots/") => {
                        let file_path = share_dir_clone.join(&path[13..]);
                        if file_path.exists() {
                            let data = std::fs::read(&file_path).unwrap_or_default();
                            let response = Response::from_data(data)
                                .with_header(Header::from_bytes("Content-Type", "image/png").unwrap());
                            let _ = request.respond(response);
                        }
                    }
                    _ => {
                        let response = Response::from_string("Not Found").with_status_code(404);
                        let _ = request.respond(response);
                    }
                }
            }
        });

        self.running = true;
        let local_ip = get_local_ip()?;
        self.base_url = format!("http://{}:{}/", local_ip, port);
        Ok(self.base_url.clone())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        // 关闭服务器，清理临时文件
        self.running = false;
        Ok(())
    }

    pub fn is_running(&self) -> bool { self.running }

    pub fn url(&self) -> Option<&str> {
        if self.running { Some(&self.base_url) } else { None }
    }
}
```

### 6.3 Tauri 命令接口

```rust
// src-tauri/src/share/mod.rs

#[tauri::command]
fn start_share(recording_id: String, state: State<AppState>) -> Result<String, String> {
    let recording = state.db.get_recording(&recording_id)?;
    let mut server = state.share_server.lock().map_err(|e| e.to_string())?;
    server.start(&recording, &state.screenshots_dir)
}

#[tauri::command]
fn stop_share(state: State<AppState>) -> Result<(), String> {
    let mut server = state.share_server.lock().map_err(|e| e.to_string())?;
    server.stop()
}

#[tauri::command]
fn get_share_status(state: State<AppState>) -> Result<ShareStatus, String> {
    let server = state.share_server.lock().map_err(|e| e.to_string())?;
    Ok(ShareStatus {
        running: server.is_running(),
        url: server.url().map(|s| s.to_string()),
    })
}
```

---

## 七、导出引擎

### 7.1 四种导出格式

| 格式 | 优先级 | 实现方式 | 适用场景 |
|------|--------|---------|---------|
| PDF | P0 | Rust `printpdf` crate | 正式文档交付 |
| HTML | P0 | 模板引擎渲染单文件 HTML | 离线查看、嵌入 |
| Markdown | P1 | 纯文本模板拼接 | 开发者文档、知识库 |
| 微信长图 | P0 | Rust `image` crate 纵向拼接 | 微信分享 |

### 7.2 微信长图导出算法

```rust
// src-tauri/src/export/image.rs

pub fn export_wechat_long_image(recording: &Recording, output_path: &Path) -> Result<String, String> {
    // 1. 加载所有 After 截图
    let images: Vec<DynamicImage> = recording.steps.iter()
        .map(|step| image::open(&step.after_screenshot_path))
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    // 2. 计算总高度 + 统一宽度
    let max_width = images.iter().map(|img| img.width()).max().unwrap_or(800);
    let margin = 40u32;  // 步骤间间距
    let header_height = 60u32;  // 顶部标题区域

    let total_height: u32 = header_height
        + images.iter().map(|img| img.height()).sum::<u32>()
        + margin * (images.len() as u32 - 1);

    // 3. 创建白色画布
    let mut canvas = DynamicImage::new_rgba8(max_width, total_height).to_rgba8();
    canvas.fill(255);  // 白色背景

    // 4. 绘制标题
    draw_text(&mut canvas, &recording.title, 20, 20, 24.0, Rgba([0x3B, 0x73, 0xB0, 255]))?;

    // 5. 逐步骤拼接截图
    let mut y_offset = header_height;
    for (i, img) in images.iter().enumerate() {
        // 步骤序号标签
        let label = format!("步骤 {}", i + 1);
        draw_text(&mut canvas, &label, 20, y_offset as i32 + 5, 16.0, Rgba([0x66, 0x66, 0x66, 255]))?;

        // 粘贴截图
        let resized = img.resize(max_width, img.height(), image::imageops::FilterType::Lanczos3);
        image::imageops::overlay(&mut canvas, &resized, 0, y_offset as i64 + 25);

        y_offset += img.height() + margin;
    }

    // 6. 保存 PNG
    canvas.save(output_path).map_err(|e| e.to_string())?;
    Ok(output_path.to_string_lossy().to_string())
}
```

---

## 八、录制引擎（架构保持，优化细节）

### 8.1 录制状态机

```
                 ┌──────────┐
         ┌──────→│  IDLE    │←──────┐
         │       │  空闲     │       │
         │       └────┬─────┘       │
         │            │ start       │ delete / cancel
         │            ▼             │
         │       ┌──────────┐      │
         │       │RECORDING │──────┘
         │       │  录制中   │
         │       └──┬───┬───┘
         │   pause  │   │  stop
         │          ▼   │
         │   ┌──────────┐│
         │   │  PAUSED  ││
         │   │  暂停中   ││
         │   └────┬─────┘│
         │ resume │      │
         └────────┘      │
                         ▼
                 ┌──────────────┐
                 │  PROCESSING   │
                 │  AI 生成中    │
                 └──────┬───────┘
                        │
                        ▼
                 ┌──────────────┐
                 │   EDITOR     │
                 │   编辑器      │
                 └──────────────┘
```

### 8.2 智能去重规则

```rust
// src-tauri/src/recorder/dedup.rs

pub fn should_record_new_step(prev: Option<&Step>, current: &RawEvent) -> bool {
    match prev {
        None => true,  // 第一步总是记录
        Some(prev) => {
            // 规则 1：同一元素连续点击 → 合并（防抖，如双击）
            if let Some(ref info) = current.element_info {
                if let Some(ref prev_info) = prev.element_info {
                    if info.automation_id == prev_info.automation_id
                        && current.action_type == ActionType::Click
                        && prev.action_type == ActionType::Click
                    {
                        return false;  // 跳过
                    }
                }
            }

            // 规则 2：无操作的截图 → 丢弃（同一屏幕未变化）
            // 通过像素差分判断
            if current.action_type == ActionType::None {
                return false;
            }

            // 规则 3：纯滚动 → 200ms 内的连续滚动合并
            // ...

            true
        }
    }
}
```

---

## 九、前端核心页面组件规范

### 9.1 设计 Token

```css
/* src/index.css — Tailwind 扩展 */
:root {
  /* 主色系 */
  --color-primary-50: #EBF3FB;
  --color-primary-100: #D6E7F7;
  --color-primary-200: #A9CDF0;
  --color-primary-300: #7BB2E8;
  --color-primary-400: #5098E0;
  --color-primary-500: #3B73B0;   /* 主色 */
  --color-primary-600: #2F5C8D;
  --color-primary-700: #23456A;
  --color-primary-800: #182E47;
  --color-primary-900: #0C1723;

  /* 中性色 */
  --color-gray-50: #F9FAFB;
  --color-gray-100: #F3F4F6;
  --color-gray-200: #E5E7EB;
  --color-gray-300: #D1D5DB;
  --color-gray-400: #9CA3AF;
  --color-gray-500: #6B7280;
  --color-gray-600: #4B5563;
  --color-gray-700: #374151;
  --color-gray-800: #1F2937;
  --color-gray-900: #111827;

  /* 功能色 */
  --color-danger: #DC2626;
  --color-success: #16A34A;
  --color-warning: #F59E0B;

  /* 布局 */
  --sidebar-width: 220px;
  --step-sidebar-width: 280px;
  --titlebar-height: 40px;
  --editor-toolbar-height: 48px;

  /* 字体 */
  font-family: system-ui, -apple-system, 'Microsoft YaHei', 'PingFang SC', sans-serif;
  font-size: 14px;
  color: var(--color-gray-900);
  background: #FFFFFF;
}
```

### 9.2 全局布局组件

```tsx
// src/App.tsx — 骨架
function App() {
  return (
    <AppProvider>
      <div className="flex flex-col h-screen bg-white">
        <TitleBar />
        <div className="flex flex-1 overflow-hidden">
          <Sidebar />
          <main className="flex-1 overflow-hidden">
            <Routes>
              <Route path="/" element={<Home />} />
              <Route path="/editor/:recordingId" element={<Editor />} />
              <Route path="/settings" element={<SettingsPanel />} />
            </Routes>
          </main>
        </div>
      </div>
      <ToastContainer />
    </AppProvider>
  );
}
```

---

## 十、实施计划（精确到接口）

### Phase 1-1：项目骨架重建（1 天）

| # | 任务 | 产出 |
|---|------|------|
| 1 | 创建 `src/` 目录结构，初始化入口文件 | `main.tsx`, `App.tsx`, `index.css` |
| 2 | 创建全局布局组件 | `TitleBar.tsx`, `Sidebar.tsx` |
| 3 | 创建通用组件 | `Button`, `Modal`, `Toast`, `EmptyState`, `LoadingSkeleton` |
| 4 | 创建 `src-tauri/src/` 模块骨架 | `main.rs`, `lib.rs`, `types.rs`，各模块 `mod.rs` |
| 5 | 定义前端类型 | `src/types/recording.ts`, `editor.ts`, `export.ts`, `settings.ts` |
| 6 | 配置 Tailwind + design tokens | `index.css` 完整 design tokens |
| 7 | `cargo build` + `pnpm dev` 确认骨架可运行 | 空白白屏 App |

### Phase 1-2：录制引擎（2 天）

| # | 任务 | 产出 |
|---|------|------|
| 8 | 实现 `recorder/mod.rs` 状态机 | IDLE → RECORDING → PAUSED → PROCESSING |
| 9 | 实现 `recorder/capture.rs` 截图 | Win32 BitBlt，保存到本地 |
| 10 | 实现 `recorder/listener.rs` 事件监听 | Win32 Hook（鼠标+键盘） |
| 11 | 实现 `recorder/uia.rs` UI 元素识别 | UIAutomation Core，提取 element_info |
| 12 | 实现 `recorder/step_builder.rs` | 原始事件 → Step 转换 |
| 13 | 实现 `recorder/dedup.rs` 智能去重 | 连续点击合并 / 静态截图丢弃 |
| 14 | 实现 `recorder/storage.rs` SQLite | rusqlite CRUD |
| 15 | 前端录制页面 | `Recording.tsx`, `RecordingWidget.tsx` |

### Phase 1-3：编辑器（3 天）

| # | 任务 | 产出 |
|---|------|------|
| 16 | 实现 `EditorLayout.tsx` 左右分栏容器 | flex row，280px + flex-1 |
| 17 | 实现 `StepSidebar.tsx` + 拖拽 | @hello-pangea/dnd |
| 18 | 实现 `StepCard.tsx` | 序号圆圈 + After 缩略图 + 标题 + 选中态 |
| 19 | 实现 `StepDetail.tsx` | Before→After 纵向堆叠 + 描述 + 操作栏 |
| 20 | 实现 `ScreenshotViewer.tsx` | 图片展示 + 红圈标注 + 缩放 |
| 21 | 实现 `StepEditForm.tsx` 内联编辑 | 点击→输入框，回车保存，Esc 取消 |
| 22 | 实现 `useEditor.ts` 状态 hook | 步骤增删改查 + 选中 |
| 23 | 实现 `useEditorHistory.ts` | 撤销/重做（操作栈） |
| 24 | 实现 `useStepReorder.ts` | 拖拽结束 → 更新 indices |
| 25 | 实现 `useKeyboardShortcuts.ts` | ↑↓ 切换步骤，Ctrl+Z/Y 撤销重做 |
| 26 | 实现 `StepContextMenu.tsx` | 右键菜单 |
| 27 | 实现 `ExportMenu.tsx` | PDF/HTML/MD/长图 下拉 |

### Phase 1-4：AI 引擎（2 天）

| # | 任务 | 产出 |
|---|------|------|
| 28 | 实现 `ai/prompt.rs` 正向引导 Prompt | 系统提示词 + 用户提示词构建 |
| 29 | 实现 `ai/client.rs` 多模型客户端 | GLM-4-flash / DeepSeek-V4 / 通义千问 VL |
| 30 | 实现 `ai/parser.rs` AI 响应解析 | JSON 提取 + 容错 |
| 31 | 实现 `ai/chinese_ui.rs` 中文术语词表 | 100+ 常见中文 UI 术语 |
| 32 | 实现 `ai/pipeline.rs` 流水线编排 | 去重→裁剪→AI→校验→保存 |
| 33 | 实现 `ai/key_manager.rs` API Key 加密存储 | AES-GCM 本地加密 |

### Phase 1-5：分享 + 导出（2 天）

| # | 任务 | 产出 |
|---|------|------|
| 34 | 实现 `share/server.rs` HTTP 分享服务 | tiny_http，随机端口，HTML 查看器 |
| 35 | 实现 `share/templates.rs` 只读查看器模板 | 嵌入式 HTML，Before→After 展示 |
| 36 | 实现 `share/auth.rs` 密码保护 | 简单密码校验 |
| 37 | 前端分享弹窗 | `ShareDialog.tsx`, `ShareLink.tsx` |
| 38 | 实现 `export/pdf.rs` | printpdf |
| 39 | 实现 `export/html.rs` | 单文件 HTML |
| 40 | 实现 `export/markdown.rs` | Markdown 模板 |
| 41 | 实现 `export/image.rs` 微信长图 | image crate 纵向拼接 |

### Phase 1-6：设置 + 构建（1 天）

| # | 任务 | 产出 |
|---|------|------|
| 42 | 实现 `settings/` 设置面板 | AI 模型 / 快捷键 / 通用 / 关于 |
| 43 | 实现 Home 页面 | 录制列表 + 搜索 + 启动录制 |
| 44 | `tauri build` 构建安装包 | .msi + 自动更新配置 |
| 45 | 端到端集成测试 | 录制→AI→编辑→分享→导出 全流程 |

---

## 十一、配色规范速查

| 用途 | HEX | Tailwind |
|------|-----|----------|
| 主按钮背景 | `#3B73B0` | `bg-primary-500` |
| 主按钮悬停 | `#2F5C8D` | `bg-primary-600` |
| 页面背景 | `#FFFFFF` | `bg-white` |
| 侧边栏背景 | `#F9FAFB` | `bg-gray-50` |
| 选中步骤 | `#EBF3FB` | `bg-primary-50` |
| 选中步骤边框 | `#3B73B0` | `border-primary-500` |
| 文字主色 | `#1F2937` | `text-gray-800` |
| 文字次要 | `#6B7280` | `text-gray-500` |
| 边框 | `#E5E7EB` | `border-gray-200` |
| 步骤序号圆圈 | `#3B73B0` / 白字 | — |
| 红圈标注 | `#DC2626` | `text-red-600` |
| Toast 成功 | `#16A34A` | `bg-green-600` |
| Toast 错误 | `#DC2626` | `bg-red-600` |

---

> 这份方案可以逐条编码执行。每条任务产出明确，模块接口清晰，无冗余依赖。从零开始，对照 `docs/` 中的分析文档和此方案，直接动工。
*（内容由AI生成，仅供参考）*
