// ============================================================
// flowio — Shared TypeScript type definitions
// ============================================================

/** Export format for generated documentation */
export type ExportFormat = 'html' | 'markdown' | 'pdf';

// ---- Recording ----

export interface Recording {
  id: string;
  title: string;
  app_name: string;
  target_app?: string;
  status: string;
  total_steps: number;
  duration_secs?: number;
  created_at: string;
  updated_at?: string;
}

export interface RecordingSave {
  title: string;
  app_name: string;
  status?: string;
}

// ---- Step ----

export interface Step {
  id: string;
  recording_id: string;
  order_index: number;
  step_number: number;
  action_type: string;
  title: string;
  description: string;
  tip: string;
  screenshot_path: string;
  annotated_path: string;
  before_screenshot_path: string;
  created_at: string;
  /** Click X coordinate (screen px) */
  x: number | null;
  /** Click Y coordinate (screen px) */
  y: number | null;
  /** Focused element rect width (screen px), Scribe-style crop anchor */
  crop_w: number | null;
  /** Focused element rect height (screen px), Scribe-style crop anchor */
  crop_h: number | null;
  /** Path to the post-action "after" screenshot */
  after_screenshot_path: string | null;
  /** Base64-encoded marked/cropped screenshot (with red circle etc.) */
  screenshot_base64: string | null;
  /** Base64-encoded clean before screenshot (full context, no annotations) */
  before_screenshot_base64: string | null;
  /** Base64-encoded after screenshot data */
  after_screenshot_base64: string | null;
  /**
   * 第二十轮：这一步挂的是图片还是视频。
   * 'image'（默认）= png / jpg / webp / **gif 动图**，走 `<img>`（动图浏览器自己会播）；
   * 'video' = mp4 / webm，走 `<video>`（用 asset 协议播本地文件，不进 base64 负载）。
   */
  media_kind?: string;
  /**
   * 第二十轮：视频的**首帧静态图**路径（图片步骤恒为空串）。
   * 缩略图 / 封面 / 发布 / 导出都用它 —— `screenshot_base64` 也是从这里读出来的。
   */
  media_poster_path?: string;
  /** Active window title at time of capture */
  window_title?: string;
  /** Unix epoch ms when the step was recorded */
  timestamp: number;
  /** 该步骤输入值是否为变量（SKILL.md 自动化导出用，每次运行可变） */
  is_variable: boolean;
  /** 变量参数名（如 order_id），导出 SKILL.md 时渲染为 {{param_name}} 占位符 */
  param_name: string;
  /** 文档块类型：'step' = 真步骤；'heading' = 大标题块（不占步骤编号、不计入步骤总数） */
  block_kind: string;
  /** 标题块层级（1/2/3），仅 block_kind === 'heading' 有意义 */
  block_level: number;
  /** 步骤「补充说明」的呈现类型：'' = 普通描述 / 'mark' 标注 / 'warn' 警告 */
  note_kind: string;
  /** 大标题的富文本正文（白名单内联 HTML 子集）；空串 = 无格式。title 恒为它的纯文本 */
  title_html: string;
  /** 图片描述：只描述「这张图里能看到什么」，渲染在截图下方、补充说明上方；空串 = 不渲染 */
  image_caption: string;
  /** 该步骤对应的页面地址（对齐插件 step.url）；本机操作步骤恒为 ''，浏览器/导入场景有值 */
  url?: string;
}

export interface StepSave {
  order_index?: number;
  step_number?: number;
  action_type?: string;
  title?: string;
  description?: string;
  tip?: string;
  screenshot_path?: string;
  annotated_path?: string;
  after_screenshot_path?: string;
}

// ---- AI ----

export interface AiProvider {
  id: string;
  name: string;
  base_url: string;
  model: string;
}

export interface AiConfig {
  provider: AiProvider;
  api_key: string;
}
