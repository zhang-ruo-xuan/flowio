---
AIGC:
    Label: "1"
    ContentProducer: 001191440300708461136T1XGW3
    ProduceID: f6e22914e720ea967e259477f775ed68_8f07586f882d11f18108525400287e28
    ReservedCode1: fH6NTjhnZD2/VbPPjZArz/i67O6heuBToUjuJE8X3ZkgyvGmaOvc7G7RFU07sdrVeF0tpvwjuYFvwUYZTnspwens7QflO1ahTUuUKJyUwZ7uwOaZ1RVwvT0cgLcMP6ukR+vDayARHzFHffGscRQ/x40jy8ZAP2i58yh59xKgGEXHRPQ9iLjoKmICX0U=
    ContentPropagator: 001191440300708461136T1XGW3
    PropagateID: f6e22914e720ea967e259477f775ed68_8f07586f882d11f18108525400287e28
    ReservedCode2: fH6NTjhnZD2/VbPPjZArz/i67O6heuBToUjuJE8X3ZkgyvGmaOvc7G7RFU07sdrVeF0tpvwjuYFvwUYZTnspwens7QflO1ahTUuUKJyUwZ7uwOaZ1RVwvT0cgLcMP6ukR+vDayARHzFHffGscRQ/x40jy8ZAP2i58yh59xKgGEXHRPQ9iLjoKmICX0U=
---

# 录步 (Flowio) 设计系统

> 文档版本：v1.0
> 创建日期：2026-07-25
> 所属阶段：阶段 1 — 产品经理阶段
> 依赖文档：AGENTS.md / spec.md / plan.md / ADR-001 / ADR-002

---

## 第 1 章：设计理念

### 1.1 设计原则

| # | 原则 | 说明 |
|---|------|------|
| 1 | **简洁克制** | 每个界面只做一件事，减少信息密度。不堆砌功能入口，不滥用图标和装饰 |
| 2 | **聚焦内容** | 步骤文档是核心产物，编辑区和预览区占据最大视觉权重，工具栏收拢到最小必要 |
| 3 | **中文原生** | 所有文案中文撰写，排版遵循中文习惯（全角标点、适当行距、两端对齐） |
| 4 | **专业但不冰冷** | 配色沉稳但不压抑，交互有反馈但不轻浮，面向企业用户但保留亲和力 |
| 5 | **操作可预期** | 每个交互有即时反馈，破坏性操作有确认，AI 生成过程有明确进度 |

### 1.2 视觉关键词

```
现代 ─── 专业 ─── 轻量 ─── 国产
  │        │        │        │
  └────────┴────────┴────────┘
              ↓
    飞书式克制 × Notion 式结构化 × 钉钉式务实
```

### 1.3 与 StepSnap 的差异化对比

| 维度 | StepSnap | 录步 Flowio |
|------|----------|-------------|
| **配色** | 深色主题为主，紫色/青色强调 | 浅色主题为主（浅灰蓝），中性克制 |
| **布局** | 单栏垂直堆叠，步骤列表居中窄宽 | 三栏：侧边导航 + 项目列表 + 编辑区 |
| **字体** | 英文优先 (Inter)，无中文优化 | 中文优先（微软雅黑），英文 Inter 为辅 |
| **组件风格** | 圆角大、阴影重、Material Design 感 | 小圆角、轻阴影、扁平化，偏飞书风格 |
| **图标** | 自定义 SVG，风格不统一 | lucide-react 统一线性图标 |
| **交互模式** | 按钮分散，操作路径不清晰 | 工具栏集中，F 型扫描路径（左上→右下） |
| **中文体验** | 完全无中文 | 全中文界面 + AI 中文步骤生成 |
| **视觉密度** | 稀疏、留白过多 | 紧凑合理，工具型应用的信息密度 |

---

## 第 2 章：色彩系统

### 2.1 主色调 — 石板蓝

专业工具不走鲜艳路线，选用偏灰的蓝色调，冷静可靠。

| Token | 色值 | 用途 | WCAG |
|-------|------|------|------|
| `primary-50` | `#EFF4FA` | 主色浅底背景 | — |
| `primary-100` | `#D6E4F3` | 选中态背景、标签底色 | — |
| `primary-200` | `#AEC9E7` | 悬停态边框 | — |
| `primary-300` | `#7DA8D6` | 禁用态填充 | — |
| `primary-400` | `#4F88C4` | 次要图标 | — |
| `primary-500` | **`#3B73B0`** | **主按钮背景、链接色、品牌色** | AA (白字) |
| `primary-600` | `#2E5D8E` | 主按钮悬停 | AA |
| `primary-700` | `#22476D` | 主按钮按下 | AAA |
| `primary-800` | `#17324D` | 深色强调文字 | — |
| `primary-900` | `#0D1E30` | 最深背景 | — |

### 2.2 辅助色

| Token | 色值 | 用途 | WCAG (白字) |
|-------|------|------|-------------|
| `success-500` | `#2DA44E` | 成功状态、导出完成 | AA |
| `success-100` | `#DCF5E4` | 成功背景 | — |
| `warning-500` | `#BF8700` | 警告提示、额度不足 | AA |
| `warning-100` | `#FFF2D6` | 警告背景 | — |
| `error-500` | `#D1242F` | 错误状态、删除按钮 | AA |
| `error-100` | `#FFDBDD` | 错误背景 | — |
| `info-500` | `#3B73B0` | 信息提示（复用 primary-500） | AA |
| `info-100` | `#EFF4FA` | 信息背景 | — |

### 2.3 中性色阶

| Token | 色值 | 用途 |
|-------|------|------|
| `white` | `#FFFFFF` | 页面背景、卡片背景 |
| `gray-50` | `#F6F7F9` | 次级背景、表头、侧边栏底色 |
| `gray-100` | `#EBEDF1` | 分割线、禁用背景 |
| `gray-200` | `#D7DBE2` | 边框默认态 |
| `gray-300` | `#B7BDC8` | 占位文字 |
| `gray-400` | `#8D95A3` | 辅助文字 |
| `gray-500` | `#656D7A` | 次要文字 |
| `gray-600` | `#444B55` | 正文文字 |
| `gray-700` | `#2E333B` | 标题文字、强调 |
| `gray-800` | `#1A1D23` | 深色标题 |
| `gray-900` | `#0D0F12` | 最深文字（慎用） |

### 2.4 深色模式配色方案

| Token | 浅色 | 深色 | 用途 |
|-------|------|------|------|
| 页面背景 | `white` (#FFF) | `gray-900` (#0D0F12) | 主窗口背景 |
| 卡片/面板 | `white` | `gray-800` (#1A1D23) | 编辑区、设置面板 |
| 侧边栏 | `gray-50` | `gray-800` | 导航背景 |
| 正文文字 | `gray-700` | `gray-100` | 主要内容 |
| 辅助文字 | `gray-500` | `gray-400` | 说明、时间戳 |
| 边框 | `gray-200` | `gray-700` | 分割线、卡片边框 |
| 主按钮 | `primary-500` | `primary-400` | 对比度适配 |
| 主色浅底 | `primary-50` | `primary-900` | 选中态 |

> 深色模式通过 Tailwind `dark:` 变体实现，切换由系统主题检测自动适配。

---

## 第 3 章：字体系统

### 3.1 字体族

| 用途 | 字体栈 | 说明 |
|------|--------|------|
| 中文正文 | `"Microsoft YaHei", "PingFang SC", "Noto Sans SC", system-ui, sans-serif` | Windows 优先微软雅黑，macOS 回落 PingFang |
| 英文/数字 | `"Inter", "Microsoft YaHei", system-ui, sans-serif` | Inter 用于英文和数字，现代几何风格 |
| 代码/等宽 | `"JetBrains Mono", "Cascadia Code", "Fira Code", monospace` | 键名、快捷键、JSON 数据 |

Tailwind 配置：
```js
// tailwind.config.js
fontFamily: {
  sans: ['"Microsoft YaHei"', '"PingFang SC"', '"Noto Sans SC"', 'system-ui', 'sans-serif'],
  mono: ['"JetBrains Mono"', '"Cascadia Code"', 'monospace'],
}
```

### 3.2 字号层级

| Token | 字号 | 行高 | 字重 | 使用场景 |
|-------|------|------|------|----------|
| `text-h1` | 28px | 1.3 (36px) | 700 | 页面大标题（录制项目名） |
| `text-h2` | 22px | 1.3 (29px) | 600 | 区域标题（设置页分组标题） |
| `text-h3` | 18px | 1.4 (25px) | 600 | 卡片标题（步骤标题） |
| `text-h4` | 16px | 1.4 (22px) | 600 | 小组件标题 |
| `text-body` | 15px | 1.6 (24px) | 400 | 正文、步骤描述 |
| `text-body-sm` | 14px | 1.5 (21px) | 400 | 次要正文、列表项 |
| `text-caption` | 13px | 1.4 (18px) | 400 | 辅助说明、时间戳、元数据 |
| `text-xs` | 12px | 1.3 (16px) | 400 | 标签、badge、快捷键提示 |
| `text-code` | 13px | 1.5 (20px) | 400 | 代码块、JSON 展示 |

Tailwind 映射：
```
text-2xl  → h1 (28px)
text-xl   → h2 (22px)
text-lg   → h3 (18px)
text-base → h4 / body (16px / 15px)
text-sm   → body-sm / caption (14px / 13px)
text-xs   → xs (12px)
```

### 3.3 排版规范

| 属性 | 值 | 说明 |
|------|-----|------|
| 正文行高 | 1.6 | 中文段落舒适阅读 |
| 标题行高 | 1.3 | 紧凑有力 |
| 字间距 | `normal` (0) | 中文不需要额外 letter-spacing |
| 英文/数字字间距 | `-0.01em` | Inter 微调更紧凑 |
| 段落间距 | `mb-4` (16px) | 段落间呼吸感 |
| 列表项间距 | `space-y-2` (8px) | 步骤列表中各项间距 |

---

## 第 4 章：间距与布局

### 4.1 基础网格：8px 系统

所有间距、尺寸均为 8 的倍数。

```tailwindcss
spacing: {
  0:    '0px',
  0.5:  '4px',    // xs  - 紧密间距、图标与文字
  1:    '8px',    // sm  - 列表项内间距、小组件 padding
  2:    '16px',   // md  - 卡片 padding、表单间距
  3:    '24px',   // lg  - 区域间距、侧边栏宽度内的 padding
  4:    '32px',   // xl  - 大区块间距
  6:    '48px',   // 2xl - 页面级间距
  8:    '64px',   // 3xl - 超大间距（极少使用）
}
```

### 4.2 间距层级速查

| Token | Tailwind | px | 使用场景 |
|-------|----------|-----|----------|
| xs | `gap-1` / `p-1` / `m-1` | 8px | 图标与标签间距、按钮内图标间距 |
| sm | `gap-2` / `p-2` / `m-2` | 16px | 卡片内边距、表单项间距、列表项 padding |
| md | `gap-3` / `p-3` / `m-3` | 24px | 面板 padding、工具栏与内容区 |
| lg | `gap-4` / `p-4` / `m-4` | 32px | 区域分隔、对话框 padding |
| xl | `gap-6` / `p-6` / `m-6` | 48px | 页面主内容区与侧边栏间距 |
| 2xl | `gap-8` / `p-8` / `m-8` | 64px | 首页引导、空状态大间距 |

### 4.3 布局模式

```
┌──────────────────────────────────────────────────────┐
│  顶部标题栏 (Titlebar)         最小化 □ 最大化 □ 关闭 × │ 高度: 40px
├────────┬─────────────────────────────────────────────┤
│        │                                             │
│ 侧边栏  │              主内容区                        │
│        │                                             │
│  220px │              flex-1                         │
│        │                                             │
│ 项目列表 │   ┌─────────────────────────────────────┐  │
│ · 录制1  │   │  工具栏 (Toolbar)                   │  │ 高度: 48px
│ · 录制2  │   ├─────────────────────────────────────┤  │
│ · 录制3  │   │                                     │  │
│        │   │  编辑区 / 预览区                       │  │
│        │   │  (StepEditor / PreviewPanel)          │  │
│        │   │                                     │  │
│        │   │                                     │  │
│        │   └─────────────────────────────────────┘  │
│        │                                             │
├────────┴─────────────────────────────────────────────┤
│  状态栏 (StatusBar)                         AI 额度: 468/500 │ 高度: 28px
└──────────────────────────────────────────────────────┘
```

**关键尺寸**：
| 区域 | 宽度/高度 | 说明 |
|------|----------|------|
| 标题栏 | 全宽 × 40px | Tauri 自绘，标题居中 |
| 侧边栏 | 220px（可拖拽调至 180-320px） | 项目列表 + 新建按钮 + 搜索 |
| 主内容区 | 剩余宽度 | flex-1，最小 480px |
| 工具栏 | 全宽 × 48px | 按钮组 + 面包屑 |
| 状态栏 | 全宽 × 28px | 录制状态 / AI 额度 / 快捷键提示 |

### 4.4 响应式断点

| 断点 | 最小宽度 | 适配策略 |
|------|----------|----------|
| `sm` | 640px | 移动端竖屏（暂不优化） |
| `md` | 768px | 小窗口：折叠侧边栏为抽屉 |
| `lg` | 1024px | 标准：侧边栏 + 内容区 |
| `xl` | 1280px | 宽屏：侧边栏 + 编辑区 + 预览面板 |
| `2xl` | 1536px | 超宽：三栏布局（列表 + 编辑 + 实时预览） |

> MVP 以 `lg` (≥1024px) 为主要目标。小窗口下侧边栏折叠为汉堡菜单抽屉。

---

## 第 5 章：组件规范

### 5.1 按钮 (Button)

#### 主按钮 Primary

```tailwindcss
/* 默认 */
bg-primary-500 text-white rounded-lg px-4 py-2 text-sm font-medium
hover:bg-primary-600 focus:ring-2 focus:ring-primary-300
disabled:bg-gray-200 disabled:text-gray-400

/* 尺寸变体 */
px-3 py-1.5 text-xs   → sm (工具栏紧凑按钮)
px-4 py-2 text-sm     → md (默认)
px-6 py-3 text-base   → lg (页面主操作按钮)
```

| 状态 | 背景 | 文字 | 边框 |
|------|------|------|------|
| 默认 | `primary-500` | white | 无 |
| 悬停 | `primary-600` | white | 无 |
| 按下 | `primary-700` | white | 无 |
| 禁用 | `gray-200` | `gray-400` | 无 |
| 加载中 | `primary-400` | white + spinner | 无 |

#### 次按钮 Secondary

```tailwindcss
bg-white text-gray-700 border border-gray-200 rounded-lg px-4 py-2 text-sm font-medium
hover:bg-gray-50 hover:border-gray-300
disabled:bg-gray-100 disabled:text-gray-400
```

#### 文字按钮 Text

```tailwindcss
text-primary-500 text-sm font-medium px-2 py-1
hover:text-primary-600 hover:bg-primary-50 rounded
```

#### 危险按钮 Danger

```tailwindcss
bg-error-500 text-white rounded-lg px-4 py-2 text-sm font-medium
hover:bg-red-600
// 文字型危险按钮
text-error-500 text-sm font-medium hover:text-red-600 hover:bg-error-100 rounded px-2 py-1
```

### 5.2 输入框 (Input)

#### 文本框

```tailwindcss
/* 默认 */
w-full px-3 py-2 text-sm border border-gray-200 rounded-lg bg-white
placeholder:text-gray-300 text-gray-700
focus:outline-none focus:border-primary-400 focus:ring-2 focus:ring-primary-100
/* 禁用 */
disabled:bg-gray-50 disabled:text-gray-400 disabled:cursor-not-allowed
/* 错误 */
border-error-500 focus:border-error-500 focus:ring-error-100
/* 成功 */
border-success-500 focus:border-success-500 focus:ring-success-100
```

#### 搜索框

```tailwindcss
w-full pl-9 pr-3 py-2 text-sm border border-gray-200 rounded-lg bg-gray-50
placeholder:text-gray-300 focus:bg-white focus:border-primary-400
// 搜索图标放在左侧 pl-9 区域
```

#### 下拉选择 (Select)

```tailwindcss
w-full px-3 py-2 text-sm border border-gray-200 rounded-lg bg-white appearance-none
bg-[url('data:image/svg+xml;...')] bg-no-repeat bg-[right_8px_center] bg-[length:16px]
// 下拉选项面板: bg-white border border-gray-200 rounded-lg shadow-lg p-1
// 选中项: bg-primary-50 text-primary-700
// 悬停项: bg-gray-50
```

#### 标签 (Badge)

```tailwindcss
// 默认
inline-flex items-center px-2 py-0.5 text-xs font-medium rounded-full bg-gray-100 text-gray-600
// 变体
bg-primary-100 text-primary-700   → 步骤序号
bg-success-100 text-success-700   → 导出成功
bg-warning-100 text-warning-700   → 额度不足
bg-error-100 text-error-700       → API 错误
```

### 5.3 卡片 (Card)

```tailwindcss
/* 基础卡片 */
bg-white border border-gray-100 rounded-xl p-4 shadow-sm
/* 悬停可交互卡片 */
hover:border-gray-200 hover:shadow-md transition-all duration-200
/* 选中态 */
border-primary-400 bg-primary-50/50 ring-1 ring-primary-200
/* 步骤卡片 (StepCard) */
bg-white border border-gray-100 rounded-xl p-4 shadow-sm
hover:border-gray-200
/* 拖拽中 */
shadow-lg border-primary-400 ring-1 ring-primary-200 rotate-1 scale-[1.02]
```

### 5.4 对话框 (Dialog / Modal)

```tailwindcss
/* 遮罩层 */
fixed inset-0 bg-black/40 backdrop-blur-sm z-50
/* 对话框容器 */
bg-white rounded-2xl shadow-xl max-w-md w-full mx-4
/* 标题栏 */
flex items-center justify-between px-6 py-4 border-b border-gray-100
/* 内容区 */
px-6 py-4 text-sm text-gray-600
/* 操作栏 */
flex justify-end gap-2 px-6 py-4 border-t border-gray-100
```

使用场景：删除确认、导出设置、设置 API Key、首次引导。

### 5.5 提示 (Toast)

```tailwindcss
/* 容器 */
fixed bottom-6 right-6 z-[100] flex flex-col gap-2
/* 单条 Toast */
flex items-center gap-2 px-4 py-3 rounded-xl shadow-lg text-sm
animate-[slideUp_0.3s_ease-out]
/* 变体 */
bg-gray-800 text-white          → 默认信息
bg-success-500 text-white       → 成功（导出完成）
bg-error-500 text-white         → 错误（API 调用失败）
bg-warning-500 text-white       → 警告（额度即将用尽）
/* 自动消失：3s 后 fadeOut + slideDown */
```

### 5.6 标签页 (Tabs)

```tailwindcss
/* 标签容器 */
flex border-b border-gray-200 gap-0
/* 标签项 默认 */
px-4 py-2.5 text-sm text-gray-500 border-b-2 border-transparent
hover:text-gray-700 hover:border-gray-300 cursor-pointer
/* 选中 */
text-primary-600 border-primary-500 font-medium
```

使用场景：设置页面分组（通用 / AI 模型 / 快捷键 / 关于）、导出格式选择。

### 5.7 步骤条 (Stepper) ⭐ 核心组件

作为录步最核心的组件，展示录制步骤列表。

```tailwindcss
/* 步骤列表 */
flex flex-col space-y-2
/* 步骤项 */
flex items-start gap-3 p-3 rounded-xl border border-transparent
hover:bg-gray-50 hover:border-gray-100 cursor-pointer
/* 当前选中 */
bg-primary-50 border-primary-200
/* 步骤序号圆圈 */
w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold
bg-primary-500 text-white flex-shrink-0
/* 步骤内容 */
flex flex-col min-w-0
/* 步骤标题 */
text-sm font-semibold text-gray-800 truncate
/* 步骤描述 */
text-xs text-gray-500 mt-0.5 line-clamp-2
/* 截图缩略图 */
w-16 h-12 rounded-lg border border-gray-200 object-cover flex-shrink-0
/* 拖拽手柄 */
text-gray-300 hover:text-gray-500 cursor-grab active:cursor-grabbing ml-auto
```

### 5.8 进度指示器 (Progress)

AI 生成时的进度反馈。

```tailwindcss
/* 线性进度条 */
w-full h-1 bg-gray-100 rounded-full overflow-hidden
/* 进度填充 */
h-full bg-primary-500 rounded-full transition-all duration-300 ease-out
/* 进度文字 */
text-xs text-gray-400 mt-1
// 内容: "正在生成步骤说明... 3/15"
/* 骨架屏（步骤卡片占位） */
animate-pulse bg-gray-100 rounded-xl h-20
```

---

## 第 6 章：布局规范

### 6.1 应用整体布局

```
┌────────────────────────────────────────────────────────────┐
│  🏠 录步 Flowio               —  □  ✕                     │
├──────────┬─────────────────────────────────────────────────┤
│          │  工具栏: [录制 ●] [导出 ⬇] [分享 ↗] [设置 ⚙]    │
│ 📁 全部   │─────────────────────────────────────────────────│
│ 📋 项目1  │                                                 │
│ 📋 项目2  │     编辑区 / 预览区                              │
│ 📋 项目3  │                                                 │
│          │                                                 │
│ [+ 新建] │                                                 │
│          │                                                 │
├──────────┴─────────────────────────────────────────────────┤
│  ● 未录制  │  AI 剩余: 468/500  │  Ctrl+R 开始录制          │
└────────────────────────────────────────────────────────────┘
```

### 6.2 录制页面

用户看到的第一屏——准备录制或正在录制的状态。

**未录制状态**：

```
┌──────────────────────────────────────┐
│                                      │
│         ◉                             │
│    准备开始录制                        │
│    按下 Ctrl+Alt+R 或点击下方按钮      │
│                                      │
│    [ 开始录制 ]                       │
│                                      │
│    提示：录制时请按正常速度操作，       │
│    每次点击会自动截图并记录步骤。        │
│                                      │
└──────────────────────────────────────┘
```

**录制中状态**：

```
┌──────────────────────────────────────┐
│  ● 录制中 · 已捕获 12 步               │
│                                      │
│  ┌─ 步骤卡片 1 ───────────────────┐  │
│  │ ① 点击「文件」菜单  [缩略图]    │  │
│  └────────────────────────────────┘  │
│  ┌─ 步骤卡片 2 ───────────────────┐  │
│  │ ② 选择「另存为」    [缩略图]    │  │
│  └────────────────────────────────┘  │
│  ┌─ 步骤卡片 3 ───────────────────┐  │
│  │ ③ 输入文件名        [缩略图]    │  │
│  └────────────────────────────────┘  │
│           ... 更多步骤 ...            │
│                                      │
│  [ ⏹ 停止录制 (Ctrl+Alt+R) ]         │
└──────────────────────────────────────┘
```

### 6.3 编辑页面

录制完成后进入。

```
┌──────────────────────────────────────────────────────────┐
│  工具栏: [↩ 撤销] [↪ 重做] │ [+] 插入步骤 │ [🤖 全部重新生成] │
│──────────────────────────────────────────────────────────│
│                                                          │
│  ┌──────────────────────┐  ┌──────────────────────────┐ │
│  │  步骤列表（可拖拽）    │  │  预览面板                  │ │
│  │                      │  │                          │ │
│  │  ≡ ① 打开文件菜单     │  │  # 步骤 1                 │ │
│  │  ≡ ② 选择另存为       │  │  点击屏幕左上角的         │ │
│  │  ≡ ③ 输入文件名       │  │  「文件」菜单...          │ │
│  │  ≡ ④ 点击保存按钮     │  │  [截图]                  │ │
│  │  ≡ ⑤ 确认保存成功     │  │                          │ │
│  │                      │  │  ---                     │ │
│  │                      │  │  # 步骤 2                 │ │
│  │                      │  │  在下拉菜单中选择...      │ │
│  │                      │  │  [截图]                  │ │
│  └──────────────────────┘  └──────────────────────────┘ │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

### 6.4 设置页面

```
┌──────────────────────────────────────────────────────────┐
│  设置                                   [✕ 关闭]          │
│──────────────────────────────────────────────────────────│
│  [ 通用 ] [ AI 模型 ] [ 快捷键 ] [ 关于 ]                  │
│──────────────────────────────────────────────────────────│
│                                                          │
│  AI 模型设置                                              │
│                                                          │
│  默认模型                                                 │
│  ┌──────────────────────────────────────────────────┐   │
│  │ ● 智谱 GLM-4-flash（推荐）          [已启用]       │   │
│  │   开箱即用，AI 费用已包含在订阅中                    │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
│  自定义模型（V1.0 即将上线）                               │
│  ┌──────────────────────────────────────────────────┐   │
│  │ ○ DeepSeek V4                       [即将上线]    │   │
│  │   需自行配置 API Key，费用自己承担                   │   │
│  ├──────────────────────────────────────────────────┤   │
│  │ ○ 通义千问                           [即将上线]    │   │
│  │   需自行配置 API Key，费用自己承担                   │   │
│  ├──────────────────────────────────────────────────┤   │
│  │ ○ OpenAI / Claude                   [即将上线]    │   │
│  │   需自行配置 API Key，费用自己承担                   │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
│  本月 AI 调用额度                                         │
│  ┌──────────────────────────────────────────────────┐   │
│  │ ████████████████░░░░░░░░░░░░░░░░░░░░  468/500    │   │
│  │             93% 已使用                             │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
│  如何获取 API Key？                                       │
│  · 智谱：https://open.bigmodel.cn → 开发者中心            │
│  · DeepSeek：https://platform.deepseek.com → API Keys    │
│  · 通义千问：https://dashscope.aliyun.com → API-KEY      │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

### 6.5 导出页面

```
┌──────────────────────────────────────────────────────────┐
│  导出文档                                                │
│──────────────────────────────────────────────────────────│
│                                                          │
│  选择导出格式：                                           │
│  ┌─────┐  ┌─────┐  ┌─────┐                              │
│  │ PDF │  │HTML │  │ MD  │                              │
│  └─────┘  └─────┘  └─────┘                              │
│                                                          │
│  导出选项：                                               │
│  [✓] 包含截图                                             │
│  [✓] 包含步骤标题                                         │
│  [✓] 包含详细描述                                         │
│  [ ] 包含页眉页脚（V1.0）                                 │
│                                                          │
│  文件名：录步文档-20260725-未命名录制                        │
│                                                          │
│  ┌──────────────────────────────────────────────────┐   │
│  │              导出预览（第一页）                    │   │
│  │                                                  │   │
│  │  [预览区域 —— 渲染前 3 步作为导出效果预览]          │   │
│  │                                                  │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
│                        [ 取消 ]  [ 导出 ]                  │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

---

## 第 7 章：交互规范

### 7.1 动画原则

| 原则 | 实践 |
|------|------|
| 快进慢出 | 出现用 `ease-out`（快速进入），消失用 `ease-in`（缓慢退出） |
| 持续时间 | 微交互 150ms，小过渡 200ms，中等过渡 300ms。不超过 300ms |
| 减少动效 | 不滥用弹跳、旋转、脉冲；拖拽排序不做入场动画 |
| 性能优先 | 动画属性仅限 `opacity` / `transform`，避免触发 layout/paint |

Tailwind 预设：
```tailwindcss
transition-all duration-150 ease-out   → 微交互（hover、focus）
transition-all duration-200 ease-out   → 小过渡（tooltip、dropdown）
transition-all duration-300 ease-out   → 中等过渡（modal、sidebar）
animate-spin                           → 加载 spinner
animate-pulse                          → 骨架屏
```

### 7.2 键盘快捷键

| 快捷键 | 作用域 | 功能 | 备注 |
|--------|--------|------|------|
| `Ctrl+Alt+R` | 全局 | 开始/停止录制 | 默认，可在设置中修改 |
| `Ctrl+Z` | 编辑器 | 撤销 | 最多 20 步 |
| `Ctrl+Y` | 编辑器 | 重做 | — |
| `Ctrl+Shift+Z` | 编辑器 | 重做（备选） | macOS 习惯 |
| `Ctrl+S` | 全局 | 保存当前项目 | 触发 SQLite 写入 |
| `Ctrl+E` | 编辑器 | 导出面板 | 打开导出对话框 |
| `Delete` | 编辑器 | 删除选中步骤 | 确认弹窗 |
| `Ctrl+D` | 编辑器 | 复制选中步骤 | 插入到当前步骤之后 |
| `↑/↓` | 编辑器 | 切换选中步骤 | 步骤列表焦点导航 |
| `Esc` | 全局 | 关闭面板/取消操作 | 关闭对话框、取消拖拽 |
| `Ctrl+,` | 全局 | 打开设置 | 通用习惯 |

### 7.3 拖拽反馈（步骤排序）

使用 `react-beautiful-dnd` 实现：

```
拖拽前（hover）:
  ┌─────────────────────────┐
  │ ≡ 步骤标题           ╎  │  边框变为 primary-200
  └─────────────────────────┘   背景变为 primary-50

拖拽中:
  ┌═════════════════════════┐
  ║ ≡ 步骤标题 (被拖拽)     ║  提升 z-index
  ╚═════════════════════════╝  shadow-lg + rotate-1 + scale-[1.02]

目标位置:
  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─  插入指示线（2px, primary-500）
  ┌─────────────────────────┐
  │   原来的步骤              │  向下/向上推开
  └─────────────────────────┘
```

### 7.4 加载状态

#### AI 生成中

```
┌──────────────────────────────────────────┐
│  🤖 正在生成步骤说明...                    │
│                                          │
│  ████████████░░░░░░░░░░░░░░░░  3/15      │
│                                          │
│  ┌──────────────────────────────────┐   │
│  │ ① 打开文件菜单          [缩略图]  │   │ ✅ 已完成
│  └──────────────────────────────────┘   │
│  ┌──────────────────────────────────┐   │
│  │ ② 选择另存为            [缩略图]  │   │ ✅ 已完成
│  └──────────────────────────────────┘   │
│  ┌──────────────────────────────────┐   │
│  │ ③ 输入文件名    ⠋ 生成中...      │   │ 🔄 进行中 (骨架屏)
│  └──────────────────────────────────┘   │
│  ┌──────────────────────────────────┐   │
│  │ ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░ │   │ ⏳ 等待中 (骨架屏)
│  └──────────────────────────────────┘   │
│                                          │
│  预计剩余时间：约 5 秒                      │
└──────────────────────────────────────────┘
```

#### 导出中

```
┌──────────────────────────────────────────┐
│                                          │
│         ⟳                                │
│     正在导出 PDF...                       │
│     正在渲染第 12/50 步                    │
│                                          │
│  ████████████░░░░░░░░░░░░░░░░░░  24%    │
│                                          │
└──────────────────────────────────────────┘
```

### 7.5 空状态

#### 无录制项目

```
┌──────────────────────────────────────────────────┐
│                                                  │
│                  📄                               │
│                                                  │
│              还没有录制项目                        │
│         按下 Ctrl+Alt+R 开始第一次录制              │
│                                                  │
│              [ 开始录制 ]                          │
│                                                  │
│  提示：录制时会自动捕获您的鼠标点击和键盘输入，      │
│  每次操作都会截图，结束后 AI 会自动生成步骤说明。     │
│                                                  │
└──────────────────────────────────────────────────┘
```

#### 录制完成但未生成 AI 描述

```
┌──────────────────────────────────────────────────┐
│                                                  │
│  已捕获 15 步操作                                  │
│                                                  │
│  点击下方按钮，让 AI 为您生成步骤说明                │
│                                                  │
│              [ 🤖 AI 生成步骤说明 ]                │
│                                                  │
└──────────────────────────────────────────────────┘
```

### 7.6 错误状态

#### API 调用失败

```
┌──────────────────────────────────────────────────┐
│                                                  │
│                  ⚠                                │
│                                                  │
│            AI 生成失败                             │
│     连接智谱 API 超时，请检查网络后重试              │
│                                                  │
│         [ 重试 ]    [ 手动编辑 ]                   │
│                                                  │
│  提示：                                         │
│  · 请确认网络连接正常                              │
│  · 如持续失败，可在设置中切换 AI 模型               │
│                                                  │
└──────────────────────────────────────────────────┘
```

#### 导出失败

Toast 提示：
```
┌─────────────────────────────────────┐
│  ✕  导出失败                         │
│     无法写入文件，请检查磁盘空间和权限  │
└─────────────────────────────────────┘
```

#### API Key 无效

```
┌──────────────────────────────────────────────────┐
│  API Key 验证失败                                  │
│  ─────────────────────────────────────────────── │
│  提供的「智谱 GLM」API Key 无效或已过期。            │
│                                                  │
│  请确认：                                         │
│  1. API Key 已正确复制（无多余空格）                │
│  2. API Key 未过期或被删除                        │
│  3. 账户余额充足                                  │
│                                                  │
│  [ 重新输入 ]    [ 获取 API Key 帮助 ]             │
└──────────────────────────────────────────────────┘
```

---

*文档完毕。下一步：基于本设计系统进入阶段 3 的 UI 开发。*
*（内容由AI生成，仅供参考）*
*（内容由AI生成，仅供参考）*
