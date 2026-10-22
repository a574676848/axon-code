//! `jarvis_decisions` 的 SQLite repository。

use super::{SqliteRuntimeStore, helpers::i64_value};
use crate::ports::{DecisionStore, DecisionStoreError, RuntimeStore};
use axon_domain::{DecisionEffect, DecisionRecord, ScopeRef};
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};

impl DecisionStore for SqliteRuntimeStore {
    fn append_decision(
        &self,
        decision: &DecisionRecord,
        expected_revision: u64,
    ) -> Result<(), DecisionStoreError> {
        if decision.actor.trim().is_empty() {
            return Err(DecisionStoreError::Conflict(
                "Decision actor 不能为空".to_owned(),
            ));
        }
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| DecisionStoreError::Conflict("Decision revision 溢出".to_owned()))?;
        if decision.revision != next_revision {
            return Err(DecisionStoreError::Conflict(format!(
                "Decision revision 必须为 {}，收到 {}",
                next_revision, decision.revision
            )));
        }
        self.initialize().map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let scope_json = serde_json::to_string(&decision.scope).map_err(serialize_error)?;
        let current_revision: u64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(revision),0) FROM jarvis_decisions WHERE scope_json=?1",
                params![scope_json],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)
            .and_then(|value| {
                u64::try_from(value).map_err(|_| {
                    DecisionStoreError::Storage("Decision revision 超出范围".to_owned())
                })
            })?;
        if current_revision != expected_revision {
            return Err(DecisionStoreError::Conflict(format!(
                "Decision revision 已过期，期望 {}，实际 {}",
                expected_revision, current_revision
            )));
        }
        transaction
            .execute(
                "INSERT INTO jarvis_decisions(decision_id,scope_json,actor,effect,revision,expires_at_ms,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    decision.decision_id.as_str(),
                    scope_json,
                    decision.actor,
                    effect_code(decision.effect),
                    i64_value(decision.revision).map_err(storage_error)?,
                    decision.expires_at_ms.map(i64_value).transpose().map_err(storage_error)?,
                    i64_value(decision.created_at_ms).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    fn get_latest_decision(
        &self,
        scope: &ScopeRef,
        _now_ms: u64,
    ) -> Result<Option<DecisionRecord>, DecisionStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let scope_json = serde_json::to_string(scope).map_err(serialize_error)?;
        connection
            .query_row(
                "SELECT decision_id,scope_json,actor,effect,revision,expires_at_ms,created_at_ms FROM jarvis_decisions WHERE scope_json=?1 ORDER BY revision DESC, created_at_ms DESC, decision_id DESC LIMIT 1",
                params![scope_json],
                read_decision,
            )
            .optional()
            .map_err(storage_error)
    }
}

fn read_decision(row: &Row<'_>) -> rusqlite::Result<DecisionRecord> {
    Ok(DecisionRecord {
        decision_id: axon_domain::DecisionId::new(row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        scope: serde_json::from_str(&row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        actor: row.get(2)?,
        effect: parse_effect(&row.get::<_, String>(3)?)?,
        revision: u64::try_from(row.get::<_, i64>(4)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        expires_at_ms: row
            .get::<_, Option<i64>>(5)?
            .map(u64::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: u64::try_from(row.get::<_, i64>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn effect_code(effect: DecisionEffect) -> &'static str {
    match effect {
        DecisionEffect::Allow => "allow",
        DecisionEffect::Deny => "deny",
        DecisionEffect::Revoke => "revoke",
    }
}

fn parse_effect(value: &str) -> rusqlite::Result<DecisionEffect> {
    match value {
        "allow" => Ok(DecisionEffect::Allow),
        "deny" => Ok(DecisionEffect::Deny),
        "revoke" => Ok(DecisionEffect::Revoke),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn storage_error(error: impl std::fmt::Display) -> DecisionStoreError {
    DecisionStoreError::Storage(error.to_string())
}

fn serialize_error(error: serde_json::Error) -> DecisionStoreError {
    DecisionStoreError::Storage(error.to_string())
}
