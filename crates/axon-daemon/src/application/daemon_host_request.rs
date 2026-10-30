//! Daemon Host 的一次性 IPC 请求分发。

use super::{
    DEFAULT_QUEUE_LIMIT, DaemonHost, DaemonHostError, UPGRADE_GRACE_PERIOD_MS,
    completion_delivery::{CLIENT_PRESENCE_TTL_MS, settle_notification_results},
    daemon_host_projection::{attach_cursor, snapshot_chunk_responses, snapshot_to_json},
};
use crate::ports::ProcessIdentity;
use axon_domain::DaemonSessionId;
use axon_protocol::{DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest, DaemonResponse};
use axon_session::{
    ClientPresenceRecord, InteractionQueryStore, InteractionStore, NotificationDeliveryState,
    RuntimeStore, UserQuestionStore,
};
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 执行一个 IPC 命令；Provider 执行仍然只发生在 Worker 中。
    pub fn handle_request(&mut self, request: DaemonRequest) -> Vec<DaemonResponse>
    where
        S: InteractionStore + InteractionQueryStore + UserQuestionStore,
    {
        let request_id = request.request_id.clone();
        let chunked_attach = matches!(
            &request.command,
            DaemonCommand::Attach { capabilities, .. }
                if capabilities.iter().any(|value| value == "chunked_snapshot")
        );
        if request.protocol_version != DAEMON_PROTOCOL_VERSION {
            return vec![protocol_error(request_id)];
        }
        match self.dispatch_command(request.command) {
            Ok(body) if chunked_attach => snapshot_chunk_responses(request_id, body),
            Ok(body) => vec![success_response(request_id, body, self.started_host().ok())],
            Err(error) => vec![request_error(request_id, error)],
        }
    }

    fn dispatch_command(
        &mut self,
        command: DaemonCommand,
    ) -> Result<serde_json::Value, DaemonHostError>
    where
        S: InteractionStore + InteractionQueryStore + UserQuestionStore,
    {
        match command {
            DaemonCommand::SubmitInteraction(request) => {
                serialize(self.submit_interaction(request)?)
            }
            DaemonCommand::GetInteraction { interaction_id } => {
                self.interaction_value(interaction_id)
            }
            DaemonCommand::QueryActivity(query) => self.activity_value(query),
            DaemonCommand::GoalTool(request) => self.goal_tool_value(request),
            DaemonCommand::StartGoal(request) => self.start_goal_value(request),
            DaemonCommand::Attach {
                session_id,
                resume_cursor,
                ..
            } => self.attach_value(session_id, resume_cursor),
            DaemonCommand::SubscribeEvents { .. } => Err(DaemonHostError::Worker(
                "SubscribeEvents 必须使用 IPC 流式分发".to_owned(),
            )),
            DaemonCommand::Detach { session_id } => {
                Ok(serde_json::json!({"sessionId": session_id, "detached": true}))
            }
            DaemonCommand::UpsertSchedule {
                schedule_id,
                session_id,
                kind,
                interval_ms,
                delivery_mode,
                payload,
            } => self.schedule_value(
                schedule_id,
                session_id,
                kind,
                interval_ms,
                delivery_mode,
                payload,
            ),
            DaemonCommand::GetState { session_id } => self.state_value(session_id),
            DaemonCommand::GetHealth => serialize(self.health_snapshot()?),
            DaemonCommand::RefreshMcpCapabilities { .. } => Err(DaemonHostError::Worker(
                "RefreshMcpCapabilities 由 Host 运行器的 IPC 层处理，不进入命令分发".to_owned(),
            )),
            DaemonCommand::RegisterClient {
                client_id,
                workspace_id,
                channel,
                destination,
            } => self.register_client_value(client_id, workspace_id, channel, destination),
            DaemonCommand::UnregisterClient { client_id } => {
                self.unregister_client_value(client_id)
            }
            DaemonCommand::PollNotifications { client_id, limit } => {
                self.poll_notifications_value(client_id, limit)
            }
            DaemonCommand::AcknowledgeNotifications {
                client_id,
                notification_ids,
            } => self.acknowledge_notifications_value(client_id, notification_ids),
            DaemonCommand::EnqueueDelivery(request) => self.enqueue_delivery_value(request),
            DaemonCommand::Drain {
                reason,
                target_version,
            } => self.drain_value(reason, target_version),
            DaemonCommand::AnswerQuestion {
                lane_id,
                call_id,
                answers,
                channel,
            } => self.answer_question_value(lane_id, call_id, answers, channel),
        }
    }

    fn answer_question_value(
        &self,
        _lane_id: String,
        call_id: String,
        answers: Vec<axon_domain::UserAnswerItem>,
        channel: String,
    ) -> Result<serde_json::Value, DaemonHostError>
    where
        S: UserQuestionStore,
    {
        let answers_json = serde_json::to_string(&answers)
            .map_err(|e| DaemonHostError::Worker(format!("答案序列化失败: {e}")))?;
        let now_ms = self.clock.now_ms() as i64;
        let settled = self
            .store
            .cas_settle_answer(&call_id, &answers_json, now_ms, Some(&channel), Some("ipc"))
            .map_err(|e| DaemonHostError::Worker(e.to_string()))?;
        Ok(serde_json::json!({
            "laneId": settled.lane_id,
            "callId": settled.tool_call_id,
            "questionId": settled.question_id,
            "state": settled.state.as_str(),
            "delivered": settled.delivered,
            "deliveryMode": settled.delivery_mode,
        }))
    }

    fn register_client_value(
        &self,
        client_id: String,
        workspace_id: String,
        channel: String,
        destination: String,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let presence = ClientPresenceRecord {
            client_id: client_id.clone(),
            workspace_id,
            channel,
            destination,
            registered_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(CLIENT_PRESENCE_TTL_MS),
        };
        self.store
            .register_client_presence(&presence)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        Ok(serde_json::json!({
            "clientId": client_id,
            "registered": true,
        }))
    }

    fn unregister_client_value(
        &self,
        client_id: String,
    ) -> Result<serde_json::Value, DaemonHostError> {
        self.store
            .unregister_client_presence(&client_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        Ok(serde_json::json!({"clientId": client_id, "registered": false}))
    }

    fn poll_notifications_value(
        &self,
        client_id: String,
        limit: u32,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let Some(presence) = self
            .store
            .client_presence(&client_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?
        else {
            return Ok(serde_json::json!({
                "clientId": client_id,
                "registered": false,
                "notifications": [],
            }));
        };
        // poll 本身即心跳：续期 presence，避免在线客户端被误判为离线。
        let renewed = ClientPresenceRecord {
            registered_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(CLIENT_PRESENCE_TTL_MS),
            ..presence.clone()
        };
        self.store
            .register_client_presence(&renewed)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let notifications = self
            .store
            .dispatched_completion_inbox(
                &presence.workspace_id,
                &presence.channel,
                &presence.destination,
                now_ms,
                limit,
            )
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        Ok(serde_json::json!({
            "clientId": client_id,
            "registered": true,
            "notifications": notifications.iter().map(inbox_entry_json).collect::<Vec<_>>(),
        }))
    }

    fn acknowledge_notifications_value(
        &self,
        client_id: String,
        notification_ids: Vec<String>,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let Some(presence) = self
            .store
            .client_presence(&client_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?
        else {
            return Ok(serde_json::json!({"clientId": client_id, "acknowledged": 0}));
        };
        let mut acknowledged = 0_u32;
        for notification_id in &notification_ids {
            let Ok(Some(notification)) = self.store.completion_notification(notification_id) else {
                continue;
            };
            if notification.workspace_id != presence.workspace_id {
                continue;
            }
            // 客户端确认展示后才把该渠道结算为已送达：只有实际存在且处于
            // dispatched 的匹配 delivery 才能确认，重复 ack 与过期条目均返回 0。
            let changed = self
                .store
                .settle_completion_delivery(
                    notification_id,
                    &presence.channel,
                    &presence.destination,
                    NotificationDeliveryState::Delivered,
                    now_ms,
                )
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            if !changed {
                continue;
            }
            // 任一渠道确认送达即可确认 Result：用户已经能看到完成消息，
            // 不允许因为其他渠道离线而让 Result 永久滞留未读。
            self.store
                .settle_completion_notification(
                    notification_id,
                    NotificationDeliveryState::Delivered,
                    now_ms,
                )
                .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
            settle_notification_results(&self.store, &notification, now_ms, false)?;
            acknowledged = acknowledged.saturating_add(1);
        }
        Ok(serde_json::json!({"clientId": client_id, "acknowledged": acknowledged}))
    }

    /// 登记一条正文已就绪的投递项，并返回它在持久收件箱里的稳定 ID。
    ///
    /// 定时任务等系统发起的投递走这里：正文已经就绪，不需要再通知主 turn；调用方
    /// 必须显式给出唯一投递目标，服务端不再替它推断「谁可能在线」。
    fn enqueue_delivery_value(
        &self,
        request: axon_protocol::EnqueueDeliveryRequest,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let channel = required_target("channel", request.channel.as_deref())?;
        let destination = required_target("destination", request.destination.as_deref())?;
        let (notification_id, targets) = self.enqueue_ready_delivery(
            &request.workspace_id,
            &request.goal_id,
            &request.delivery_id,
            channel,
            destination,
            &request.content,
        )?;
        serialize(axon_protocol::EnqueueDeliveryReceipt {
            notification_id,
            targets,
        })
    }

    fn interaction_value(
        &self,
        interaction_id: String,
    ) -> Result<serde_json::Value, DaemonHostError>
    where
        S: InteractionQueryStore,
    {
        let interaction_id = axon_domain::InteractionId::new(interaction_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let interaction = self
            .store
            .get_interaction(&interaction_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        serialize(interaction)
    }

    fn activity_value(
        &self,
        query: axon_domain::ActivityQuery,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let snapshot = axon_session::ActivityService::new(self.store.clone())
            .read(&query)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        serialize(snapshot)
    }

    fn attach_value(
        &self,
        session_id: String,
        resume_cursor: Option<axon_domain::ConsumerCursor>,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let session_id = DaemonSessionId::new(session_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        let (generation, sequence) = resume_cursor
            .map(|cursor| (Some(cursor.generation), cursor.sequence))
            .unwrap_or((None, 0));
        let snapshot = self.attach(
            &session_id,
            generation.as_deref(),
            sequence,
            DEFAULT_QUEUE_LIMIT,
        )?;
        serialize(snapshot_to_json(snapshot))
    }

    fn schedule_value(
        &self,
        schedule_id: String,
        session_id: Option<String>,
        kind: String,
        interval_ms: u64,
        delivery_mode: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let session_id = session_id
            .map(DaemonSessionId::new)
            .transpose()
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        serialize(self.upsert_interval_schedule(
            schedule_id,
            session_id,
            kind,
            interval_ms,
            delivery_mode,
            payload,
        )?)
    }

    fn state_value(&self, session_id: String) -> Result<serde_json::Value, DaemonHostError> {
        let session_id = DaemonSessionId::new(session_id)
            .map_err(|error| DaemonHostError::Worker(error.to_string()))?;
        serialize(self.store.get_session(&session_id)?)
    }

    fn drain_value(
        &mut self,
        reason: String,
        target_version: String,
    ) -> Result<serde_json::Value, DaemonHostError> {
        let deadline_at_ms = self.clock.now_ms().saturating_add(UPGRADE_GRACE_PERIOD_MS);
        let admission = self.request_upgrade(&target_version, deadline_at_ms)?;
        Ok(serde_json::json!({
            "draining": true,
            "reason": reason,
            "admission": admission
        }))
    }
}

fn serialize(value: impl serde::Serialize) -> Result<serde_json::Value, DaemonHostError> {
    serde_json::to_value(value).map_err(|error| DaemonHostError::Worker(error.to_string()))
}

fn success_response(
    request_id: String,
    body: serde_json::Value,
    host: Option<&axon_session::DaemonHostRecord>,
) -> DaemonResponse {
    let cursor = attach_cursor(&body, host);
    let mut response = DaemonResponse::success(request_id, body);
    response.cursor = cursor;
    response
}

fn protocol_error(request_id: String) -> DaemonResponse {
    DaemonResponse {
        request_id,
        protocol_version: DAEMON_PROTOCOL_VERSION,
        ok: false,
        body: serde_json::json!({"error": "不支持的 Daemon 协议版本"}),
        cursor: None,
    }
}

fn request_error(request_id: String, error: DaemonHostError) -> DaemonResponse {
    DaemonResponse {
        request_id,
        protocol_version: DAEMON_PROTOCOL_VERSION,
        ok: false,
        body: serde_json::json!({"error": error.to_string()}),
        cursor: None,
    }
}

/// 取必填的投递目标字段；缺省或空白一律视为请求不合法。
///
/// `EnqueueDelivery` 的唯一职责就是「把这段正文送到调用方指定的地方」，没有目标时
/// 返回成功回执等于制造一次假投递：正文根本不会落库，调用方却以为已经登记。
fn required_target<'a>(field: &str, value: Option<&'a str>) -> Result<&'a str, DaemonHostError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DaemonHostError::Worker(format!("EnqueueDelivery 必须提供非空 {field}")))
}

fn inbox_entry_json(entry: &axon_session::ClientInboxEntry) -> serde_json::Value {
    serde_json::json!({
        "notificationId": entry.notification_id,
        "goalId": entry.goal_id,
        "executionId": entry.execution_id,
        "channel": entry.channel,
        "destination": entry.destination,
        "content": entry.content,
        "createdAtMs": entry.created_at_ms,
        "expiresAtMs": entry.expires_at_ms,
    })
}
