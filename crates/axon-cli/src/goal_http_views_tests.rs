//! 目标看板视图测试：截断口径、工作区解析、最近活动时间与详情上限。

use super::{
    DETAIL_EXECUTION_LIMIT, DETAIL_ITEM_CHARS, DETAIL_LIST_ITEMS, DETAIL_OBJECTIVE_CHARS,
    PREVIEW_CHARS, goal_card_view, goal_detail_view, status_code, truncate_chars,
};
use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionRecord, ExecutionStatus, GoalId, GoalPlan, GoalRecord,
    ResultId, ResultRecord, ScopeRef,
};
use axon_session::WorkspaceRecord;

fn goal_record(objective: &str, steps: Vec<String>, now_ms: u64) -> GoalRecord {
    GoalRecord::new(
        GoalId::new("goal-view").expect("goal id"),
        ScopeRef::global(),
        "视图目标",
        GoalPlan {
            objective: objective.to_owned(),
            steps,
        },
        vec!["通过".to_owned()],
        now_ms,
    )
    .expect("goal")
}

fn workspace(id: &str, root: &str) -> WorkspaceRecord {
    WorkspaceRecord {
        workspace_id: id.to_owned(),
        display_name: id.to_owned(),
        workspace_root: root.to_owned(),
        git_remote: None,
        default_branch: None,
        aliases: Vec::new(),
        memory_namespace: id.to_owned(),
        pinned: false,
        last_seen_at_ms: 1,
    }
}

fn execution(id: &str, created_at_ms: u64, updated_at_ms: u64) -> ExecutionRecord {
    let mut record = ExecutionRecord::new(
        ExecutionId::new(id).expect("execution id"),
        GoalId::new("goal-view").expect("goal id"),
        created_at_ms,
    );
    record.session_id = Some(DaemonSessionId::new(format!("goal-session-{id}")).expect("session"));
    record.status = ExecutionStatus::Running;
    record.updated_at_ms = updated_at_ms;
    record
}

#[test]
fn preview_truncates_on_character_boundary() {
    // 160 个字符不截断，161 个字符截断并补省略号；中文按字符而非字节计数。
    let exact = "目".repeat(PREVIEW_CHARS);
    let (kept, cut) = truncate_chars(&exact, PREVIEW_CHARS);
    assert_eq!(kept, exact);
    assert!(!cut);

    let long = format!("{exact}标");
    let (truncated, cut) = truncate_chars(&long, PREVIEW_CHARS);
    assert!(cut);
    assert_eq!(truncated.chars().count(), PREVIEW_CHARS + 1);
    assert!(truncated.ends_with('…'));
}

#[test]
fn card_view_uses_preview_and_reports_missing_workspace_as_empty() {
    let goal = goal_record(
        &"目".repeat(PREVIEW_CHARS + 5),
        vec!["第一步".to_owned()],
        100,
    );
    let card = goal_card_view(&goal, None, None, None);
    assert_eq!(card.objective_preview.chars().count(), PREVIEW_CHARS + 1);
    assert_eq!(card.goal_id, "goal-view");
    assert_eq!(card.status, status_code(axon_domain::GoalStatus::Proposed));
    assert_eq!(card.step_count, 1);
    // 全局目标解析不到工作区时三个字段一律为空，不伪造归属。
    assert!(card.workspace_id.is_none());
    assert!(card.workspace_name.is_none());
    assert!(card.workspace_path.is_none());
    assert_eq!(card.scope_kind, "global");
}

#[test]
fn card_view_fills_workspace_for_workspace_scope() {
    let mut goal = goal_record("修复登录回归", vec!["定位失败用例".to_owned()], 100);
    goal.scope = ScopeRef::workspace("workspace-1").expect("scope");
    let card = goal_card_view(
        &goal,
        None,
        None,
        Some(&workspace("workspace-1", "E:/repo")),
    );
    assert_eq!(card.scope_kind, "workspace");
    assert_eq!(card.workspace_id.as_deref(), Some("workspace-1"));
    assert_eq!(card.workspace_path.as_deref(), Some("E:/repo"));
}

#[test]
fn workspace_scope_keeps_identifier_even_without_registry_entry() {
    // 注册表缺这项时只留名称与路径为空：标识是作用域事实，不能跟着一起丢。
    let mut goal = goal_record("工作区目标", vec!["一步".to_owned()], 100);
    goal.scope = ScopeRef::workspace("workspace-unknown").expect("scope");
    let card = goal_card_view(&goal, None, None, None);
    assert_eq!(card.scope_kind, "workspace");
    assert_eq!(card.workspace_id.as_deref(), Some("workspace-unknown"));
    assert!(card.workspace_name.is_none());
    assert!(card.workspace_path.is_none());
}

#[test]
fn last_activity_takes_the_maximum_of_three_facts() {
    let mut goal = goal_record("补回归测试", vec!["改代码".to_owned()], 100);
    goal.updated_at_ms = 200;
    let execution = execution("execution-a", 300, 900);
    let result = ResultRecord::success(
        ResultId::new("result-a").expect("result id"),
        goal.goal_id.clone(),
        execution.execution_id.clone(),
        "全部用例通过",
        1_500,
    );
    let card = goal_card_view(&goal, Some(&execution), Some(&result), None);
    assert_eq!(card.last_activity_at_ms, 1_500);
    assert_eq!(
        card.latest_execution.as_ref().map(|view| view.status),
        Some("running")
    );
    assert_eq!(
        card.latest_result
            .as_ref()
            .map(|view| view.summary_preview.as_str()),
        Some("全部用例通过")
    );

    // 没有执行与结果时等于 updatedAtMs。
    let bare = goal_card_view(&goal, None, None, None);
    assert_eq!(bare.last_activity_at_ms, 200);
}

#[test]
fn result_summary_is_truncated_to_preview_length() {
    let goal = goal_record("结果摘要", vec!["步骤".to_owned()], 100);
    let execution = execution("execution-b", 100, 100);
    let result = ResultRecord::success(
        ResultId::new("result-b").expect("result id"),
        goal.goal_id.clone(),
        execution.execution_id.clone(),
        "好".repeat(PREVIEW_CHARS + 1),
        120,
    );
    let card = goal_card_view(&goal, Some(&execution), Some(&result), None);
    let preview = card
        .latest_result
        .as_ref()
        .map(|view| view.summary_preview.clone())
        .expect("result");
    assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1);
    assert!(preview.ends_with('…'));
}

#[test]
fn detail_bounds_objective_lists_and_executions() {
    let mut goal = goal_record(
        &"目".repeat(DETAIL_OBJECTIVE_CHARS + 1),
        (0..DETAIL_LIST_ITEMS + 1)
            .map(|index| format!("{index}"))
            .collect(),
        100,
    );
    goal.acceptance = vec!["验".repeat(DETAIL_ITEM_CHARS + 1)];
    let executions: Vec<ExecutionRecord> = (0..DETAIL_EXECUTION_LIMIT as usize + 3)
        .map(|index| execution(&format!("execution-{index}"), index as u64, index as u64))
        .collect();
    let card = goal_card_view(&goal, executions.first(), None, None);
    let detail = goal_detail_view(card, &goal, &executions, None);

    assert!(detail.objective_truncated);
    assert_eq!(detail.objective.chars().count(), DETAIL_OBJECTIVE_CHARS + 1);
    assert_eq!(detail.steps.len(), DETAIL_LIST_ITEMS);
    assert!(detail.lists_truncated);
    assert_eq!(detail.acceptance[0].chars().count(), DETAIL_ITEM_CHARS + 1);
    assert_eq!(
        detail.recent_executions.len(),
        DETAIL_EXECUTION_LIMIT as usize
    );
}

#[test]
fn detail_keeps_short_content_untruncated() {
    // 未超限时不置任何截断标志，界面因此不会平白警告一次。
    let goal = goal_record("短目标", vec!["一步".to_owned(), "两步".to_owned()], 100);
    let card = goal_card_view(&goal, None, None, None);
    let detail = goal_detail_view(card, &goal, &[], None);
    assert!(!detail.objective_truncated);
    assert!(!detail.lists_truncated);
    assert_eq!(detail.steps, vec!["一步".to_owned(), "两步".to_owned()]);
    assert!(detail.recent_executions.is_empty());
    assert!(detail.cancellation_reason.is_none());
}
