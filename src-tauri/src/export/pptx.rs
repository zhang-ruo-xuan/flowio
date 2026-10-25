//! PPTX 生成 —— 零第三方 OOXML 依赖，手写 DrawingML。
//!
//! 与插件端 `flowio-extension/src/lib/pptx.ts` 对齐（v0.65.365 口径）：
//! 封面 / 目录 / 步骤页 / 章节页全部走同一套品牌版式（顶部品牌色带、橙色徽章、
//! 动作胶囊、截图投影、页脚左网址右页码）。桌面端此前的 ppt-rs 方案只有
//! 「标题 + 一张图」，与插件版式差距过大，故整体替换（B 批对齐项 ②）。
//!
//! 产物结构（最小可用 PPTX，缺一个 part 或一条关系，PowerPoint 就会提示"需要修复"）：
//!   [Content_Types].xml
//!   _rels/.rels                  → ppt/presentation.xml
//!   ppt/presentation.xml         → 母版 + 主题 + 每一页
//!   ppt/_rels/presentation.xml.rels
//!   ppt/slideMasters/slideMaster1.xml    (+rels)
//!   ppt/slideLayouts/slideLayout1.xml    (+rels)
//!   ppt/theme/theme1.xml
//!   ppt/slides/slideN.xml        (+rels → 图片)
//!   ppt/media/imageN.<ext>
//!
//! 压缩方式一律 **store**（不压缩）：截图本身已是 PNG/JPEG，XML 体量可忽略；
//! 少一层 deflate 就少一层出错的可能（插件端 zipStore 同样 store）。

use image::{GenericImageView, ImageFormat};

const SLIDE_W: i64 = 12_192_000; // 16:9 13.333in × 7.5in
const SLIDE_H: i64 = 6_858_000;
const MARGIN: i64 = 640_080; // 0.7in
const CONTENT_W: i64 = SLIDE_W - MARGIN * 2;
const FONT: &str = "微软雅黑";
const ACCENT: &str = "FF6B35"; // 品牌橙
const EMU_PER_PX: i64 = 9_525; // 96dpi
const MAX_IMAGE_UPSCALE: i64 = 2;
const PPTX_HEADING_SIZES: [i64; 4] = [5250, 4200, 3400, 2850]; // 章节 H1~H4
const PPTX_COVER_TITLE_SIZE: i64 = 5700;
const PPTX_STEP_TITLE_SIZE: i64 = 3200;

/// 一步（或一个章节块）的 PPTX 展示所需信息，由 commands.rs 组装。
pub struct PptxStep {
    pub title: String,
    /// 大标题的富文本正文（白名单内联 HTML 子集），空 = 用 title 纯文本。
    pub title_html: String,
    /// Some(lv) = 章节页（不印步骤徽章，字号按层级）；None = 普通步骤页。
    pub heading_level: Option<i32>,
    pub description: String,
    /// 动作标签（点击/输入/滚动…），显示在标题右侧胶囊里。
    pub action_label: String,
    pub url: String,
    /// 已处理（焦点/光标）的截图字节；None = 只出文字页（不塞占位图）。
    pub image: Option<Vec<u8>>,
    /// 步骤编号。`> 0` = 真实步骤（画橙色编号徽章）；`<= 0` = 无编号文字页
    /// （桌面端 summary/intro 文档块，不画徽章、不占目录编号）。
    pub step_number: i32,
}

/// 生成 .pptx 的 zip 条目（store 模式），由调用方写盘。纯函数：同样输入必得同样字节。
pub fn build_pptx_entries(
    title: &str,
    created_at: &str,
    steps: &[PptxStep],
    include_toc: bool,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let footer = "由 Flowio 录步 导出";
    let with_toc = include_toc && !steps.is_empty();
    let slide_count = steps.len() + 1 + if with_toc { 1 } else { 0 };
    let mut slide_parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut media_parts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut media_exts: Vec<String> = Vec::new();

    // ── 封面 ──
    {
        let mut date_line = format!("共 {} 个步骤", steps.len());
        if !created_at.trim().is_empty() {
            date_line.push_str("  ·  ");
            date_line.push_str(created_at.trim());
        }
        let shapes = vec![
            shape_box(5, "品牌色带", &Box_ { x: 0, y: 0, cx: SLIDE_W, cy: 108_000 }, ACCENT, None),
            shape_box(
                2,
                "类型胶囊",
                &Box_ { x: MARGIN, y: 1_850_000, cx: 1_560_000, cy: 500_000 },
                "FFE9DE",
                Some(ShapeOpts {
                    round: Some(30_000),
                    text: Some("操作指南".to_string()),
                    text_color: Some(ACCENT.to_string()),
                    text_size: Some(1400),
                    text_bold: Some(true),
                }),
            ),
            text_box_al(
                3,
                "封面标题",
                &Box_ { x: MARGIN, y: 2_600_000, cx: CONTENT_W, cy: 1_300_000 },
                &paragraphs(if title.trim().is_empty() { "操作指南" } else { title }, &RunOpts {
                    size: PPTX_COVER_TITLE_SIZE,
                    bold: true,
                    color: Some("1F2937".to_string()),
                    ..Default::default()
                }),
                "l",
                "ctr",
            ),
            shape_box(
                6,
                "标题下划线",
                &Box_ { x: MARGIN, y: 4_120_000, cx: 760_000, cy: 60_000 },
                ACCENT,
                Some(ShapeOpts { round: Some(0), ..Default::default() }),
            ),
            text_box(
                4,
                "封面副标题",
                &Box_ { x: MARGIN, y: 4_420_000, cx: CONTENT_W, cy: 500_000 },
                &paragraphs(&date_line, &RunOpts { size: 1600, color: Some("6B7280".to_string()), ..Default::default() }),
            ),
            text_box(
                7,
                "封面落款",
                &Box_ { x: MARGIN, y: SLIDE_H - 520_000, cx: CONTENT_W - 1_800_000, cy: 360_000 },
                &paragraphs(footer, &RunOpts { size: 1200, color: Some("9CA3AF".to_string()), ..Default::default() }),
            ),
            text_box_al(
                8,
                "封面官网",
                &Box_ { x: SLIDE_W - MARGIN - 1_800_000, y: SLIDE_H - 520_000, cx: 1_800_000, cy: 360_000 },
                &paragraphs("flowio.cn", &RunOpts { size: 1200, color: Some("9CA3AF".to_string()), ..Default::default() }),
                "r",
                "ctr",
            ),
        ];
        slide_parts.push(("ppt/slides/slide1.xml".to_string(), slide_xml(&shapes).into_bytes()));
        // 封面没有图片/外链 ⇒ 不产 rels part
    }

    // ── 目录（对齐插件 v0.65.302 起）──
    if with_toc {
        let entries: Vec<TocEntry> = steps
            .iter()
            .enumerate()
            .filter(|(_, st)| st.heading_level.is_some() || st.step_number > 0)
            .map(|(i, st)| {
                if st.heading_level.is_some() {
                    TocEntry {
                        no: None,
                        title: if st.title.trim().is_empty() {
                            format!("章节 {}", i + 1)
                        } else {
                            st.title.clone()
                        },
                        head: true,
                    }
                } else {
                    TocEntry {
                        no: Some(st.step_number),
                        title: if st.title.trim().is_empty() {
                            format!("步骤 {}", i + 1)
                        } else {
                            st.title.clone()
                        },
                        head: false,
                    }
                }
            })
            .collect();
        let mut shapes = vec![
            shape_box(5, "品牌色带", &Box_ { x: 0, y: 0, cx: SLIDE_W, cy: 108_000 }, ACCENT, None),
            text_box(
                3,
                "目录标题",
                &Box_ { x: MARGIN, y: 380_000, cx: CONTENT_W, cy: 640_000 },
                &paragraphs("目录", &RunOpts { size: 3400, bold: true, color: Some("1F2937".to_string()), ..Default::default() }),
            ),
            shape_box(
                6,
                "标题下划线",
                &Box_ { x: MARGIN, y: 1_040_000, cx: 760_000, cy: 60_000 },
                ACCENT,
                Some(ShapeOpts { round: Some(0), ..Default::default() }),
            ),
            text_box(
                7,
                "目录说明",
                &Box_ { x: MARGIN, y: 1_120_000, cx: CONTENT_W, cy: 260_000 },
                &paragraphs(&format!("共 {} 个步骤", steps.len()), &RunOpts { size: 1100, color: Some("9CA3AF".to_string()), ..Default::default() }),
            ),
        ];
        shapes.push(toc_row_shapes(&entries, 20));
        shapes.push(shape_box(
            9,
            "页脚分隔线",
            &Box_ { x: MARGIN, y: SLIDE_H - 500_000, cx: CONTENT_W, cy: 12_700 },
            "E5E7EB",
            Some(ShapeOpts { round: Some(0), ..Default::default() }),
        ));
        shapes.push(text_box(
            8,
            "目录页脚",
            &Box_ { x: MARGIN, y: SLIDE_H - 420_000, cx: CONTENT_W, cy: 280_000 },
            &paragraphs(footer, &RunOpts { size: 1000, color: Some("9CA3AF".to_string()), ..Default::default() }),
        ));
        slide_parts.push(("ppt/slides/slide2.xml".to_string(), slide_xml(&shapes).into_bytes()));
    }

    // ── 步骤页 / 章节页 ──
    for (i, st) in steps.iter().enumerate() {
        let no = i + 1;
        let file_name = format!("slide{}.xml", no + 1 + if with_toc { 1 } else { 0 });
        let mut rel_items: Vec<(String, String, String)> = Vec::new();
        let mut shapes: Vec<String> = Vec::new();

        let is_head = st.heading_level.is_some();
        let is_numbered = !is_head && st.step_number > 0;
        if is_numbered {
            shapes.push(shape_box(
                7,
                "步骤徽章",
                &Box_ { x: MARGIN, y: 320_000, cx: 560_000, cy: 560_000 },
                ACCENT,
                Some(ShapeOpts {
                    round: Some(28_000),
                    text: Some(st.step_number.to_string()),
                    text_color: Some("FFFFFF".to_string()),
                    text_size: Some(2000),
                    text_bold: Some(true),
                }),
            ));
            shapes.push(text_box_al(
                3,
                "步骤标题",
                &Box_ { x: MARGIN + 700_000, y: 290_000, cx: CONTENT_W - 2_930_000, cy: 700_000 },
                &rich_paragraphs(
                    if !st.title_html.trim().is_empty() { &st.title_html } else { &st.title },
                    &RunOpts { size: PPTX_STEP_TITLE_SIZE, bold: true, color: Some("1F2937".to_string()), ..Default::default() },
                ),
                "l",
                "ctr",
            ));
            if !st.action_label.trim().is_empty() {
                shapes.push(shape_box(
                    8,
                    "动作标签",
                    &Box_ { x: SLIDE_W - MARGIN - 2_000_000, y: 390_000, cx: 2_000_000, cy: 440_000 },
                    "FFE9DE",
                    Some(ShapeOpts {
                        round: Some(40_000),
                        text: Some(st.action_label.clone()),
                        text_color: Some(ACCENT.to_string()),
                        text_size: Some(1200),
                        text_bold: Some(true),
                    }),
                ));
            }
            if !st.description.trim().is_empty() {
                shapes.push(text_box(
                    5,
                    "步骤说明",
                    &Box_ { x: MARGIN + 700_000, y: 1_010_000, cx: CONTENT_W - 700_000, cy: 420_000 },
                    &paragraphs(&st.description, &RunOpts { size: 1300, color: Some("6B7280".to_string()), ..Default::default() }),
                ));
            }
        } else if is_head {
            let lv = st.heading_level.unwrap_or(1).clamp(1, 4) as usize - 1;
            shapes.push(shape_box(
                7,
                "章节竖条",
                &Box_ { x: MARGIN, y: 2_820_000, cx: 90_000, cy: 920_000 },
                ACCENT,
                Some(ShapeOpts { round: Some(0), ..Default::default() }),
            ));
            shapes.push(text_box_al(
                3,
                "章节标题",
                &Box_ { x: MARGIN + 280_000, y: 2_620_000, cx: CONTENT_W - 280_000, cy: 1_320_000 },
                &rich_paragraphs(
                    if !st.title_html.trim().is_empty() { &st.title_html } else { &st.title },
                    &RunOpts { size: PPTX_HEADING_SIZES[lv], bold: true, color: Some("1F2937".to_string()), ..Default::default() },
                ),
                "l",
                "ctr",
            ));
        } else {
            // 无编号文字页（summary/intro 文档块）：标题靠左，下面挂描述
            shapes.push(text_box_al(
                3,
                "文档块标题",
                &Box_ { x: MARGIN, y: 300_000, cx: CONTENT_W, cy: 700_000 },
                &rich_paragraphs(
                    if !st.title_html.trim().is_empty() { &st.title_html } else { &st.title },
                    &RunOpts { size: PPTX_STEP_TITLE_SIZE, bold: true, color: Some("1F2937".to_string()), ..Default::default() },
                ),
                "l",
                "ctr",
            ));
            if !st.description.trim().is_empty() {
                shapes.push(text_box(
                    5,
                    "文档块描述",
                    &Box_ { x: MARGIN, y: 1_010_000, cx: CONTENT_W, cy: 420_000 },
                    &paragraphs(&st.description, &RunOpts { size: 1300, color: Some("6B7280".to_string()), ..Default::default() }),
                ));
            }
        }

        if let Some(bytes) = &st.image {
            let rid = "rId1";
            let (ext, dims) = image_meta(bytes)?;
            let media_name = format!("image{}.{}", media_parts.len() + 1, ext);
            media_parts.push((format!("ppt/media/{}", media_name), bytes.clone()));
            if !media_exts.contains(&ext) {
                media_exts.push(ext.clone());
            }
            rel_items.push((rid.to_string(), "image".to_string(), format!("../media/{}", media_name)));
            let fitted = scale_box_centered(
                &fit_image_box(dims, &Box_ { x: MARGIN, y: 1_500_000, cx: CONTENT_W, cy: SLIDE_H - 2_040_000 }),
                1.0,
            );
            shapes.push(picture(4, rid, &format!("步骤 {} 截图", no), &fitted));
        }

        // 页脚：细分隔线 + 左网址 + 右「N / 总数」
        shapes.push(shape_box(
            9,
            "页脚分隔线",
            &Box_ { x: MARGIN, y: SLIDE_H - 500_000, cx: CONTENT_W, cy: 12_700 },
            "E5E7EB",
            Some(ShapeOpts { round: Some(0), ..Default::default() }),
        ));
        if !st.url.trim().is_empty() {
            shapes.push(text_box(
                6,
                "页面地址",
                &Box_ { x: MARGIN, y: SLIDE_H - 420_000, cx: CONTENT_W - 1_300_000, cy: 280_000 },
                &paragraphs(&st.url, &RunOpts { size: 1000, color: Some("9CA3AF".to_string()), ..Default::default() }),
            ));
        }
        shapes.push(text_box_al(
            10,
            "页码",
            &Box_ { x: SLIDE_W - MARGIN - 1_200_000, y: SLIDE_H - 420_000, cx: 1_200_000, cy: 280_000 },
            &paragraphs(&format!("{} / {}", no, steps.len()), &RunOpts { size: 1000, color: Some("9CA3AF".to_string()), ..Default::default() }),
            "r",
            "ctr",
        ));

        slide_parts.push((format!("ppt/slides/{}", file_name), slide_xml(&shapes).into_bytes()));
        if !rel_items.is_empty() {
            let rels = rels_xml(&rel_items);
            slide_parts.push((format!("ppt/slides/_rels/{}.rels", file_name), rels.into_bytes()));
        }
    }

    // ── presentation.xml + rels ──
    let mut presentation_rels: Vec<(String, String, String)> = vec![
        ("rId1".to_string(), "slideMaster".to_string(), "slideMasters/slideMaster1.xml".to_string()),
        ("rId2".to_string(), "theme".to_string(), "theme/theme1.xml".to_string()),
    ];
    let mut sld_ids = String::new();
    for n in 1..=slide_count {
        let rid = format!("rId{}", n + 2);
        presentation_rels.push((rid.clone(), "slide".to_string(), format!("slides/slide{}.xml", n)));
        sld_ids.push_str(&format!("<p:sldId id=\"{}\" r:id=\"{}\"/>", 255 + n, rid));
    }
    let presentation_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
         <p:presentation xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" saveSubsetFonts=\"1\">\
         <p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>\
         <p:sldIdLst>{}</p:sldIdLst>\
         <p:sldSz cx=\"{}\" cy=\"{}\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/>\
         </p:presentation>",
        sld_ids, SLIDE_W, SLIDE_H
    );

    // ── [Content_Types].xml ──
    let mut overrides = String::new();
    for (part, ct) in [
        ("/ppt/presentation.xml", "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"),
        ("/ppt/slideMasters/slideMaster1.xml", "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"),
        ("/ppt/slideLayouts/slideLayout1.xml", "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"),
        ("/ppt/theme/theme1.xml", "application/vnd.openxmlformats-officedocument.theme+xml"),
    ] {
        overrides.push_str(&format!(
            "<Override PartName=\"{}\" ContentType=\"{}\"/>",
            part, ct
        ));
    }
    for n in 1..=slide_count {
        overrides.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
            n
        ));
    }
    let mut ext_defaults = String::new();
    for e in &media_exts {
        let mime = match e.as_str() {
            "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => "image/png",
        };
        ext_defaults.push_str(&format!(
            "<Default Extension=\"{}\" ContentType=\"{}\"/>",
            e, mime
        ));
    }
    let content_types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>{}{}</Types>",
        ext_defaults, overrides
    );

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    entries.push(("[Content_Types].xml".to_string(), content_types.into_bytes()));
    entries.push((
        "_rels/.rels".to_string(),
        rels_xml(&[("rId1".to_string(), "officeDocument".to_string(), "ppt/presentation.xml".to_string())]).into_bytes(),
    ));
    entries.push(("ppt/presentation.xml".to_string(), presentation_xml.into_bytes()));
    entries.push(("ppt/_rels/presentation.xml.rels".to_string(), rels_xml(&presentation_rels).into_bytes()));
    entries.push(("ppt/slideMasters/slideMaster1.xml".to_string(), master_xml().into_bytes()));
    entries.push((
        "ppt/slideMasters/_rels/slideMaster1.xml.rels".to_string(),
        rels_xml(&[
            ("rId1".to_string(), "slideLayout".to_string(), "../slideLayouts/slideLayout1.xml".to_string()),
            ("rId2".to_string(), "theme".to_string(), "../theme/theme1.xml".to_string()),
        ])
        .into_bytes(),
    ));
    entries.push(("ppt/slideLayouts/slideLayout1.xml".to_string(), layout_xml().into_bytes()));
    entries.push((
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels".to_string(),
        rels_xml(&[("rId1".to_string(), "slideMaster".to_string(), "../slideMasters/slideMaster1.xml".to_string())]).into_bytes(),
    ));
    entries.push(("ppt/theme/theme1.xml".to_string(), theme_xml().into_bytes()));
    entries.extend(slide_parts);
    entries.extend(media_parts);
    Ok(entries)
}

// ==================== 富文本 ====================

#[derive(Clone, Default)]
struct RunOpts {
    size: i64,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: Option<String>,
    highlight: Option<String>,
}

#[derive(Clone, Default)]
struct Run {
    text: String,
    opts: RunOpts,
}

/// 白名单 HTML → run 列表（对齐前端 richText.ts：b/strong/i/em/u/s/strike/del + span 样式）。
fn parse_inline_runs(html: &str) -> Vec<Run> {
    let chars: Vec<char> = html.chars().collect();
    let mut runs: Vec<Run> = Vec::new();
    let mut buf = String::new();
    let mut stack: Vec<RunOpts> = vec![RunOpts::default()];
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '<' {
            buf.push(chars[i]);
            i += 1;
            continue;
        }
        // 结束标签？
        let close = i + 1 < chars.len() && chars[i + 1] == '/';
        let mut j = if close { i + 2 } else { i + 1 };
        while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '-') {
            j += 1;
        }
        let tag: String = chars[i + 1..j].iter().collect::<String>().to_lowercase();
        // 标签属性（span 的 style）
        let mut attrs = String::new();
        let mut k = j;
        while k < chars.len() && chars[k] != '>' {
            attrs.push(chars[k]);
            k += 1;
        }
        if k < chars.len() {
            k += 1; // 跳过 '>'
        }
        match tag.as_str() {
            "b" | "strong" => push_tag(&mut buf, &mut runs, &mut stack, close, |o| o.bold = true),
            "i" | "em" => push_tag(&mut buf, &mut runs, &mut stack, close, |o| o.italic = true),
            "u" => push_tag(&mut buf, &mut runs, &mut stack, close, |o| o.underline = true),
            "s" | "strike" | "del" => push_tag(&mut buf, &mut runs, &mut stack, close, |o| o.strike = true),
            "br" => {
                if !close {
                    buf.push('\n');
                }
            }
            "span" if !close => {
                flush(&mut buf, &mut runs, stack.last().unwrap());
                if let Some(style) = extract_style(&attrs) {
                    let mut o = stack.last().unwrap().clone();
                    apply_style(&mut o, &style);
                    stack.push(o);
                } else {
                    stack.push(stack.last().unwrap().clone());
                }
            }
            "span" => {
                // 未知属性的 span 关闭（有 style 的 span 已在 close 分支被吞，这里防御性弹栈）
                pop_tag(&mut buf, &mut runs, &mut stack);
            }
            _ => {
                if close {
                    pop_tag(&mut buf, &mut runs, &mut stack);
                } else {
                    // 未知开始标签：透明化（保留内容），入栈同款
                    stack.push(stack.last().unwrap().clone());
                }
            }
        }
        i = k;
    }
    flush(&mut buf, &mut runs, stack.last().unwrap());
    runs
}

/// 开启/关闭白名单行内标签：开启时克隆当前样式并施加修改入栈，关闭时弹栈。
/// 每次入栈/出栈前都 flush 缓冲，保证 run 边界正确。
fn push_tag<F: Fn(&mut RunOpts)>(buf: &mut String, runs: &mut Vec<Run>, stack: &mut Vec<RunOpts>, close: bool, f: F) {
    if close {
        pop_tag(buf, runs, stack);
        return;
    }
    flush(buf, runs, stack.last().unwrap());
    let mut o = stack.last().unwrap().clone();
    f(&mut o);
    stack.push(o);
}

/// 弹出一个样式层级（栈底保留，绝不弹空）。
fn pop_tag(buf: &mut String, runs: &mut Vec<Run>, stack: &mut Vec<RunOpts>) {
    if stack.len() > 1 {
        flush(buf, runs, stack.last().unwrap());
        stack.pop();
    }
}

fn flush(buf: &mut String, runs: &mut Vec<Run>, opts: &RunOpts) {
    if !buf.is_empty() {
        runs.push(Run { text: std::mem::take(buf), opts: opts.clone() });
    }
}

/// 从 span 标签属性里抠 style="..."（白名单：color/background-color/font-style/font-weight/text-decoration）。
fn extract_style(attrs: &str) -> Option<String> {
    let lower = attrs.to_lowercase();
    let start = lower.find("style=\"")? + 7;
    let rest = &lower[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn apply_style(o: &mut RunOpts, style: &str) {
    for decl in style.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let (k, v) = match decl.split_once(':') {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim()),
            None => continue,
        };
        match k.as_str() {
            "color" => {
                if let Some(h) = to_hex6(v) {
                    o.color = Some(h);
                }
            }
            "background-color" => {
                if let Some(h) = to_hex6(v) {
                    o.highlight = Some(h);
                }
            }
            "font-style" => o.italic = v == "italic",
            "font-weight" => o.bold = v == "bold" || v.parse::<i32>().map(|n| n >= 600).unwrap_or(false),
            "text-decoration" => {
                o.underline = v.contains("underline");
                o.strike = v.contains("line-through");
            }
            _ => {}
        }
    }
}

fn to_hex6(v: &str) -> Option<String> {
    let s = v.trim().to_lowercase();
    let s = s.strip_prefix('#').unwrap_or(&s);
    if s.len() == 3 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        let mut out = String::with_capacity(6);
        for c in s.chars() {
            out.push(c);
            out.push(c);
        }
        return Some(out.to_uppercase());
    }
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(s.to_uppercase());
    }
    None
}

fn hex6(v: &str) -> String {
    to_hex6(v).unwrap_or_else(|| "333333".to_string())
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' => {}
            _ => out.push(c),
        }
    }
    out
}

fn run(text: &str, opts: &RunOpts) -> String {
    let color = hex6(opts.color.as_deref().unwrap_or("333333"));
    let b = if opts.bold { " b=\"1\"" } else { "" };
    let i = if opts.italic { " i=\"1\"" } else { "" };
    let u = if opts.underline { " u=\"sng\"" } else { "" };
    let st = if opts.strike { " strike=\"sngStrike\"" } else { "" };
    let hl = match &opts.highlight {
        Some(h) => format!("<a:highlight><a:srgbClr val=\"{}\"/></a:highlight>", hex6(h)),
        None => String::new(),
    };
    format!(
        "<a:r><a:rPr lang=\"zh-CN\" altLang=\"en-US\" sz=\"{}\"{}{}{}{} dirty=\"0\">\
         <a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>{}\
         <a:latin typeface=\"{}\"/><a:ea typeface=\"{}\"/><a:cs typeface=\"\"/>\
         </a:rPr><a:t>{}</a:t></a:r>",
        opts.size,
        b,
        i,
        u,
        st,
        color,
        hl,
        FONT,
        FONT,
        xml_escape(text),
    )
}

/// 多行文本 → `<a:p>` 数组（`\n` 拆段；空行给一个空 run）。
fn paragraphs(text: &str, opts: &RunOpts) -> String {
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    if lines.is_empty() {
        return format!("<a:p>{}</a:p>", run("", opts));
    }
    lines
        .iter()
        .map(|l| format!("<a:p>{}</a:p>", run(l, opts)))
        .collect::<Vec<_>>()
        .join("")
}

/// 富文本标题 → 单段多 run（无格式位则走纯文本老路径）。
fn rich_paragraphs(html: &str, opts: &RunOpts) -> String {
    if html.trim().is_empty() {
        return paragraphs("", opts);
    }
    let runs = parse_inline_runs(html);
    let plain: String = runs.iter().map(|r| r.text.as_str()).collect();
    if plain.trim().is_empty() {
        return paragraphs("", opts);
    }
    let rich = runs.iter().any(|r| {
        r.opts.bold || r.opts.italic || r.opts.underline || r.opts.strike
            || r.opts.color.is_some() || r.opts.highlight.is_some()
    });
    if !rich {
        return paragraphs(&plain, opts);
    }
    let body = runs
        .iter()
        .map(|r| {
            run(&r.text, &RunOpts {
                size: opts.size,
                bold: opts.bold || r.opts.bold,
                italic: r.opts.italic,
                underline: r.opts.underline,
                strike: r.opts.strike,
                color: r.opts.color.clone().or_else(|| opts.color.clone()),
                highlight: r.opts.highlight.clone(),
            })
        })
        .collect::<Vec<_>>()
        .join("");
    format!("<a:p>{}</a:p>", body)
}

// ==================== 形状 ====================

struct Box_ {
    x: i64,
    y: i64,
    cx: i64,
    cy: i64,
}

#[derive(Default)]
struct ShapeOpts {
    round: Option<i64>,
    text: Option<String>,
    text_color: Option<String>,
    text_size: Option<i64>,
    text_bold: Option<bool>,
}

/// 文本框（默认左对齐、垂直居中）。
fn text_box(id: i64, name: &str, box_: &Box_, body: &str) -> String {
    text_box_al(id, name, box_, body, "l", "ctr")
}

/// 文本框（可指定对齐方式）。
fn text_box_al(id: i64, name: &str, box_: &Box_, body: &str, align: &str, anchor: &str) -> String {
    let p = body.replace("<a:p>", &format!("<a:p><a:pPr algn=\"{}\"/>", align));
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{}\" name=\"{}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>\
         <p:txBody><a:bodyPr wrap=\"square\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" anchor=\"{}\">\
         <a:normAutofit/></a:bodyPr><a:lstStyle/>{}</p:txBody></p:sp>",
        id,
        xml_escape(name),
        box_.x,
        box_.y,
        box_.cx,
        box_.cy,
        anchor,
        p,
    )
}

fn shape_box(id: i64, name: &str, box_: &Box_, fill: &str, opts: Option<ShapeOpts>) -> String {
    let opts = opts.unwrap_or_default();
    let adj = match opts.round {
        Some(r) => format!("<a:avLst><a:gd name=\"adj\" fmla=\"val {}\"/></a:avLst>", r),
        None => "<a:avLst/>".to_string(),
    };
    let geom = if opts.round == Some(0) { "rect" } else { "roundRect" };
    let body = match &opts.text {
        Some(t) => {
            let size = opts.text_size.unwrap_or(1200);
            let color = opts.text_color.as_deref().unwrap_or("FFFFFF");
            format!(
                "<a:p><a:pPr algn=\"ctr\"/>{}</a:p>",
                run(t, &RunOpts { size, bold: opts.text_bold.unwrap_or(false), color: Some(color.to_string()), ..Default::default() })
            )
        }
        None => "<a:p><a:pPr algn=\"ctr\"/><a:endParaRPr lang=\"zh-CN\"/></a:p>".to_string(),
    };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{}\" name=\"{}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>\
         <a:prstGeom prst=\"{}\">{}</a:prstGeom>\
         <a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill><a:ln><a:noFill/></a:ln></p:spPr>\
         <p:txBody><a:bodyPr wrap=\"square\" lIns=\"18000\" tIns=\"0\" rIns=\"18000\" bIns=\"0\" anchor=\"ctr\"/>\
         <a:lstStyle/>{}</p:txBody></p:sp>",
        id,
        xml_escape(name),
        box_.x,
        box_.y,
        box_.cx,
        box_.cy,
        geom,
        adj,
        fill,
        body,
    )
}

fn picture(id: i64, r_id: &str, name: &str, box_: &Box_) -> String {
    format!(
        "<p:pic><p:nvPicPr><p:cNvPr id=\"{}\" name=\"{}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
         <p:blipFill><a:blip r:embed=\"{}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>\
         <p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom>\
         <a:effectLst><a:outerShdw blurRad=\"63500\" dist=\"25400\" dir=\"5400000\" rotWithShape=\"0\">\
         <a:srgbClr val=\"1F2937\"><a:alpha val=\"18000\"/></a:srgbClr></a:outerShdw></a:effectLst>\
         </p:spPr></p:pic>",
        id,
        xml_escape(name),
        r_id,
        box_.x,
        box_.y,
        box_.cx,
        box_.cy,
    )
}

fn slide_xml(shapes: &[String]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
         <p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
         <p:cSld><p:spTree>\
         <p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
         <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/>\
         <a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>\
         {}\
         </p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>",
        shapes.join("")
    )
}

const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn rels_xml(items: &[(String, String, String)]) -> String {
    let inner = items
        .iter()
        .map(|(id, typ, target)| {
            format!(
                "<Relationship Id=\"{}\" Type=\"{}/{}\" Target=\"{}\"/>",
                id, REL_NS, typ, target
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{}</Relationships>",
        inner
    )
}

fn theme_xml() -> String {
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let ln = format!(
        "<a:ln w=\"6350\" cap=\"flat\" cmpd=\"sng\" algn=\"ctr\">{}\
         <a:prstDash val=\"solid\"/></a:ln>",
        fill
    );
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
         <a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Flowio\">\
         <a:themeElements>\
         <a:clrScheme name=\"Flowio\">\
         <a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1>\
         <a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1>\
         <a:dk2><a:srgbClr val=\"44546A\"/></a:dk2><a:lt2><a:srgbClr val=\"F2F2F2\"/></a:lt2>\
         <a:accent1><a:srgbClr val=\"{}\"/></a:accent1>\
         <a:accent2><a:srgbClr val=\"1F2937\"/></a:accent2>\
         <a:accent3><a:srgbClr val=\"9CA3AF\"/></a:accent3>\
         <a:accent4><a:srgbClr val=\"2563EB\"/></a:accent4>\
         <a:accent5><a:srgbClr val=\"16A34A\"/></a:accent5>\
         <a:accent6><a:srgbClr val=\"DC2626\"/></a:accent6>\
         <a:hlink><a:srgbClr val=\"2563EB\"/></a:hlink><a:folHlink><a:srgbClr val=\"7C3AED\"/></a:folHlink>\
         </a:clrScheme>\
         <a:fontScheme name=\"Flowio\">\
         <a:majorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"{}\"/><a:cs typeface=\"\"/></a:majorFont>\
         <a:minorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"{}\"/><a:cs typeface=\"\"/></a:minorFont>\
         </a:fontScheme>\
         <a:fmtScheme name=\"Flowio\">\
         <a:fillStyleLst>{}{}{}</a:fillStyleLst>\
         <a:lnStyleLst>{}{}{}</a:lnStyleLst>\
         <a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle>\
         <a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>\
         <a:bgFillStyleLst>{}{}{}</a:bgFillStyleLst>\
         </a:fmtScheme></a:themeElements></a:theme>",
        ACCENT, FONT, FONT, FONT, FONT, fill, fill, fill, ln, ln, ln, fill, fill, fill
    )
}

fn master_xml() -> String {
    let lvl = "<a:lvl1pPr algn=\"l\"><a:defRPr sz=\"1800\"><a:solidFill><a:schemeClr val=\"tx1\"/></a:solidFill>\
              <a:latin typeface=\"+mn-lt\"/><a:ea typeface=\"+mn-ea\"/></a:defRPr></a:lvl1pPr>";
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
         <p:sldMaster xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:r=\"{}\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
         <p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val=\"lt1\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>\
         <p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
         <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/>\
         <a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld>\
         <p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" \
         accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>\
         <p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst>\
         <p:txStyles><p:titleStyle>{}</p:titleStyle><p:bodyStyle>{}</p:bodyStyle>\
         <p:otherStyle>{}</p:otherStyle></p:txStyles></p:sldMaster>",
        REL_NS, lvl, lvl, lvl
    )
}

fn layout_xml() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n\
         <p:sldLayout xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
         xmlns:r=\"{}\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" \
         type=\"blank\" preserve=\"1\">\
         <p:cSld name=\"空白\"><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
         <p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/>\
         <a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld>\
         <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>",
        REL_NS
    )
}

// ==================== 版面计算 ====================

/// 在给定框内按原始比例内切（contain），永远不超出 area、保比例、放大不超过 2 倍。
fn fit_image_box(natural: Option<(u32, u32)>, area: &Box_) -> Box_ {
    let (w, h) = match natural {
        Some((w, h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => return Box_ { x: area.x, y: area.y, cx: area.cx, cy: area.cy },
    };
    let scale = (area.cx as f64 / w)
        .min(area.cy as f64 / h)
        .min((EMU_PER_PX * MAX_IMAGE_UPSCALE) as f64);
    let cx = (w * scale).round() as i64;
    let cy = (h * scale).round() as i64;
    Box_ {
        x: area.x + (area.cx - cx) / 2,
        y: area.y + (area.cy - cy) / 2,
        cx,
        cy,
    }
}

/// 按缩放档缩小并保持居中（桌面端参数档位已收敛为唯一档 1.0，保留函数以对齐插件）。
fn scale_box_centered(box_: &Box_, zoom: f64) -> Box_ {
    if !(zoom > 0.0 && zoom < 1.0) {
        return Box_ { x: box_.x, y: box_.y, cx: box_.cx, cy: box_.cy };
    }
    let cx = ((box_.cx as f64) * zoom).round().max(1.0) as i64;
    let cy = ((box_.cy as f64) * zoom).round().max(1.0) as i64;
    Box_ {
        x: box_.x + (box_.cx - cx) / 2,
        y: box_.y + (box_.cy - cy) / 2,
        cx,
        cy,
    }
}

/// 从字节里识别图片扩展名与尺寸（PNG/JPEG/GIF/WebP，对齐插件 decodeDataUrl 的四类）。
fn image_meta(bytes: &[u8]) -> Result<(String, Option<(u32, u32)>), String> {
    let fmt = image::guess_format(bytes).map_err(|e| format!("无法识别截图格式: {}", e))?;
    let ext = match fmt {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::Gif => "gif",
        ImageFormat::WebP => "webp",
        _ => return Err(format!("不支持的截图格式: {:?}", fmt)),
    };
    let dims = image::load_from_memory(bytes)
        .ok()
        .map(|img| img.dimensions());
    Ok((ext.to_string(), dims))
}

// ==================== 目录网格 ====================

struct TocEntry {
    no: Option<i32>,
    title: String,
    head: bool,
}

fn toc_grid(n: usize) -> (usize, usize) {
    let cols = if n <= 7 { 1 } else if n <= 14 { 2 } else { 3 };
    let rows = ((n + cols - 1) / cols).max(1);
    (cols, rows)
}

/// 目录条目区：橙色圆形序号牌 + 标题（分组标签 = 橙色竖条 + 加粗橙字），条目多自动分栏。
fn toc_row_shapes(entries: &[TocEntry], first_id: i64) -> String {
    let n = entries.len();
    let (cols, rows) = toc_grid(n);
    let top = 1_360_000;
    let bottom = SLIDE_H - 640_000;
    let avail = (bottom - top).max(200_000);
    let row_h = (avail / rows as i64).min(820_000);
    let y0 = top + ((avail - row_h * rows as i64) / 2).max(0);
    let col_w = CONTENT_W / cols as i64;
    let title_size = if rows > 11 {
        1050
    } else if rows > 8 {
        1200
    } else if rows > 5 {
        1350
    } else {
        1500
    };
    let chip = (row_h - 140_000).min(460_000).max(180_000);
    let mut out = String::new();
    let mut id = first_id;
    for (i, e) in entries.iter().enumerate() {
        let x = MARGIN + (i as i64 / rows as i64) * col_w;
        let y = y0 + (i as i64 % rows as i64) * row_h;
        if e.head {
            out.push_str(&shape_box(
                id,
                "目录分组条",
                &Box_ { x, y: y + (row_h - chip * 8 / 10) / 2, cx: 60_000, cy: chip * 8 / 10 },
                ACCENT,
                Some(ShapeOpts { round: Some(0), ..Default::default() }),
            ));
            id += 1;
            out.push_str(&text_box_al(
                id,
                "目录分组",
                &Box_ { x: x + 200_000, y, cx: col_w - 260_000, cy: row_h },
                &paragraphs(&e.title, &RunOpts { size: title_size + 150, bold: true, color: Some(ACCENT.to_string()), ..Default::default() }),
                "l",
                "ctr",
            ));
            id += 1;
        } else {
            let no_text = e.no.unwrap_or(i as i32 + 1).to_string();
            out.push_str(&shape_box(
                id,
                "目录序号",
                &Box_ { x, y: y + (row_h - chip) / 2, cx: chip, cy: chip },
                ACCENT,
                Some(ShapeOpts {
                    round: Some(50_000),
                    text: Some(no_text),
                    text_color: Some("FFFFFF".to_string()),
                    text_size: Some((title_size as f64 * 0.86) as i64),
                    text_bold: Some(true),
                }),
            ));
            id += 1;
            out.push_str(&text_box_al(
                id,
                "目录条目",
                &Box_ { x: x + chip + 180_000, y, cx: (col_w - chip - 360_000).max(200_000), cy: row_h },
                &paragraphs(&e.title, &RunOpts { size: title_size, color: Some("1F2937".to_string()), ..Default::default() }),
                "l",
                "ctr",
            ));
            id += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(no: i32, title: &str) -> PptxStep {
        PptxStep {
            title: title.to_string(),
            title_html: String::new(),
            heading_level: None,
            description: String::new(),
            action_label: String::new(),
            url: String::new(),
            image: None,
            step_number: no,
        }
    }

    #[test]
    fn test_parse_inline_runs_basic() {
        let runs = parse_inline_runs("打开<b>设置</b>面板");
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "打开");
        assert!(runs[1].opts.bold);
        assert_eq!(runs[1].text, "设置");
        assert_eq!(runs[2].text, "面板");
    }

    #[test]
    fn test_parse_inline_runs_span_color() {
        let runs = parse_inline_runs("点<span style=\"color:#c0392b\">红色</span>按钮");
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].opts.color.as_deref(), Some("C0392B"));
    }

    #[test]
    fn test_xml_escape_controls() {
        assert_eq!(xml_escape("a<b>&\"'"), "a&lt;b&gt;&amp;&quot;&apos;");
        assert_eq!(xml_escape("bad\u{0}char"), "badchar");
    }

    #[test]
    fn test_fit_image_box_contain() {
        // 1000×600 → 版心 10911600×4818000：按高 8030 缩放
        let area = Box_ { x: MARGIN, y: 1_500_000, cx: CONTENT_W, cy: SLIDE_H - 2_040_000 };
        let b = fit_image_box(Some((1000, 600)), &area);
        assert!(b.cx <= area.cx && b.cy <= area.cy);
        assert_eq!((b.cx as f64 / 1000.0).round() as i64, (b.cy as f64 / 600.0).round() as i64);
    }

    #[test]
    fn test_fit_image_box_small_upscale_capped() {
        // 64px 小图放大不超过 2 倍
        let area = Box_ { x: MARGIN, y: 1_500_000, cx: CONTENT_W, cy: SLIDE_H - 2_040_000 };
        let b = fit_image_box(Some((64, 40)), &area);
        assert!(b.cx <= 64 * EMU_PER_PX * MAX_IMAGE_UPSCALE);
    }

    #[test]
    fn test_build_pptx_structure() {
        let steps = vec![
            step(1, "打开应用"),
            step(2, "点击设置"),
            PptxStep {
                title: "第一章".to_string(),
                title_html: String::new(),
                heading_level: Some(1),
                description: String::new(),
                action_label: String::new(),
                url: String::new(),
                image: None,
                step_number: 0,
            },
            step(3, "完成"),
        ];
        let entries = build_pptx_entries("测试教程", "2026-09-23", &steps, true).unwrap();
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"[Content_Types].xml"));
        assert!(names.contains(&"ppt/slides/slide1.xml")); // 封面
        assert!(names.contains(&"ppt/slides/slide2.xml")); // 目录
        assert!(names.contains(&"ppt/slides/slide3.xml")); // 步骤 1
        assert!(names.contains(&"ppt/slides/slide6.xml")); // 步骤 3（含章节页）
        assert!(names.contains(&"ppt/presentation.xml"));
        assert!(names.contains(&"ppt/theme/theme1.xml"));
        // presentation.xml 里应有 6 页
        let pres = entries.iter().find(|(n, _)| n == "ppt/presentation.xml").unwrap().1.clone();
        let pres_str = String::from_utf8(pres).unwrap();
        assert_eq!(pres_str.matches("<p:sldId ").count(), 6);
    }

    #[test]
    fn test_build_pptx_with_image_ext() {
        // 造一张 1×1 PNG 字节，验证 media 条目与 Content_Types 的 Default 出现
        let png = image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(png)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let steps = vec![PptxStep {
            title: "带图步骤".to_string(),
            title_html: String::new(),
            heading_level: None,
            description: String::new(),
            action_label: String::new(),
            url: "https://example.com/page".to_string(),
            image: Some(buf.into_inner()),
            step_number: 1,
        }];
        let entries = build_pptx_entries("带图教程", "", &steps, false).unwrap();
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.iter().any(|n| n.starts_with("ppt/media/image1.png")));
        assert!(names.iter().any(|n| n.starts_with("ppt/slides/_rels/")));
        let ct = entries
            .iter()
            .find(|(n, _)| n == "[Content_Types].xml")
            .unwrap()
            .1
            .clone();
        let ct_str = String::from_utf8(ct).unwrap();
        assert!(ct_str.contains("Default Extension=\"png\""));
    }

    #[test]
    fn test_build_pptx_zip_roundtrip() {
        // 端到端：build_pptx_entries → store 模式 ZIP 写盘 → 标准 zip 读取校验
        let png = image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 128, 0, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(png)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let steps = vec![PptxStep {
            title: "步骤一 <b>加粗</b> & 特殊字符</title>".to_string(),
            title_html: "<b>加粗</b>".to_string(),
            heading_level: None,
            description: "描述文本\n第二行".to_string(),
            action_label: "点击按钮".to_string(),
            url: "https://example.com/a?b=1&c=2".to_string(),
            image: Some(buf.into_inner()),
            step_number: 1,
        }];
        let entries = build_pptx_entries("端到端教程", "2026-09-23", &steps, true).unwrap();
        let out = std::env::temp_dir().join("flowio_pptx_roundtrip.pptx");
        super::super::commands::write_zip_store(&out, &entries).unwrap();

        // 用 zip 库重新读取，验证每个条目可解压且 XML 良构
        let f = std::fs::File::open(&out).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        assert!(zip.len() >= 8, "ZIP 条目数偏少: {}", zip.len());
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).unwrap();
            let name = entry.name().to_string();
            assert!(!name.is_empty());
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            if name.ends_with(".xml") || name.ends_with(".rels") {
                let s = String::from_utf8_lossy(&bytes);
                // 粗校验：< 数量 >= > 数量（未闭合标签会失衡）
                let opens = s.matches('<').count();
                let closes = s.matches('>').count();
                assert!(opens >= closes, "XML 标签失衡: {}", name);
            }
        }
        let _ = std::fs::remove_file(&out);
    }
}
