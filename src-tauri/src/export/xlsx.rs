//! XLSX（.xlsx）导出 —— 零额外依赖的 OOXML 生成器（对齐插件 `src/lib/xlsx.ts`）。
//!
//! 产物为真 OOXML 包：Excel / WPS / 飞书表格 / Numbers 均可直接打开，
//! 不像 CSV 在长文本、逗号、换行上翻车（CSV 入口保留，Excel 是更好的表格默认）。
//!
//! 与插件同一套视觉语言：
//!   · 单元格用 **inlineStr**（内联字符串），不引 sharedStrings part；
//!   · 表头「品牌橙底 + 白字加粗」；
//!   · 冻结首行（表头常驻）。
//!
//! 列与 CSV 导出同口径（桌面端语义）：序号 / 类型 / 标题 / 说明 / URL / 敏感。
//! 文档块（大标题/摘要/简介）不占编号、序号列留空；敏感列暂恒为「否」——
//! 桌面端尚无步骤级敏感标记（与 CSV 一致，待字段落地后回填）。

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CT_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

/// 品牌橙（与 PPT / HTML 导出同一套视觉语言）。
const ACCENT: &str = "FF6B35";

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// 0→A, 25→Z, 26→AA … 标准列字母编号。
fn col_letter(idx: usize) -> String {
    let mut s = String::new();
    let mut n = idx as isize;
    while n >= 0 {
        s.insert(0, char::from_u32(65 + (n % 26) as u32).unwrap());
        n = n / 26 - 1;
    }
    s
}

fn styles_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="{MAIN_NS}"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><color rgb="FFFFFFFF"/><name val="Calibri"/></font></fonts><fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="{ACCENT}"/><bgColor indexed="64"/></patternFill></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="2" borderId="0" xfId="0" applyFont="1" applyFill="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>"#,
    )
}

/// 单个工作表 XML：首行表头（橙底白字），冻结首行。
fn sheet_xml(rows: &[Vec<(String, bool)>]) -> String {
    let mut rows_xml = String::new();
    for (ri, row) in rows.iter().enumerate() {
        let r = ri + 1;
        let mut cells = String::new();
        for (ci, (val, header)) in row.iter().enumerate() {
            let ref_ = format!("{}{}", col_letter(ci), r);
            let style = if *header { r#" s="1""# } else { "" };
            let t = xml_escape(val);
            cells.push_str(&format!(
                r#"<c r="{ref_}"{style} t="inlineStr"><is><t xml:space="preserve">{t}</t></is></c>"#
            ));
        }
        rows_xml.push_str(&format!(r#"<row r="{r}">{cells}</row>"#));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="{MAIN_NS}" xmlns:r="{REL_NS}"><sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews><sheetData>{rows_xml}</sheetData></worksheet>"#,
    )
}

/// 把录制整理成 .xlsx 字节（纯函数：同样的输入必得同样的字节，便于回归比对）。
/// 输入为「单工作表」的行数据：每行 `(单元格文本, 是否表头)`。
/// 列与 CSV 导出同口径（序号 / 类型 / 标题 / 说明 / URL / 敏感），
/// 文档块是否占编号等语义由调用方（commands.rs）装配时决定。
pub fn build_xlsx_bytes(sheet_name: &str, rows: &[Vec<(String, bool)>]) -> Result<Vec<u8>, String> {
    let enc = |s: &str| s.as_bytes().to_vec();
    let worksheet_name = "xl/worksheets/sheet1.xml";

    let content_types = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="{CT_NS}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
    );
    let root_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="{PKG_NS}"><Relationship Id="rId1" Type="{REL_NS}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
    );
    let workbook = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="{MAIN_NS}" xmlns:r="{REL_NS}"><sheets><sheet name="{}" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        xml_escape(sheet_name),
    );
    let workbook_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="{PKG_NS}"><Relationship Id="rId1" Type="{REL_NS}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{REL_NS}/styles" Target="styles.xml"/></Relationships>"#,
    );
    let worksheet = sheet_xml(rows);

    let entries: Vec<(&str, Vec<u8>)> = vec![
        ("[Content_Types].xml", enc(&content_types)),
        ("_rels/.rels", enc(&root_rels)),
        ("xl/workbook.xml", enc(&workbook)),
        ("xl/_rels/workbook.xml.rels", enc(&workbook_rels)),
        ("xl/styles.xml", enc(&styles_xml())),
        (worksheet_name, enc(&worksheet)),
    ];

    // zip crate（deflate）打包成真 .xlsx 容器
    let cursor = Cursor::new(Vec::new());
    let mut zw = ZipWriter::new(cursor);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in entries {
        zw.start_file(name.to_string(), opts)
            .map_err(|e| format!("xlsx: start_file {} failed: {}", name, e))?;
        zw.write_all(&data)
            .map_err(|e| format!("xlsx: write {} failed: {}", name, e))?;
    }
    let cursor = zw.finish().map_err(|e| format!("xlsx: zip finish failed: {}", e))?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_row() -> Vec<(String, bool)> {
        vec![
            ("序号".to_string(), true),
            ("类型".to_string(), true),
            ("标题".to_string(), true),
            ("说明".to_string(), true),
            ("URL".to_string(), true),
            ("敏感".to_string(), true),
        ]
    }

    fn data_row(no: &str, kind: &str, title: &str, desc: &str, url: &str) -> Vec<(String, bool)> {
        vec![
            (no.to_string(), false),
            (kind.to_string(), false),
            (title.to_string(), false),
            (desc.to_string(), false),
            (url.to_string(), false),
            ("否".to_string(), false),
        ]
    }

    #[test]
    fn xlsx_bytes_are_real_ooxml_zip() {
        let rows = vec![
            header_row(),
            data_row("1", "click", "打开应用", "点击图标", "https://example.com/a"),
            data_row("2", "type", "输入关键词", "含 <尖括号> 与 & 符号", ""),
        ];
        let bytes = build_xlsx_bytes("步骤清单", &rows).expect("build_xlsx_bytes ok");
        // ZIP magic
        assert_eq!(&bytes[..2], b"PK");

        // 解压并核对 part 列表与内容
        let reader = std::io::Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader).expect("zip open ok");
        let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
        for expected in ["[Content_Types].xml", "_rels/.rels", "xl/workbook.xml", "xl/_rels/workbook.xml.rels", "xl/styles.xml", "xl/worksheets/sheet1.xml"] {
            assert!(names.iter().any(|n| n == expected), "missing part {expected}: {names:?}");
        }

        let mut sheet_xml_str = {
            let mut sheet = zip.by_name("xl/worksheets/sheet1.xml").expect("sheet1 exists");
            let mut xml = String::new();
            use std::io::Read;
            sheet.read_to_string(&mut xml).unwrap();
            xml
        };
        // 冻结首行
        assert!(sheet_xml_str.contains(r#"<pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/>"#), "frozen pane missing");
        // 表头 6 列（inlineStr）
        for h in ["序号", "类型", "标题", "说明", "URL", "敏感"] {
            assert!(sheet_xml_str.contains(h), "header cell {h} missing");
        }
        // 表头样式 s="1"
        assert!(sheet_xml_str.contains(r#"<c r="A1" s="1""#), "header style missing");
        // 数据转义：尖括号/& 已转义
        assert!(sheet_xml_str.contains("&lt;尖括号&gt; 与 &amp; 符号"), "xml escape broken");
        // 两行数据
        assert!(sheet_xml_str.contains(r#"<row r="2">"#) && sheet_xml_str.contains("打开应用"), "row2 missing");
        assert!(sheet_xml_str.contains(r#"<row r="3">"#) && sheet_xml_str.contains("输入关键词"), "row3 missing");

        let mut wbxml = {
            let mut wb = zip.by_name("xl/workbook.xml").unwrap();
            let mut wbxml = String::new();
            use std::io::Read;
            wb.read_to_string(&mut wbxml).unwrap();
            wbxml
        };
        assert!(wbxml.contains(r#"<sheet name="步骤清单""#), "sheet name missing");
    }

    #[test]
    fn xlsx_handles_empty_number_cell() {
        // 调用方（commands.rs）会把文档块序号留空 → 这里验证空单元格可正常写入
        let rows = vec![
            header_row(),
            data_row("", "标题", "大标题", "", ""),
            data_row("1", "click", "真步骤", "", ""),
        ];
        let bytes = build_xlsx_bytes("步骤清单", &rows).unwrap();
        let reader = std::io::Cursor::new(bytes);
        let mut zip = zip::ZipArchive::new(reader).unwrap();
        let xml = {
            let mut sheet = zip.by_name("xl/worksheets/sheet1.xml").unwrap();
            let mut xml = String::new();
            use std::io::Read;
            sheet.read_to_string(&mut xml).unwrap();
            xml
        };
        // 大标题行 A2 序号为空
        assert!(xml.contains(r#"<c r="A2" t="inlineStr"><is><t xml:space="preserve"></t></is></c>"#), "empty number cell broken");
        // 真步骤序号为 1
        assert!(xml.contains(r#"<c r="A3" t="inlineStr"><is><t xml:space="preserve">1</t></is></c>"#), "real step numbering broken");
    }
}
