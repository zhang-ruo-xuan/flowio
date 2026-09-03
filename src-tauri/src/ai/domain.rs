//! 应用领域分类框架。
//!
//! 把「按应用/内容分域生成」从仅支持表格升级为通用框架：录制实时标题
//! 与 AI 步骤生成都按录制的应用领域（表格/浏览器/代码/终端/其他）走对应规则。
//!
//! - [`AppDomain`]：领域枚举（Spreadsheet/Browser/Code/Terminal/Other）。
//! - [`detect_domain`]：依据窗口标题自动推断领域。
//! - 每个领域的方法描述其专属行为：`full_image` / `skip_redaction` /
//!   `before_after` / `step_hint`。

/// 录制/生成的应用领域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppDomain {
    /// 表格/电子表格（WPS 表格 / Excel / Calc / Numbers …）
    Spreadsheet,
    /// 浏览器/网页（Chrome / Edge / Firefox …）
    Browser,
    /// 代码编辑器/IDE（VS Code / PyCharm / Vim …）
    Code,
    /// 终端/命令行（终端 / cmd / PowerShell / Terminal …）
    Terminal,
    /// 未识别（或无需特殊规则）
    Other,
    /// 录制指定软件（target_app）：「整软件全图」模式。full_image / skip_redaction /
    /// before_after 三项全开（整图原样送视觉模型、前后对比），但 step hint 不再硬塞
    /// 「表格软件」，改由每步按真实窗口标题检测出的领域给出（识别不到则中性）。
    TargetApp,
}

impl AppDomain {
    /// 稳定字符串 key（用于前端下拉、持久化、日志）。
    pub fn key(&self) -> &'static str {
        match self {
            AppDomain::Spreadsheet => "spreadsheet",
            AppDomain::Browser => "browser",
            AppDomain::Code => "code",
            AppDomain::Terminal => "terminal",
            AppDomain::Other => "other",
            AppDomain::TargetApp => "target_app",
        }
    }

    /// 中文展示名（用于 UI 与日志）。
    pub fn label(&self) -> &'static str {
        match self {
            AppDomain::Spreadsheet => "表格软件",
            AppDomain::Browser => "浏览器",
            AppDomain::Code => "代码编辑器",
            AppDomain::Terminal => "终端",
            AppDomain::Other => "其他",
            AppDomain::TargetApp => "指定软件",
        }
    }

    /// 从前端传来的字符串 key 解析回枚举（非法值回退 Other）。
    pub fn parse(s: &str) -> AppDomain {
        match s {
            "spreadsheet" => AppDomain::Spreadsheet,
            "browser" => AppDomain::Browser,
            "code" => AppDomain::Code,
            "terminal" => AppDomain::Terminal,
            "target_app" => AppDomain::TargetApp,
            _ => AppDomain::Other,
        }
    }

    /// 该领域是否需要发送全图（而非聚焦裁剪图）给视觉模型。
    /// 仅表格软件跨整窗布局，需全图才能看清单元格/公式栏。
    pub fn full_image(&self) -> bool {
        matches!(self, AppDomain::Spreadsheet | AppDomain::TargetApp)
    }

    /// 该领域是否跳过 OCR 脱敏。
    /// 表格内容（列标/函数名/数据取值）需保持清晰，跳过脱敏以避免被糊掉。
    /// ⚠️ 隐私权衡：表格内容会原样送往视觉模型，仅建议用于非敏感演示数据。
    pub fn skip_redaction(&self) -> bool {
        matches!(self, AppDomain::Spreadsheet | AppDomain::TargetApp)
    }

    /// 该领域是否在有 before/after 双图时提示模型对比前后状态
    /// （如「向下填充到 B31」「整列复制」仅在前后差异中显现）。
    pub fn before_after(&self) -> bool {
        matches!(self, AppDomain::Spreadsheet | AppDomain::TargetApp)
    }

    /// 注入 step prompt 的【领域】强化指令（system prompt 之外的逐步骤补充）。
    /// `Other` 不注入。表格保留「【表格软件】」标签以兼容既有测试。
    pub fn step_hint(&self) -> String {
        match self {
            AppDomain::Spreadsheet => {
                "【表格软件】此步骤疑似电子表格操作，请严格按「表格/电子表格软件专项规则」撰写：标题必须含精确单元格引用（如 B22），涉及函数时写明函数名与入口，描述补全关键参数取值与填充范围。\n".to_string()
            }
            AppDomain::Browser => {
                "【浏览器/网页】此步骤发生在浏览器中。标题聚焦网页内的操作对象（按钮/链接/输入框/菜单），不要写浏览器自身控件（地址栏/标签页/书签栏）；若窗口标题形如「网页名 - 浏览器」，请聚焦网页名所代表的页面操作。\n".to_string()
            }
            AppDomain::Code => {
                "【代码编辑器】此步骤在代码编辑器/IDE 中。标题/描述写明操作对象（文件/函数/行号/侧边栏项/调试按钮等）与预期结果（如「在第 12 行打断点」「折叠 main 函数」），使用代码与界面原文术语，不要臆造不存在的符号。\n".to_string()
            }
            AppDomain::Terminal => {
                "【终端/命令行】此步骤在终端/命令行中。标题/描述聚焦具体命令与输出（如「运行 npm install」「执行 git status 查看分支」）；若无可辨识命令，则描述界面操作（如「点击终端标签切换到 logs」）。\n".to_string()
            }
            AppDomain::TargetApp => String::new(),
            AppDomain::Other => String::new(),
        }
    }
}

impl std::fmt::Display for AppDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// 已知浏览器窗口标题的尾部应用签名（小写），用于精准剥离取网页名。
const BROWSER_SUFFIXES: &[&str] = &[
    " - google chrome",
    " - chrome",
    " - chromium",
    " - microsoft edge",
    " - edge",
    " - firefox",
    " - 火狐",
    " - firefox 开发者版",
    " - safari",
    " - opera",
    " - brave",
];

/// 强签名后缀 → 领域（优先用「 - 应用名」结尾判断，避免关键词误伤）。
const APP_SUFFIXES: &[(&str, AppDomain)] = &[
    (" - google chrome", AppDomain::Browser),
    (" - chrome", AppDomain::Browser),
    (" - chromium", AppDomain::Browser),
    (" - microsoft edge", AppDomain::Browser),
    (" - edge", AppDomain::Browser),
    (" - firefox", AppDomain::Browser),
    (" - 火狐", AppDomain::Browser),
    (" - safari", AppDomain::Browser),
    (" - opera", AppDomain::Browser),
    (" - brave", AppDomain::Browser),
    (" - visual studio code", AppDomain::Code),
    (" - vscode", AppDomain::Code),
    (" - vs code", AppDomain::Code),
    (" - pycharm", AppDomain::Code),
    (" - webstorm", AppDomain::Code),
    (" - intellij", AppDomain::Code),
    (" - idea", AppDomain::Code),
    (" - android studio", AppDomain::Code),
    (" - xcode", AppDomain::Code),
    (" - sublime text", AppDomain::Code),
    (" - notepad++", AppDomain::Code),
    (" - wps 表格", AppDomain::Spreadsheet),
    (" - wps表格", AppDomain::Spreadsheet),
    (" - excel", AppDomain::Spreadsheet),
    (" - calc", AppDomain::Spreadsheet),
    (" - numbers", AppDomain::Spreadsheet),
    (" - 终端", AppDomain::Terminal),
    (" - terminal", AppDomain::Terminal),
    (" - windows terminal", AppDomain::Terminal),
    (" - cmd", AppDomain::Terminal),
    (" - 命令提示符", AppDomain::Terminal),
    (" - powershell", AppDomain::Terminal),
];

/// 终端/命令行的关键词（放在最前，避免被「编辑器」误伤）。
const TERMINAL_KEYWORDS: &[&str] = &[
    "终端",
    "terminal",
    "cmd",
    "命令提示符",
    "powershell",
    "windows terminal",
    "iterm",
    "konsole",
    "tmux",
    "bash",
    "zsh",
];

/// 表格/电子表格的关键词（保留既有关键词集合，保证表格识别不退化）。
const SPREADSHEET_KEYWORDS: &[&str] = &[
    "wps 表格",
    "wps表格",
    "excel",
    "calc",
    "numbers",
    "电子表格",
    "单元格",
    "工作表",
    ".xlsx",
    ".xls",
    ".et",
    ".ods",
    "spreadsheet",
    "sheet",
];

/// 代码编辑器/IDE 的关键词。
const CODE_KEYWORDS: &[&str] = &[
    "vscode",
    "visual studio code",
    "visual studio",
    "intellij",
    "idea",
    "pycharm",
    "webstorm",
    "clion",
    "goland",
    "android studio",
    "xcode",
    "code - ",
    "编辑器",
    "sublime",
    "sublime text",
    "vim",
    "neovim",
    "notepad++",
    ".rs",
    ".go",
    ".py",
    ".js",
    ".ts",
    ".java",
    ".c",
    ".cpp",
    ".cs",
    ".html",
    ".css",
    ".json",
    "main.rs",
    "main.go",
    "main.py",
];

/// 浏览器的关键词。
const BROWSER_KEYWORDS: &[&str] = &[
    "google chrome",
    "chrome",
    "chromium",
    "microsoft edge",
    "edge",
    "firefox",
    "火狐",
    "浏览器",
    "网页",
    "百度",
    "google",
    "bing",
    "safari",
    "opera",
    "brave",
];

/// 根据窗口标题推断应用领域。
///
/// 先用强签名后缀（「 - 应用名」结尾）判断，命中即返回对应领域；
/// 未命中再按关键词优先级回退：终端 → 表格 → 代码 → 浏览器 → 其他。
pub fn detect_domain(window_title: &str) -> AppDomain {
    let t = window_title.to_lowercase();

    // 1) 强签名后缀优先（最精准，避免「Python 教程 - VS Code」被浏览器关键词命中）
    for (suf, dom) in APP_SUFFIXES {
        if t.ends_with(suf) {
            return *dom;
        }
    }

    // 2) 关键词回退，按误伤代价排序
    if TERMINAL_KEYWORDS.iter().any(|k| t.contains(k)) {
        return AppDomain::Terminal;
    }
    if SPREADSHEET_KEYWORDS.iter().any(|k| t.contains(k)) {
        return AppDomain::Spreadsheet;
    }
    if CODE_KEYWORDS.iter().any(|k| t.contains(k)) {
        return AppDomain::Code;
    }
    if BROWSER_KEYWORDS.iter().any(|k| t.contains(k)) {
        return AppDomain::Browser;
    }

    AppDomain::Other
}

/// 浏览器窗口标题形如「网页名 - Google Chrome」「网页名 - Microsoft Edge」，
/// 去掉尾部浏览器签名只保留网页名，用于更干净的实时标题。
/// 无签名（如纯「Chrome」）原样返回。
pub fn browser_page_name(window_title: &str) -> String {
    let lower = window_title.to_lowercase();
    for suf in BROWSER_SUFFIXES {
        if let Some(idx) = lower.rfind(suf) {
            // 后缀为 ASCII，idx 是合法字符边界，前导部分可直接切片
            let page = window_title[..idx].trim_end();
            if !page.is_empty() {
                return page.to_string();
            }
        }
    }
    window_title.to_string()
}

/// 兼容旧调用点：判断窗口标题是否疑似表格软件。
pub(crate) fn is_spreadsheet_window(title: &str) -> bool {
    detect_domain(title) == AppDomain::Spreadsheet
}

/// 根据 target_app 进程名生成「软件身份 + UI 先验」文本，注入 AI prompt。
///
/// 让 AI 知道正在分析哪款软件的截图，并给出该软件常见 UI 结构的先验知识，
/// 帮助模型准确识别对话框字段名、面板名称等（解决 VLOOKUP 对话框「查找值」
/// 与「数据表」认错等问题）。
pub fn app_context(process_name: &str) -> String {
    let p = process_name.to_lowercase();
    if p.contains("wps") || p.contains("et") || p.contains("wpsoffice") {
        "录制目标软件：WPS Office（电子表格/文字/演示）。\n\
         WPS 表格常见对话框字段请严格按界面原文称呼：\n\
         - VLOOKUP 函数：查找值 / 数据表 / 列序数 / 匹配条件\n\
         - IF 函数：逻辑条件 / 真值 / 假值\n\
         - 插入函数：函数搜索框 / 类别列表 / 选中函数说明\n\
         - 单元格格式：数字 / 对齐 / 字体 / 边框 / 填充 / 保护\n\
         - 页面设置：页面 / 页边距 / 页眉页脚 / 工作表\n\
         请根据截图中的实际焦点位置判断用户操作的是哪个字段/按钮，不要猜测。\n"
            .to_string()
    } else if p.contains("excel") {
        "录制目标软件：Microsoft Excel。\n\
         Excel 常见对话框字段请严格按界面原文称呼：\n\
         - VLOOKUP 函数：Lookup_value / Table_array / Col_index_num / Range_lookup\n\
         - IF 函数：Logical_test / Value_if_true / Value_if_false\n\
         - 单元格格式：Number / Alignment / Font / Border / Fill / Protection\n\
         请根据截图中的实际焦点位置判断用户操作的是哪个字段/按钮，不要猜测。\n"
            .to_string()
    } else if p.contains("code") || p.contains("vscode") {
        "录制目标软件：Visual Studio Code。\n\
         VS Code 常见 UI 区域：侧边栏（资源管理器/搜索/源代码管理/调试/扩展）、\n\
         编辑器分组、终端面板、输出面板、调试工具栏、状态栏。\n\
         请根据截图中的实际焦点位置判断操作对象，使用界面原文术语。\n"
            .to_string()
    } else if p.contains("chrome") || p.contains("msedge") || p.contains("edge") {
        "录制目标软件：浏览器（Chrome / Edge）。\n\
         浏览器常见 UI 区域：地址栏、标签页栏、书签栏、开发者工具（Elements/Console/Network）、\n\
         扩展弹出窗、下载栏。\n\
         网页内的操作请聚焦网页内容本身，不要把浏览器控件误认为网页元素。\n"
            .to_string()
    } else if !p.is_empty() {
        format!("录制目标软件：{}。\n", process_name)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod domain_tests {
    use super::*;

    #[test]
    fn detect_spreadsheet_by_keyword() {
        assert_eq!(detect_domain("销售表.xlsx - WPS 表格"), AppDomain::Spreadsheet);
        assert_eq!(detect_domain("Excel - 工作簿1"), AppDomain::Spreadsheet);
        assert_eq!(detect_domain("工资表.et"), AppDomain::Spreadsheet);
        assert_eq!(detect_domain("Sheet1 - LibreOffice Calc"), AppDomain::Spreadsheet);
    }

    #[test]
    fn detect_browser_by_suffix_and_keyword() {
        assert_eq!(detect_domain("百度 - Google Chrome"), AppDomain::Browser);
        assert_eq!(detect_domain("GitHub - Microsoft Edge"), AppDomain::Browser);
        assert_eq!(detect_domain("MDN - Firefox"), AppDomain::Browser);
        assert_eq!(detect_domain("某网页 - 火狐"), AppDomain::Browser);
    }

    #[test]
    fn detect_code_by_suffix_and_keyword() {
        assert_eq!(detect_domain("main.rs - Visual Studio Code"), AppDomain::Code);
        assert_eq!(detect_domain("app.py - PyCharm"), AppDomain::Code);
        assert_eq!(detect_domain("index.html - VS Code"), AppDomain::Code);
        assert_eq!(detect_domain("leetcode - 代码编辑器"), AppDomain::Code);
    }

    #[test]
    fn detect_terminal_by_keyword() {
        assert_eq!(detect_domain("终端"), AppDomain::Terminal);
        assert_eq!(detect_domain("C:\\> cmd"), AppDomain::Terminal);
        assert_eq!(detect_domain("PowerShell 7"), AppDomain::Terminal);
        assert_eq!(detect_domain("logs - Windows Terminal"), AppDomain::Terminal);
    }

    #[test]
    fn detect_other_for_plain_apps() {
        assert_eq!(detect_domain("记事本"), AppDomain::Other);
        assert_eq!(detect_domain("微信"), AppDomain::Other);
        assert_eq!(detect_domain(""), AppDomain::Other);
    }

    #[test]
    fn suffix_beats_generic_keyword() {
        // 「Python 教程 - VS Code」含 chrome 无关，但含「vs code」后缀 → Code，非 Browser
        assert_eq!(detect_domain("Python 教程 - VS Code"), AppDomain::Code);
        // 「index.html - VS Code」含 .html 但后缀是 vs code → Code
        assert_eq!(detect_domain("index.html - VS Code"), AppDomain::Code);
        // 「销售表 - WPS 表格」后缀命中表格，不被浏览器关键词（若有）误伤
        assert_eq!(detect_domain("百度统计表 - WPS 表格"), AppDomain::Spreadsheet);
    }

    #[test]
    fn browser_page_name_strips_suffix() {
        assert_eq!(browser_page_name("百度 - Google Chrome"), "百度");
        assert_eq!(browser_page_name("GitHub - Microsoft Edge"), "GitHub");
        assert_eq!(browser_page_name("MDN - Firefox"), "MDN");
        // 纯浏览器名无网页名 → 原样返回
        assert_eq!(browser_page_name("Chrome"), "Chrome");
        // 无空格后缀不变
        assert_eq!(browser_page_name("我的网页"), "我的网页");
    }

    #[test]
    fn domain_key_parse_roundtrip() {
        for d in [
            AppDomain::Spreadsheet,
            AppDomain::Browser,
            AppDomain::Code,
            AppDomain::Terminal,
            AppDomain::Other,
            AppDomain::TargetApp,
        ] {
            assert_eq!(AppDomain::parse(d.key()), d);
        }
        assert_eq!(AppDomain::parse("unknown"), AppDomain::Other);
        assert_eq!(AppDomain::parse(""), AppDomain::Other);
    }

    #[test]
    fn domain_behavior_flags() {
        assert!(AppDomain::Spreadsheet.full_image());
        assert!(AppDomain::Spreadsheet.skip_redaction());
        assert!(AppDomain::Spreadsheet.before_after());
        assert!(AppDomain::TargetApp.full_image());
        assert!(AppDomain::TargetApp.skip_redaction());
        assert!(AppDomain::TargetApp.before_after());
        assert!(AppDomain::TargetApp.step_hint().is_empty());
        assert!(!AppDomain::Browser.full_image());
        assert!(!AppDomain::Browser.skip_redaction());
        assert!(!AppDomain::Other.full_image());
    }

    #[test]
    fn step_hint_contains_domain_marker() {
        assert!(AppDomain::Spreadsheet.step_hint().contains("【表格软件】"));
        assert!(AppDomain::Browser.step_hint().contains("【浏览器/网页】"));
        assert!(AppDomain::Code.step_hint().contains("【代码编辑器】"));
        assert!(AppDomain::Terminal.step_hint().contains("【终端/命令行】"));
        assert!(AppDomain::Other.step_hint().is_empty());
    }

    #[test]
    fn is_spreadsheet_window_compat() {
        assert!(is_spreadsheet_window("a.xlsx - WPS 表格"));
        assert!(!is_spreadsheet_window("记事本"));
        assert!(!is_spreadsheet_window("百度 - Chrome"));
    }
}
