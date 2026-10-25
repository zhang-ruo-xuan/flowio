import { useState } from 'react';
import { confirm } from '@tauri-apps/plugin-dialog';
import type { Step } from '../types';
import { healthOfRecording } from '../lib/previewHealth';

export type ExportFormat =
  | 'word' | 'pdf' | 'markdown' | 'txt' | 'csv' | 'xlsx'
  | 'pptx' | 'quick_video' | 'recording_video' | 'images'
  | 'html';

export interface ExportOptions {
  /** 是否在导出图片/视频里高亮鼠标标注 */
  mouseAnnotation: boolean;
  /** 图片导出方式：long=长截图（拼接），individual=逐张文件 */
  imageMode: 'long' | 'individual';
  /** 是否包含目录（PPT / Word / PDF / HTML） */
  includeToc: boolean;
  /** 是否在每步显示来源网址 🔗（对齐插件 opts.links，**缺省显示**） */
  links: boolean;
  /** 图片/视频导出时是否把视频片段转成 GIF 动图 */
  gif: boolean;
  /** 导出图片缩放比例（对齐插件：唯一档 1，界面不暴露比例控件） */
  imageZoom: number;
  /* ── 视频「其他设置」 ── */
  /** 字幕样式：bg=带背板 / plain=纯文字（对齐插件 SUB_STYLES，无「不显示」档） */
  subtitleStyle: 'bg' | 'plain';
  /** 字幕大小 */
  subtitleSize: 'small' | 'medium' | 'large';
  /** 配音（系统语音朗读每步标题） */
  voiceover: boolean;
  /** 镜头缩放（画面缓慢推近，缺省开启） */
  kenBurns: boolean;
  /** 视频封面（额外导出一张封面图，缺省开启） */
  cover: boolean;
  /** 播放倍速：1 / 1.25 / 1.5（对齐插件 SPEED_CHOICES） */
  speed: number;
  /** 去除右下角品牌水印（缺省关闭 = 显示「录步 Flowio」品牌水印，对齐插件默认档） */
  removeWatermark: boolean;
  /** 自定义品牌水印文字（空 = 不加） */
  brandWatermark: string;
  /** 高清（2 倍分辨率 + 更高码率） */
  hd: boolean;
  /** 教程的线上地址（对齐插件 opts.publicUrl）。桌面端没有发布链路，由用户手动填；
   *  空串 = 没发布过，页脚二维码不出现。仅 HTML / PDF 生效（Word 与插件一致无码）。 */
  publicUrl: string;
  /** 页脚是否带「扫码看线上版」二维码（对齐插件 opts.qr，缺省 true） */
  qr: boolean;
}

export const DEFAULT_EXPORT_OPTIONS: ExportOptions = {
  mouseAnnotation: true,
  imageMode: 'long',
  includeToc: true,
  links: true,
  gif: false,
  imageZoom: 1,
  subtitleStyle: 'bg',
  subtitleSize: 'medium',
  voiceover: true,
  kenBurns: true,
  cover: true,
  speed: 1,
  removeWatermark: false,
  brandWatermark: '',
  hd: false,
  publicUrl: '',
  qr: true,
};

interface Props {
  isOpen: boolean;
  onClose: () => void;
  onSelect: (format: ExportFormat, options: ExportOptions) => void;
  /** 步骤列表（可选）：提供后导出前跑成片体检，有 error 级问题先弹确认（StepEditor / Dashboard 传入） */
  steps?: Step[];
}

/** 文本/表格导出不依赖截图，跳过成片体检拦截 */
const HEALTH_SKIP_FORMATS = new Set(['csv', 'xlsx', 'markdown', 'txt']);

const SECTIONS: {
  title: string;
  items: { id: ExportFormat; label: string; icon: string; desc: string }[];
}[] = [
  {
    title: '文档',
    items: [
      { id: 'word', label: 'Word', icon: '📝', desc: '导出可编辑的 Word 文档' },
      { id: 'pdf', label: 'PDF', icon: '📄', desc: '导出适合打印/分享的 PDF' },
      { id: 'markdown', label: 'Markdown', icon: '📑', desc: '导出 Markdown 源文件' },
      { id: 'txt', label: 'TXT', icon: '📃', desc: '导出纯文本，只保留步骤说明' },
      { id: 'csv', label: 'CSV', icon: '📈', desc: '导出表格，可用 Excel/WPS 打开' },
      { id: 'xlsx', label: 'Excel', icon: '📊', desc: '导出真正的 Excel 表格（样式/冻结首行）' },
    ],
  },
  {
    title: '演示',
    items: [
      { id: 'pptx', label: 'PPT', icon: '📊', desc: '导出 PPT，每步一张幻灯片' },
      { id: 'recording_video', label: '录屏视频', icon: '🎬', desc: '按原节奏合成 MP4 视频（默认）' },
      { id: 'quick_video', label: '速览视频', icon: '⏩', desc: '把步骤快速串成 MP4 概览' },
      { id: 'images', label: '图片', icon: '🖼', desc: '导出长截图或逐张截图' },
    ],
  },
  {
    title: '网页',
    items: [
      { id: 'html', label: 'HTML', icon: '🌐', desc: '导出单个可双击打开的 HTML 文件' },
    ],
  },
];

const FORMAT_LABELS: Record<ExportFormat, string> = {
  word: 'Word',
  pdf: 'PDF',
  markdown: 'Markdown',
  txt: 'TXT',
  csv: 'CSV',
  xlsx: 'Excel',
  pptx: 'PPT',
  quick_video: '速览视频',
  recording_video: '录屏视频',
  images: '图片',
  html: 'HTML',
};

// 缩放档位：★ 对齐插件 —— 插件那边的「文档显示宽度」已收敛为**唯一档 1**（铺满版心，
// 界面撤了比例设置，老存档值也一并收敛回 1）。桌面端同样不再暴露比例控件，imageZoom 恒为 1。
// 播放速度：对齐插件 SPEED_CHOICES = [1, 1.25, 1.5]。
const SPEED_PRESETS = [1, 1.25, 1.5];

/** 竞品同款拨动开关 */
function Toggle({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      className={`export-opt-toggle${on ? ' on' : ''}`}
      onClick={() => onChange(!on)}
      aria-pressed={on}
    >
      <span className="export-opt-toggle-knob" />
    </button>
  );
}

export default function ExportDialog({ isOpen, onClose, onSelect, steps }: Props) {
  const [opt, setOpt] = useState<ExportOptions>(DEFAULT_EXPORT_OPTIONS);
  // 默认落在「录屏视频」：用户 2026-09-21 明确要求导出视频默认是录屏视频（非速览）
  const [selected, setSelected] = useState<ExportFormat | null>('recording_video');

  if (!isOpen) return null;

  const patch = (p: Partial<ExportOptions>) => setOpt((prev) => ({ ...prev, ...p }));

  const handleExport = async () => {
    if (!selected) return;
    // 导出前成片体检：有 error 级问题（缺截图/未知操作）先弹确认，避免导出后才在成片里断掉。
    // 复用成片预览同一套 verifyStep 口径（previewHealth.ts，对齐插件 verify.ts）。
    if (steps && steps.length > 0 && !HEALTH_SKIP_FORMATS.has(selected)) {
      const errors = healthOfRecording(steps).filter((i) => i.level === 'error');
      if (errors.length > 0) {
        const preview = errors
          .slice(0, 5)
          .map((i) => `第 ${i.step} 步：${i.message}`)
          .join('\n');
        const more = errors.length > 5 ? `\n…还有 ${errors.length - 5} 条` : '';
        const ok = await confirm(
          `成片体检发现 ${errors.length} 处会断片的问题：\n${preview}${more}\n\n仍要导出吗？`,
          { kind: 'warning', title: '导出前检查', cancelLabel: '返回修改', okLabel: '仍然导出' },
        );
        if (!ok) return;
      }
    }
    onSelect(selected, opt);
    onClose();
  };

  // 各选项的显示条件
  // CSV/XLSX 是表格导出，没有截图相关选项（与 markdown / txt 同组处理）
  const showMouseAnnotation = selected && !['markdown', 'txt', 'csv', 'xlsx'].includes(selected);
  const showToc = selected === 'pptx';
  // 会呈现步骤来源网址的格式（与插件 opts.links 同语义：缺省显示）
  // CSV 不在其中：插件的 CSV 恒含 URL 列，不受 links 开关控制
  const showLinks = selected && ['html', 'pdf', 'word', 'markdown', 'txt'].includes(selected);
  // 页脚二维码只在 HTML / PDF 出现（PDF 走同一份 HTML 渲染；Word 与插件一致不带码）
  const showPublicUrl = selected === 'html' || selected === 'pdf';
  const showImageMode = selected === 'images';
  const showGif = selected === 'images';
  // 缩放档位已按插件收敛为唯一档 1，界面不再提供比例控件
  const isVideo = selected === 'quick_video' || selected === 'recording_video';

  const hasAnyOption =
    showMouseAnnotation || showToc || showLinks || showImageMode || showGif || showPublicUrl;

  return (
    <div
      className="export-dialog-overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className={`export-dialog${isVideo ? ' export-dialog-wide' : ''}`}>
        <div className="export-dialog-header">
          <h3>导出教程</h3>
          <button className="export-dialog-close" onClick={onClose} aria-label="关闭">✕</button>
        </div>

        <div className="export-dialog-body">
          {SECTIONS.map((sec) => (
            <div key={sec.title} className="export-dialog-section">
              <h4 className="export-dialog-section-title">{sec.title}</h4>
              <div className="export-dialog-grid">
                {sec.items.map((item) => {
                  const active = selected === item.id;
                  return (
                    <button
                      key={item.id}
                      className={`export-dialog-card ${active ? 'export-dialog-card-active' : ''}`}
                      onClick={() => setSelected(item.id)}
                      title={item.desc}
                    >
                      <span className="export-dialog-card-icon">{item.icon}</span>
                      <span className="export-dialog-card-label">{item.label}</span>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}

          {/* 视频「其他设置」：字幕 / 配音 / 镜头 / 封面 / 速度 / 水印 / 高清 */}
          {isVideo && (
            <div className="export-dialog-advanced">
              <h4 className="export-dialog-advanced-title">其他设置</h4>
              <div className="export-dialog-advanced-grid">
                <label className="export-opt">
                  <span className="export-opt-label">字幕样式</span>
                  <select
                    className="export-opt-select"
                    value={opt.subtitleStyle}
                    onChange={(e) => patch({ subtitleStyle: e.target.value as ExportOptions['subtitleStyle'] })}
                  >
                    <option value="bg">字幕带背板</option>
                    <option value="plain">纯文字</option>
                  </select>
                </label>

                <label className="export-opt">
                  <span className="export-opt-label">字幕大小</span>
                  <select
                    className="export-opt-select"
                    value={opt.subtitleSize}
                    onChange={(e) => patch({ subtitleSize: e.target.value as ExportOptions['subtitleSize'] })}
                  >
                    <option value="small">小</option>
                    <option value="medium">中（推荐）</option>
                    <option value="large">大</option>
                  </select>
                </label>

                <div className="export-opt">
                  <span className="export-opt-label">配音</span>
                  <Toggle on={opt.voiceover} onChange={(v) => patch({ voiceover: v })} />
                </div>

                <div className="export-opt">
                  <span className="export-opt-label">镜头缩放</span>
                  <Toggle on={opt.kenBurns} onChange={(v) => patch({ kenBurns: v })} />
                </div>

                <div className="export-opt">
                  <span className="export-opt-label">视频封面</span>
                  <Toggle on={opt.cover} onChange={(v) => patch({ cover: v })} />
                </div>

                <label className="export-opt">
                  <span className="export-opt-label">速度</span>
                  <select
                    className="export-opt-select"
                    value={String(opt.speed)}
                    onChange={(e) => patch({ speed: Number(e.target.value) })}
                  >
                    {SPEED_PRESETS.map((s) => (
                      <option key={s} value={String(s)}>{s}x</option>
                    ))}
                  </select>
                </label>

                <div className="export-opt">
                  <span className="export-opt-label">去除水印</span>
                  <Toggle on={opt.removeWatermark} onChange={(v) => patch({ removeWatermark: v })} />
                </div>

                <div className="export-opt">
                  <span className="export-opt-label">高清</span>
                  <Toggle on={opt.hd} onChange={(v) => patch({ hd: v })} />
                </div>

                <div className="export-opt export-opt-wide">
                  <span className="export-opt-label">自定义品牌水印</span>
                  <input
                    className="export-opt-input"
                    placeholder="留空用默认「录步 Flowio」"
                    value={opt.brandWatermark}
                    onChange={(e) => patch({ brandWatermark: e.target.value })}
                  />
                </div>
              </div>
            </div>
          )}
        </div>

        {hasAnyOption && (
          <div className="export-dialog-options">
            <div className="export-dialog-option-row">
              <span className="export-dialog-option-label">选项</span>

              {showToc && (
                <button
                  className={`export-dialog-chip ${opt.includeToc ? 'active' : ''}`}
                  onClick={() => patch({ includeToc: !opt.includeToc })}
                >
                  目录
                </button>
              )}

              {showLinks && (
                <button
                  className={`export-dialog-chip ${opt.links ? 'active' : ''}`}
                  onClick={() => patch({ links: !opt.links })}
                >
                  来源网址
                </button>
              )}

              {showPublicUrl && (
                <>
                  <button
                    className={`export-dialog-chip ${opt.qr ? 'active' : ''}`}
                    onClick={() => patch({ qr: !opt.qr })}
                    title="填了公开链接后在页脚生成「扫码看线上版」二维码"
                  >
                    页脚二维码
                  </button>
                  <input
                    className="export-opt-input export-dialog-public-url"
                    placeholder="公开链接（可选）"
                    value={opt.publicUrl}
                    onChange={(e) => patch({ publicUrl: e.target.value })}
                  />
                </>
              )}

              {showMouseAnnotation && (
                <button
                  className={`export-dialog-chip ${opt.mouseAnnotation ? 'active' : ''}`}
                  onClick={() => patch({ mouseAnnotation: !opt.mouseAnnotation })}
                >
                  鼠标标注
                </button>
              )}

              {showImageMode && (
                <div className="export-dialog-segmented">
                  <button
                    className={opt.imageMode === 'long' ? 'active' : ''}
                    onClick={() => patch({ imageMode: 'long' })}
                  >长截图</button>
                  <button
                    className={opt.imageMode === 'individual' ? 'active' : ''}
                    onClick={() => patch({ imageMode: 'individual' })}
                  >逐张文件</button>
                </div>
              )}

              {showGif && (
                <button
                  className={`export-dialog-chip ${opt.gif ? 'active' : ''}`}
                  onClick={() => patch({ gif: !opt.gif })}
                >
                  动图
                </button>
              )}
            </div>
          </div>
        )}

        <div className="export-dialog-footer">
          <button
            className="export-dialog-submit"
            disabled={!selected}
            onClick={handleExport}
          >
            {selected ? `导出${FORMAT_LABELS[selected]}` : '导出'}
          </button>
        </div>
      </div>
    </div>
  );
}
