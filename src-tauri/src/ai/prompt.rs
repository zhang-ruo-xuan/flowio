use crate::ai::domain::{detect_domain, AppDomain};
use crate::ai::types::StepInput;

/// Build the system prompt defining the AI role as a Scribe-style
/// Chinese operation guide expert.
pub fn build_system_prompt() -> String {
    r#"你是 Scribe 风格的操作指南撰写专家。擅长将软件操作截图转化为简洁、专业的中文分步教程。

能力：
1. 识别 UI 元素（按钮/菜单/对话框/输入框/选项卡/图标），使用规范中文术语
2. 从截图的界面状态推断用户完成了什么操作，用动词开头撰写标题
3. 描述时强调操作对象和预期结果，不啰嗦

原则：
- 每个步骤标题 2-20 字，必须包含动词（点击/输入/选择/拖拽/勾选/滚动/切换）
- 描述 10-200 字，包含操作对象、位置、预期结果
- 按钮名称用原文，界面文字用原文，其他描述用中文
- 不要猜测截图中不存在的元素
- 不要描述不相关的界面细节（颜色/形状等，除非是操作的关键标识）

表格/电子表格软件专项规则（当截图出现单元格网格、列标 A/B/C、行号、公式栏、函数对话框时适用，如 WPS 表格、Excel、LibreOffice Calc、Numbers 等）：
- 标题必须包含具体单元格引用（如「单击 B22」「双击 B22 填充柄」），严禁写「点击单元格」这类笼统描述
- 涉及函数时，标题或描述写明函数名（如 VLOOKUP、SUM、IF）及所在入口（如「公式」选项卡 → 「插入函数」）
- 描述中写明关键参数与取值（如 查找值=A22、数据表=$A$2:$E$21、列序=2、匹配条件=FALSE）
- 双击单元格右下角填充柄并下拖到目标行时，标题用「向下填充到 B31」等明确范围
- 窗口标题含工作表/工作簿名（如「销售表」）时，描述可引用该名称
- 排序/筛选：标题写明依据列与方向，如「按销售额降序排序」「筛选出发货状态为已发货的行」
- 插入/删除行或列：写明位置，如「在第 3 行上方插入行」「删除 C 列」
- 复制粘贴区域：写明源区域与目标，如「将 A2:A10 复制到 C2」「填充序列至 B31」
- 设置单元格格式：写明格式类型，如「将 B 列设为货币格式」「把 A 列设为日期格式」
- 在单元格输入公式（以 = 开头）时，标题用「输入公式」并尽量写出完整公式，如「输入公式 =VLOOKUP(A22,$A$2:$E$21,2,FALSE)」；输入普通文本/数字用「输入」，如「在 B22 输入 销售表」
- 插入图表/数据透视表：写明图表类型与数据来源，如「基于 A1:D10 插入柱状图」「对销售明细插入数据透视表」
- 查找替换：写明查找内容与替换值，如「查找 待发货 替换为 已发货」
- 合并单元格/跨列居中：写明范围，如「合并 A1:C1 并居中」
- 条件格式/数据验证：写明规则，如「对销售额列设置大于 1000 高亮」「限制 B 列只能输入日期」
- 公式或引用跨工作表时，标题/描述注明源工作表名，如「输入公式 =Sheet2!A1*2」「引用 Sheet2 的 A1」；跨工作簿时注明工作簿名（如 =[Book2.xlsx]Sheet1!A1）
- 窗口标题含「- 工作表名」（如 销售表 - Sheet2）时，说明当前操作发生在该工作表上，描述可注明，如「在 Sheet2 的 B22 输入公式」

截图说明：
你会收到一张截图：
- （after）操作后的截图：以点击坐标为中心裁剪的 400×400 局部区域（无有效坐标时为全图），用于判断用户在该界面上的操作。
请根据该截图判断用户完成了什么操作。

输出要求：
严格按以下 JSON 格式输出，不要加任何解释或说明文字：
{"title": "2-20字中文标题，动词开头", "description": "10-200字中文描述", "action_type": "click/input/select/scroll/navigate/other", "tip": "可选的操作提示或注意事项"}
"#
    .to_string()
}

/// Build a single-step prompt for the vision model.
///
/// Includes step number, title, description (from recording metadata),
/// and instructs the model to produce a structured JSON output.
/// When `target_app` is provided, injects app identity + UI context so the AI
/// knows which software it's analyzing and can correctly identify dialog fields.
pub fn build_step_prompt(step: &StepInput, step_number: usize, force_domain: Option<AppDomain>, target_app: Option<&str>) -> String {
    let context_line = if !step.window_title.is_empty() {
        format!("操作窗口：{}\n", step.window_title)
    } else {
        String::new()
    };

    let action_hint = if !step.action_type.is_empty() {
        format!("录制动作类型：{}\n", step.action_type)
    } else {
        String::new()
    };

    // 自动模式按窗口标题推断领域，手动/指定软件模式强制使用指定领域。
    let domain = force_domain.unwrap_or_else(|| detect_domain(&step.window_title));
    // 录制指定软件（TargetApp）不硬塞「表格软件」提示，改按该步真实窗口标题
    // 检测出的领域给对应提示（VS Code→代码、Chrome→浏览器，识别不到则中性）。
    let spreadsheet_hint = match domain {
        AppDomain::TargetApp => detect_domain(&step.window_title).step_hint(),
        _ => domain.step_hint(),
    };

    // 录制目标软件身份 + UI 先验（解决 VLOOKUP 对话框字段认错等问题）
    let app_ctx = target_app
        .map(|name| crate::ai::domain::app_context(name))
        .unwrap_or_default();

    format!(
        r#"请为以下第 {step_number} 个操作步骤生成中文标题、描述、操作类型和提示。

{app_ctx}{context}{action}{spreadsheet_hint}原始标题：{original_title}
原始描述：{original_desc}

请根据操作截图推断用户实际执行的操作，输出 JSON：
{{"title": "...", "description": "...", "action_type": "click/input/select/scroll/navigate/other", "tip": "..."}}

要求：
- title：2-20 字，动词开头，如"点击登录按钮"
- description：10-200 字，描述操作对象、位置和预期结果
- action_type：click（点击）/ input（输入）/ select（选择）/ scroll（滚动）/ navigate（导航）/ other（其他）
- tip：操作提示或注意事项，如无则为空字符串

【最重要的规则】原始录制动作类型和原始标题来自用户录制的真实操作，你必须以它们为基准。你的任务是对标题和描述做语言润色（如统一措辞、补充更清晰的界面描述），严禁改变操作对象或动作含义。即使截图看起来像是在做别的事，也必须以原始录制动作为准。

特别注意：如果 after 截图显示的是一个完全不同的窗口或桌面（而非操作界面本身），说明用户的动作可能是关闭窗口、切换标签页、最小化窗口等窗口管理操作。此时必须根据录制的原始动作类型和标题来描述操作，严禁把"关闭浏览器"描述成网页内的操作（如投稿、点赞等）。
"#,
        step_number = step_number,
        app_ctx = app_ctx,
        context = context_line,
        action = action_hint,
        original_title = step.title,
        original_desc = step.description,
    )
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use crate::ai::domain::AppDomain;
    use crate::ai::types::StepInput;

    fn input(title: &str, desc: &str, at: &str, wt: &str) -> StepInput {
        StepInput {
            title: title.into(),
            screenshot_base64: String::new(),
            before_screenshot_base64: String::new(),
            description: desc.into(),
            click_x: None,
            click_y: None,
            crop_w: None,
            crop_h: None,
            action_type: at.into(),
            window_title: wt.into(),
        }
    }

    #[test]
    fn build_system_prompt_is_non_empty_and_contains_json_requirement() {
        let p = build_system_prompt();
        assert!(!p.is_empty());
        assert!(p.contains("JSON"));
        assert!(p.contains("title"));
        assert!(p.contains("description"));
    }

    #[test]
    fn build_system_prompt_includes_spreadsheet_rules() {
        let p = build_system_prompt();
        assert!(p.contains("表格"), "system prompt 应包含表格软件专项规则");
        assert!(p.contains("单元格"), "应提及单元格引用");
        assert!(p.contains("VLOOKUP") || p.contains("函数名"), "应提及函数名识别");
        assert!(
            p.contains("排序") || p.contains("筛选") || p.contains("插入") || p.contains("填充"),
            "应覆盖常见表格操作（排序/筛选/插入行列/填充）"
        );
    }

    #[test]
    fn build_system_prompt_includes_more_spreadsheet_operations() {
        let p = build_system_prompt();
        assert!(p.contains("数据透视表"), "应覆盖图表/数据透视表操作");
        assert!(p.contains("查找替换"), "应覆盖查找替换操作");
        assert!(p.contains("合并单元格"), "应覆盖合并单元格操作");
        assert!(p.contains("条件格式") || p.contains("数据验证"), "应覆盖条件格式/数据验证");
        assert!(
            p.contains("跨工作表") || p.contains("Sheet2") || p.contains("工作簿"),
            "应覆盖跨工作表/跨工作簿引用"
        );
    }

    #[test]
    fn build_step_prompt_includes_metadata() {
        let p = build_step_prompt(&input("点击保存", "描述", "click", "记事本"), 1, None, None);
        assert!(p.contains("第 1 个操作步骤"));
        assert!(p.contains("操作窗口：记事本"));
        assert!(p.contains("录制动作类型：click"));
        assert!(p.contains("原始标题：点击保存"));
        assert!(p.contains("原始描述：描述"));
    }

    #[test]
    fn build_step_prompt_injects_spreadsheet_hint_for_spreadsheet_window() {
        let p = build_step_prompt(
            &input("单击", "", "click", "销售表.xlsx - WPS 表格"),
            2,
            None,
            None,
        );
        assert!(p.contains("【表格软件】"), "表格窗口应注入表格软件强化指令");
        assert!(p.contains("B22") || p.contains("单元格"), "应提示精确单元格引用");
    }

    #[test]
    fn build_step_prompt_omits_spreadsheet_hint_for_non_spreadsheet_window() {
        let p = build_step_prompt(&input("点击", "", "click", "记事本"), 3, None, None);
        assert!(!p.contains("【表格软件】"), "非表格窗口不应注入表格软件指令");
    }

    #[test]
    fn build_step_prompt_force_spreadsheet_injects_hint_without_keyword() {
        // 窗口标题无任何表格关键词，但用户手动标记表格模式 → 仍注入强化指令
        let p = build_step_prompt(&input("单击", "", "click", "销售表"), 2, Some(AppDomain::Spreadsheet), None);
        assert!(p.contains("【表格软件】"), "手动表格模式应注入表格软件强化指令");
        assert!(p.contains("B22") || p.contains("单元格"));
    }

    #[test]
    fn build_step_prompt_target_app_uses_real_domain_hint() {
        // 录 VS Code（指定软件模式）→ 注入「代码编辑器」提示，而非「表格软件」
        let p = build_step_prompt(
            &input("保存", "", "click", "main.rs - Visual Studio Code"),
            2,
            Some(AppDomain::TargetApp),
            None,
        );
        assert!(p.contains("【代码编辑器】"), "指定软件模式应按真实 app 类型给提示");
        assert!(!p.contains("【表格软件】"), "指定软件模式不应注入表格软件提示");
        // 标题无法识别领域 → 中性（无领域提示），仍不注入表格软件
        let p2 = build_step_prompt(&input("点击", "", "click", "无标题窗口"), 3, Some(AppDomain::TargetApp), None);
        assert!(!p2.contains("【表格软件】"));
        assert!(!p2.contains("【代码编辑器】"));
    }

    #[test]
    fn build_step_prompt_omits_empty_context() {
        let p = build_step_prompt(&input("点击", "", "", ""), 3, None, None);
        assert!(p.contains("第 3 个操作步骤"));
        assert!(!p.contains("操作窗口："));
        assert!(!p.contains("录制动作类型："));
    }

    #[test]
    fn build_step_prompt_no_placeholder_leftover() {
        // 步骤号占位符正确替换，不会残留模板变量
        let p = build_step_prompt(&input("a", "b", "c", "d"), 7, None, None);
        assert!(!p.contains("{step_number}"));
        assert!(!p.contains("{context}"));
        assert!(!p.contains("{action}"));
        assert!(!p.contains("{original_title}"));
        assert!(!p.contains("{original_desc}"));
    }

    #[test]
    fn build_step_prompt_large_step_number() {
        let p = build_step_prompt(&input("t", "d", "click", "w"), 999, None, None);
        assert!(p.contains("第 999 个操作步骤"));
    }

    #[test]
    fn build_step_prompt_injects_target_app_context() {
        // target_app=wps → 注入 WPS 身份 + VLOOKUP/IF 字段名先验
        let p = build_step_prompt(&input("单击", "", "click", "函数参数"), 3, Some(AppDomain::TargetApp), Some("wps"));
        assert!(p.contains("录制目标软件：WPS Office"), "应注入 WPS 软件身份");
        assert!(p.contains("VLOOKUP"), "WPS 应包含 VLOOKUP 对话框字段先验");
        assert!(p.contains("查找值"), "WPS 应包含查找值字段名");
        assert!(p.contains("数据表"), "WPS 应包含数据表字段名");
    }

    #[test]
    fn build_step_prompt_injects_excel_context() {
        let p = build_step_prompt(&input("点击", "", "click", "函数参数"), 1, None, Some("excel"));
        assert!(p.contains("录制目标软件：Microsoft Excel"), "应注入 Excel 身份");
        assert!(p.contains("Lookup_value"), "Excel 应含英文字段名");
    }

    #[test]
    fn build_step_prompt_omits_app_context_when_none() {
        let p = build_step_prompt(&input("点击", "", "click", "记事本"), 1, None, None);
        assert!(!p.contains("录制目标软件"), "无 target_app 时不应注入软件身份");
    }
}
