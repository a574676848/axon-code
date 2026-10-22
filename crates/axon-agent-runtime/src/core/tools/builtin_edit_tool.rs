//! 内置 edit tool 的唯一模型与运行时定义。

use axon_domain::{
    ToolDefinition, ToolFreeformFormat, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema,
    ToolSchemaField, ToolSchemaFieldKind,
};

/// 内置 edit tool 名称。
pub const EDIT_TOOL_NAME: &str = "edit";
/// edit tool 在本机注册表中的完整名称。
pub const EDIT_TOOL_QUALIFIED_NAME: &str = "builtin.edit";
/// OpenAI Responses custom tool 使用的 wire 名称。
pub const EDIT_TOOL_CUSTOM_WIRE_NAME: &str = "apply_patch";
/// edit tool schema version。
pub const EDIT_TOOL_SCHEMA_VERSION: u32 = 1;
/// edit tool ID。
pub const EDIT_TOOL_ID: ToolId = ToolId(37);
/// edit tool 必填 input 字段。
pub const EDIT_TOOL_INPUT_FIELD: &str = "input";
/// edit tool 可选 summary 字段。
pub const EDIT_TOOL_SUMMARY_FIELD: &str = "summary";
const EDIT_FREEFORM_FORMAT_TYPE: &str = "grammar";
const EDIT_FREEFORM_SYNTAX: &str = "lark";
const EDIT_FREEFORM_GRAMMAR: &str = include_str!("edit.lark");

fn edit_tool_description() -> String {
    "编辑工作区文件，编辑文本支持 Hashline 与 PathDiff 两种格式。\
     参数格式由 provider schema 决定：\
     JSON function-call 必须传对象并把编辑文本放入 input 字段；\
     freeform/custom tool 直接传编辑文本。\
     编辑文本格式严格二选一，禁止混用。\
     选型：改已存在的单个文件优先 Hashline；整文件重写或新建用 *** Add File:（目标已存在即整文件重写，不必先 Delete）；只有必须原子的多文件批处理（含删除与移动）才写 PathDiff 的 Update 块。\
     每次成功编辑都会改写该文件的 [path#TAG]，后续编辑必须用回执里的新 header 或重新读取；只把要改的行纳入范围，纯新增用 INS.* 。\
     同一批次中每个源路径和目标路径只能出现一次；\
     同一文件多处修改必须合并到同一个 *** Update File: 块，\
     禁止重复 Update/Add/Delete，也禁止把 [path#TAG] 放入 PathDiff。\
     Update 块中上下文行必须以空格开头、删除行以 - 开头、新增行以 + 开头；\
     独立占位符 '// ... existing code ...' 或 '# ... existing code ...' 表示 Lazy Edit 边界，\
     按 @@ 处理；带 +、- 或空格前缀的同名文本仍按真实代码处理。"
        .to_owned()
}

fn edit_input_description() -> String {
    "编辑文本格式严格二选一（Hashline 或 PathDiff），起始标记与约束见工具描述。\
     成功回执提供 files[].diff、files[].hashline.header、LSP 诊断和 nextActions。"
        .to_owned()
}

pub(crate) fn edit_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        EDIT_TOOL_SCHEMA_VERSION,
        [EDIT_TOOL_INPUT_FIELD],
        vec![
            ToolSchemaField::new(
                EDIT_TOOL_INPUT_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                edit_input_description(),
            ),
            ToolSchemaField::new(
                EDIT_TOOL_SUMMARY_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "可选的简短修改摘要。",
            ),
        ],
    );
    schema.input_schema["examples"] = serde_json::json!([
        { "input": "[src/lib.rs#A1B2C3]\nINS.TAIL:\n+fn added() {}" },
        { "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@ fn target()\n-old()\n+new()\n*** End Patch" },
        { "input": "*** Begin Patch\n*** Update File: src/lib.rs\n// ... existing code ...\n-old()\n+new()\n// ... existing code ...\n*** End Patch" },
        { "input": "*** Begin Patch\n*** Add File: scripts/check.py\n+print(\"ok\")\n*** End Patch" }
    ]);
    ToolDefinition {
        id: EDIT_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: EDIT_TOOL_NAME.to_owned(),
        description: edit_tool_description(),
        schema: ToolSchema::freeform(
            EDIT_TOOL_SCHEMA_VERSION,
            schema.input_schema,
            EDIT_TOOL_INPUT_FIELD,
            ToolFreeformFormat {
                format_type: EDIT_FREEFORM_FORMAT_TYPE.to_owned(),
                syntax: EDIT_FREEFORM_SYNTAX.to_owned(),
                definition: EDIT_FREEFORM_GRAMMAR.to_owned(),
            },
        ),
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::WorkspaceCoordinated,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::GovernedWrite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 度量 edit 工具定义的字符预算；`description` 与 `input_schema` 会随每次
    /// provider 请求原样下发，是每轮固定开销的主要来源。
    fn edit_tool_definition_size() -> (usize, usize) {
        let tool = edit_tool_definition();
        (
            tool.description.chars().count(),
            tool.schema.input_schema.to_string().chars().count(),
        )
    }

    /// 去重前的工具定义字符数，用作降幅基线。
    const EDIT_TOOL_DEFINITION_BASELINE_CHARS: usize = 1_720;
    /// 契约要求的字符数下降比例（百分比）。
    const EDIT_TOOL_DEFINITION_REQUIRED_REDUCTION_PCT: usize = 15;
    /// 跨调用规约迁移前的工具定义字符数（P0-03 完成时实测），用作迁移效果基线。
    const EDIT_TOOL_DEFINITION_PRE_GUIDANCE_CHARS: usize = 1_388;
    /// `description + input_schema` 的交付时实测字符数，用作**回归地板**。
    ///
    /// 契约 G3 对外引用的就是这个数（基线 `1 720` → 迁移前 `1 388` → 现在这里是当前值）。
    /// 此前只有「≤ 预算」这一上界断言，而预算为 `1 462`：声明值可上浮 **25%** 仍全绿，
    /// **声明本身不受保护**。
    ///
    /// **重基线记录**：
    /// - `1 169 → 1 258`（+89）：编辑选型路由「整文件重写或新建用 `*** Add File:`（目标已存在即
    ///   整文件重写，不必先 Delete）」，用于消除实测故障类：模型把整篇内容塞进
    ///   `*** Update File:` 块导致 hunk 解析失败。
    /// - `1 258 → 1 333`（+75）：借鉴 omp 的操作规范补一句「每次成功编辑都会改写该文件的
    ///   `[path#TAG]`，后续编辑必须用回执里的新 header 或重新读取；只把要改的行纳入范围，
    ///   纯新增用 `INS.*`」：重锚规则是陈旧 TAG 故障类的直接预防。完整操作规范（重锚、
    ///   范围收紧、正文即终态、块操作取舍、禁止用编辑工具格式化）由
    ///   `<tool_execution_and_receipt_protocol>` 片段承载，schema 不得复述跨调用规约。
    ///
    /// 上界未放宽：`PRE_GUIDANCE`（1 388）与折算预算（1 462）两条断言仍然生效（当前 1 333）。
    const EDIT_TOOL_DEFINITION_CURRENT_CHARS: usize = 1_333;
    /// 自由格式载荷（`format_type` + `syntax` + `definition`）的字符数上界。
    ///
    /// 该 grammar（`edit.lark`）经 `freeform.definition` 随 provider 请求下发
    /// （Responses / custom-tool 路径），与 `description` 同属**每轮固定开销**，
    /// 却从未被 G3 口径覆盖：而它比被统计的两项之和还大。
    const EDIT_TOOL_FREEFORM_PAYLOAD_MAX_CHARS: usize = 1_800;

    /// 度量随请求下发的 freeform 载荷字符数；非 freeform schema 返回 `None`。
    fn edit_tool_freeform_payload_chars() -> Option<usize> {
        let tool = edit_tool_definition();
        let axon_domain::ToolInputFormat::Freeform { format, .. } = &tool.schema.input_format
        else {
            return None;
        };
        Some(
            format.format_type.chars().count()
                + format.syntax.chars().count()
                + format.definition.chars().count(),
        )
    }

    /// 字符预算由基线按契约要求的降幅折算，而非直接写死目标值。
    fn edit_tool_definition_budget() -> usize {
        EDIT_TOOL_DEFINITION_BASELINE_CHARS * (100 - EDIT_TOOL_DEFINITION_REQUIRED_REDUCTION_PCT)
            / 100
    }

    #[test]
    fn cross_call_guidance_migration_shrinks_the_tool_definition() {
        let (description, schema) = edit_tool_definition_size();
        let total = description + schema;
        println!(
            "edit 工具定义字符数：迁移前 {EDIT_TOOL_DEFINITION_PRE_GUIDANCE_CHARS}，当前 {total}"
        );

        assert!(
            total < EDIT_TOOL_DEFINITION_PRE_GUIDANCE_CHARS,
            "跨调用规约迁出后 edit 工具定义应低于迁移前的 \
             {EDIT_TOOL_DEFINITION_PRE_GUIDANCE_CHARS} 字符，实际 {total}"
        );
    }

    #[test]
    fn edit_tool_definition_stays_within_budget() {
        let (description, schema) = edit_tool_definition_size();
        let total = description + schema;
        let budget = edit_tool_definition_budget();

        assert!(
            total <= budget,
            "edit 工具定义膨胀至 {total} 字符（description={description} schema={schema}），\
             超过预算 {budget}；基线 {EDIT_TOOL_DEFINITION_BASELINE_CHARS}，\
             契约要求下降 {EDIT_TOOL_DEFINITION_REQUIRED_REDUCTION_PCT}%"
        );
    }

    /// R-28 回归（一）：契约对外引用的字符数必须**被断言**，而不只是被上界覆盖。
    ///
    /// 缺了这条，`ACCEPTANCE.md` 里的头条度量就只是一个无人守的数字。
    #[test]
    fn edit_tool_definition_does_not_regress_above_the_delivered_size() {
        let (description, schema) = edit_tool_definition_size();
        let total = description + schema;
        println!("edit 工具定义：当前 {total}，交付时实测 {EDIT_TOOL_DEFINITION_CURRENT_CHARS}");

        assert!(
            total <= EDIT_TOOL_DEFINITION_CURRENT_CHARS,
            "工具定义字符数不得高于交付时实测值 {EDIT_TOOL_DEFINITION_CURRENT_CHARS}，\
             实际 {total}（description={description} schema={schema}）"
        );
    }

    /// R-28 回归（二）：随请求下发的 freeform 载荷同样必须有界。
    #[test]
    fn edit_tool_freeform_payload_stays_within_budget() {
        let chars = edit_tool_freeform_payload_chars().expect("edit 工具必须是 freeform schema");
        println!("edit freeform 下发载荷：当前 {chars} 字符");

        assert!(
            chars <= EDIT_TOOL_FREEFORM_PAYLOAD_MAX_CHARS,
            "freeform 载荷（grammar 等）不得超过 {EDIT_TOOL_FREEFORM_PAYLOAD_MAX_CHARS} 字符，\
             实际 {chars}"
        );
    }

    /// C1 回归：edit 工具定义内**不得存在长重复块**。
    ///
    /// P0-03 的核心收益是消除 `description` 与 `input_schema` 之间的重复文本
    /// （两者都随每次 provider 请求原样下发，按字符计费）。该项原有的守卫依赖
    /// `EDIT_SCHEMA_SCENARIOS` 常量与「仅出现一次」的计数断言，而该常量在后续
    /// 重构中被移除：**守卫随之消失，验收证据却仍标 ✅**。
    ///
    /// 本用例以与符号无关的方式重建该不变量：滑动取 `description` 的长度 40 窗口，
    /// 断言其不出现在 schema JSON 中。阈值取得足够长，避免把正常的术语重合误判为重复。
    #[test]
    fn edit_tool_definition_has_no_duplicated_long_blocks() {
        /// 判定「重复文本块」的最小长度；短于此的公共片段属正常术语重合。
        const WINDOW_CHARS: usize = 40;

        let tool = edit_tool_definition();
        let description = tool.description.chars().collect::<Vec<_>>();
        let schema = tool.schema.input_schema.to_string();

        assert!(
            description.len() > WINDOW_CHARS,
            "描述过短（{} 字符），本用例将失去判别力",
            description.len()
        );

        let duplicated = (0..=description.len() - WINDOW_CHARS)
            .map(|start| {
                description[start..start + WINDOW_CHARS]
                    .iter()
                    .collect::<String>()
            })
            .find(|window| schema.contains(window));

        assert!(
            duplicated.is_none(),
            "edit 工具描述与 input_schema 之间存在重复文本块（每轮请求重复计费）：{duplicated:?}"
        );
    }

    #[test]
    fn edit_cross_call_guidance_lives_in_the_fragment_not_the_schema() {
        let tool = edit_tool_definition();
        let schema_text = format!("{}{}", tool.description, tool.schema.input_schema);
        let guidance = axon_prompts::render_tool_usage_guidance();

        // 双向断言：既确认已迁出 schema，也确认没有在迁移中丢失语义。
        for phrase in [
            "仅适用于已读取且持有有效 `[path#TAG]` 快照的文件",
            "适用于删除、重命名/移动文件、跨文件原子批处理",
            "整文件重写与新建用 `*** Add File:`",
            "执行系统命令一律走 `builtin.shell_command`",
        ] {
            assert!(
                !schema_text.contains(phrase),
                "跨调用规约「{phrase}」不应再出现在工具 schema 中"
            );
            assert!(
                guidance.contains(phrase),
                "跨调用规约「{phrase}」必须保留在 <tool_execution_and_receipt_protocol> 片段中"
            );
        }
        assert!(
            !schema_text.contains("<tool_execution_and_receipt_protocol>"),
            "工具 schema 不得引用内部片段标签，选型规约只由稳定 developer 片段承载"
        );
    }

    #[test]
    fn edit_tool_advertises_hashline_and_pathdiff_formats() {
        let tool = edit_tool_definition();
        let format = match tool.schema.input_format {
            axon_domain::ToolInputFormat::Freeform { format, .. } => format,
            axon_domain::ToolInputFormat::JsonSchema => panic!("edit 必须保留 freeform 输入"),
        };

        assert!(format.definition.contains("hashline_patch"));
        assert!(format.definition.contains("pathdiff_patch"));
        assert!(tool.description.contains("Hashline"));
        assert!(tool.description.contains("PathDiff"));
        assert!(
            tool.description
                .contains("编辑文本格式严格二选一，禁止混用")
        );
        assert!(
            tool.description
                .contains("JSON function-call 必须传对象并把编辑文本放入 input 字段")
        );
        assert!(
            tool.description
                .contains("freeform/custom tool 直接传编辑文本")
        );
        assert!(
            tool.description
                .contains("每个源路径和目标路径只能出现一次")
        );
        assert!(
            tool.description
                .contains("多处修改必须合并到同一个 *** Update File:")
        );
        assert!(tool.description.contains("禁止把 [path#TAG] 放入 PathDiff"));
        assert!(tool.description.contains("上下文行必须以空格开头"));
        assert!(tool.description.contains("表示 Lazy Edit 边界"));
        assert!(
            tool.description
                .contains("带 +、- 或空格前缀的同名文本仍按真实代码处理")
        );
        assert!(
            tool.description
                .contains("每次成功编辑都会改写该文件的 [path#TAG]"),
            "重锚规则必须出现在工具描述里：编辑后 TAG 变化是最易踩的坑"
        );
        assert!(tool.description.contains("纯新增用 INS.*"));
        assert_eq!(
            tool.schema.input_schema["examples"]
                .as_array()
                .map(Vec::len),
            Some(4)
        );
    }
}
