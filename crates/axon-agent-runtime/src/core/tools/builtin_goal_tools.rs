//! 主 turn Goal 能力工具定义。

use axon_domain::{
    GoalCancelCommand, GoalCreateCommand, GoalCreationScope, GoalDetailQuery, GoalId,
    GoalListQuery, GoalQueryScope, GoalStatus, GoalUpdateCommand, ToolConcurrencyPolicy,
    ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema, ToolSchemaField,
    ToolSchemaFieldKind,
};
use axon_protocol::{GoalStartCommand, GoalToolCommand};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// 创建 Goal 工具名。
pub const GOAL_CREATE_TOOL_NAME: &str = "goal_create";
/// 列出 Goal 工具名。
pub const GOAL_LIST_TOOL_NAME: &str = "goal_list";
/// 获取 Goal 工具名。
pub const GOAL_GET_TOOL_NAME: &str = "goal_get";
/// 更新 Goal 工具名。
pub const GOAL_UPDATE_TOOL_NAME: &str = "goal_update";
/// 取消 Goal 工具名。
pub const GOAL_CANCEL_TOOL_NAME: &str = "goal_cancel";
/// 启动或继续 Goal 工具名。
pub const GOAL_START_TOOL_NAME: &str = "goal_start";
/// Goal 查询单次最大记录数。
pub const GOAL_QUERY_MAX_LIMIT: u32 = 256;
/// builtin 工具命名空间。
const BUILTIN_TOOL_NAMESPACE: &str = "builtin";
/// Goal 工具裸名清单，顺序与工具定义一致。
const GOAL_TOOL_NAMES: [&str; 6] = [
    GOAL_CREATE_TOOL_NAME,
    GOAL_LIST_TOOL_NAME,
    GOAL_GET_TOOL_NAME,
    GOAL_UPDATE_TOOL_NAME,
    GOAL_CANCEL_TOOL_NAME,
    GOAL_START_TOOL_NAME,
];

/// Goal 创建工具输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalCreateToolInput {
    /// 创建作用域：global 或 current_workspace。
    pub scope: String,
    /// Goal 标题。
    pub title: String,
    /// 可验收目标。
    pub objective: String,
    /// 计划步骤。
    pub steps: Vec<String>,
    /// 验收条件。
    pub acceptance: Vec<String>,
    /// 是否同时创建首个 queued Execution。
    #[serde(default)]
    pub create_execution: bool,
}

/// Goal 列表工具输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalListToolInput {
    /// 查询作用域：global、current_workspace、current_context 或 all_visible。
    pub scope: String,
    /// 可选状态过滤。
    #[serde(default)]
    pub status: Vec<GoalStatus>,
    /// 单页数量上限；缺省使用默认页大小。
    pub limit: Option<u32>,
    /// 上一页返回的游标。
    pub cursor: Option<String>,
}

/// Goal 详情工具输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalGetToolInput {
    /// Goal 标识。
    pub goal_id: String,
    /// 是否返回最近 Execution。
    #[serde(default)]
    pub include_executions: bool,
    /// 是否返回审计时间线。
    #[serde(default)]
    pub include_events: bool,
    /// Activity 记录上限。
    pub limit: Option<u32>,
}

/// Goal 工具输入解析器。
#[derive(Debug, Default, Clone, Copy)]
pub struct GoalToolRuntime;

impl GoalToolRuntime {
    /// 判定工具是否属于主 turn Goal 能力。
    #[must_use]
    pub fn is_goal_tool(tool_name: &str) -> bool {
        Self::goal_tool_name(tool_name).is_some()
    }

    /// 从限定名解析 Goal 工具裸名；非 Goal 工具返回 None。
    #[must_use]
    pub fn goal_tool_name(tool_name: &str) -> Option<&str> {
        let bare_name = tool_name
            .strip_prefix(BUILTIN_TOOL_NAMESPACE)?
            .strip_prefix('.')?;
        GOAL_TOOL_NAMES.contains(&bare_name).then_some(bare_name)
    }

    /// 解析一次 Goal 工具调用，产出唯一的 wire 命令。
    ///
    /// 这是全链路**唯一**的 Goal 工具参数解析实现：CLI host 面与 daemon 面都不再
    /// 持有逐命令的名称或字段知识，只透传这里的结果。
    pub fn parse_command(
        tool_name: &str,
        value: &serde_json::Value,
    ) -> axon_domain::AxonResult<GoalToolCommand> {
        let bare_name = Self::goal_tool_name(tool_name).ok_or_else(|| {
            axon_domain::AxonError::tool_invalid_arguments(format!("未知 Goal 工具：{tool_name}"))
        })?;
        match bare_name {
            GOAL_CREATE_TOOL_NAME => Self::parse_create(value).map(GoalToolCommand::Create),
            GOAL_LIST_TOOL_NAME => Self::parse_list(value).map(GoalToolCommand::List),
            GOAL_GET_TOOL_NAME => Self::parse_get(value).map(GoalToolCommand::Get),
            GOAL_UPDATE_TOOL_NAME => Self::parse_update(value).map(GoalToolCommand::Update),
            GOAL_CANCEL_TOOL_NAME => Self::parse_cancel(value).map(GoalToolCommand::Cancel),
            GOAL_START_TOOL_NAME => {
                let command: GoalStartCommand = deserialize_input(value)?;
                GoalId::new(command.goal_id.clone()).map_err(|error| {
                    axon_domain::AxonError::tool_invalid_arguments(error.to_string())
                })?;
                Ok(GoalToolCommand::Start(command))
            }
            other => Err(axon_domain::AxonError::tool_invalid_arguments(format!(
                "未知 Goal 工具：{other}"
            ))),
        }
    }

    /// 解析创建输入并映射为领域命令。
    pub fn parse_create(value: &serde_json::Value) -> axon_domain::AxonResult<GoalCreateCommand> {
        let input: GoalCreateToolInput = deserialize_input(value)?;
        let scope = match input.scope.as_str() {
            "global" => GoalCreationScope::Global,
            "current_workspace" => GoalCreationScope::CurrentWorkspace,
            _ => {
                return Err(axon_domain::AxonError::tool_invalid_arguments(
                    "Goal 创建 scope 无效",
                ));
            }
        };
        let command = GoalCreateCommand {
            scope,
            title: input.title,
            objective: input.objective,
            steps: input.steps,
            acceptance: input.acceptance,
            create_execution: input.create_execution,
        };
        command
            .validate()
            .map_err(|error| axon_domain::AxonError::tool_invalid_arguments(error.to_string()))?;
        Ok(command)
    }

    /// 解析列表输入并映射为领域查询。
    pub fn parse_list(value: &serde_json::Value) -> axon_domain::AxonResult<GoalListQuery> {
        let input: GoalListToolInput = deserialize_input(value)?;
        let scope = parse_query_scope(&input.scope)?;
        let query = GoalListQuery {
            scope,
            statuses: input.status,
            limit: input.limit.unwrap_or(20),
            cursor: input.cursor,
        };
        query
            .validate(GOAL_QUERY_MAX_LIMIT)
            .map_err(|error| axon_domain::AxonError::tool_invalid_arguments(error.to_string()))?;
        Ok(query)
    }

    /// 解析详情输入并映射为领域查询。
    pub fn parse_get(value: &serde_json::Value) -> axon_domain::AxonResult<GoalDetailQuery> {
        let input: GoalGetToolInput = deserialize_input(value)?;
        let goal_id = GoalId::new(input.goal_id)
            .map_err(|error| axon_domain::AxonError::tool_invalid_arguments(error.to_string()))?;
        let query = GoalDetailQuery {
            goal_id,
            include_executions: input.include_executions,
            include_events: input.include_events,
            limit: input.limit.unwrap_or(20),
        };
        if query.limit == 0 || query.limit > GOAL_QUERY_MAX_LIMIT {
            return Err(axon_domain::AxonError::tool_invalid_arguments(
                "Goal Activity limit 超出上限",
            ));
        }
        Ok(query)
    }

    /// 解析更新输入并校验 CAS 必填字段。
    pub fn parse_update(value: &serde_json::Value) -> axon_domain::AxonResult<GoalUpdateCommand> {
        let command: GoalUpdateCommand = deserialize_input(value)?;
        command
            .validate()
            .map_err(|error| axon_domain::AxonError::tool_invalid_arguments(error.to_string()))?;
        Ok(command)
    }

    /// 解析取消输入并校验取消原因。
    pub fn parse_cancel(value: &serde_json::Value) -> axon_domain::AxonResult<GoalCancelCommand> {
        let command: GoalCancelCommand = deserialize_input(value)?;
        command
            .validate()
            .map_err(|error| axon_domain::AxonError::tool_invalid_arguments(error.to_string()))?;
        Ok(command)
    }
}

/// 反序列化工具输入；失败统一映射为工具参数错误。
fn deserialize_input<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
) -> axon_domain::AxonResult<T> {
    serde_json::from_value(value.clone())
        .map_err(|error| axon_domain::AxonError::tool_invalid_arguments(error.to_string()))
}

fn parse_query_scope(value: &str) -> axon_domain::AxonResult<GoalQueryScope> {
    match value {
        "global" => Ok(GoalQueryScope::Global),
        "current_workspace" => Ok(GoalQueryScope::CurrentWorkspace),
        "current_context" => Ok(GoalQueryScope::CurrentContext),
        "all_visible" => Ok(GoalQueryScope::AllVisible),
        _ => Err(axon_domain::AxonError::tool_invalid_arguments(
            "Goal scope 无效",
        )),
    }
}

const GOAL_CREATE_TOOL_ID: ToolId = ToolId(60);
const GOAL_LIST_TOOL_ID: ToolId = ToolId(61);
const GOAL_GET_TOOL_ID: ToolId = ToolId(62);
const GOAL_UPDATE_TOOL_ID: ToolId = ToolId(63);
const GOAL_CANCEL_TOOL_ID: ToolId = ToolId(64);
const GOAL_START_TOOL_ID: ToolId = ToolId(65);

/// 返回主 turn Goal 工具。
pub fn goal_tool_definitions() -> [ToolDefinition; 6] {
    [
        goal_create(),
        goal_list(),
        goal_get(),
        goal_update(),
        goal_cancel(),
        goal_start(),
    ]
}

fn goal_create() -> ToolDefinition {
    let schema = object_schema(
        1,
        ["scope", "title", "objective", "steps", "acceptance"],
        vec![
            field(
                "scope",
                ToolSchemaFieldKind::NonBlankString,
                "global 或 current_workspace。",
            ),
            field("title", ToolSchemaFieldKind::NonBlankString, "Goal 标题。"),
            field(
                "objective",
                ToolSchemaFieldKind::NonBlankString,
                "可验收目标。",
            ),
            field(
                "steps",
                ToolSchemaFieldKind::NonBlankStringArray,
                "有序执行步骤。",
            ),
            field(
                "acceptance",
                ToolSchemaFieldKind::NonBlankStringArray,
                "验收条件。",
            ),
            field(
                "createExecution",
                ToolSchemaFieldKind::Boolean,
                "是否立即后台执行；由你根据任务需要决定，无需额外询问用户。",
            ),
        ],
    );
    definition(
        GOAL_CREATE_TOOL_ID,
        GOAL_CREATE_TOOL_NAME,
        "创建持久 Goal；仅用于跨 turn 或需要后台执行的长期承诺。",
        schema,
        false,
    )
}

fn goal_list() -> ToolDefinition {
    let mut schema = object_schema(
        1,
        ["scope"],
        vec![
            field(
                "scope",
                ToolSchemaFieldKind::NonBlankString,
                "global、current_workspace、current_context 或 all_visible。",
            ),
            field(
                "status",
                ToolSchemaFieldKind::StringArray,
                "可选结构化状态过滤。",
            ),
            field("limit", ToolSchemaFieldKind::Integer, "单页数量上限。"),
            field(
                "cursor",
                ToolSchemaFieldKind::NonBlankString,
                "上一页返回的游标。",
            ),
        ],
    );
    schema.input_schema["properties"]["limit"]["minimum"] = json!(1);
    schema.input_schema["properties"]["scope"]["enum"] = json!([
        "global",
        "current_workspace",
        "current_context",
        "all_visible"
    ]);
    schema.input_schema["properties"]["status"]["items"] = json!({
        "type": "string",
        "enum": ["proposed", "active", "waiting", "paused", "completed", "failed", "cancelled"]
    });
    definition(
        GOAL_LIST_TOOL_ID,
        GOAL_LIST_TOOL_NAME,
        "分页检索当前受众可见的 Goal 摘要。",
        schema,
        true,
    )
}

fn goal_get() -> ToolDefinition {
    let schema = object_schema(
        1,
        ["goalId"],
        vec![
            field("goalId", ToolSchemaFieldKind::NonBlankString, "Goal 标识。"),
            field(
                "includeExecutions",
                ToolSchemaFieldKind::Boolean,
                "是否返回最近 Execution。",
            ),
            field(
                "includeEvents",
                ToolSchemaFieldKind::Boolean,
                "是否返回运行历史。",
            ),
            field("limit", ToolSchemaFieldKind::Integer, "Activity 记录上限。"),
        ],
    );
    definition(
        GOAL_GET_TOOL_ID,
        GOAL_GET_TOOL_NAME,
        "读取指定 Goal 的进度、执行记录、结果和运行历史。",
        schema,
        true,
    )
}

fn goal_update() -> ToolDefinition {
    let mut schema = object_schema(
        1,
        ["goalId", "expectedRevision"],
        vec![
            field("goalId", ToolSchemaFieldKind::NonBlankString, "Goal 标识。"),
            field(
                "expectedRevision",
                ToolSchemaFieldKind::Integer,
                "最近一次查询返回的 revision；版本变化时重新查询后决定。",
            ),
            field("title", ToolSchemaFieldKind::NonBlankString, "可选新标题。"),
            field(
                "waitingReason",
                ToolSchemaFieldKind::NonBlankString,
                "可选等待原因。",
            ),
            field(
                "nextStep",
                ToolSchemaFieldKind::NonBlankString,
                "可选下一步。",
            ),
            field("priority", ToolSchemaFieldKind::Integer, "可选优先级。"),
            field(
                "status",
                ToolSchemaFieldKind::NonBlankString,
                "waiting 表示等待输入，paused 表示暂停执行。",
            ),
        ],
    );
    schema.input_schema["properties"]["status"]["enum"] = json!(["waiting", "paused"]);
    definition(
        GOAL_UPDATE_TOOL_ID,
        GOAL_UPDATE_TOOL_NAME,
        "修改任务标题、下一步、优先级，或等待、暂停任务。由你自主决定；继续执行用 goal_start。",
        schema,
        false,
    )
}

fn goal_cancel() -> ToolDefinition {
    let schema = object_schema(
        1,
        ["goalId", "expectedRevision", "reason"],
        vec![
            field("goalId", ToolSchemaFieldKind::NonBlankString, "Goal 标识。"),
            field(
                "expectedRevision",
                ToolSchemaFieldKind::Integer,
                "预期的 Goal 版本号；不一致时拒绝取消。",
            ),
            field("reason", ToolSchemaFieldKind::NonBlankString, "取消原因。"),
        ],
    );
    definition(
        GOAL_CANCEL_TOOL_ID,
        GOAL_CANCEL_TOOL_NAME,
        "取消 Goal 并停止其后续执行。",
        schema,
        false,
    )
}

fn goal_start() -> ToolDefinition {
    definition(
        GOAL_START_TOOL_ID,
        GOAL_START_TOOL_NAME,
        "启动尚未执行的 Goal，或继续等待、暂停的 Goal。由你自主决定，无需额外询问用户；结果未知时先核实外部事实。",
        object_schema(
            1,
            ["goalId", "expectedRevision"],
            vec![
                field(
                    "goalId",
                    ToolSchemaFieldKind::NonBlankString,
                    "要继续的任务标识。",
                ),
                field(
                    "expectedRevision",
                    ToolSchemaFieldKind::Integer,
                    "最近一次查询返回的 revision。",
                ),
            ],
        ),
        false,
    )
}

fn object_schema<const N: usize>(
    version: u32,
    required: [&str; N],
    fields: Vec<ToolSchemaField>,
) -> ToolSchema {
    ToolSchema::object(version, required, fields)
}

fn field(name: &str, kind: ToolSchemaFieldKind, description: &str) -> ToolSchemaField {
    ToolSchemaField::new(name, kind, description)
}

fn definition(
    id: ToolId,
    name: &str,
    description: &str,
    schema: ToolSchema,
    read_only: bool,
) -> ToolDefinition {
    ToolDefinition {
        id,
        namespace: BUILTIN_TOOL_NAMESPACE.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        schema,
        enabled: true,
        concurrency_policy: if read_only {
            ToolConcurrencyPolicy::ParallelSafe
        } else {
            ToolConcurrencyPolicy::Exclusive
        },
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: if read_only {
            ToolPermissionPolicy::ReadOnly
        } else {
            ToolPermissionPolicy::GovernedWrite
        },
    }
}

#[cfg(test)]
#[path = "builtin_goal_tools_tests.rs"]
mod tests;
