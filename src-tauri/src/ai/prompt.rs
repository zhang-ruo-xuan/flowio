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

/// Heuristic check: does the window title suggest a spreadsheet application?
///
/// Used to inject spreadsheet-specific guidance into the per-step prompt, and to
/// decide whether the full (uncropped) screenshot should be sent to the vision
/// model. Keywords cover WPS 表格 / Excel / LibreOffice Calc / Numbers and common
/// file extensions and UI terms.
pub(crate) fn is_spreadsheet_window(title: &str) -> bool {
    let t = title.to_lowercase();
    [
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
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// Build a single-step prompt for the vision model.
///
/// Includes step number, title, description (from recording metadata),
/// and instructs the model to produce a structured JSON output.
pub fn build_step_prompt(step: &StepInput, step_number: usize, force_spreadsheet: bool) -> String {
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

    let spreadsheet_hint = if is_spreadsheet_window(&step.window_title) || force_spreadsheet {
        "【表格软件】此步骤疑似电子表格操作，请严格按「表格/电子表格软件专项规则」撰写：标题必须含精确单元格引用（如 B22），涉及函数时写明函数名与入口，描述补全关键参数取值与填充范围。\n".to_string()
    } else {
        String::new()
    };

    format!(
        r#"请为以下第 {step_number} 个操作步骤生成中文标题、描述、操作类型和提示。

{context}{action}{spreadsheet_hint}原始标题：{original_title}
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
        context = context_line,
        action = action_hint,
        original_title = step.title,
        original_desc = step.description,
    )
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
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
    }

    #[test]
    fn build_step_prompt_includes_metadata() {
        let p = build_step_prompt(&input("点击保存", "描述", "click", "记事本"), 1, false);
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
            false,
        );
        assert!(p.contains("【表格软件】"), "表格窗口应注入表格软件强化指令");
        assert!(p.contains("B22") || p.contains("单元格"), "应提示精确单元格引用");
    }

    #[test]
    fn build_step_prompt_omits_spreadsheet_hint_for_non_spreadsheet_window() {
        let p = build_step_prompt(&input("点击", "", "click", "记事本"), 3, false);
        assert!(!p.contains("【表格软件】"), "非表格窗口不应注入表格软件指令");
    }

    #[test]
    fn build_step_prompt_force_spreadsheet_injects_hint_without_keyword() {
        // 窗口标题无任何表格关键词，但用户手动标记表格模式 → 仍注入强化指令
        let p = build_step_prompt(&input("单击", "", "click", "销售表"), 2, true);
        assert!(p.contains("【表格软件】"), "手动表格模式应注入表格软件强化指令");
        assert!(p.contains("B22") || p.contains("单元格"));
    }

    #[test]
    fn build_step_prompt_omits_empty_context() {
        let p = build_step_prompt(&input("点击", "", "", ""), 3, false);
        assert!(p.contains("第 3 个操作步骤"));
        assert!(!p.contains("操作窗口："));
        assert!(!p.contains("录制动作类型："));
    }

    #[test]
    fn build_step_prompt_no_placeholder_leftover() {
        // 步骤号占位符正确替换，不会残留模板变量
        let p = build_step_prompt(&input("a", "b", "c", "d"), 7, false);
        assert!(!p.contains("{step_number}"));
        assert!(!p.contains("{context}"));
        assert!(!p.contains("{action}"));
        assert!(!p.contains("{original_title}"));
        assert!(!p.contains("{original_desc}"));
    }

    #[test]
    fn build_step_prompt_large_step_number() {
        let p = build_step_prompt(&input("t", "d", "click", "w"), 999, false);
        assert!(p.contains("第 999 个操作步骤"));
    }
}
