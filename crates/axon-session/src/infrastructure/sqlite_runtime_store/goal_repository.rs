//! `jarvis_goals` 的 SQLite repository。

use super::{
    SqliteRuntimeStore,
    helpers::{i64_value, u64_value},
};
use crate::ports::{GoalMutationStore, GoalPage, GoalStore, GoalStoreError, RuntimeStore};
use axon_domain::{GoalId, GoalPlan, GoalRecord, GoalStatus, JarvisEvent, ScopeRef};
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};
use uuid::Uuid;

impl GoalStore for SqliteRuntimeStore {
    fn create_goal(&self, goal: &GoalRecord) -> Result<bool, GoalStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let inserted = connection
            .execute(
                "INSERT INTO jarvis_goals(goal_id,scope_json,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) ON CONFLICT(goal_id) DO NOTHING",
                params![
                    goal.goal_id.as_str(),
                    serde_json::to_string(&goal.scope).map_err(serialize_error)?,
                    goal.title,
                    status_code(goal.status),
                    i64_value(goal.revision).map_err(runtime_error)?,
                    serde_json::to_string(&goal.plan).map_err(serialize_error)?,
                    serde_json::to_string(&goal.acceptance).map_err(serialize_error)?,
                    goal.progress.as_ref().map(serde_json::to_string).transpose().map_err(serialize_error)?,
                    goal.waiting_reason,
                    goal.next_step,
                    goal.priority,
                    goal.deadline_ms.map(i64_value).transpose().map_err(runtime_error)?,
                    goal.created_from_interaction_id,
                    goal.cancellation_reason,
                    goal.cancellation_requested_at_ms.map(i64_value).transpose().map_err(runtime_error)?,
                    i64_value(goal.created_at_ms).map_err(runtime_error)?,
                    i64_value(goal.updated_at_ms).map_err(runtime_error)?,
                ],
            )
            .map_err(storage_error)?;
        if inserted == 1 {
            return Ok(true);
        }
        let existing = self
            .get_goal(&goal.goal_id)?
            .ok_or_else(|| GoalStoreError::Conflict("Goal 并发创建后无法读取".to_owned()))?;
        if existing == *goal {
            Ok(false)
        } else {
            Err(GoalStoreError::Conflict(
                "goal_id 已绑定不同内容".to_owned(),
            ))
        }
    }

    fn get_goal(&self, goal_id: &GoalId) -> Result<Option<GoalRecord>, GoalStoreError> {
        let connection = self.connection().map_err(runtime_error)?;
        connection
            .query_row(
                "SELECT goal_id,scope_json,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms FROM jarvis_goals WHERE goal_id=?1",
                params![goal_id.as_str()],
                read_goal,
            )
            .optional()
            .map_err(storage_error)
    }

    fn list_active_goals(&self, limit: u32) -> Result<Vec<GoalRecord>, GoalStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection().map_err(runtime_error)?;
        let mut statement = connection
            .prepare("SELECT goal_id,scope_json,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms FROM jarvis_goals WHERE status NOT IN ('completed','failed','cancelled') ORDER BY updated_at_ms ASC, goal_id ASC LIMIT ?1")
            .map_err(storage_error)?;
        statement
            .query_map([i64::from(limit)], read_goal)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn list_active_goals_for_scope(
        &self,
        scope: &ScopeRef,
        limit: u32,
    ) -> Result<Vec<GoalRecord>, GoalStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection().map_err(runtime_error)?;
        let scope_json = serde_json::to_string(scope).map_err(serialize_error)?;
        let mut statement = connection
            .prepare("SELECT goal_id,scope_json,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms FROM jarvis_goals WHERE scope_json=?1 AND status NOT IN ('completed','failed','cancelled') ORDER BY updated_at_ms ASC, goal_id ASC LIMIT ?2")
            .map_err(storage_error)?;
        statement
            .query_map(params![scope_json, i64::from(limit)], read_goal)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn update_goal(&self, goal: &GoalRecord, expected_revision: u64) -> Result<(), GoalStoreError> {
        self.initialize().map_err(runtime_error)?;
        let changed = {
            let connection = self.connection().map_err(runtime_error)?;
            connection
            .execute(
                "UPDATE jarvis_goals SET scope_json=?1,title=?2,status=?3,revision=?4,plan_json=?5,acceptance_json=?6,progress_json=?7,waiting_reason=?8,next_step=?9,priority=?10,deadline_ms=?11,created_from_interaction_id=?12,cancellation_reason=?13,cancellation_requested_at_ms=?14,updated_at_ms=?15 WHERE goal_id=?16 AND revision=?17",
                params![
                    serde_json::to_string(&goal.scope).map_err(serialize_error)?,
                    goal.title,
                    status_code(goal.status),
                    i64_value(goal.revision).map_err(runtime_error)?,
                    serde_json::to_string(&goal.plan).map_err(serialize_error)?,
                    serde_json::to_string(&goal.acceptance).map_err(serialize_error)?,
                    goal.progress.as_ref().map(serde_json::to_string).transpose().map_err(serialize_error)?,
                    goal.waiting_reason,
                    goal.next_step,
                    goal.priority,
                    goal.deadline_ms.map(i64_value).transpose().map_err(runtime_error)?,
                    goal.created_from_interaction_id,
                    goal.cancellation_reason,
                    goal.cancellation_requested_at_ms.map(i64_value).transpose().map_err(runtime_error)?,
                    i64_value(goal.updated_at_ms).map_err(runtime_error)?,
                    goal.goal_id.as_str(),
                    i64_value(expected_revision).map_err(runtime_error)?,
                ],
            )
            .map_err(storage_error)?
        };
        if changed == 1 {
            return Ok(());
        }
        if self.get_goal(&goal.goal_id)?.is_none() {
            Err(GoalStoreError::NotFound(goal.goal_id.clone()))
        } else {
            Err(GoalStoreError::Conflict("Goal revision 已过期".to_owned()))
        }
    }

    fn request_cancellation(
        &self,
        goal_id: &GoalId,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), GoalStoreError> {
        if reason.trim().is_empty() {
            return Err(GoalStoreError::Conflict("取消原因不能为空".to_owned()));
        }
        self.initialize().map_err(runtime_error)?;
        let mut connection = self.connection().map_err(runtime_error)?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let now = i64_value(now_ms).map_err(runtime_error)?;
        transaction
            .execute(
                "UPDATE jarvis_executions SET status='cancelled',cancelled_at_ms=?1,updated_at_ms=?1,lease_json=NULL,claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL WHERE goal_id=?2 AND status='queued'",
                params![now, goal_id.as_str()],
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "UPDATE jarvis_executions SET cancellation_requested_at_ms=?1,cancellation_reason=?2,updated_at_ms=?1 WHERE goal_id=?3 AND status='running'",
                params![now, reason, goal_id.as_str()],
            )
            .map_err(storage_error)?;
        let sequence: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM jarvis_events",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let event_id = Uuid::new_v4().to_string();
        let payload =
            serde_json::json!({"goal_id": goal_id.as_str(), "reason": reason}).to_string();
        transaction
            .execute(
                "INSERT INTO jarvis_events(event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms) VALUES(?1,?2,'goal.cancelled',?3,NULL,?4,?5)",
                params![event_id, sequence, format!("goal.cancelled:{}", goal_id.as_str()), payload, now],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    fn cancel_goal(
        &self,
        goal: &GoalRecord,
        expected_revision: u64,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), GoalStoreError> {
        if reason.trim().is_empty() {
            return Err(GoalStoreError::Conflict("取消原因不能为空".to_owned()));
        }
        self.initialize().map_err(runtime_error)?;
        let mut connection = self.connection().map_err(runtime_error)?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let changed = transaction.execute(
            "UPDATE jarvis_goals SET status='cancelled',revision=?1,cancellation_reason=?2,cancellation_requested_at_ms=?3,updated_at_ms=?3 WHERE goal_id=?4 AND revision=?5 AND status NOT IN ('completed','failed','cancelled')",
            params![i64_value(goal.revision).map_err(runtime_error)?, reason, i64_value(now_ms).map_err(runtime_error)?, goal.goal_id.as_str(), i64_value(expected_revision).map_err(runtime_error)?],
        ).map_err(storage_error)?;
        if changed != 1 {
            return Err(GoalStoreError::Conflict(
                "Goal revision 或状态已变化".to_owned(),
            ));
        }
        let now = i64_value(now_ms).map_err(runtime_error)?;
        transaction.execute(
            "UPDATE jarvis_executions SET status='cancelled',cancelled_at_ms=?1,updated_at_ms=?1,lease_json=NULL,claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL WHERE goal_id=?2 AND status='queued'",
            params![now, goal.goal_id.as_str()],
        ).map_err(storage_error)?;
        transaction.execute(
            "UPDATE jarvis_executions SET cancellation_requested_at_ms=?1,cancellation_reason=?2,updated_at_ms=?1 WHERE goal_id=?3 AND status='running'",
            params![now, reason, goal.goal_id.as_str()],
        ).map_err(storage_error)?;
        let sequence: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM jarvis_events",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        transaction.execute(
            "INSERT INTO jarvis_events(event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms) VALUES(?1,?2,'goal.cancelled',?3,NULL,?4,?5)",
            params![Uuid::new_v4().to_string(), sequence, format!("goal.cancelled:{}:{}", goal.goal_id.as_str(), expected_revision), serde_json::json!({"goal_id": goal.goal_id.as_str(), "reason": reason}).to_string(), now],
        ).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    fn list_goals(
        &self,
        scope: &ScopeRef,
        query: &axon_domain::GoalListQuery,
    ) -> Result<GoalPage, GoalStoreError> {
        self.list_goals_in_scopes(std::slice::from_ref(scope), query)
    }

    fn list_goals_in_scopes(
        &self,
        scopes: &[ScopeRef],
        query: &axon_domain::GoalListQuery,
    ) -> Result<GoalPage, GoalStoreError> {
        const MAX_PAGE_LIMIT: u32 = 256;
        if scopes.is_empty() {
            return Err(GoalStoreError::Conflict(
                "Goal 查询必须指定作用域".to_owned(),
            ));
        }
        if query.limit == 0 || query.limit > MAX_PAGE_LIMIT {
            return Err(GoalStoreError::Conflict(
                "Goal 查询 limit 超出上限".to_owned(),
            ));
        }
        let connection = self.connection().map_err(runtime_error)?;
        let scope_values = scopes
            .iter()
            .map(|scope| serde_json::to_string(scope).map_err(serialize_error))
            .collect::<Result<Vec<_>, _>>()?;
        let scope_placeholders = (1..=scope_values.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut sql = format!(
            "SELECT goal_id,scope_json,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms FROM jarvis_goals WHERE scope_json IN ({scope_placeholders})"
        );
        let mut bind_values: Vec<Box<dyn rusqlite::ToSql>> = scope_values
            .into_iter()
            .map(|value| Box::new(value) as Box<dyn rusqlite::ToSql>)
            .collect();
        if !query.statuses.is_empty() {
            let placeholders = (0..query.statuses.len())
                .map(|index| format!("?{}", bind_values.len() + index + 1))
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(" AND status IN (");
            sql.push_str(&placeholders);
            sql.push(')');
            for status in &query.statuses {
                bind_values.push(Box::new(status_code(*status)));
            }
        }
        let cursor_index = bind_values.len() + 1;
        let fingerprint = cursor_fingerprint(scopes, &query.statuses)?;
        if let Some(cursor) = &query.cursor {
            let (cursor_fingerprint, updated_at_ms, goal_id) = decode_cursor(cursor)
                .ok_or_else(|| GoalStoreError::Conflict("Goal 分页游标无效".to_owned()))?;
            if cursor_fingerprint != fingerprint {
                return Err(GoalStoreError::Conflict(
                    "Goal 分页游标与查询过滤器不匹配".to_owned(),
                ));
            }
            sql.push_str(&format!(
                " AND (updated_at_ms > ?{} OR (updated_at_ms = ?{} AND goal_id > ?{}))",
                cursor_index,
                cursor_index,
                cursor_index + 1
            ));
            bind_values.push(Box::new(updated_at_ms));
            bind_values.push(Box::new(goal_id));
        }
        let limit_index = bind_values.len() + 1;
        sql.push_str(&format!(
            " ORDER BY updated_at_ms ASC, goal_id ASC LIMIT ?{}",
            limit_index
        ));
        bind_values.push(Box::new(i64::from(query.limit.saturating_add(1))));
        let params = rusqlite::params_from_iter(bind_values.iter().map(|value| value.as_ref()));
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let mut goals = statement
            .query_map(params, read_goal)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        let next_cursor = if goals.len() > query.limit as usize {
            goals
                .pop()
                .map(|goal| encode_cursor(&fingerprint, goal.updated_at_ms, goal.goal_id.as_str()))
        } else {
            None
        };
        Ok(GoalPage { goals, next_cursor })
    }
}

impl GoalMutationStore for SqliteRuntimeStore {
    fn update_goal_with_event(
        &self,
        goal: &GoalRecord,
        expected_revision: u64,
        event: &JarvisEvent,
    ) -> Result<(), GoalStoreError> {
        if event.event_type.trim().is_empty() || event.idempotency_key.trim().is_empty() {
            return Err(GoalStoreError::Conflict("Goal 更新 Event 无效".to_owned()));
        }
        self.initialize().map_err(runtime_error)?;
        let mut connection = self.connection().map_err(runtime_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let changed = transaction
            .execute(
                "UPDATE jarvis_goals SET scope_json=?1,title=?2,status=?3,revision=?4,plan_json=?5,acceptance_json=?6,progress_json=?7,waiting_reason=?8,next_step=?9,priority=?10,deadline_ms=?11,created_from_interaction_id=?12,cancellation_reason=?13,cancellation_requested_at_ms=?14,updated_at_ms=?15 WHERE goal_id=?16 AND revision=?17",
                params![
                    serde_json::to_string(&goal.scope).map_err(serialize_error)?,
                    goal.title,
                    status_code(goal.status),
                    i64_value(goal.revision).map_err(runtime_error)?,
                    serde_json::to_string(&goal.plan).map_err(serialize_error)?,
                    serde_json::to_string(&goal.acceptance).map_err(serialize_error)?,
                    goal.progress.as_ref().map(serde_json::to_string).transpose().map_err(serialize_error)?,
                    goal.waiting_reason,
                    goal.next_step,
                    goal.priority,
                    goal.deadline_ms.map(i64_value).transpose().map_err(runtime_error)?,
                    goal.created_from_interaction_id,
                    goal.cancellation_reason,
                    goal.cancellation_requested_at_ms.map(i64_value).transpose().map_err(runtime_error)?,
                    i64_value(goal.updated_at_ms).map_err(runtime_error)?,
                    goal.goal_id.as_str(),
                    i64_value(expected_revision).map_err(runtime_error)?,
                ],
            )
            .map_err(storage_error)?;
        if changed != 1 {
            return Err(GoalStoreError::Conflict("Goal revision 已过期".to_owned()));
        }
        let sequence: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM jarvis_events",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO jarvis_events(event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    event.event_id.as_str(),
                    sequence,
                    event.event_type,
                    event.idempotency_key,
                    event.cause_event_id.as_ref().map(ToString::to_string),
                    event.payload_json,
                    i64_value(event.occurred_at_ms).map_err(runtime_error)?,
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }
}

fn encode_cursor(fingerprint: &str, updated_at_ms: u64, goal_id: &str) -> String {
    format!("{fingerprint}:{updated_at_ms}:{goal_id}")
}

fn decode_cursor(value: &str) -> Option<(String, i64, String)> {
    let mut parts = value.splitn(3, ':');
    let fingerprint = parts.next()?.to_owned();
    let updated_at_ms = parts.next()?.parse().ok()?;
    let goal_id = parts.next()?.to_owned();
    Some((fingerprint, updated_at_ms, goal_id))
}

fn cursor_fingerprint(
    scopes: &[ScopeRef],
    statuses: &[GoalStatus],
) -> Result<String, GoalStoreError> {
    let scopes_json = serde_json::to_string(scopes).map_err(serialize_error)?;
    let statuses_json = serde_json::to_string(statuses).map_err(serialize_error)?;
    let mut bytes = scopes_json.into_bytes();
    bytes.push(b'|');
    bytes.extend(statuses_json.as_bytes());
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_goal(row: &Row<'_>) -> rusqlite::Result<GoalRecord> {
    Ok(GoalRecord {
        goal_id: GoalId::new(row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        scope: serde_json::from_str(&row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        parent_goal_id: None,
        title: row.get(2)?,
        status: parse_status(&row.get::<_, String>(3)?)?,
        revision: u64_value(row.get(4)?)?,
        plan: serde_json::from_str::<GoalPlan>(&row.get::<_, String>(5)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        acceptance: serde_json::from_str(&row.get::<_, String>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        progress: row
            .get::<_, Option<String>>(7)?
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        waiting_reason: row.get(8)?,
        next_step: row.get(9)?,
        priority: row.get(10)?,
        deadline_ms: row.get::<_, Option<i64>>(11)?.map(u64_value).transpose()?,
        created_from_interaction_id: row.get(12)?,
        cancellation_reason: row.get(13)?,
        cancellation_requested_at_ms: row.get::<_, Option<i64>>(14)?.map(u64_value).transpose()?,
        created_at_ms: u64_value(row.get(15)?)?,
        updated_at_ms: u64_value(row.get(16)?)?,
    })
}

fn parse_status(value: &str) -> rusqlite::Result<GoalStatus> {
    match value {
        "proposed" => Ok(GoalStatus::Proposed),
        "active" => Ok(GoalStatus::Active),
        "waiting" => Ok(GoalStatus::Waiting),
        "paused" => Ok(GoalStatus::Paused),
        "completed" => Ok(GoalStatus::Completed),
        "failed" => Ok(GoalStatus::Failed),
        "cancelled" => Ok(GoalStatus::Cancelled),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn status_code(status: GoalStatus) -> &'static str {
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

fn runtime_error(error: impl std::fmt::Display) -> GoalStoreError {
    GoalStoreError::Storage(error.to_string())
}

fn storage_error(error: rusqlite::Error) -> GoalStoreError {
    GoalStoreError::Storage(error.to_string())
}

fn serialize_error(error: serde_json::Error) -> GoalStoreError {
    GoalStoreError::Storage(error.to_string())
}
