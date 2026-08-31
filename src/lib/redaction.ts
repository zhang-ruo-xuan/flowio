// P2-05 隐私安全：分享前敏感信息检测（前端侧）
// 正则与 Rust 侧 src-tauri/src/redaction/mod.rs 保持一致：
// 身份证（18 位）/ 手机号（11 位）/ 银行卡（13-19 位）/ 邮箱 / API Key /
// 统一社会信用代码 / QQ号·微信号 / 密码口令 / 内网IP·MAC / URL 敏感参数 / 自定义。

export type SensitiveKind =
  | "id_card"
  | "phone"
  | "bank_card"
  | "email"
  | "api_key"
  | "credit_code"
  | "qq_wechat"
  | "password_field"
  | "ip_mac"
  | "url_param"
  | "custom";

export const SENSITIVE_KIND_LABEL: Record<SensitiveKind, string> = {
  id_card: "身份证号",
  phone: "手机号",
  bank_card: "银行卡号",
  email: "邮箱",
  api_key: "API Key / Token",
  credit_code: "统一社会信用代码",
  qq_wechat: "QQ号 / 微信号",
  password_field: "密码 / 口令",
  ip_mac: "内网IP / MAC",
  url_param: "URL 敏感参数",
  custom: "自定义规则",
};

export interface SensitiveHit {
  kind: SensitiveKind;
  label: string;
  start: number;
  end: number;
  text: string;
}

export interface SensitiveScanSummary {
  total: number;
  byKind: Partial<Record<SensitiveKind, number>>;
  hits: SensitiveHit[];
}

// 注意：与 Rust 侧一致，这里不用 look-around（部分正则引擎不支持），
// 改为捕获组 + ASCII 边界检查实现等价语义（前后非数字 / 前后非邮箱字符）。
const RE_ID_CARD =
  /([1-9][0-9]{5}(?:19|20)[0-9]{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12][0-9]|3[01])[0-9]{3}[0-9Xx])/g;
const RE_PHONE = /(1[3-9][0-9]{9})/g;
const RE_BANK_CARD = /([0-9]{13,19})/g;
const RE_EMAIL = /([A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})/g;
const RE_API_KEY =
  /(sk-[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|Bearer\s+[A-Za-z0-9._~+/=-]{16,})/gi;
const RE_CREDIT_CODE = /([0-9A-HJ-NPQRTUWXY]{2}[0-9]{6}[0-9A-HJ-NPQRTUWXY]{10})/g;
const RE_QQ = /([1-9][0-9]{5,9})/g;
const RE_WECHAT = /(wxid_[A-Za-z0-9_-]{6,}|微信号[:：]\s*[A-Za-z][A-Za-z0-9_-]{5,19})/g;
const RE_PASSWORD =
  /((?:密码|口令|password|pwd|passwd)\s*[:：=]\s*[^\s,，;；。]+)/gi;
const RE_INTRANET_IP =
  /((?:10\.|172\.(?:1[6-9]|2[0-9]|3[01])\.|192\.168\.)[0-9]{1,3}\.[0-9]{1,3})/g;
const RE_MAC = /((?:[0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2})/g;
const RE_URL_PARAM =
  /([?&](?:token|key|secret|password|pwd|passwd|sessionid|session_id|api_key|apikey|sign|sig|auth)=[^&\s"'<>]+)/gi;

function isAsciiDigit(ch: string): boolean {
  return ch >= "0" && ch <= "9";
}

function isEmailChar(ch: string): boolean {
  return /[A-Za-z0-9._%+-]/.test(ch);
}

function isWordChar(ch: string): boolean {
  return /[A-Za-z0-9_]/.test(ch);
}

/** 数字类（身份证/手机号/银行卡/QQ）：要求匹配前后均非数字 */
function okDigitBoundary(text: string, start: number, end: number): boolean {
  if (start > 0 && isAsciiDigit(text[start - 1])) return false;
  if (end < text.length && isAsciiDigit(text[end])) return false;
  return true;
}

/** 邮箱：要求匹配前后均非邮箱字符 */
function okEmailBoundary(text: string, start: number, end: number): boolean {
  if (start > 0 && isEmailChar(text[start - 1])) return false;
  if (end < text.length && isEmailChar(text[end])) return false;
  return true;
}

/** 词边界（API Key/信用代码/微信号/URL 参数等）：要求匹配前后均非字母数字下划线 */
function okWordBoundary(text: string, start: number, end: number): boolean {
  if (start > 0 && isWordChar(text[start - 1])) return false;
  if (end < text.length && isWordChar(text[end])) return false;
  return true;
}

function collect(
  text: string,
  re: RegExp,
  kind: SensitiveKind,
  boundary: (t: string, s: number, e: number) => boolean,
): SensitiveHit[] {
  const hits: SensitiveHit[] = [];
  re.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const start = m.index;
    const end = m.index + m[0].length;
    if (boundary(text, start, end)) {
      hits.push({ kind, label: SENSITIVE_KIND_LABEL[kind], start, end, text: m[0] });
    }
    if (m.index === re.lastIndex) re.lastIndex += 1;
  }
  return hits;
}

function collectNoBoundary(
  text: string,
  re: RegExp,
  kind: SensitiveKind,
): SensitiveHit[] {
  const hits: SensitiveHit[] = [];
  re.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    hits.push({ kind, label: SENSITIVE_KIND_LABEL[kind], start: m.index, end: m.index + m[0].length, text: m[0] });
    if (m.index === re.lastIndex) re.lastIndex += 1;
  }
  return hits;
}

/** 合并重叠区间（与 Rust 侧行为对齐：重叠时保留更长的先出现片段） */
function mergeHits(all: SensitiveHit[]): SensitiveHit[] {
  all.sort((a, b) => a.start - b.start || b.end - a.end);
  const merged: SensitiveHit[] = [];
  for (const hit of all) {
    const prev = merged[merged.length - 1];
    if (prev && hit.start < prev.end) {
      if (hit.end > prev.end && hit.start <= prev.start) {
        merged.pop();
        merged.push(hit);
      }
      continue;
    }
    merged.push(hit);
  }
  return merged;
}

/** 扫描单个文本，返回所有敏感片段（互不重叠，按出现顺序）。使用默认全量配置。 */
export function findSensitiveInText(text: string): SensitiveHit[] {
  return findSensitiveInTextWithConfig(text, undefined, []);
}

/** 扫描单个文本（可按启用的类别过滤 + 自定义正则）。enabled 为空/undefined 表示全部启用。 */
export function findSensitiveInTextWithConfig(
  text: string,
  enabled: Set<SensitiveKind> | undefined,
  customPatterns: string[],
): SensitiveHit[] {
  if (!text) return [];
  const on = (k: SensitiveKind) => !enabled || enabled.has(k);

  const all: SensitiveHit[] = [];
  if (on("id_card")) all.push(...collect(text, RE_ID_CARD, "id_card", okDigitBoundary));
  if (on("phone")) all.push(...collect(text, RE_PHONE, "phone", okDigitBoundary));
  if (on("bank_card")) all.push(...collect(text, RE_BANK_CARD, "bank_card", okDigitBoundary));
  if (on("email")) all.push(...collect(text, RE_EMAIL, "email", okEmailBoundary));
  if (on("api_key")) all.push(...collect(text, RE_API_KEY, "api_key", okWordBoundary));
  if (on("credit_code")) all.push(...collect(text, RE_CREDIT_CODE, "credit_code", okWordBoundary));
  if (on("qq_wechat")) {
    all.push(...collect(text, RE_QQ, "qq_wechat", okDigitBoundary));
    all.push(...collect(text, RE_WECHAT, "qq_wechat", okWordBoundary));
  }
  if (on("password_field")) all.push(...collectNoBoundary(text, RE_PASSWORD, "password_field"));
  if (on("ip_mac")) {
    all.push(...collectNoBoundary(text, RE_INTRANET_IP, "ip_mac"));
    all.push(...collectNoBoundary(text, RE_MAC, "ip_mac"));
  }
  if (on("url_param")) all.push(...collectNoBoundary(text, RE_URL_PARAM, "url_param"));
  if (on("custom")) {
    for (const pat of customPatterns) {
      const trimmed = pat.trim();
      if (!trimmed) continue;
      try {
        all.push(...collectNoBoundary(text, new RegExp(trimmed, "g"), "custom"));
      } catch {
        // 非法正则跳过
      }
    }
  }
  return mergeHits(all);
}

/** 扫描录制全量文本（标题 + 步骤 title/description/tip），返回汇总。 */
export function scanRecordingForSensitive(
  recordingTitle: string,
  steps: Array<{ title?: string; description?: string; tip?: string }>,
): SensitiveScanSummary {
  const hits: SensitiveHit[] = [];
  if (recordingTitle) hits.push(...findSensitiveInText(recordingTitle));
  for (const step of steps) {
    if (step.title) hits.push(...findSensitiveInText(step.title));
    if (step.description) hits.push(...findSensitiveInText(step.description));
    if (step.tip) hits.push(...findSensitiveInText(step.tip));
  }
  const byKind: Partial<Record<SensitiveKind, number>> = {};
  for (const hit of hits) {
    byKind[hit.kind] = (byKind[hit.kind] || 0) + 1;
  }
  return { total: hits.length, byKind, hits };
}
