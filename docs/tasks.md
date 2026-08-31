---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_b0622c05882e11f1b66e525400e6dd8f
    ReservedCode1: 8TC1+zU3eUao685U9e8X+iLSQTg/z7RVcQvWVmoZGqlLBkIcwHQkmQl/N9hTCJxKOvC4LjbaeCNatCEz7oyh/HOAD2q/E1O62Kg+TBUgWznlf1PsY3ChswlZH7Cj1a60X1D42BN3NU8QsMxzISHz0q7LbMthoR0ZFFMPVM25rc7j2eAJMgVlbM+oM8Q=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_b0622c05882e11f1b66e525400e6dd8f
    ReservedCode2: 8TC1+zU3eUao685U9e8X+iLSQTg/z7RVcQvWVmoZGqlLBkIcwHQkmQl/N9hTCJxKOvC4LjbaeCNatCEz7oyh/HOAD2q/E1O62Kg+TBUgWznlf1PsY3ChswlZH7Cj1a60X1D42BN3NU8QsMxzISHz0q7LbMthoR0ZFFMPVM25rc7j2eAJMgVlbM+oM8Q=
---

# 录步 (Flowio) 任务清单

> 文档版本：v1.0
> 创建日期：2026-07-25
> 所属阶段：阶段 2 — 开发阶段
> 依赖文档：AGENTS.md / spec.md / plan.md / design-system.md / stepsnap-analysis.md

---

## 总览

> ⚡ **2 周冲刺模式，AI 辅助开发。** 任务内容、依赖关系、验收条件保持不变，工时按原值折半。

| Week | 主题 | 任务数 | 预估总工时 | 里程碑 |
|------|------|--------|-----------|--------|
| Week 1 | 录制引擎 + AI 处理管道 | 14 | ~37.5h | 录制 10 步 → AI 生成中文步骤 → 前端展示 |
| Week 2 | 编辑器 + 导出 + 设置 + 分享 + 收尾 | 16 | ~37.5h | MVP 完整闭环：编辑 → 导出 → 测试 → 可发布 |
| **合计** | | **30** | **~75h** | |

---

## Week 1：录制引擎 + AI 处理管道

**周里程碑**：录制 10 步 → AI 生成中文步骤 → 前端流式展示，录制引擎 + AI 管道全通。

---

### T1.1：初始化 Tauri 2 项目骨架，配置 Tailwind + TypeScript strict

| 属性 | 内容 |
|------|------|
| **编号** | T1.1 |
| **标题** | 初始化 Tauri 2 项目骨架，配置 Tailwind + TypeScript strict |
| **前置任务** | 无 |
| **描述** | 基于 StepSnap 的 Tauri 配置（`tauri.conf.json`、`Cargo.toml`）创建录步的项目骨架。主要工作：<br>1. 在 `D:\Projects\flowio\` 下初始化 Vite + React-TS 前端脚手架<br>2. 初始化 Tauri 2（窗口标题"录步 Flowio"，窗口尺寸 1200×800，最小 800×600）<br>3. 配置 `tsconfig.json` strict mode（`strict: true`, `noUnusedLocals: true`, `noUnusedParameters: true`）<br>4. 配置 Tailwind CSS v4（`@tailwindcss/vite` 插件，引入 design-system.md 色板/字体/间距）<br>5. 配置 `.gitignore`（保护 `.env.local`、`target/`、`node_modules/`）<br>6. 安装 lucide-react 图标库 |
| **涉及文件** | `package.json`, `tsconfig.json`, `vite.config.ts`, `tailwind.config.ts`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src/App.tsx`, `src/index.css` |
| **验收条件** | 1. `pnpm install && pnpm build` 前端编译通过（0 error）<br>2. `cargo check` 后端编译通过（0 error）<br>3. `npm run tauri dev` 启动后窗口标题显示"录步 Flowio"<br>4. TypeScript strict mode 生效（写入 `any` 类型报编译错误）<br>5. Tailwind 色板可用（`bg-primary-500` 等 class 生效）<br>6. 窗口尺寸 1200×800，最小 800×600 |
| **预估时间** | 2h |

---

### T1.2：实现 Rust 后端 DOM 事件捕获模块

| 属性 | 内容 |
|------|------|
| **编号** | T1.2 |
| **标题** | 实现 Rust 后端 DOM 事件捕获模块（基于 StepSnap 逻辑重写） |
| **前置任务** | T1.1 |
| **描述** | 从 StepSnap 的 `recorder.rs`（1104 行）提取核心录制逻辑，重写为录步的模块结构。主要工作：<br>1. 创建 `src-tauri/src/recorder/` 模块目录<br>2. 实现 `listener.rs`：基于 `rdev` 的全局事件监听（鼠标点击/释放、键盘按下/释放、滚轮），在独立线程中运行<br>3. 实现 `step_builder.rs`：将 rdev 事件转换为录步的 `Step` 结构体（含 UUID、时间戳、坐标、输入文本、step_type）<br>4. 实现 `mod.rs`：录制状态管理（`RecordingState`），控制开始/停止<br>5. 实现自过滤逻辑：检测并忽略录步自身窗口的操作事件<br>6. 实现密码字段哨兵机制：`is_password` 标记 → AI 生成阶段过滤<br>7. 单次录制最大步数限制：200 步 |
| **涉及文件** | `src-tauri/src/recorder/mod.rs`, `src-tauri/src/recorder/listener.rs`, `src-tauri/src/recorder/step_builder.rs`, `src-tauri/Cargo.toml`（rdev 依赖） |
| **验收条件** | 1. 启动录制后可捕获鼠标左键/右键/中键点击事件（含坐标）<br>2. 可捕获键盘输入（含文本内容累积）<br>3. 可捕获滚轮操作（方向+距离）<br>4. 录步自身窗口的操作被正确过滤<br>5. 事件捕获延迟 ≤ 10ms |
| **预估时间** | 4h |

---

### T1.3：实现截图模块

| 属性 | 内容 |
|------|------|
| **编号** | T1.3 |
| **标题** | 实现截图模块（xcap 集成，区域截取 + 光标位置） |
| **前置任务** | T1.2 |
| **描述** | 基于 StepSnap 的截图机制，实现录步的截图模块。主要工作：<br>1. 创建 `src-tauri/src/recorder/screenshot.rs`<br>2. 封装 `xcap::Monitor` 全屏截图能力，每次事件触发后自动截图<br>3. JPEG 保存至用户数据目录（默认 `%APPDATA%/Flowio/screenshots/`），文件名 `{project_id}_{step_index}.jpg`<br>4. 实现 after-frame 等待机制（UI 稳定后截图，最大等待 2s）<br>5. 截图质量 JPEG 85%，最大分辨率 1920px（长边），文件大小 ≤ 200KB/张<br>6. 截图不包含 EXIF 元数据 |
| **涉及文件** | `src-tauri/src/recorder/screenshot.rs`, `src-tauri/Cargo.toml`（xcap, image 依赖） |
| **验收条件** | 1. 每次鼠标点击后自动生成 JPEG 截图文件<br>2. 截图文件可正常打开，画面与操作瞬间一致<br>3. 截图延迟 ≤ 200ms<br>4. 截图文件 ≤ 200KB<br>5. 截图不含 EXIF 元数据 |
| **预估时间** | 3h |

---

### T1.4：实现 UI 元素识别模块

| 属性 | 内容 |
|------|------|
| **编号** | T1.4 |
| **标题** | 实现 UI 元素识别模块（Windows UI Automation API） |
| **前置任务** | T1.1 |
| **描述** | 从 StepSnap 的 `accessibility.rs`（650 行）提取 Windows UIA 实现，重写为录步模块。主要工作：<br>1. 创建 `src-tauri/src/accessibility/` 模块目录<br>2. 实现 `windows.rs`：初始化 COM → `ElementFromPoint()` 根据坐标获取 UI 元素 → 读取 `CurrentName` / `CurrentLocalizedControlType` / `CurrentClassName`<br>3. 实现 `types.rs`：`ElementInfo` 结构体（name / element_type / element_value / is_password / app_name）<br>4. 实现宿主应用名获取：向上遍历父节点（最多 10 层），取顶层窗口标题<br>5. 实现密码字段检测：`IsPasswordProperty` → `is_password = true`<br>6. 实现聚焦字段值读取：`GetFocusedElement()` → 三种方式读取值（ValuePattern / TextPattern / Legacy IAccessible）<br>7. 值截断最大 2000 字符<br>8. 平台适配入口 `mod.rs`：`#[cfg(target_os = "windows")]` |
| **涉及文件** | `src-tauri/src/accessibility/mod.rs`, `src-tauri/src/accessibility/windows.rs`, `src-tauri/src/accessibility/types.rs`, `src-tauri/Cargo.toml`（windows crate 依赖） |
| **验收条件** | 1. 点击 Windows 资源管理器"文件"菜单 → 获取 `element_name = "文件"`<br>2. 点击记事本编辑区 → 获取 `app_name = "记事本"`<br>3. 在密码框中输入 → 检测 `is_password = true`<br>4. 在文本框中输入"测试" → 获取 `element_value = "测试"`<br>5. 元素识别延迟 ≤ 50ms |
| **预估时间** | 3h |

---

### T1.5：搭建前端录制页面基础布局

| 属性 | 内容 |
|------|------|
| **编号** | T1.5 |
| **标题** | 搭建前端录制页面基础布局（侧边栏 + 主区域） |
| **前置任务** | T1.1 |
| **描述** | 按 design-system.md 第 6 章布局规范，搭建录步主界面。主要工作：<br>1. 实现 `AppLayout` 组件：220px 侧边栏（项目列表 + 搜索 + 新建按钮）+ 弹性主内容区<br>2. 实现 `Sidebar` 组件：项目列表（含空状态）、搜索框、"+ 新建"按钮<br>3. 实现 `StatusBar` 组件：录制状态指示器、AI 额度显示、快捷键提示（28px 高）<br>4. 实现 `Titlebar` 组件：Tauri 自绘标题栏，标题"录步 Flowio"居中（40px 高）<br>5. 实现 `Toolbar` 组件：录制按钮、导出按钮、分享按钮、设置图标（48px 高）<br>6. 实现基础路由：录制视图 / 编辑视图（用 React state 切换，不引入路由库）<br>7. 严格按照 design-system.md 的色板/字体/间距规范 |
| **涉及文件** | `src/components/layout/AppLayout.tsx`, `src/components/layout/Sidebar.tsx`, `src/components/layout/StatusBar.tsx`, `src/components/layout/Titlebar.tsx`, `src/components/layout/Toolbar.tsx`, `src/components/project/ProjectList.tsx`, `src/App.tsx` |
| **验收条件** | 1. 界面分为侧边栏（220px）+ 主内容区，布局与 design-system.md 线框图一致<br>2. 侧边栏显示"全部"分组 + 项目列表 + "+ 新建"按钮<br>3. 状态栏显示"● 未录制"状态<br>4. 工具栏显示录制/导出/分享/设置按钮<br>5. 配色、字体、间距符合 design-system.md 规范<br>6. 窗口缩至 800px 宽时侧边栏保持可用 |
| **预估时间** | 3h |

---

### T1.6：实现 Tauri Bridge 层

| 属性 | 内容 |
|------|------|
| **编号** | T1.6 |
| **标题** | 实现 Tauri Bridge 层（Rust ↔ React 通信接口） |
| **前置任务** | T1.2, T1.3, T1.4, T1.5 |
| **描述** | 建立前后端通信桥梁，实现录制引擎与前端 UI 的实时数据同步。主要工作：<br>1. 在 `lib.rs` 中注册所有 Tauri Command：`start_recording`, `stop_recording`, `get_recording_status`<br>2. 实现 Tauri Events（Rust → 前端）：`recording-started`, `recording-step`, `recording-stopped`<br>3. 前端 `useEffect` 监听事件，更新步骤列表状态<br>4. 实现 `tauri-plugin-global-shortcut` 注册 Ctrl+Alt+R 快捷键，触发开始/停止录制<br>5. 实现录制状态同步：系统托盘图标变化（录制中绿色圆点）、悬浮指示器<br>6. 实现 `get_projects` / `get_steps` command（为 T1.7 数据库集成做准备，先返回 mock 数据） |
| **涉及文件** | `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src/hooks/useRecording.ts`, `src/hooks/useTauriEvents.ts`, `src/App.tsx` |
| **验收条件** | 1. 前端点击"开始录制"→ Rust 端收到命令 → 返回成功<br>2. Rust 端 emit `recording-step` → 前端收到并更新步骤列表<br>3. 按下 Ctrl+Alt+R → 触发录制开始（全局，即使录步窗口不在焦点）<br>4. 录制中系统托盘图标显示绿色圆点<br>5. 录制停止后托盘恢复默认图标 |
| **预估时间** | 3h |

---

### T1.7：端到端录制流程联调 + 编译验证 + 一级自测

| 属性 | 内容 |
|------|------|
| **编号** | T1.7 |
| **标题** | 端到端录制流程联调 + 编译验证 + 一级自测 |
| **前置任务** | T1.2, T1.3, T1.4, T1.5, T1.6 |
| **描述** | 联调 Week 1 所有模块，确保录制全流程可用。主要工作：<br>1. 集成 T1.2（事件捕获）+ T1.3（截图）+ T1.4（元素识别）到 T1.6 的 Bridge 层<br>2. 实现 `database.rs` SQLite 初始化和基本 CRUD（projects 表 + steps 表，按 plan.md 3.6 设计）<br>3. 录制停止后将 Step 数组序列化存入 SQLite<br>4. 前端停止录制后从 SQLite 读取步骤列表并展示<br>5. 实现前端步骤列表 UI（按 design-system.md 5.7 Stepper 组件规范）<br>6. 运行 `pnpm build && cargo check`，确保 0 error<br>7. 一级自测（AI 自测）：模拟用户执行 10 步操作，验证全流程 |
| **涉及文件** | `src-tauri/src/database.rs`, `src-tauri/src/lib.rs`, `src/components/recorder/StepList.tsx`, `src/components/recorder/StepCard.tsx`, `src/components/recorder/RecordingControls.tsx` |
| **验收条件** | 1. 按下 Ctrl+Alt+R → 开始录制 → 执行 10 步操作（点击不同应用）→ 再次按下 Ctrl+Alt+R → 停止<br>2. 前端实时展示步骤列表（含序号、操作类型、元素名称、缩略图）<br>3. 步骤数据正确存入 SQLite（projects 1 条 + steps 10 条）<br>4. `pnpm build` 通过（0 error）<br>5. `cargo check` 通过（0 error）<br>6. 录制过程中 CPU ≤ 5%（任务管理器验证）<br>7. 一级自测清单全部打勾 |
| **预估时间** | 2h |

---

### AI 处理管道

**子里程碑**：录制完成 → 自动触发 AI 生成 → 15 步 ≤ 10s 生成完毕 → 中文描述准确 → 前端流式展示。

---

### T2.1：设计 Prompt 模板系统

| 属性 | 内容 |
|------|------|
| **编号** | T2.1 |
| **标题** | 设计 Prompt 模板系统（步骤生成 / 标题生成 / 描述润色） |
| **前置任务** | T1.7 |
| **描述** | 创建 Prompt 模板管理系统，支持多个场景的模板和运行时变量注入。主要工作：<br>1. 创建 `src/prompts/` 目录，存储 `.txt` 模板文件<br>2. 设计 `step-generation.txt`：步骤列表 → AI 生成标题+描述的主模板（按 plan.md 3.2 设计）<br>3. 设计 `step-regenerate.txt`：单步重新生成模板（含原步骤上下文）<br>4. 设计 `title-polish.txt`：标题润色模板（压缩到 ≤20 字）<br>5. 设计 `sensitive-filter.txt`：敏感信息过滤规则（身份证/手机号/银行卡正则）<br>6. 实现 Rust 端 `prompt_template.rs`：读取模板文件 + 变量占位符替换（`{steps_json}`, `{step_index}`）<br>7. 模板变量设计：`{steps_json}`, `{step_index}`, `{prev_title}`, `{prev_description}` |
| **涉及文件** | `src/prompts/step-generation.txt`, `src/prompts/step-regenerate.txt`, `src/prompts/title-polish.txt`, `src/prompts/sensitive-filter.txt`, `src-tauri/src/ai/prompt_template.rs` |
| **验收条件** | 1. 四个模板文件内容完整、中文语法正确<br>2. Rust 端可正确读取模板并替换变量<br>3. 替换后 Prompt 中无残留的 `{xxx}` 占位符<br>4. 敏感信息过滤正则可匹配 18 位身份证/11 位手机号/16-19 位银行卡号 |
| **预估时间** | 2h |

---

### T2.2：Rust 端 API 调用模块

| 属性 | 内容 |
|------|------|
| **编号** | T2.2 |
| **标题** | Rust 端 API 调用模块（OpenAI 兼容格式、流式 SSE、30s 超时、3 次重试） |
| **前置任务** | T2.1 |
| **描述** | 实现 Rust 端的 AI 服务调用核心模块。主要工作：<br>1. 创建 `src-tauri/src/ai/` 模块目录<br>2. 实现 `client.rs`：基于 `reqwest` 的 HTTP Client，支持 OpenAI 兼容格式 POST 请求<br>3. 实现流式 SSE 解析：逐行读取 `data: {json}\n\n`，解析 `delta.content`<br>4. 实现重试机制：网络错误自动重试 3 次，间隔 1s/2s/4s（指数退避）<br>5. 实现超时控制：单次请求 30s 超时<br>6. 实现 `model_config.rs`：三层模型配置（按 plan.md 4.1），支持 endpoint + model ID 动态切换<br>7. 实现 `AIModelProvider` trait：`build_request()`, `parse_stream_chunk()`, `get_endpoint()`<br>8. 实现具体 provider：`ZhipuProvider`, `DeepSeekProvider`（预留结构，MVP 只激活智谱） |
| **涉及文件** | `src-tauri/src/ai/mod.rs`, `src-tauri/src/ai/client.rs`, `src-tauri/src/ai/model_config.rs`, `src-tauri/src/ai/providers/zhipu.rs`, `src-tauri/src/ai/providers/deepseek.rs`, `src-tauri/src/ai/providers/qwen.rs`, `src-tauri/Cargo.toml`（reqwest, serde_json 依赖） |
| **验收条件** | 1. 构造 OpenAI 兼容格式 POST 请求 → 智谱 API 返回 200<br>2. SSE 流式响应逐 chunk 解析正确<br>3. 模拟网络中断 → 自动重试 3 次 → 第 4 次返回 error<br>4. 请求超过 30s → 触发超时 error<br>5. 切换 model_config 中的 endpoint → 请求正确发送到目标地址<br>6. `cargo check` 通过 |
| **预估时间** | 4h |

---

### T2.3：智谱 GLM-4-flash 对接和联调

| 属性 | 内容 |
|------|------|
| **编号** | T2.3 |
| **标题** | 智谱 GLM-4-flash 对接和联调 |
| **前置任务** | T2.2 |
| **描述** | 完成智谱 GLM-4-flash 的完整对接，包括 Prompt 构建、截图 Base64 嵌入、步骤解析。主要工作：<br>1. 实现 `ai_pipeline.rs`：Rust 端 AI 生成主流程——读取步骤数据 → 调用 T2.1 Prompt 模板 → 压缩截图并 Base64 编码 → 调用 T2.2 client → 流式推送到前端<br>2. 每步截图单独嵌入请求（作为多模态 image_url 字段）<br>3. 温度参数 `temperature=0.3`，`max_tokens=2048`<br>4. 实现 Tauri Command `generate_step_descriptions(project_id)`，通过 `app.emit("ai-stream-chunk")` 推送流式结果<br>5. 实现 `ai-stream-chunk` 事件格式：`{ step_id, field: "title"|"description", content, is_done }`<br>6. 配置智谱 API Key 来源：优先从 Tauri keyring 读取用户自定义 Key，若为空则使用内置 Key（从 `.env.local` 读取）<br>7. 实现 `ai-stream-error` 事件：失败时推送错误详情 |
| **涉及文件** | `src-tauri/src/ai/ai_pipeline.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/ai/providers/zhipu.rs` |
| **验收条件** | 1. 停止录制后调用 `generate_step_descriptions` → 前端收到 `ai-stream-chunk` 事件<br>2. 15 步以内生成时间 ≤ 10s<br>3. 生成的标题含操作对象名称（如"点击「文件」菜单"）<br>4. 生成的描述 ≤ 100 字且语义通顺<br>5. JSON 返回格式正确解析（`[{id, title, description}]`）<br>6. AI 调用失败时前端收到 `ai-stream-error` 事件 |
| **预估时间** | 3h |

---

### T2.4：截图压缩优化

| 属性 | 内容 |
|------|------|
| **编号** | T2.4 |
| **标题** | 截图压缩优化（平衡质量与 Token 消耗） |
| **前置任务** | T1.3 |
| **描述** | 优化 AI 调用前的截图处理管线，在图像质量和 Token 消耗之间取得平衡。主要工作：<br>1. 实现截图预处理管线：JPEG 质量 60%，最大宽度 800px，等比缩放<br>2. Base64 编码后大小目标 ≤ 50KB/张<br>3. 实现批量压缩：多张截图在后台线程并行处理<br>4. 智能跳过：纯白/纯黑/重复截图自动标记为"无需截图"，减少 Token 消耗<br>5. 实现压缩质量配置（可调整质量/分辨率参数） |
| **涉及文件** | `src-tauri/src/ai/image_processor.rs`, `src-tauri/src/recorder/screenshot.rs` |
| **验收条件** | 1. 原始截图 200KB → 压缩后 ≤ 50KB Base64<br>2. 压缩后图片仍可辨认 UI 元素和文字<br>3. 100 步截图压缩总耗时 ≤ 5s<br>4. 重复截图被正确识别并跳过 |
| **预估时间** | 2h |

---

### T2.5：步骤解析器

| 属性 | 内容 |
|------|------|
| **编号** | T2.5 |
| **标题** | 步骤解析器（AI 返回 JSON → 结构化步骤） |
| **前置任务** | T2.3 |
| **描述** | 将 AI 流式返回的 JSON chunk 解析并映射到 Step 结构体。主要工作：<br>1. 实现 `step_parser.rs`：流式 JSON 累加 + 解析逻辑<br>2. 处理 JSON 不完整的情况（流式中途截断）：等待 `]` 闭合后再解析<br>3. 解析完成后按 `step_id` 匹配 Step 结构体，填充 `ai_title` / `ai_description` 字段<br>4. 实现容错处理：AI 返回格式异常时尝试提取有效片段<br>5. 解析结果写入 SQLite（更新 steps 表的 ai_title / ai_description 字段）<br>6. 实现 `ai-stream-done` 事件：全部步骤解析完成通知前端 |
| **涉及文件** | `src-tauri/src/ai/step_parser.rs`, `src-tauri/src/ai/ai_pipeline.rs` |
| **验收条件** | 1. 完整 JSON 数组解析正确（10 步 → 10 个 Step 更新）<br>2. 流式中途截断的 JSON 等待闭合后正确解析<br>3. AI 返回格式异常时至少提取出有效的 title/description<br>4. 解析完成后 SQLite 中 ai_title / ai_description 字段非空<br>5. 前端收到 `ai-stream-done` 事件 |
| **预估时间** | 2h |

---

### T2.6：前端 AI 生成进度 UI

| 属性 | 内容 |
|------|------|
| **编号** | T2.6 |
| **标题** | 前端 AI 生成进度 UI（流式展示 + 骨架屏加载态） |
| **前置任务** | T2.3 |
| **描述** | 按 design-system.md 7.4 节设计，实现 AI 生成时的前端进度反馈。主要工作：<br>1. 实现 `AiProgressPanel` 组件：顶部进度条 + 步骤生成状态列表<br>2. 监听 `ai-stream-chunk` 事件，逐 Step 更新标题/描述（打字机效果）<br>3. 步骤状态标识：✅ 已完成 / 🔄 进行中（骨架屏动画） / ⏳ 等待中<br>4. 实现进度条（`ProgressBar` 组件）：百分比 + "正在生成步骤说明... 3/15"<br>5. 实现预计剩余时间显示<br>6. 实现 `AiErrorState` 组件：API 调用失败时的错误提示 + 重试按钮（按 design-system.md 7.6）<br>7. 生成完成后自动切换到编辑视图 |
| **涉及文件** | `src/components/ai/AiProgressPanel.tsx`, `src/components/ai/ProgressBar.tsx`, `src/components/ai/AiErrorState.tsx`, `src/hooks/useAiGeneration.ts` |
| **验收条件** | 1. 触发 AI 生成后显示进度面板<br>2. 已完成的步骤显示绿色 ✅ + 标题/描述<br>3. 进行中的步骤显示骨架屏脉冲动画<br>4. 进度条数值与实际步骤数一致<br>5. 全部生成完成后自动跳转编辑视图<br>6. AI 调用失败时显示错误面板 + 重试按钮 |
| **预估时间** | 2.5h |

---

### T2.7：端到端 AI 管道联调 + 编译验证 + 一级自测

| 属性 | 内容 |
|------|------|
| **编号** | T2.7 |
| **标题** | 端到端 AI 管道联调 + 编译验证 + 一级自测 |
| **前置任务** | T2.1—T2.6 |
| **描述** | 联调 Week 2 所有模块，确保 AI 生成全流程可用。主要工作：<br>1. 集成 T2.1（Prompt 模板）+ T2.2（API Client）+ T2.3（智谱对接）+ T2.4（截图压缩）+ T2.5（步骤解析）+ T2.6（前端 UI）<br>2. 端到端测试：录制 5 步 → AI 生成 → 验证输出质量<br>3. 测试边界情况：0 步、1 步、50 步、100 步<br>4. 测试错误处理：断网、API Key 无效、超时、返回格式异常<br>5. 性能测试：15 步 ≤ 10s，50 步 ≤ 30s<br>6. `pnpm build && cargo check` 确保 0 error<br>7. 一级自测清单 |
| **涉及文件** | `src-tauri/src/ai/ai_pipeline.rs`, `src-tauri/src/lib.rs`, `src/hooks/useAiGeneration.ts`, `src/components/ai/AiProgressPanel.tsx` |
| **验收条件** | 1. 录制 → AI 生成 → 前端流式展示完整流程可走通<br>2. 15 步生成时间 ≤ 10s<br>3. 中文描述准确（含操作对象名称，无语法错误）<br>4. 0 步/1 步/50 步/100 步场景均不崩溃<br>5. 断网 → 自动重试 3 次 → 显示错误提示<br>6. `pnpm build` 通过（0 error）<br>7. `cargo check` 通过（0 error）<br>8. 一级自测清单全部打勾 |
| **预估时间** | 2h |

---

## Week 2：编辑器 + 导出 + 设置 + 分享 + 收尾

**周里程碑**：MVP 完整闭环 — 录制 → AI 生成 → 编辑 3 步 → 导出 PDF/HTML/MD → 设置 → 测试通过 → 可发布。

---

### T3.1：步骤列表组件

| 属性 | 内容 |
|------|------|
| **编号** | T3.1 |
| **标题** | 步骤列表组件（展开/折叠、拖拽排序、选中态） |
| **前置任务** | T1.7 |
| **描述** | 按 design-system.md 5.7 节 Stepper 组件规范，实现完整的步骤列表组件。主要工作：<br>1. 实现 `StepList` 组件：垂直列表容器，`space-y-2` 间距<br>2. 实现 `StepCard` 组件：序号圆圈 + 截图缩略图（64×48）+ 标题 + 描述（两行截断）+ 拖拽手柄<br>3. 实现选中态：点击卡片 → `bg-primary-50 border-primary-200`<br>4. 实现展开/折叠：点击步骤 → 展开完整描述和操作详情（坐标/元素信息/时间戳）<br>5. 安装 `react-beautiful-dnd`，实现拖拽排序（按 design-system.md 7.3 反馈设计）<br>6. 拖拽后通过 Tauri Command `reorder_steps` 更新 SQLite<br>7. 实现拖拽动画：被拖拽卡片 `shadow-lg rotate-1 scale-[1.02]`<br>8. 实现键盘导航：↑/↓ 切换选中步骤 |
| **涉及文件** | `src/components/editor/StepList.tsx`, `src/components/editor/StepCard.tsx`, `src/hooks/useStepReorder.ts`, `package.json`（react-beautiful-dnd） |
| **验收条件** | 1. 步骤列表正确渲染所有步骤（序号、缩略图、标题、描述）<br>2. 点击步骤卡片 → 选中态高亮<br>3. 拖拽步骤到新位置 → 顺序更新，SQLite 同步<br>4. 拖拽动画流畅（60fps），反馈与设计一致<br>5. ↑/↓ 键可切换选中步骤<br>6. 展开步骤显示完整信息 |
| **预估时间** | 3h |

---

### T3.2：步骤编辑功能

| 属性 | 内容 |
|------|------|
| **编号** | T3.2 |
| **标题** | 步骤编辑功能（增删改、合并拆分、撤销重做 20 步） |
| **前置任务** | T3.1 |
| **描述** | 实现完整的步骤编辑器，覆盖 spec.md F03 全部编辑操作。主要工作：<br>1. 实现标题/描述内联编辑：双击 → 变为可编辑 input/textarea → Enter 保存 → Escape 取消<br>2. 实现删除步骤：选中 → Delete 键或右键菜单 → 确认弹窗 → 删除 SQLite 记录<br>3. 实现插入步骤：点击"+"按钮 → 弹出文件选择（上传截图）+ 输入标题/描述 → 插入到指定位置<br>4. 实现合并步骤：选中相邻两步 → "合并"按钮 → 拼接描述 → 删除第二步<br>5. 实现拆步骤：选中一步 → "拆分" → 拆为两步（用户分别编辑）<br>6. 实现撤销/重做：useReducer 维护 `history: Step[][]` + `historyIndex`，Ctrl+Z / Ctrl+Y，最多 20 步<br>7. 实现右键菜单：删除、复制、合并且上一步、合并且下一步、拆分、重新生成<br>8. 实现 AI 重新生成单步：选中步骤 → "重新生成" → 调用 AI 仅对该步重新生成描述<br>9. 所有编辑操作即时保存到 SQLite |
| **涉及文件** | `src/components/editor/StepEditor.tsx`, `src/components/editor/StepInlineEdit.tsx`, `src/components/editor/StepContextMenu.tsx`, `src/hooks/useStepEditor.ts`, `src/hooks/useUndoRedo.ts` |
| **验收条件** | 1. 双击标题 → 进入编辑模式 → 修改后 Enter 保存<br>2. 删除步骤后显示确认弹窗，确认后步骤消失<br>3. 在位置 3 插入步骤 → 原步骤 3-N 依次后移<br>4. 选中步骤 3、4 → 合并 → 新步骤描述为两者拼接<br>5. Ctrl+Z 撤销最近编辑 → 步骤恢复原状（最多 20 步）<br>6. Ctrl+Y 重做 → 编辑重新应用<br>7. 右键菜单 6 项操作全部可用<br>8. 单步 AI 重新生成 → 新描述替换旧描述<br>9. 所有编辑操作后 SQLite 数据同步 |
| **预估时间** | 4h |

---

### T3.3：PDF 导出

| 属性 | 内容 |
|------|------|
| **编号** | T3.3 |
| **标题** | PDF 导出（printpdf + 思源黑体嵌入、封面 + 步骤页） |
| **前置任务** | T1.7 |
| **描述** | 按 plan.md 3.4 导出引擎设计，实现 PDF 导出。主要工作：<br>1. 创建 `src-tauri/src/export/` 模块目录<br>2. 实现 `export_pdf.rs`：初始化 printpdf 文档（A4, 210mm×297mm）→ 嵌入思源黑体（Source Han Sans SC Regular + Bold，约 5MB，嵌入子集减少体积）<br>3. 封面页：文档标题（20pt 加粗居中）+ 生成日期 + 步骤数统计<br>4. 步骤页（每步一页）：步骤序号 + 标题（14pt 加粗）+ 截图 JPEG（等比例缩放至宽度 170mm）+ 描述（10pt 自动换行）<br>5. 页脚：页码 / 总页数<br>6. 实现 Tauri Command `export_pdf(project_id, save_path)`<br>7. 集成 `tauri-plugin-dialog` 弹出保存文件对话框<br>8. 导出过程通过 `app.emit("export-progress")` 推送进度<br>9. 思源黑体文件放在 `src-tauri/assets/fonts/` 目录 |
| **涉及文件** | `src-tauri/src/export/mod.rs`, `src-tauri/src/export/export_pdf.rs`, `src-tauri/assets/fonts/SourceHanSansSC-Regular.otf`, `src-tauri/assets/fonts/SourceHanSansSC-Bold.otf`, `src-tauri/Cargo.toml`（printpdf 依赖） |
| **验收条件** | 1. 50 步 PDF 导出时间 ≤ 5s<br>2. PDF 中文正常显示（无乱码、无 tofu 方块）<br>3. PDF 截图清晰可辨认<br>4. 封面页信息完整（标题、日期、步骤数）<br>5. 页码正确<br>6. 文件大小 ≤ 10MB（50 步含截图）<br>7. 保存对话框正常弹出，选择路径后保存成功 |
| **预估时间** | 3h |

---

### T3.4：HTML 导出

| 属性 | 内容 |
|------|------|
| **编号** | T3.4 |
| **标题** | HTML 导出（自包含单文件、离线可用） |
| **前置任务** | T1.7 |
| **描述** | 实现 HTML 格式导出，生成自包含的单文件。主要工作：<br>1. 前端实现 `exportHtml()` 函数：生成包含 CSS + 截图 Base64 + 步骤数据的 HTML 字符串<br>2. HTML 样式复用 design-system.md 的排版规范（字体/字号/间距/色板）<br>3. 截图以 Base64 `<img src="data:image/jpeg;base64,..." />` 内嵌，无外部依赖<br>4. 响应式布局：桌面端两栏（步骤描述 + 截图），移动端单栏堆叠<br>5. 通过 Tauri invoke 写入文件系统（`export_html` command）<br>6. 集成文件保存对话框 |
| **涉及文件** | `src/lib/export/htmlExporter.ts`, `src-tauri/src/export/export_html.rs`, `src/components/export/ExportDialog.tsx` |
| **验收条件** | 1. 生成的 HTML 文件可在浏览器中独立打开（断开网络验证）<br>2. 截图正常显示<br>3. 中文排版正常（字体回退至系统默认中文字体）<br>4. 响应式布局在 1024px 和 480px 下均可正常阅读<br>5. 50 步 HTML 导出 ≤ 3s |
| **预估时间** | 2h |

---

### T3.5：Markdown 导出

| 属性 | 内容 |
|------|------|
| **编号** | T3.5 |
| **标题** | Markdown 导出 |
| **前置任务** | T1.7 |
| **描述** | 实现 Markdown 格式导出，适合对接内部知识库。主要工作：<br>1. 前端实现 `exportMarkdown()` 函数：生成标准 Markdown 字符串<br>2. 格式：`# 标题` + `## 步骤 N` + 描述 + `![截图](data:image/jpeg;base64,...)`<br>3. 截图以 Base64 data URI 内嵌<br>4. 通过 Tauri invoke 写入 `.md` 文件<br>5. 集成文件保存对话框 |
| **涉及文件** | `src/lib/export/mdExporter.ts`, `src-tauri/src/export/export_markdown.rs` |
| **验收条件** | 1. 生成的 .md 文件可用 VS Code / Typora 正常预览<br>2. 截图正常显示<br>3. 标题层级正确（# → ##）<br>4. 50 步 MD 导出 ≤ 2s |
| **预估时间** | 1.5h |

---

### T3.6：图片导出

| 属性 | 内容 |
|------|------|
| **编号** | T3.6 |
| **标题** | 图片导出（单步骤截图 + 全流程长图） |
| **前置任务** | T1.7 |
| **描述** | 实现纯图片导出。主要工作：<br>1. 单步骤截图导出：将录制的原始 JPEG 截图复制到指定目录<br>2. 全流程长图导出：使用 `image` crate 将所有步骤截图垂直拼接为一张长图（含步骤序号和标题标注）<br>3. 长图标注：每张截图上方添加步骤序号 + 标题文字（思源黑体 14pt 白色文字 + 半透明黑底条）<br>4. 长图最大高度限制 30000px，超出时分多张<br>5. 通过 Tauri Command 保存，集成文件保存对话框 |
| **涉及文件** | `src-tauri/src/export/export_image.rs`, `src/lib/export/imageExporter.ts` |
| **验收条件** | 1. 单步骤截图可导出为 JPEG 文件<br>2. 10 步长图拼接正确，无错位<br>3. 长图中每步有清晰步骤序号和标题<br>4. 30 步长图导出 ≤ 5s<br>5. 超出 30000px 时自动分张 |
| **预估时间** | 2h |

---

### T3.7：编辑器 + 导出联调 + 编译验证 + 一级自测

| 属性 | 内容 |
|------|------|
| **编号** | T3.7 |
| **标题** | 编辑器 + 导出联调 + 编译验证 + 一级自测 |
| **前置任务** | T3.1—T3.6 |
| **描述** | 联调 Week 3 所有模块，确保编辑和导出全流程可用。主要工作：<br>1. 完整流程测试：录制 → AI 生成 → 编辑 3 步（改标题/调顺序/合并）→ 依次导出 PDF/HTML/MD/图片<br>2. 验证 SQLite 数据在编辑后的一致性<br>3. 验证导出文件的完整性（用外部工具打开验证）<br>4. `pnpm build && cargo check` 确保 0 error<br>5. 一级自测清单 |
| **涉及文件** | 全部 Week 3 涉及文件 |
| **验收条件** | 1. 完整流程可走通：录制 → AI → 编辑 → 四种导出<br>2. PDF 中文不乱码，截图清晰<br>3. HTML 浏览器可独立打开<br>4. MD Typora/VS Code 可正常预览<br>5. 长图拼接正确<br>6. 编辑后 SQLite 数据一致<br>7. `pnpm build` 通过（0 error）<br>8. `cargo check` 通过（0 error）<br>9. 一级自测清单全部打勾 |
| **预估时间** | 2h |

---

### 设置 + 分享 + 收尾

**子里程碑**：MVP 全部功能可用、三级测试通过、安装包 ≤ 15MB、中文界面无英文残留。

---

### T4.1：设置页面 UI

| 属性 | 内容 |
|------|------|
| **编号** | T4.1 |
| **标题** | 设置页面 UI（分层设置项、深色模式切换） |
| **前置任务** | T1.5 |
| **描述** | 按 design-system.md 6.4 设置页面线框图，实现完整的设置界面。主要工作：<br>1. 实现 `SettingsPanel` 组件：模态对话框，按 design-system.md 样式<br>2. 实现标签页导航：通用 / AI 模型 / 快捷键 / 关于（按 design-system.md 5.6 Tabs 组件）<br>3. "通用"标签：数据目录设置（浏览文件夹对话框）、语言选择（中文/English，MVP 仅中文可选）<br>4. "AI 模型"标签：按 design-system.md 6.4 设计——默认内置模型卡片（智谱 GLM-4-flash 已启用）+ 自定义模型列表（DeepSeek/通义千问/OpenAI，"即将上线"）+ AI 额度进度条 + "如何获取 API Key"引导链接<br>5. "快捷键"标签：录制快捷键自定义输入框 + 冲突检测提示<br>6. "关于"标签：版本号、GitHub 链接、反馈邮箱、许可信息<br>7. 深色模式切换：`dark:` 变体适配（按 design-system.md 2.4），切换存储在 tauri-plugin-store，跟随系统主题 |
| **涉及文件** | `src/components/settings/SettingsPanel.tsx`, `src/components/settings/GeneralTab.tsx`, `src/components/settings/AiModelTab.tsx`, `src/components/settings/ShortcutTab.tsx`, `src/components/settings/AboutTab.tsx`, `src/hooks/useSettings.ts` |
| **验收条件** | 1. 设置页面 4 个标签页切换正常<br>2. AI 模型标签按三层优先级展示：智谱置顶已启用 → 国产「即将上线」→ 国外垫底<br>3. AI 额度进度条显示正确（468/500）<br>4. 快捷键修改后可保存并立即生效<br>5. 数据目录浏览对话框可正常选择文件夹<br>6. 深色模式切换：系统主题变化 → 应用跟随<br>7. 设置项变更后重启应用仍保持 |
| **预估时间** | 3h |

---

### T4.2：API Key 管理

| 属性 | 内容 |
|------|------|
| **编号** | T4.2 |
| **标题** | API Key 管理（Tauri keyring 读写、模型切换 UI 三层结构） |
| **前置任务** | T4.1 |
| **描述** | 实现 API Key 的加密存储和管理。主要工作：<br>1. 实现 Rust 端 `set_api_key(provider, key)` command：写入 Tauri keyring（Windows Credential Manager）<br>2. 实现 Rust 端 `get_api_key(provider)` command：从 keyring 读取（仅 Rust 端内部调用，前端不获取明文）<br>3. 实现 Rust 端 `test_api_connection(provider, key)` command：发送一个最小请求验证 Key 有效性<br>4. 前端实现 API Key 输入 UI：密码框 + 显示/隐藏切换 + "测试连接"按钮 + "保存"按钮<br>5. 测试连接反馈：成功 → 绿色 Toast "API Key 验证成功"；失败 → 红色 Toast "API Key 无效或已过期"<br>6. 模型切换逻辑：选中自定义模型 → 要求先填入 API Key → 测试连接 → 保存<br>7. MVP 阶段自定义模型入口显示「即将上线」文案 + 邮箱订阅引导 |
| **涉及文件** | `src-tauri/src/ai/key_manager.rs`, `src/components/settings/AiModelTab.tsx`, `src/components/settings/ApiKeyInput.tsx` |
| **验收条件** | 1. API Key 填入后点击"测试连接"→ 返回验证结果<br>2. API Key 保存后存储在 Windows Credential Manager 中<br>3. 前端 state 中不出现 API Key 明文<br>4. 无效 Key → 红色 Toast 错误提示<br>5. 有效 Key → 绿色 Toast 成功提示<br>6. 自定义模型入口标注「V1.0 即将上线」 |
| **预估时间** | 2h |

---

### T4.3：分享链接生成

| 属性 | 内容 |
|------|------|
| **编号** | T4.3 |
| **标题** | 分享链接生成（本地 HTTP 服务器 + 加密链接 + 有效期） |
| **前置任务** | T3.7 |
| **描述** | 按 spec.md F05 实现分享链接生成的 MVP 版本（本地方案）。主要工作：<br>1. 实现 Rust 端本地 HTTP 服务器（基于 `actix-web` 或 `tiny_http`），监听随机端口<br>2. 生成加密分享链接：`http://localhost:{port}/share/{uuid}`，UUID 映射到项目数据<br>3. 分享页面渲染：HTML 模板，展示步骤列表 + 截图（Base64 内嵌）<br>4. 有效期控制：链接生成后 24h 有效，过期返回 410 Gone<br>5. 复制链接按钮 → 写入剪贴板<br>6. 停止分享按钮 → 关闭 HTTP 服务器<br>7. ⚠️ 本地分享仅在局域网内可访问，外网需用户自行端口映射 |
| **涉及文件** | `src-tauri/src/share/mod.rs`, `src-tauri/src/share/http_server.rs`, `src-tauri/src/share/share_page.rs`, `src/components/share/ShareDialog.tsx`, `src-tauri/Cargo.toml`（tiny_http / actix-web 依赖） |
| **验收条件** | 1. 点击"分享"→ 生成 `http://localhost:{port}/share/{uuid}` 链接<br>2. 浏览器打开链接 → 显示步骤文档（含截图）<br>3. 24h 后链接返回 410 或提示"分享已过期"<br>4. 点击"停止分享"→ 服务器关闭 → 链接不可访问<br>5. 复制链接按钮可正常复制到剪贴板<br>6. `cargo check` 通过 |
| **预估时间** | 3h |

---

### T4.4：空状态和错误状态 UI

| 属性 | 内容 |
|------|------|
| **编号** | T4.4 |
| **标题** | 空状态和错误状态 UI（按 design-system.md 第 7 章） |
| **前置任务** | T1.5 |
| **描述** | 实现 design-system.md 第 7 章定义的所有空状态和错误状态。主要工作：<br>1. 空状态 1 — 无录制项目：引导页面，居中图标 + 文案 + "开始录制"按钮（7.5.1）<br>2. 空状态 2 — 录制完成未生成 AI 描述：提示 + "AI 生成步骤说明"按钮（7.5.2）<br>3. 空状态 3 — 侧边栏无项目：提示"还没有录制项目，按 Ctrl+Alt+R 开始"<br>4. 错误状态 1 — API 调用失败：对话框（7.6.1），含错误描述 + 重试按钮 + 手动编辑按钮<br>5. 错误状态 2 — 导出失败：Toast 提示（7.6.2），含原因说明<br>6. 错误状态 3 — API Key 无效：对话框（7.6.3），含原因列表 + 操作按钮<br>7. **全局 Toast 系统**：实现 ToastContext + ToastContainer，支持 4 种变体（info/success/warning/error），自动消失 3s<br>8. 所有文案使用中文，友好且可操作 |
| **涉及文件** | `src/components/common/EmptyState.tsx`, `src/components/common/ErrorDialog.tsx`, `src/components/common/Toast.tsx`, `src/context/ToastContext.tsx`, `src/components/ai/AiErrorState.tsx` |
| **验收条件** | 1. 首次启动 → 显示无录制项目空状态<br>2. 录制完成未生成 AI → 显示对应空状态<br>3. 模拟网络错误 → 显示 API 调用失败对话框<br>4. 导出失败（磁盘满）→ Toast 显示错误原因<br>5. 输入无效 API Key → 显示 API Key 验证失败对话框<br>6. Toast 3s 后自动消失<br>7. 所有错误文案为中文且不含技术术语（如 HTTP 500 Internal Server Error） |
| **预估时间** | 2h |

---

### T4.5：键盘快捷键全局绑定

| 属性 | 内容 |
|------|------|
| **编号** | T4.5 |
| **标题** | 键盘快捷键全局绑定 |
| **前置任务** | T3.2, T4.1 |
| **描述** | 按 design-system.md 7.2 快捷键表，实现所有全局和应用级快捷键。主要工作：<br>1. 注册 `tauri-plugin-global-shortcut`：Ctrl+Alt+R 录制控制（全局，即使录步窗口不在焦点）<br>2. 前端 `useEffect` 监听键盘事件：Ctrl+Z 撤销、Ctrl+Y/Ctrl+Shift+Z 重做、Ctrl+S 保存、Ctrl+E 导出面板、Delete 删除步骤、Ctrl+D 复制步骤、Ctrl+, 打开设置<br>3. 实现快捷键冲突检测：用户自定义快捷键时检查是否与系统快捷键冲突<br>4. 实现快捷键提示：工具栏按钮 tooltip 显示对应快捷键（如"撤销 (Ctrl+Z)"）<br>5. 快捷键配置从 tauri-plugin-store 读取，修改后即时生效 |
| **涉及文件** | `src/hooks/useKeyboardShortcuts.ts`, `src-tauri/src/lib.rs`, `src/components/layout/Toolbar.tsx` |
| **验收条件** | 1. 任意应用前台时 Ctrl+Alt+R 触发录制（全局）<br>2. 编辑器中 Ctrl+Z/Y 撤销/重做正常<br>3. Ctrl+S → SQLite 保存<br>4. Ctrl+E → 导出面板弹出<br>5. Delete → 删除确认弹窗<br>6. Ctrl+, → 打开设置<br>7. 自定义快捷键后旧快捷键失效，新快捷键生效<br>8. ESC 关闭所有面板/对话框 |
| **预估时间** | 2h |

---

### T4.6：二级联合测试

| 属性 | 内容 |
|------|------|
| **编号** | T4.6 |
| **标题** | 二级联合测试（状态冲突/路由冲突/依赖冲突/UI 冲突） |
| **前置任务** | T1.7, T2.7, T3.7, T4.1—T4.5 |
| **描述** | 按 spec.md 第 6 章验收标准，执行联合测试（邀请 3-5 名内部用户）。主要工作：<br>1. **状态冲突测试**：录制中切换设置 → 录制中打开导出 → 录制中关闭窗口 → AI 生成中切换项目 → AI 生成中开始新录制<br>2. **边界测试**：录制 0 步 → 录制 200 步（上限）→ 录制 201 步（超限提示）→ 单步超长输入（2000 字）→ 截图超大分辨率（4K 显示器）<br>3. **异常场景**：录制中突然断网 → AI 生成中断网 → 导出时磁盘满 → 数据库文件损坏 → 快捷键被其他应用占用<br>4. **多窗口**：尝试打开两个录步实例 → 检测并阻止或提示<br>5. **编译产物验证**：`.msi` 安装在干净 Windows 10/11 虚拟机，验证注册表/文件路径/卸载残留<br>6. 整理联合测试报告（含截图和复现步骤） |
| **涉及文件** | `docs/test-report-week4-joint.md`（测试报告，写入 `D:\Projects\flowio\docs\`） |
| **验收条件** | 1. 状态冲突：AI 生成中切换项目 → 提示"正在生成中，切换将丢失当前进度"<br>2. 边界：录制 201 步 → 提示"已达到最大步数限制（200 步），录制已自动停止"<br>3. 异常：导储磁盘满 → Toast 提示"无法写入文件，请检查磁盘空间"<br>4. 多窗口：第二个实例启动 → 提示"录步已在运行中"<br>5. 卸载：无残留文件/注册表项<br>6. 联合测试报告完成 |
| **预估时间** | 3h |

---

### T4.7：三级用户视角测试

| 属性 | 内容 |
|------|------|
| **编号** | T4.7 |
| **标题** | 三级用户视角测试（首次使用 / 核心功能 / 出错场景 / 非理想场景） |
| **前置任务** | T4.6 |
| **描述** | 模拟 spec.md 三个用户画像的真实使用场景，执行用户视角测试。主要工作：<br>1. **画像 1：培训主管小李** — 录制 Word 文档操作 30 步 → AI 生成 → 编辑 5 步 → 导出 PDF → 检查 PDF 排版和中文质量<br>2. **画像 2：IT 运维老王** — 录制 CMD 命令操作 15 步 → 验证密码字段自动过滤 → 导出 MD → 检查格式对接知识库<br>3. **画像 3：产品经理小张** — 录制浏览器操作 20 步 → 合并相邻步骤 → 导出 HTML → 嵌入产品文档<br>4. **非理想场景**：快速连击 → 拖拽操作 → 多显示器切换 → 中文输入法（搜狗/微软拼音）→ 屏幕缩放 125%/150%<br>5. **AI 质量评估**：对比 Scribe 相同操作流程的输出，评估中文描述的准确性、完整性和可读性<br>6. 整理用户视角测试报告 |
| **涉及文件** | `docs/test-report-week4-user.md`（测试报告，写入 `D:\Projects\flowio\docs\`） |
| **验收条件** | 1. 三个用户画像场景全流程走通<br>2. 密码字段在所有场景下正确过滤<br>3. 中文描述准确率 ≥ 90%（与 Scribe 对比）<br>4. 中文输入法下文本捕获正确<br>5. 屏幕缩放 125%/150% 下截图清晰<br>6. 用户视角测试报告完成 |
| **预估时间** | 3h |

---

### T4.8：代码审计 + 语义漂移检测 + 全量编译验证

| 属性 | 内容 |
|------|------|
| **编号** | T4.8 |
| **标题** | 代码审计 + 语义漂移检测 + 全量编译验证 |
| **前置任务** | T4.7 |
| **描述** | 收尾阶段的质量把关。主要工作：<br>1. **代码审计**：按 AGENTS.md 逐条检查——strict mode 无 `any`、组件 ≤ 300 行、Rust 无 `unwrap()`（测试除外）、API Key 无硬编码<br>2. **语义漂移检测**：对照 spec.md 45 项功能验收条件，逐项检查代码实现是否与 spec 一致<br>3. **中文审查**：逐页检查所有 UI 文本、错误提示、tooltip、按钮文案，确保无英文残留<br>4. **依赖审计**：`pnpm audit` + `cargo audit`，检查已知漏洞<br>5. **全量编译验证**：`pnpm build && cargo build --release` 确保 0 error，记录 warning 清单<br>6. **安装包构建**：`npm run tauri build` 生成 Windows .msi，验证 ≤ 15MB<br>7. **二进制体积分析**：用 `cargo bloat` 分析 Rust 二进制大小，优化大依赖 |
| **涉及文件** | 全部项目文件 |
| **验收条件** | 1. `tsc --noEmit` 0 error<br>2. `eslint` 0 error 0 warning<br>3. `pnpm build` 0 error<br>4. `cargo build --release` 0 error<br>5. `cargo clippy` 0 warning（关键项）<br>6. `.msi` 安装包 ≤ 15MB<br>7. 中文审查：0 英文 UI 文本残留<br>8. 语义漂移检测：spec 的 45 项全部对应到代码实现 |
| **预估时间** | 2h |

---

### T4.9：更新 CONTEXT.md + CHANGELOG.md

| 属性 | 内容 |
|------|------|
| **编号** | T4.9 |
| **标题** | 更新 CONTEXT.md + CHANGELOG.md |
| **前置任务** | T4.8 |
| **描述** | 更新项目元文档，标记阶段完成。主要工作：<br>1. **CONTEXT.md**：<br>   - 当前阶段改为"阶段 2：开发阶段，状态：已完成"<br>   - 已完成列表追加 Week 1-4 全部任务<br>   - 当前任务改为"编写 README.md + 发布 MVP v0.1.0"<br>   - 下一步改为"阶段 3：V1.0 功能开发（浏览器扩展 + macOS 支持）"<br>2. **CHANGELOG.md**：<br>   - 版本 v0.1.0（MVP）<br>   - 按功能分类（新增 / 修复 / 优化 / 文档）<br>   - 新增：录制引擎、AI 生成、步骤编辑、PDF/HTML/MD/图片导出、分享链接、设置面板<br>   - 修复：列出 Week 4 测试阶段修复的问题<br>   - 已知问题：列出待 V1.0 修复的已知局限<br>3. **README.md**（如果尚未创建则新建）：<br>   - 安装说明（Windows .msi 下载 + 安装步骤）<br>   - 使用说明（Ctrl+Alt+R 录制 → AI 生成 → 编辑 → 导出）<br>   - 开发说明（技术栈、环境要求、构建命令）<br>   - 部署说明（GitHub Releases 发布流程） |
| **涉及文件** | `D:\Projects\flowio\CONTEXT.md`, `D:\Projects\flowio\CHANGELOG.md`, `D:\Projects\flowio\README.md` |
| **验收条件** | 1. CONTEXT.md 阶段信息更新正确<br>2. CHANGELOG.md 格式规范，版本号 v0.1.0<br>3. README.md 包含安装/使用/开发/部署四个章节<br>4. 三个文件均为中文撰写 |
| **预估时间** | 1h |

---

## 任务依赖关系图

```
Week 1（录制引擎 + AI 管道）:
T1.1 ──┬── T1.2 ──┬── T1.6 ── T1.7
       │          │
       ├── T1.3 ──┤
       │          │
       ├── T1.4 ──┤
       │          │
       └── T1.5 ──┘

T1.7 ── T2.1 ── T2.2 ── T2.3 ──┬── T2.6 ── T2.7
              │                 │
              └── T2.5 ─────────┘
T1.3 ── T2.4 ──────────────────────┘

Week 2（编辑器 + 导出 + 设置 + 分享 + 收尾）:
T1.7 ── T3.1 ── T3.2 ────────────── T3.7
T1.7 ── T3.3 ──────────────────────┘
T1.7 ── T3.4 ──────────────────────┘
T1.7 ── T3.5 ──────────────────────┘
T1.7 ── T3.6 ──────────────────────┘

T1.5 ── T4.1 ── T4.2
T3.7 ── T4.3
T1.5 ── T4.4
T3.2 + T4.1 ── T4.5
T1.7 + T2.7 + T3.7 + T4.1~T4.5 ── T4.6 ── T4.7 ── T4.8 ── T4.9
```

---

> **文档版本**：v2.0（2 周冲刺版）
> *任务内容、依赖关系、验收条件均保持不变，仅压缩时间规划。*
> *下一步：基于本 tasks.md 开始 Week 1 开发。*
> *（内容由 AI 生成，仅供参考）*
