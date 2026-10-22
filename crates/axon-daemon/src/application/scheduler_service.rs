//! 通用持久调度器。

use super::interaction_ingress::{ActiveTurnSteerSink, NoActiveTurnSteerSink, PrimaryTurnIngress};
use axon_crypto::sha256_hex;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, SYSTEM_SOURCE_CHANNEL, ScopeRef, SessionRuntimeStatus,
};
use axon_session::{
    DaemonHostRecord, InteractionStoreError, RuntimeScheduleRecord, RuntimeStore, RuntimeStoreError,
};
use serde_json::{Map, Value};
use thiserror::Error;

const SCHEDULE_KIND_HEARTBEAT: &str = "heartbeat";
const DELIVERY_MODE_FOLLOW_UP: &str = "follow_up";
const DEFAULT_DEFER_MS: u64 = 30_000;
const SCHEDULE_INTERACTION_ID_PREFIX: &str = "schedule-interaction-";
const SCHEDULE_SOURCE_ID_PREFIX: &str = "schedule:";
const SCHEDULE_WORKSPACE_PAYLOAD_KEY: &str = "workspaceId";
const SCHEDULE_PROMPT_PAYLOAD_KEY: &str = "prompt";

/// 调度 tick 报告。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedulerTickReport {
    /// 本轮 claim 数量。
    pub claimed: u32,
    /// 成功提交到 Interaction durable inbox 的数量。
    pub interactions_submitted: u32,
    /// 因 Session 忙碌而延期的数量。
    pub deferred: u32,
    /// 失败数量。
    pub failed: u32,
}

/// 调度错误。
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// Store 错误。
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    /// Interaction 持久化或 ingress 策略错误。
    #[error(transparent)]
    Interaction(#[from] InteractionStoreError),
}

/// 只生产持久 Execution 的 Scheduler。
pub struct SchedulerService<S, K = NoActiveTurnSteerSink> {
    store: S,
    steer_sink: K,
    schedule_lease_duration_ms: u64,
}

impl<S: RuntimeStore> SchedulerService<S, NoActiveTurnSteerSink> {
    /// 创建不尝试 active steer 的 Scheduler；Interaction 仍会进入 durable inbox。
    #[must_use]
    pub fn new(store: S, schedule_lease_duration_ms: u64) -> Self {
        Self {
            store,
            steer_sink: NoActiveTurnSteerSink,
            schedule_lease_duration_ms,
        }
    }
}

impl<S, K> SchedulerService<S, K>
where
    S: RuntimeStore + Clone,
    K: ActiveTurnSteerSink + Clone,
{
    /// 创建可复用 active steer sink 的 Scheduler。
    #[must_use]
    pub fn new_with_steer_sink(store: S, steer_sink: K, schedule_lease_duration_ms: u64) -> Self {
        Self {
            store,
            steer_sink,
            schedule_lease_duration_ms,
        }
    }

    /// 执行一个限量 tick。
    pub fn tick(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<SchedulerTickReport, SchedulerError> {
        let schedules =
            self.store
                .claim_due_schedules(host, now_ms, self.schedule_lease_duration_ms, limit)?;
        let mut report = SchedulerTickReport {
            claimed: schedules.len() as u32,
            ..SchedulerTickReport::default()
        };
        let ingress = PrimaryTurnIngress::new(
            self.store.clone(),
            self.steer_sink.clone(),
            now_ms,
            self.schedule_lease_duration_ms,
        );
        for schedule in schedules {
            if self.should_defer(&schedule)? {
                let next = now_ms.saturating_add(DEFAULT_DEFER_MS);
                self.store.settle_schedule(
                    &schedule.schedule_id,
                    host,
                    Some(next),
                    now_ms,
                    false,
                )?;
                report.deferred = report.deferred.saturating_add(1);
                continue;
            }
            if schedule.session_id.is_none() {
                self.store.settle_schedule(
                    &schedule.schedule_id,
                    host,
                    next_run(&schedule, now_ms),
                    now_ms,
                    false,
                )?;
                continue;
            }
            let fire_at_ms = schedule.next_run_at_ms.unwrap_or(now_ms);
            let interaction = scheduled_interaction(&schedule, fire_at_ms)?;
            match ingress.submit(&interaction) {
                Ok(_) => {
                    self.store.settle_schedule(
                        &schedule.schedule_id,
                        host,
                        next_run(&schedule, fire_at_ms),
                        now_ms,
                        false,
                    )?;
                    report.interactions_submitted = report.interactions_submitted.saturating_add(1);
                }
                Err(error) => {
                    self.store.settle_schedule(
                        &schedule.schedule_id,
                        host,
                        Some(now_ms.saturating_add(DEFAULT_DEFER_MS)),
                        now_ms,
                        true,
                    )?;
                    return Err(error.into());
                }
            }
        }
        Ok(report)
    }

    fn should_defer(&self, schedule: &RuntimeScheduleRecord) -> Result<bool, RuntimeStoreError> {
        let Some(session_id) = schedule.session_id.as_ref() else {
            return Ok(false);
        };
        if schedule.delivery_mode != DELIVERY_MODE_FOLLOW_UP
            && schedule.kind != SCHEDULE_KIND_HEARTBEAT
        {
            return Ok(false);
        }
        let runtime = self.store.get_session(session_id)?;
        Ok(!matches!(
            runtime.status,
            SessionRuntimeStatus::Idle | SessionRuntimeStatus::Created
        ))
    }
}

fn scheduled_interaction(
    schedule: &RuntimeScheduleRecord,
    fire_at_ms: u64,
) -> Result<InteractionRecord, RuntimeStoreError> {
    let payload = serde_json::from_str::<Map<String, Value>>(&schedule.payload_json)
        .map_err(|error| RuntimeStoreError::Storage(format!("schedule payload 无效：{error}")))?;
    let prompt = payload
        .get(SCHEDULE_PROMPT_PAYLOAD_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RuntimeStoreError::Storage("schedule payload 缺少非空 prompt".to_owned()))?;
    let workspace_id = payload
        .get(SCHEDULE_WORKSPACE_PAYLOAD_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            RuntimeStoreError::InvalidId("RuntimeSchedule 必须提供 workspaceId".to_owned())
        })?;
    let source_identity = format!("{}:{fire_at_ms}", schedule.schedule_id);
    let interaction_id = InteractionId::new(format!(
        "{SCHEDULE_INTERACTION_ID_PREFIX}{}",
        sha256_hex(source_identity.as_bytes())
    ))
    .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    let source = InteractionSource::new(
        SYSTEM_SOURCE_CHANNEL,
        format!("{SCHEDULE_SOURCE_ID_PREFIX}{source_identity}"),
    )
    .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    let scope = ScopeRef::workspace(workspace_id.clone())
        .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    let route = ReplyRoute::new(SYSTEM_SOURCE_CHANNEL, workspace_id)
        .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    InteractionRecord::new(
        interaction_id,
        source,
        scope,
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        route,
        prompt,
        fire_at_ms,
    )
    .map_err(|error| RuntimeStoreError::Storage(error.to_string()))
}

fn next_run(schedule: &RuntimeScheduleRecord, fired_at_ms: u64) -> Option<u64> {
    schedule
        .expression
        .strip_prefix("interval_ms:")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|interval| fired_at_ms.saturating_add(interval))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{DaemonSessionId, RuntimeHostId};
    use axon_session::{InteractionStore, RuntimeStore, SqliteRuntimeStore};

    #[test]
    fn due_heartbeat_is_enqueued_as_persistent_action() {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let host_id = RuntimeHostId::new("scheduler-host").expect("host ID");
        let host = store
            .claim_host(&host_id, 1, None, 10, 100)
            .expect("领取 host");
        let session_id = DaemonSessionId::new("session-lane-workspace-a").expect("session ID");
        store
            .create_session(&session_id, ".", 10)
            .expect("创建 session");
        store
            .upsert_schedule(&RuntimeScheduleRecord {
                schedule_id: "heartbeat-a".to_owned(),
                session_id: Some(session_id.clone()),
                kind: SCHEDULE_KIND_HEARTBEAT.to_owned(),
                expression: "interval_ms:1000".to_owned(),
                payload_json: serde_json::json!({
                    SCHEDULE_PROMPT_PAYLOAD_KEY: "检查进度",
                    SCHEDULE_WORKSPACE_PAYLOAD_KEY: "workspace-a",
                })
                .to_string(),
                delivery_mode: DELIVERY_MODE_FOLLOW_UP.to_owned(),
                status: "active".to_owned(),
                next_run_at_ms: Some(10),
                lease_owner: None,
                lease_expires_at_ms: None,
                failure_count: 0,
            })
            .expect("保存 schedule");
        let report = SchedulerService::new(store.clone(), 100)
            .tick(&host, 10, 10)
            .expect("调度");
        assert_eq!(report.interactions_submitted, 1);
        let interactions = store
            .claim_processing(10, 10, 100)
            .expect("读取 Interaction");
        assert_eq!(interactions.len(), 1);
        assert_eq!(
            interactions[0].interaction.source.channel(),
            SYSTEM_SOURCE_CHANNEL
        );
        assert_eq!(interactions[0].interaction.content, "检查进度");
    }
}
