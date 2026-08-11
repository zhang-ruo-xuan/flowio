---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_ae2780ba8d7d11f1bfea525400e6dd8f
    ReservedCode1: CfXze3YuGhzFEURsc/QAKQfTWslVVrMDh/szpeM/iF7hShQsOaKjybQruT31/DAb07DI7N3xJi4BxTMAS335qXqULT+lpO6qCFyuhwpOnyWIyOSmrKB08fV1tB6w16DBGKzMO/SLxhdi93fL7EwsRVy/z+tKzYO0yBMMXCqrya4f0ifCPK+r//TVUo8=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_ae2780ba8d7d11f1bfea525400e6dd8f
    ReservedCode2: CfXze3YuGhzFEURsc/QAKQfTWslVVrMDh/szpeM/iF7hShQsOaKjybQruT31/DAb07DI7N3xJi4BxTMAS335qXqULT+lpO6qCFyuhwpOnyWIyOSmrKB08fV1tB6w16DBGKzMO/SLxhdi93fL7EwsRVy/z+tKzYO0yBMMXCqrya4f0ifCPK+r//TVUo8=
---

# Flowio 系统架构

> 版本：v2.0 · 日期：2026-08-01
> 前置阅读：`docs/scribe-full-product-matrix.md`（产品矩阵）、`docs/scribe-deep-analysis.md`（竞品分析）

---

## 一、宏观架构（四端一平台）

```
                            ┌─────────────────────────────────────────────┐
                            │             Flowio Cloud                     │
                            │  ┌───────────────────────────────────────┐  │
                            │  │  API Gateway (Nginx / Kong)           │  │
                            │  └───────┬───────────┬───────────┬───────┘  │
                            │          │           │           │          │
                            │  ┌───────▼──┐ ┌──────▼───┐ ┌─────▼──────┐  │
                            │  │  Auth    │ │ Document │ │ Team       │  │
                            │  │  Service │ │ Service  │ │ Service    │  │
                            │  └───────┬──┘ └──────┬───┘ └─────┬──────┘  │
                            │          │           │           │          │
                            │  ┌───────▼───────────▼───────────▼──────┐  │
                            │  │           PostgreSQL + OSS            │  │
                            │  └──────────────────────────────────────┘  │
                            └─────────────────────────────────────────────┘
                   HTTPS ▲                ▲ HTTPS              ▲ HTTPS
                         │                │                    │
    ┌────────────────────┼────────────────┼────────────────────┼──────────┐
    │                    │                │                    │          │
    │              ┌─────┴─────┐    ┌─────┴─────┐       ┌─────┴─────┐    │
    │              │  Desktop  │    │ Extension │       │  Web App  │    │
    │              │  (Tauri)  │    │  (Chrome) │       │ (Next.js) │    │
    │              └─────┬─────┘    └─────┬─────┘       └─────┬─────┘    │
    │                    │               │                     │          │
    │              ┌─────┴─────┐         │                     │          │
    │              │  SQLite   │         │                     │          │
    │              │  (本地)   │         │                     │          │
    │              └───────────┘         │                     │          │
    │                                    │                     │          │
    │   ┌────────────────────────────────┴─────────────────────┘          │
    │   │                                                                 │
    │   │                    局域网分享（HTTP 直连）                        │
    │   │     Desktop 启动本地 HTTP Server → 同网络设备浏览器直接访问        │
    │   │                                                                 │
    │   └──────────────────────┬───────────────────────┬──────────────────┘
    │                          │                       │
    │                   ┌──────▼──────┐          ┌─────▼──────┐
    │                   │  小程序查看端 │          │  移动浏览器  │
    │                   │  (微信原生)  │          │  (WebView) │
    │                   └─────────────┘          └────────────┘
    │
    └──────────────────────────────────────────────────────────────────────
```

### 各端角色

| 端 | 数据存储 | 核心能力 | 联网需求 |
|----|---------|---------|:---:|
| **Desktop** | 本地 SQLite | 录制 + 编辑 + AI + 导出 | 离线可用，AI 需联网 |
| **Extension** | 浏览器 LocalStorage | 录制网页操作 | 推送到 Desktop 或 Cloud |
| **Web App** | Cloud PostgreSQL | 管理 + 团队协作 + Analytics | 始终在线 |
| **小程序** | Cloud PostgreSQL | 查看分享 + 转发 | 始终在线 |

---

## 二、Desktop 桌面端架构

### 2.1 进程架构

```
┌──────────────────────────────────────────────────────┐
│                   Tauri App 主进程                     │
│                                                       │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐            │
│  │ WebView  │  │  Rust    │  │  System  │            │
│  │ (React)  │  │ Backend  │  │  Tray    │            │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘            │
│       │             │             │                   │
│       │   invoke    │             │                   │
│       │◄───────────►│             │                   │
│       │   (IPC)     │             │                   │
│       │             │             │                   │
│  ┌────▼─────────────▼─────────────▼─────┐             │
│  │          Tauri Core Runtime           │             │
│  └──────────────────────────────────────┘             │
│                                                       │
│  ┌──────────────────────────────────────┐             │
│  │        独立线程 / 后台任务             │             │
│  │  ┌──────────┐ ┌──────────┐ ┌──────┐ │             │
│  │  │ Recorder │ │AI Pipeline│ │Share │ │             │
│  │  │  Thread  │ │  Worker  │ │Server│ │             │
│  │  └──────────┘ └──────────┘ └──────┘ │             │
│  └──────────────────────────────────────┘             │
└──────────────────────────────────────────────────────┘
```

### 2.2 Rust 模块关系图

```
src-tauri/src/
│
├── main.rs                     # 入口，启动 Tauri
├── lib.rs                      # 注册所有 Tauri Command
├── types.rs                    # 跨模块共享类型
│
├── recorder/                   # 录制引擎
│   ├── mod.rs                  # 状态机：Idle → Recording → Paused → Processing
│   ├── capture.rs              # 截图：Win32 BitBlt + DXGI
│   ├── listener.rs             # 全局 Hook：SetWindowsHookEx (WH_MOUSE_LL + WH_KEYBOARD_LL)
│   ├── uia.rs                  # UI 元素：UIAutomation Core (element_info)
│   ├── step_builder.rs         # 原始事件 → Step 转换
│   ├── dedup.rs                # 去重：连续点击合并 / 静止截图丢弃
│   └── storage.rs              # 持久化：rusqlite → SQLite
│
├── ai/                         # AI 引擎
│   ├── mod.rs                  # 入口
│   ├── pipeline.rs             # 流水线编排：去重→截图裁剪→AI→校验→保存
│   ├── client.rs               # HTTP 客户端：GLM-4 / DeepSeek / 通义千问
│   ├── prompt.rs               # Prompt 模板：正向引导式中文 Prompt
│   ├── parser.rs               # 响应解析：JSON 提取 + 容错
│   ├── chinese_ui.rs           # 中文术语：100+ UI 术语词表 + 模糊匹配
│   └── key_manager.rs          # API Key：AES-GCM 加密存储
│
├── editor/                     # 编辑器后端
│   ├── mod.rs                  # 入口
│   ├── commands.rs             # Tauri 命令：CRUD 步骤、重排序、合并
│   └── history.rs              # 撤销/重做栈
│
├── export/                     # 导出引擎
│   ├── mod.rs                  # 入口
│   ├── pdf.rs                  # PDF：printpdf crate
│   ├── html.rs                 # HTML：单文件查看器模板
│   ├── markdown.rs             # Markdown：模板拼接
│   └── image.rs                # 微信长图：image crate 纵向拼接
│
├── share/                      # 分享服务
│   ├── mod.rs                  # 入口
│   ├── server.rs               # HTTP Server：tiny_http，随机端口
│   ├── templates.rs            # HTML 查看器模板（只读）
│   └── auth.rs                 # 可选密码保护
│
└── settings/                   # 设置
    ├── mod.rs                  # 入口
    └── config.rs               # 配置文件读写（JSON）
```

### 2.3 数据流：录制 → AI → 编辑

```
用户操作
   │
   ▼
┌──────────────────────────────────────────────┐
│  1. Recorder Thread                          │
│     WH_MOUSE_LL / WH_KEYBOARD_LL 监听        │
│     → RawEvent { type, x, y, key, ... }      │
│     → UIA 查询 → ElementInfo                 │
│     → capture.rs 截图 → screenshot_path      │
│     → step_builder → Step (暂存内存)         │
└────────┬─────────────────────────────────────┘
         │ 用户点击「完成录制」
         ▼
┌──────────────────────────────────────────────┐
│  2. 持久化                                    │
│     storage.rs → INSERT INTO recordings      │
│     → recording_id                           │
└────────┬─────────────────────────────────────┘
         │ 自动或手动触发 AI
         ▼
┌──────────────────────────────────────────────┐
│  3. AI Pipeline (独立线程)                   │
│     a. dedup: 合并连续点击 / 丢弃静止截图     │
│     b. 对每步 After 截图智能裁剪              │
│     c. prompt: 构建中文 Prompt               │
│     d. client: HTTP POST → GLM-4-flash       │
│     e. parser: 解析 AI 返回的 title + desc    │
│     f. chinese_ui: 术语校验                   │
│     g. 写回 Step.ai_title / ai_description   │
│     h. 流式推送进度给前端 (Event)             │
└────────┬─────────────────────────────────────┘
         │
         ▼
┌──────────────────────────────────────────────┐
│  4. 编辑器                                    │
│     React 通过 invoke 读取 steps[]            │
│     用户编辑 → invoke 写回 → 前端更新         │
└──────────────────────────────────────────────┘
```

### 2.4 Tauri Command 清单（IPC 接口）

```rust
// === 录制 ===
#[tauri::command] fn start_recording(app_name: String) -> Result<String, String>
#[tauri::command] fn pause_recording() -> Result<(), String>
#[tauri::command] fn resume_recording() -> Result<(), String>
#[tauri::command] fn stop_recording(title: String) -> Result<Recording, String>
#[tauri::command] fn cancel_recording() -> Result<(), String>
#[tauri::command] fn get_recording_status() -> Result<RecordingStatus, String>

// === AI ===
#[tauri::command] fn generate_ai(recordingId: String) -> Result<(), String>
#[tauri::command] fn get_ai_progress() -> Result<AiProgress, String>
#[tauri::command] fn cancel_ai_generation() -> Result<(), String>

// === 编辑器 ===
#[tauri::command] fn get_recording(recordingId: String) -> Result<Recording, String>
#[tauri::command] fn update_step_title(stepId: String, title: String) -> Result<(), String>
#[tauri::command] fn update_step_description(stepId: String, description: String) -> Result<(), String>
#[tauri::command] fn delete_step(stepId: String) -> Result<(), String>
#[tauri::command] fn reorder_steps(recordingId: String, fromIndex: usize, toIndex: usize) -> Result<(), String>
#[tauri::command] fn merge_steps(stepIdA: String, stepIdB: String) -> Result<Step, String>
#[tauri::command] fn add_step(recordingId: String, afterIndex: usize) -> Result<Step, String>
#[tauri::command] fn recapture_screenshot(stepId: String) -> Result<String, String>  // 重新截图
#[tauri::command] fn add_annotation(stepId: String, annotation: Annotation) -> Result<(), String>

// === 导出 ===
#[tauri::command] fn exportPdf(recordingId: String, outputPath: String) -> Result<String, String>
#[tauri::command] fn exportHtml(recordingId: String, outputPath: String) -> Result<String, String>
#[tauri::command] fn exportMarkdown(recordingId: String, outputPath: String) -> Result<String, String>
#[tauri::command] fn exportLongImage(recordingId: String, outputPath: String) -> Result<String, String>

// === 分享 ===
#[tauri::command] fn startShare(recordingId: String) -> Result<String, String>
#[tauri::command] fn stopShare() -> Result<(), String>
#[tauri::command] fn getShareStatus() -> Result<ShareStatus, String>

// === 设置 ===
#[tauri::command] fn getSettings() -> Result<Settings, String>
#[tauri::command] fn updateSettings(settings: Settings) -> Result<(), String>
#[tauri::command] fn validateApiKey(model: String, key: String) -> Result<bool, String>

// === 管理 ===
#[tauri::command] fn listRecordings(filter: RecordingFilter) -> Result<Vec<RecordingSummary>, String>
#[tauri::command] fn deleteRecording(recordingId: String) -> Result<(), String>
#[tauri::command] fn duplicateRecording(recordingId: String) -> Result<Recording, String>
```

---

## 三、Extension 浏览器扩展架构

### 3.1 组件架构

```
┌─────────────────────────────────────────────────────────────┐
│                     Flowio Extension                         │
│                                                              │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────────┐ │
│  │  Popup       │   │  Background  │   │  Content Script  │ │
│  │  (popup.html)│   │  (SW)        │   │  (注入目标页面)   │ │
│  │              │   │              │   │                  │ │
│  │ • 开始/停止  │   │ • 状态管理   │   │ • DOM 事件监听   │ │
│  │ • 录制状态   │◄──► • 消息路由   │◄──►│ • 元素信息提取   │ │
│  │ • 历史列表   │   │ • 截图调度   │   │ • 录制浮窗 UI    │ │
│  └──────────────┘   │ • 数据持久化 │   │ • 页面截图       │ │
│                      └──────┬───────┘   └──────────────────┘ │
│                             │                                 │
│                      ┌──────▼───────┐                         │
│                      │   Storage    │                         │
│                      │ (IndexedDB)  │                         │
│                      └──────────────┘                         │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Native Messaging (可选)                              │   │
│  │  与 Desktop App 通信 → 推送录制数据到桌面端编辑器       │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 录制流程

```
用户点击 Popup「开始录制」
          │
          ▼
Background Worker 设置状态 = RECORDING
          │
          ▼
Content Script 注入当前标签页
  ├── 监听 DOM 事件（click / input / change / focus）
  ├── 提取元素信息（aria-label / text / CSS selector）
  ├── 在页面右下角渲染浮动红点控件
  └── 每次有效操作后 → 发送消息给 Background
          │
          ▼
Background Worker
  ├── 收到「新步骤」消息
  ├── 调用 chrome.tabs.captureVisibleTab() 截图
  ├── 构建 Step 对象
  └── 存入 IndexedDB
          │
          ▼
用户点击「完成录制」
          │
          ▼
Background Worker 停止录制
  ├── 弹出完成提示
  ├── 提供选项：
  │   ├── 「在桌面端编辑」→ 通过 Native Messaging 推送到 Desktop
  │   ├── 「用云端 AI 生成」→ 直接调用 Flowio Cloud API
  │   └── 「稍后处理」→ 保留在 IndexedDB
```

### 3.3 与 Desktop 端的数据同步

```
Extension (IndexedDB)
        │
        │  Native Messaging
        │  / WebSocket (localhost)
        ▼
Desktop App (Tauri)
        │
        │  导入为 Recording
        ▼
  SQLite → 编辑器 → AI Pipeline
```

---

## 四、Web 控制台架构

### 4.1 服务架构

```
                    ┌───────────────────────────┐
                    │      CDN (OSS + CDN)       │
                    │  截图 / 静态资源 / HTML     │
                    └───────────────────────────┘
                                  │
                    ┌─────────────▼─────────────┐
                    │     Nginx (反向代理)        │
                    │  • HTTPS 终结              │
                    │  • 限流 / 防 DDoS           │
                    │  • 静态文件服务             │
                    └─────────────┬─────────────┘
                                  │
              ┌───────────────────┼───────────────────┐
              │                   │                   │
    ┌─────────▼──────┐  ┌────────▼───────┐  ┌────────▼──────┐
    │  Next.js SSR   │  │  API Server    │  │  WebSocket    │
    │  (React 前端)   │  │  (Node/Go)    │  │  Server       │
    │                │  │               │  │  (实时推送)    │
    │  • SSR 首屏    │  │  • Auth       │  │  • AI 进度    │
    │  • React 水合  │  │  • Document   │  │  • 协作编辑    │
    │  • 路由        │  │  • Team       │  │  • 通知       │
    └────────────────┘  │  • Analytics  │  └───────────────┘
                        │  • Export     │
                        │  • Billing    │
                        └───────┬───────┘
                                │
                    ┌───────────▼───────────┐
                    │    PostgreSQL          │
                    │  ┌──────────────────┐  │
                    │  │ users            │  │
                    │  │ recordings       │  │
                    │  │ steps            │  │
                    │  │ teams            │  │
                    │  │ team_members     │  │
                    │  │ share_links      │  │
                    │  │ analytics_events │  │
                    │  │ billing          │  │
                    │  └──────────────────┘  │
                    └───────────────────────┘
```

### 4.2 Cloud 数据库核心表

```sql
-- 用户
CREATE TABLE users (
    id UUID PRIMARY KEY,
    phone VARCHAR(20) UNIQUE,
    wechat_union_id VARCHAR(64) UNIQUE,
    feishu_union_id VARCHAR(64),
    dingtalk_union_id VARCHAR(64),
    nickname VARCHAR(100),
    avatar_url TEXT,
    created_at TIMESTAMP DEFAULT NOW()
);

-- 录制文档
CREATE TABLE recordings (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users(id),
    team_id UUID REFERENCES teams(id),
    title VARCHAR(200),
    app_name VARCHAR(100),
    status VARCHAR(20),  -- draft / published / archived
    is_public BOOLEAN DEFAULT FALSE,
    view_count INT DEFAULT 0,
    version INT DEFAULT 1,         -- 编辑版本号，用于分享链接自动同步
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

-- 步骤（云存储，用于分享查看器渲染）
CREATE TABLE steps (
    id UUID PRIMARY KEY,
    recording_id UUID REFERENCES recordings(id) ON DELETE CASCADE,
    index INT NOT NULL,
    screenshot_url TEXT,           -- OSS 地址
    after_screenshot_url TEXT,     -- OSS 地址
    ai_title VARCHAR(100),
    ai_description TEXT,
    user_title VARCHAR(100),
    user_description TEXT,
    action_type VARCHAR(20),
    position_x INT,
    position_y INT,
    annotations JSONB DEFAULT '[]',
    redactions JSONB DEFAULT '[]',
    created_at TIMESTAMP DEFAULT NOW()
);

-- 团队
CREATE TABLE teams (
    id UUID PRIMARY KEY,
    name VARCHAR(200),
    logo_url TEXT,
    brand_color VARCHAR(7),        -- #3B73B0
    created_at TIMESTAMP DEFAULT NOW()
);

-- 团队成员
CREATE TABLE team_members (
    team_id UUID REFERENCES teams(id),
    user_id UUID REFERENCES users(id),
    role VARCHAR(20),              -- admin / editor / viewer
    joined_at TIMESTAMP DEFAULT NOW(),
    PRIMARY KEY (team_id, user_id)
);

-- 分享链接
CREATE TABLE share_links (
    id UUID PRIMARY KEY,
    recording_id UUID REFERENCES recordings(id),
    share_code VARCHAR(20) UNIQUE,  -- 短码，用于 URL
    password_hash VARCHAR(256),
    expires_at TIMESTAMP,
    view_count INT DEFAULT 0,
    created_at TIMESTAMP DEFAULT NOW()
);

-- 分析事件
CREATE TABLE analytics_events (
    id UUID PRIMARY KEY,
    recording_id UUID REFERENCES recordings(id),
    event_type VARCHAR(50),        -- view / complete / abandon / step_view
    step_index INT,
    viewer_ip VARCHAR(45),
    viewer_user_agent TEXT,
    created_at TIMESTAMP DEFAULT NOW()
);
```

---

## 五、数据关系全景

```
                      ┌──────────────┐
                      │    User      │
                      └──────┬───────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
    ┌─────────▼────┐  ┌──────▼──────┐  ┌────▼────────┐
    │  Recording   │  │ TeamMember  │  │  ApiKey     │
    │  (本地/云端)  │  └──────┬──────┘  └─────────────┘
    └──────┬───────┘         │
           │          ┌──────▼──────┐
    ┌──────▼───────┐  │    Team     │
    │    Step      │  └─────────────┘
    │  (1:N)       │
    └──────┬───────┘
           │
    ┌──────┼───────┬──────────┐
    │      │       │          │
  ┌─▼──┐ ┌─▼───┐ ┌─▼──────┐ ┌▼────────┐
  │Anno│ │Tip  │ │Redact  │ │ShareLink│
  └────┘ └─────┘ └────────┘ └─────────┘
```

### 本地 vs 云端存储策略

| 数据 | 本地 (SQLite) | 云端 (PostgreSQL) |
|------|:---:|:---:|
| Recording 元数据 | ✅ | 可选同步 |
| Step 列表 | ✅ | 可选同步 |
| 截图文件 | ✅ 本地路径 | 可选上传 OSS |
| 用户账户 | ❌ | ✅ |
| 团队信息 | ❌ | ✅ |
| 分享链接 | 本地 HTTP 临时 | ✅ 持久化 |
| Analytics | ❌ | ✅ |
| 设置 | ✅ | ❌（纯本地） |
| API Key | ✅ 加密 | ❌（不上传） |

---

## 六、安全架构

### 6.1 数据安全分层

```
┌─────────────────────────────────────────────┐
│  Layer 4: 云端数据                           │
│  • 传输：HTTPS TLS 1.3                       │
│  • 存储：PostgreSQL 加密 + OSS 服务端加密     │
│  • 脱敏：截图上传前自动脱敏                    │
│  • 合规：数据不出境，阿里云中国区              │
├─────────────────────────────────────────────┤
│  Layer 3: 分享安全                           │
│  • 局域网：HTTP + 密码保护 + 临时目录          │
│  • 云端：HTTPS + 密码 + 过期时间 + 访问次数限制 │
│  • 查看器：禁止右键/下载（前端级，非绝对）      │
├─────────────────────────────────────────────┤
│  Layer 2: 本地存储                           │
│  • SQLite 文件权限（用户目录）                │
│  • API Key：AES-256-GCM 加密                 │
│  • 截图：用户 AppData 目录                   │
├─────────────────────────────────────────────┤
│  Layer 1: 录制安全                           │
│  • 录制前弹窗告知采集范围                     │
│  • 密码框自动跳过截图（UIA PasswordMask 检测） │
│  • 录制中红点可见状态指示                     │
└─────────────────────────────────────────────┘
```

### 6.2 隐私脱敏流水线

```
截图 (PNG)
    │
    ▼
┌────────────────┐
│ OCR 文字识别    │  ← tesseract / 腾讯云 OCR / 百度 OCR
│ 提取所有文字区域 │
└───────┬────────┘
        │
        ▼
┌────────────────┐
│ 敏感信息匹配    │
│ • 身份证号 18位 │  正则匹配  → 标记 [x1,y1,w1,h1]
│ • 银行卡号 16-19│  正则匹配  → 标记 [x2,y2,w2,h2]
│ • 手机号 11位  │  正则匹配  → 标记 [x3,y3,w3,h3]
│ • 邮箱          │  正则匹配  → 标记 [x4,y4,w4,h4]
│ • 统一社会信用代码│ 正则匹配  → 标记 [x5,y5,w5,h5]
│ • 密码框        │  UIA 标记  → 自动跳过
└───────┬────────┘
        │
        ▼
┌────────────────┐
│ 模糊处理        │
│ Gaussian Blur   │
│ 或纯色矩形覆盖   │
└───────┬────────┘
        │
        ▼
脱敏后截图 → 保存 / 上传
```

---

## 七、部署架构

### 7.1 Cloud 部署方案

```
┌──────────────────────────────────────────────┐
│              阿里云 / 腾讯云                    │
│                                              │
│  ┌──────────────┐    ┌──────────────┐        │
│  │  SLB 负载均衡  │    │  CDN + OSS   │        │
│  │  (按量付费)   │    │  (截图/静态)  │        │
│  └──────┬───────┘    └──────────────┘        │
│         │                                    │
│  ┌──────▼────────────────────────┐           │
│  │  ECS 云服务器 (2台+)           │           │
│  │  ┌────────┐  ┌─────────────┐  │           │
│  │  │ Nginx  │  │ Next.js SSR │  │           │
│  │  │ 反代    │  │ + API Svr   │  │           │
│  │  └────────┘  └─────────────┘  │           │
│  └───────────────────────────────┘           │
│                                              │
│  ┌──────────────────────────────┐            │
│  │  RDS PostgreSQL (高可用版)    │            │
│  └──────────────────────────────┘            │
│                                              │
│  ┌──────────────────────────────┐            │
│  │  Redis (缓存 + Session)       │            │
│  └──────────────────────────────┘            │
│                                              │
└──────────────────────────────────────────────┘
```

### 7.2 私有化部署方案（Enterprise）

```
┌──────────────────────────────────────────────┐
│           客户内网 / 私有云                     │
│                                              │
│  ┌──────────────────────────────────────┐    │
│  │  Docker Compose / K8s                 │    │
│  │                                       │    │
│  │  ┌─────────┐ ┌─────────┐ ┌────────┐  │    │
│  │  │ flowio- │ │ flowio- │ │ PostgreSQL│  │    │
│  │  │ web     │ │ api     │ │         │  │    │
│  │  └─────────┘ └─────────┘ └────────┘  │    │
│  │                                       │    │
│  │  ┌─────────┐ ┌─────────┐              │    │
│  │  │  Redis  │ │ MinIO   │ (对象存储)   │    │
│  │  └─────────┘ └─────────┘              │    │
│  └──────────────────────────────────────┘    │
│                                              │
│  ┌──────────────────────────────────────┐    │
│  │  企业基础设施集成                       │    │
│  │  • LDAP / AD 用户目录                  │    │
│  │  • SAML / OIDC SSO                   │    │
│  │  • 企业审计系统对接 (Syslog)            │    │
│  └──────────────────────────────────────┘    │
│                                              │
└──────────────────────────────────────────────┘
```

---

## 八、关键技术决策记录

| # | 决策 | 理由 |
|---|------|------|
| 1 | Desktop 用 Tauri 2 而非 Electron | 体积小（~5MB vs ~150MB），Rust 性能优于 Node.js |
| 2 | 本地 SQLite 而非 IndexedDB | 截图量大，SQLite 更适合文件路径索引，跨进程共享 |
| 3 | AI 本地优先，云端兜底 | 隐私保护 + 离线场景，国产模型 API 支持 |
| 4 | 分享用本地 HTTP 而非强制云端 | 局域网分享零延迟，不上传截图即分享 |
| 5 | 浏览器扩展用 Manifest V3 | Chrome 强制要求，Edge 兼容 |
| 6 | Web 用 Next.js SSR | SEO 友好 + 首屏性能 + 与 React 共享组件 |
| 7 | 移动端用微信小程序而非 RN/Flutter | 零安装、微信生态闭环、社交裂变 |
| 8 | API 网关上云而非自建 | 初期成本低，按量付费，随规模迁移 |
| 9 | Prompt 正向引导而非禁则列表 | 国产大模型对正向指令响应更好 |
| 10 | 截图 OSS 按需上传而非全量同步 | 节省带宽，用户主动分享时才上传 |

---

> 本架构文档定义了 Flowio 四端一平台的完整技术骨架。从 Desktop 的 Rust 模块图到 Cloud 的数据库 Schema，从录制数据流到分享安全模型，可以直接作为各端开发的架构基准。
*（内容由AI生成，仅供参考）*
