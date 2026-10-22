//! 完成通知的正文、结果确认和失效规则。
use super::{COMPLETION_EXPIRY_FALLBACK_MS, COMPLETION_SUMMARY_MAX_CHARS, DaemonHostError};
use axon_domain::{GoalId, GoalRecord, ResultId, ResultRecord};
use axon_session::{CompletionNotificationRecord, RuntimeStore};
use chrono::{Local, LocalResult, TimeZone};
/// 构造后台 Goal 的完成通知。
///
/// 通知必须携带成败与 Result 摘要，供主 turn 汇总。
pub(super) fn completion_notification(
    goal: &GoalRecord,
    execution: &axon_domain::ExecutionRecord,
    result: Option<&ResultRecord>,
) -> String {
    let source = serde_json::from_str::<serde_json::Value>(&execution.payload_json)
        .ok()
        .and_then(|value| value.get("transportContext")?.get("source").cloned())
        .map_or_else(
            || "一次性 Goal".to_owned(),
            |value| {
                let kind = value.get("kind").and_then(serde_json::Value::as_str);
                match kind {
                    Some("axon_cron") => value
                        .get("job_id")
                        .and_then(serde_json::Value::as_str)
                        .map_or_else(|| "Axon cron".to_owned(), |id| format!("Axon cron（{id}）")),
                    _ => "一次性 Goal".to_owned(),
                }
            },
        );
    let (status, summary, artifacts) = match result {
        Some(result) => (
            if result.succeeded { "成功" } else { "失败" },
            bounded_summary(&result.summary),
            result.artifact_refs.join("；"),
        ),
        None => ("未知", "读取不到执行结果".to_owned(), String::new()),
    };
    let artifact_line = if artifacts.is_empty() {
        String::new()
    } else {
        format!("\n结果文档：{artifacts}")
    };
    format!(
        "后台 Goal 阶段结束\n来源：{source}\nGoal：{}\n目标：{}\nGoal 状态：{:?}\n本阶段：{status}\n结果：{summary}{artifact_line}",
        goal.title, goal.plan.objective, goal.status
    )
}

pub(super) fn bounded_summary(summary: &str) -> String {
    let trimmed = summary.trim();
    let mut chars = trimmed.chars();
    let head = chars
        .by_ref()
        .take(COMPLETION_SUMMARY_MAX_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// 结算一条通知负责的 Result。
///
/// `expiring` 为真表示通知隔天未读自动失效：Result 以「过期作废」来源落账，
/// 而不是伪造成「用户已读」。这是禁止静默 ack 与保留清理之间的边界。
pub(in crate::application) fn settle_notification_results<S>(
    store: &S,
    notification: &CompletionNotificationRecord,
    now_ms: u64,
    expiring: bool,
) -> Result<(), DaemonHostError>
where
    S: RuntimeStore,
{
    let result_ids = notification
        .result_ids
        .iter()
        .filter_map(|value| ResultId::new(value.clone()).ok())
        .collect::<Vec<_>>();
    if result_ids.is_empty() {
        return Ok(());
    }
    let goal_id = GoalId::new(notification.goal_id.clone())
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
    let goal = store
        .get_goal(&goal_id)
        .map_err(|error| DaemonHostError::Worker(error.to_string()))?
        .ok_or_else(|| DaemonHostError::Worker("通知关联 Goal 不存在".to_owned()))?;
    let scope = goal.scope;
    let confirmed = if expiring {
        store.expire_results(&scope, &result_ids, now_ms)
    } else {
        store.acknowledge_results(&scope, &result_ids, now_ms)
    };
    confirmed
        .map(|_| ())
        .map_err(|error| DaemonHostError::Worker(error.to_string()))
}

/// 完成通知的失效时刻：创建时刻之后的第一个本地自然日边界。
///
/// 产品定义「隔天未读自动失效」以用户本地日界为准，因此取本地次日零点；时区
/// 换算不可用时退化为「创建后 24 小时」，保证失效窗口始终有界。
pub(in crate::application) fn completion_expiry_at_ms(created_at_ms: u64) -> u64 {
    let fallback = created_at_ms.saturating_add(COMPLETION_EXPIRY_FALLBACK_MS);
    let Ok(created_ms) = i64::try_from(created_at_ms) else {
        return fallback;
    };
    let Some(created) = Local.timestamp_millis_opt(created_ms).single() else {
        return fallback;
    };
    let Some(midnight) = created
        .date_naive()
        .succ_opt()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
    else {
        return fallback;
    };
    match Local.from_local_datetime(&midnight) {
        LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => {
            u64::try_from(value.timestamp_millis()).unwrap_or(fallback)
        }
        LocalResult::None => fallback,
    }
}
