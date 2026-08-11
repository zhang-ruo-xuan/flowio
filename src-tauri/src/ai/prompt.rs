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

截图说明：
你会收到两张截图（按顺序）：
- 第 1 张（before）：操作前的干净界面截图，用于理解操作上下文
- 第 2 张（marked）：操作后的截图，点击类操作会有红圈标注点击位置，键盘类操作聚焦在输入区域
请结合两张截图综合判断用户完成了什么操作。

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
pub fn build_step_prompt(step: &StepInput, step_number: usize) -> String {
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

    format!(
        r#"请为以下第 {step_number} 个操作步骤生成中文标题、描述、操作类型和提示。

{context}{action}原始标题：{original_title}
原始描述：{original_desc}

请根据操作截图推断用户实际执行的操作，输出 JSON：
{{"title": "...", "description": "...", "action_type": "click/input/select/scroll/navigate/other", "tip": "..."}}

要求：
- title：2-20 字，动词开头，如"点击登录按钮"
- description：10-200 字，描述操作对象、位置和预期结果
- action_type：click（点击）/ input（输入）/ select（选择）/ scroll（滚动）/ navigate（导航）/ other（其他）
- tip：操作提示或注意事项，如无则为空字符串

【最重要的规则】原始录制动作类型和原始标题来自用户录制的真实操作，你必须以它们为基准。你的任务是对标题和描述做语言润色（如统一措辞、补充更清晰的界面描述），严禁改变操作对象或动作含义。即使截图看起来像是在做别的事，也必须以原始录制动作为准。

特别注意：如果 after 截图显示的是一个完全不同的窗口或桌面，而非 before 截图中的网页或应用界面，说明用户的动作可能是关闭窗口、切换标签页、最小化窗口等窗口管理操作。此时必须根据录制的原始动作类型和标题来描述操作，严禁把"关闭浏览器"描述成网页内的操作（如投稿、点赞等）。before 截图中残留的网页内容只是背景，不代表用户在该网页上操作。
"#,
        step_number = step_number,
        context = context_line,
        action = action_hint,
        original_title = step.title,
        original_desc = step.description,
    )
}
