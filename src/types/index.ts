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
  /** Path to the post-action "after" screenshot */
  after_screenshot_path: string | null;
  /** Base64-encoded marked/cropped screenshot (with red circle etc.) */
  screenshot_base64: string | null;
  /** Base64-encoded clean before screenshot (full context, no annotations) */
  before_screenshot_base64: string | null;
  /** Base64-encoded after screenshot data */
  after_screenshot_base64: string | null;
  /** Active window title at time of capture */
  window_title?: string;
  /** Unix epoch ms when the step was recorded */
  timestamp: number;
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
