//! 目标看板只读视图：把 Goal / Execution / Result 事实投影成网关载荷。
//!
//! 视图显式声明字段并使用小驼峰，与工作台既有协议保持一致；刻意不复用领域类型的
//! 序列化，避免为了协议改名而破坏落库数据。列表与详情共用同一份 [`GoalCardView`]
//! 构造器，详情只在它之上追加契约字段，两处结构因此不会各自漂移。

use axon_domain::{ExecutionRecord, GoalRecord, GoalStatus, ResultRecord, ScopeRef};
use axon_session::WorkspaceRecord;
use serde::Serialize;

/// 列表卡片里目标预览与结果摘要的字符上限。
pub(crate) const PREVIEW_CHARS: usize = 160;
/// 详情正文的字符上限。
pub(crate) const DETAIL_OBJECTIVE_CHARS: usize = 20_000;
/// 详情里步骤与验收各自的条数上限。
pub(crate) const DETAIL_LIST_ITEMS: usize = 64;
/// 详情里步骤与验收单条的字符上限。
pub(crate) const DETAIL_ITEM_CHARS: usize = 2_000;
/// 详情里最近执行条的条数上限。
pub(crate) const DETAIL_EXECUTION_LIMIT: u32 = 8;

/// 按 Unicode 字符截断；超出时补省略号并报告已截断。
///
/// 不按字节截断：多字节字符被切一半会变成替换符，中文目标尤其明显。
/// **省略号计入配额**，因此返回值恒不超过 `limit` 个字符，正文上限因此可以逐字落实。
pub(crate) fn truncate_chars(value: &str, limit: usize) -> (String, bool) {
    if value.chars().count() <= limit {
        return (value.to_owned(), false);
    }
    if limit == 0 {
        return (String::new(), true);
    }
    // 留一个字符位给省略号，剩下的才是正文配额。
    let keep = limit - 1;
    let boundary = value
        .char_indices()
        .nth(keep)
        .map_or(value.len(), |(index, _)| index);
    let mut truncated = String::with_capacity(boundary + '…'.len_utf8());
    truncated.push_str(&value[..boundary]);
    truncated.push('…');
    (truncated, true)
}

/// Goal 状态在协议里的取值。
pub(crate) const fn status_code(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Proposed => "proposed",
        GoalStatus::Active => "active",
        GoalStatus::Waiting => "waiting",
        GoalStatus::Paused => "paused",
        GoalStatus::Completed => "completed",
        GoalStatus::Failed => "failed",
        GoalStatus::Cancelled => "cancelled",
    }
}

/// Execution 状态在协议里的取值。
fn execution_status_code(status: axon_domain::ExecutionStatus) -> &'static str {
    use axon_domain::ExecutionStatus;
    match status {
        ExecutionStatus::Queued => "queued",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Waiting => "waiting",
        ExecutionStatus::Uncertain => "uncertain",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
    }
}

/// 一次目标执行的只读投影。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecutionView {
    /// 执行标识。
    pub(crate) execution_id: String,
    /// 执行状态。
    pub(crate) status: &'static str,
    /// 尝试次数。
    pub(crate) attempt: u32,
    /// 绑定的运行时会话标识；未绑定时为空。
    pub(crate) session_id: Option<String>,
    /// 创建时间。
    pub(crate) created_at_ms: u64,
    /// 更新时间。
    pub(crate) updated_at_ms: u64,
}

impl ExecutionView {
    /// 由领域执行记录投影。
    pub(crate) fn from_record(record: &ExecutionRecord) -> Self {
        Self {
            execution_id: record.execution_id.as_str().to_owned(),
            status: execution_status_code(record.status),
            attempt: record.attempt,
            session_id: record.session_id.as_ref().map(|id| id.as_str().to_owned()),
            created_at_ms: record.created_at_ms,
            updated_at_ms: record.updated_at_ms,
        }
    }
}

/// 一次目标结果的只读投影。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResultView {
    /// 结果标识。
    pub(crate) result_id: String,
    /// 是否成功。
    pub(crate) succeeded: bool,
    /// 截断后的摘要。
    pub(crate) summary_preview: String,
    /// 产生时间。
    pub(crate) created_at_ms: u64,
}

impl ResultView {
    /// 由领域结果记录投影；摘要按 [`PREVIEW_CHARS`] 截断。
    pub(crate) fn from_record(record: &ResultRecord) -> Self {
        Self {
            result_id: record.result_id.as_str().to_owned(),
            succeeded: record.succeeded,
            summary_preview: truncate_chars(&record.summary, PREVIEW_CHARS).0,
            created_at_ms: record.created_at_ms,
        }
    }
}

/// 看板卡片视图；详情复用同一份结构。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalCardView {
    /// 目标标识。
    pub(crate) goal_id: String,
    /// 简短标题。
    pub(crate) title: String,
    /// 目标状态。
    pub(crate) status: &'static str,
    /// 乐观并发版本。
    pub(crate) revision: u64,
    /// 截断后的目标预览。
    pub(crate) objective_preview: String,
    /// 计划步骤数。
    pub(crate) step_count: usize,
    /// 下一步。
    pub(crate) next_step: Option<String>,
    /// 等待原因。
    pub(crate) waiting_reason: Option<String>,
    /// 调度优先级。
    pub(crate) priority: i32,
    /// 截止时间。
    pub(crate) deadline_ms: Option<u64>,
    /// 作用域类型：`workspace` 或 `global`。
    pub(crate) scope_kind: &'static str,
    /// 工作区标识；全局目标无法解析工作区时为空。
    pub(crate) workspace_id: Option<String>,
    /// 工作区显示名。
    pub(crate) workspace_name: Option<String>,
    /// 工作区根目录。
    pub(crate) workspace_path: Option<String>,
    /// 创建时间。
    pub(crate) created_at_ms: u64,
    /// 更新时间。
    pub(crate) updated_at_ms: u64,
    /// 最近活动时间：更新时间、最近执行更新时间与最近结果时间三者取最大。
    pub(crate) last_activity_at_ms: u64,
    /// 最近一次执行。
    pub(crate) latest_execution: Option<ExecutionView>,
    /// 最近一次结果。
    pub(crate) latest_result: Option<ResultView>,
}

/// 把三个事实合成一张卡片视图。
///
/// 工作区标识取解析结果，解析不到才回落到作用域标识；名称与路径完全依赖注册表记录。
/// 列表与详情的解析深度不同：列表只按作用域命中注册表，全局目标因此三段字段为空；
/// 详情会再按最近执行的会话 cwd 解析一次，唯一命中时回填。
pub(crate) fn goal_card_view(
    goal: &GoalRecord,
    latest_execution: Option<&ExecutionRecord>,
    latest_result: Option<&ResultRecord>,
    workspace: Option<&WorkspaceRecord>,
) -> GoalCardView {
    let last_activity_at_ms = goal
        .updated_at_ms
        .max(
            latest_execution
                .map(|record| record.updated_at_ms)
                .unwrap_or(0),
        )
        .max(
            latest_result
                .map(|record| record.created_at_ms)
                .unwrap_or(0),
        );
    // 标识取解析结果；解析不到时回落到作用域标识（Workspace 作用域恒有标识），
    // 名称与路径才完全依赖注册表记录。
    let workspace_id = workspace
        .map(|record| record.workspace_id.clone())
        .or_else(|| goal.scope.workspace_id().map(|id| id.as_str().to_owned()));
    let workspace_name = workspace.map(|record| record.display_name.clone());
    let workspace_path = workspace.map(|record| record.workspace_root.clone());
    GoalCardView {
        goal_id: goal.goal_id.as_str().to_owned(),
        title: goal.title.clone(),
        status: status_code(goal.status),
        revision: goal.revision,
        objective_preview: truncate_chars(&goal.plan.objective, PREVIEW_CHARS).0,
        step_count: goal.plan.steps.len(),
        next_step: goal.next_step.clone(),
        waiting_reason: goal.waiting_reason.clone(),
        priority: goal.priority,
        deadline_ms: goal.deadline_ms,
        scope_kind: scope_kind(&goal.scope),
        workspace_id,
        workspace_name,
        workspace_path,
        created_at_ms: goal.created_at_ms,
        updated_at_ms: goal.updated_at_ms,
        last_activity_at_ms,
        latest_execution: latest_execution.map(ExecutionView::from_record),
        latest_result: latest_result.map(ResultView::from_record),
    }
}

/// 作用域类型在协议里的取值。
const fn scope_kind(scope: &ScopeRef) -> &'static str {
    match scope {
        ScopeRef::Global => "global",
        ScopeRef::Workspace(_) => "workspace",
    }
}

/// 目标详情视图：卡片视图加契约字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalDetailView {
    /// 与列表同构的卡片。
    pub(crate) goal: GoalCardView,
    /// 完整目标。
    pub(crate) objective: String,
    /// 计划步骤。
    pub(crate) steps: Vec<String>,
    /// 验收条件。
    pub(crate) acceptance: Vec<String>,
    /// 取消原因。
    pub(crate) cancellation_reason: Option<String>,
    /// 目标正文是否被截断。
    pub(crate) objective_truncated: bool,
    /// 步骤或验收是否被截断。
    pub(crate) lists_truncated: bool,
    /// 最近执行，按创建时间降序。
    pub(crate) recent_executions: Vec<ExecutionView>,
    /// 最近一次结果。
    pub(crate) latest_result: Option<ResultView>,
}

/// 组装详情视图。
///
/// 步骤与验收在这里做条数与单条长度收敛，任一超出即置 `listsTruncated`；
/// 最近执行同样在这里收敛到 [`DETAIL_EXECUTION_LIMIT`] 条，避免调用方漏改上限。
/// `executions` 必须已按 `created_at_ms` 降序。
pub(crate) fn goal_detail_view(
    card: GoalCardView,
    goal: &GoalRecord,
    executions: &[ExecutionRecord],
    latest_result: Option<&ResultRecord>,
) -> GoalDetailView {
    let (objective, objective_truncated) =
        truncate_chars(&goal.plan.objective, DETAIL_OBJECTIVE_CHARS);
    let (steps, steps_truncated) = bounded_items(&goal.plan.steps);
    let (acceptance, acceptance_truncated) = bounded_items(&goal.acceptance);
    GoalDetailView {
        goal: card,
        objective,
        steps,
        acceptance,
        cancellation_reason: goal.cancellation_reason.clone(),
        objective_truncated,
        lists_truncated: steps_truncated || acceptance_truncated,
        recent_executions: executions
            .iter()
            .take(DETAIL_EXECUTION_LIMIT as usize)
            .map(ExecutionView::from_record)
            .collect(),
        latest_result: latest_result.map(ResultView::from_record),
    }
}

/// 按条数与单条长度收敛一组文本；报告是否发生截断。
fn bounded_items(items: &[String]) -> (Vec<String>, bool) {
    let mut truncated = items.len() > DETAIL_LIST_ITEMS;
    let bounded = items
        .iter()
        .take(DETAIL_LIST_ITEMS)
        .map(|item| {
            let (text, cut) = truncate_chars(item, DETAIL_ITEM_CHARS);
            truncated |= cut;
            text
        })
        .collect();
    (bounded, truncated)
}

#[cfg(test)]
#[path = "goal_http_views_tests.rs"]
mod goal_http_views_tests;
