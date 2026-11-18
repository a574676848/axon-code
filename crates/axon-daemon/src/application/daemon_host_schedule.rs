//! Daemon Host 的调度面：任务定义侧落定目标会话，唤醒侧只读该绑定。
//!
//! 单独成文件是职责切分：调度记录的创建校验（间隔、投递模式、载荷形状、目标会话）
//! 与 Host 的启动、续租、附件投影无关，后者已经很长。

use axon_session::{RuntimeScheduleRecord, RuntimeStore, ScheduleSessionRequest};
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

use crate::ports::ProcessIdentity;

use super::{DaemonHost, DaemonHostError};

/// interval schedule 的表达式前缀；调度侧按它解析间隔。
const INTERVAL_SCHEDULE_EXPRESSION_PREFIX: &str = "interval_ms:";
/// 唤醒投递模式：按常规轮注入。
const DELIVERY_MODE_FOLLOW_UP: &str = "follow_up";
/// 唤醒投递模式：插进正在运行的轮次。
const DELIVERY_MODE_STEER: &str = "steer";
/// 新建调度记录的初始状态。
const SCHEDULE_STATUS_ACTIVE: &str = "active";
/// 调度载荷里任务归属 Workspace 的键：与调度侧读的是同一个键。
const SCHEDULE_WORKSPACE_PAYLOAD_KEY: &str = "workspaceId";

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 创建或更新只由 Daemon Scheduler 领取的 interval schedule。
    ///
    /// # Errors
    ///
    /// 间隔为零、投递模式非法、载荷不是 JSON object、显式会话未登记，或目标会话无法落定。
    pub fn upsert_interval_schedule(
        &self,
        schedule_id: String,
        session_id: Option<axon_domain::DaemonSessionId>,
        kind: String,
        interval_ms: u64,
        delivery_mode: String,
        payload: serde_json::Value,
    ) -> Result<RuntimeScheduleRecord, DaemonHostError> {
        let _ = self.started_host()?;
        if self.status() != crate::model::DaemonHostStatus::Running {
            return Err(DaemonHostError::Draining);
        }
        if interval_ms == 0 {
            return Err(DaemonHostError::Worker(
                "schedule interval 必须大于 0".to_owned(),
            ));
        }
        if !matches!(
            delivery_mode.as_str(),
            DELIVERY_MODE_FOLLOW_UP | DELIVERY_MODE_STEER
        ) {
            return Err(DaemonHostError::Worker(
                "schedule delivery mode 无效".to_owned(),
            ));
        }
        if !payload.is_object() {
            return Err(DaemonHostError::Worker(
                "schedule payload 必须是 JSON object".to_owned(),
            ));
        }
        let now_ms = self.clock.now_ms();
        // 任务定义侧必须落定目标会话：唤醒路径不接受没有会话的调度记录，
        // 因此省略 `--session-id` 时在这里解析（或新建）一条定时任务会话。
        let session_id = match session_id {
            Some(session_id) => {
                self.store.get_session(&session_id)?;
                session_id
            }
            None => {
                let workspace_id = payload
                    .get(SCHEDULE_WORKSPACE_PAYLOAD_KEY)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                self.store
                    .resolve_schedule_session(&ScheduleSessionRequest {
                        schedule_id: schedule_id.clone(),
                        workspace_id,
                        now_ms,
                    })?
                    .session_id
            }
        };
        let schedule = RuntimeScheduleRecord {
            schedule_id,
            session_id: Some(session_id),
            kind,
            expression: format!("{INTERVAL_SCHEDULE_EXPRESSION_PREFIX}{interval_ms}"),
            payload_json: payload.to_string(),
            delivery_mode,
            status: SCHEDULE_STATUS_ACTIVE.to_owned(),
            next_run_at_ms: Some(now_ms.saturating_add(interval_ms)),
            lease_owner: None,
            lease_expires_at_ms: None,
            failure_count: 0,
        };
        self.store.upsert_schedule(&schedule)?;
        Ok(schedule)
    }
}
