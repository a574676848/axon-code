//! `persona.sqlite3`：人格状态、转移日志与习得风格的持久化。
//!
//! 建库范式与运行态账本一致：首次建库在 IMMEDIATE 事务内完成，schema 身份不符即
//! 快速失败，不做运行时迁移扫描。

use std::{fmt, path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

use crate::{
    constants::{SETTLEMENT_RETRY_LIMIT, TRANSITION_RETENTION},
    domain::{
        persona_event::PersonaEvent,
        persona_state::PersonaState,
        scene_frame::Audience,
        state_machine::{PersonaError, transition},
    },
};

#[path = "sqlite_persona_style.rs"]
mod sqlite_persona_style;

/// 人格库 schema 身份。
pub const PERSONA_SCHEMA_IDENTITY: &str = "axon-persona-v1";

const SCHEMA_DDL: &str = "
    CREATE TABLE IF NOT EXISTS persona_schema_identity (identity TEXT PRIMARY KEY NOT NULL);
    CREATE TABLE IF NOT EXISTS persona_state (
        owner_id TEXT PRIMARY KEY NOT NULL,
        revision INTEGER NOT NULL,
        warmth_offset INTEGER NOT NULL,
        familiarity INTEGER NOT NULL,
        user_affect TEXT NOT NULL,
        failure_streak INTEGER NOT NULL,
        drift_guard TEXT,
        last_cause TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS persona_transitions (
        transition_id TEXT PRIMARY KEY NOT NULL,
        owner_id TEXT NOT NULL,
        from_revision INTEGER NOT NULL,
        to_revision INTEGER NOT NULL,
        event_json TEXT NOT NULL,
        cause TEXT NOT NULL,
        idempotency_key TEXT NOT NULL UNIQUE,
        occurred_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS persona_transitions_owner_time
        ON persona_transitions(owner_id, occurred_at_ms DESC);
    CREATE TABLE IF NOT EXISTS persona_learned_style (
        style_id TEXT PRIMARY KEY NOT NULL,
        owner_id TEXT NOT NULL,
        text TEXT NOT NULL,
        evidence_ref TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('pending','published','retired')),
        created_at_ms INTEGER NOT NULL,
        published_at_ms INTEGER
    );
    CREATE INDEX IF NOT EXISTS persona_learned_style_owner_status
        ON persona_learned_style(owner_id, status, created_at_ms DESC);
    CREATE TABLE IF NOT EXISTS persona_style_publication (
        owner_id TEXT PRIMARY KEY NOT NULL,
        revision INTEGER NOT NULL,
        published_at_ms INTEGER NOT NULL
    );
";

/// 人格库错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonaStoreError {
    /// schema 身份不符，需要手工升级或重建。
    SchemaMismatch {
        /// 库中记录的标识。
        found: String,
    },
    /// 目录或数据库不可用。
    Storage(String),
    /// 并发冲突在重试上限内未消解。
    Conflict {
        /// 尝试次数。
        attempts: u32,
    },
    /// 状态机拒绝该事件。
    Domain(PersonaError),
}

impl fmt::Display for PersonaStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaMismatch { found } => write!(
                formatter,
                "人格库 schema 身份不符（{found}），需要 {PERSONA_SCHEMA_IDENTITY}"
            ),
            Self::Storage(detail) => write!(formatter, "人格库不可用：{detail}"),
            Self::Conflict { attempts } => {
                write!(formatter, "人格库并发冲突：{attempts} 次尝试后仍失败")
            }
            Self::Domain(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PersonaStoreError {}

impl From<rusqlite::Error> for PersonaStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

/// 一次结算的落地结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementOutcome {
    /// 已写入并返回新状态。
    Applied {
        /// 结算后的状态。
        state: PersonaState,
        /// 本次变化原因。
        cause: String,
    },
    /// 该幂等键此前已生效，本次不重复写入。
    AlreadyApplied,
    /// 状态未变化（群聊只读），未写入。
    Unchanged,
}

/// 一条人格转移记录（只读视图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionRecord {
    /// 转移后的版本。
    pub revision: u64,
    /// 变化原因。
    pub cause: String,
    /// 发生时刻，Unix 毫秒。
    pub occurred_at_ms: u64,
}

/// 一条已发布的习得风格。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleEntry {
    /// 风格条目标识。
    pub style_id: String,
    /// 风格正文。
    pub text: String,
    /// 发布时间，Unix 毫秒。
    pub published_at_ms: Option<u64>,
}

/// 已发布习得风格集合。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishedStyle {
    /// 发布版本。
    pub revision: u64,
    /// 已发布条目。
    pub entries: Vec<StyleEntry>,
}

/// 一条习得风格的只读视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleListing {
    /// 条目标识。
    pub style_id: String,
    /// 条目正文。
    pub text: String,
    /// 状态：pending / published / retired。
    pub status: String,
    /// 发布时间。
    pub published_at_ms: Option<u64>,
}

/// 人格库默认路径：`<axon root>/data/persona/persona.sqlite3`。
#[must_use]
pub fn persona_store_path(axon_root: &Path) -> std::path::PathBuf {
    axon_root
        .join("data")
        .join("persona")
        .join("persona.sqlite3")
}
/// 人格库句柄。
pub struct PersonaStore {
    connection: Connection,
}

impl fmt::Debug for PersonaStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersonaStore")
            .finish_non_exhaustive()
    }
}

impl PersonaStore {
    /// 打开或创建人格库。
    ///
    /// # Errors
    ///
    /// 目录不可建、数据库不可打开、schema 身份不符或 DDL 失败时返回
    /// [`PersonaStoreError`]。
    pub fn open(path: &Path) -> Result<Self, PersonaStoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| PersonaStoreError::Storage(error.to_string()))?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(
            crate::constants::PERSONA_WRITE_BUSY_TIMEOUT_MS,
        ))?;
        ensure_wal(&connection)?;
        ensure_schema(&mut connection)?;
        Ok(Self { connection })
    }

    /// 只读打开已存在的人格库：只校验 schema 身份，不建库、不开写事务。
    ///
    /// 轮前装配走这条路径，避免在首字关键路径上获取写锁。busy 等待被压到
    /// [`crate::constants::PERSONA_READ_BUSY_TIMEOUT_MS`]，让「库被锁」最多只耗这一小段，
    /// 而不是拖满结算路径的 2 秒。
    ///
    /// # Errors
    ///
    /// 库不存在、schema 身份不符或打开失败时返回 [`PersonaStoreError`]。
    pub fn open_read_only(path: &Path) -> Result<Self, PersonaStoreError> {
        if !path.exists() {
            return Err(PersonaStoreError::Storage("人格库不存在".to_owned()));
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(
            crate::constants::PERSONA_READ_BUSY_TIMEOUT_MS,
        ))?;
        verify_schema_identity(&connection)?;
        Ok(Self { connection })
    }

    /// 读取 owner 的状态；不存在时返回 `None`（不得伪造基线）。
    ///
    /// # Errors
    ///
    /// 查询或解码失败时返回 [`PersonaStoreError`]。
    pub fn load_state(&self, owner_id: &str) -> Result<Option<PersonaState>, PersonaStoreError> {
        let raw = self
            .connection
            .query_row(
                "SELECT revision, warmth_offset, familiarity, user_affect, failure_streak,
                        drift_guard, last_cause, updated_at_ms
                 FROM persona_state WHERE owner_id = ?1",
                params![owner_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        Ok(Some(decode_state(raw)?))
    }

    /// 结算一个事件：读取状态、执行状态机、幂等写入。
    ///
    /// # Errors
    ///
    /// 状态机拒绝事件、存储不可用，或并发冲突超过重试上限时返回
    /// [`PersonaStoreError`]。
    pub fn settle(
        &mut self,
        owner_id: &str,
        event: &PersonaEvent,
        audience: Audience,
        idempotency_key: &str,
    ) -> Result<SettlementOutcome, PersonaStoreError> {
        for _ in 0..SETTLEMENT_RETRY_LIMIT {
            if self.is_applied(idempotency_key)? {
                return Ok(SettlementOutcome::AlreadyApplied);
            }
            let current = self.load_state(owner_id)?;
            let now_ms = event_now_ms(event);
            let base = current
                .clone()
                .unwrap_or_else(|| PersonaState::baseline(now_ms));
            let transition =
                transition(&base, event, audience).map_err(PersonaStoreError::Domain)?;
            if !transition.changed {
                return Ok(SettlementOutcome::Unchanged);
            }
            match self.write_settlement(
                owner_id,
                &base,
                &transition.state,
                &transition.cause,
                event,
                idempotency_key,
            )? {
                true => {
                    return Ok(SettlementOutcome::Applied {
                        state: transition.state,
                        cause: transition.cause,
                    });
                }
                false => continue,
            }
        }
        Err(PersonaStoreError::Conflict {
            attempts: SETTLEMENT_RETRY_LIMIT,
        })
    }

    /// 读取某 owner 最近的转移日志条数，用于保留策略核验。
    ///
    /// # Errors
    ///
    /// 查询失败时返回 [`PersonaStoreError`]。
    pub fn transition_count(&self, owner_id: &str) -> Result<u64, PersonaStoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM persona_transitions WHERE owner_id = ?1",
            params![owner_id],
            |row| row.get(0),
        )?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// 测试专用：插入一条合成的转移记录，用于验证保留裁剪。
    #[cfg(test)]
    pub(crate) fn insert_synthetic_transition(
        &mut self,
        owner_id: &str,
        key: &str,
        occurred_at_ms: u64,
    ) -> Result<(), PersonaStoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO persona_transitions (
                transition_id, owner_id, from_revision, to_revision,
                event_json, cause, idempotency_key, occurred_at_ms
             ) VALUES (?1, ?2, 0, 0, '{}', 'synthetic', ?3, ?4)",
            params![
                format!("{owner_id}:{key}"),
                owner_id,
                key,
                to_i64(occurred_at_ms)?,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// 测试专用：按给定上限裁剪转移日志，并返回剩余条数。
    #[cfg(test)]
    pub(crate) fn prune_for_test(
        &mut self,
        owner_id: &str,
        limit: u32,
    ) -> Result<u64, PersonaStoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        prune_transitions(&transaction, owner_id, limit)?;
        transaction.commit()?;
        self.transition_count(owner_id)
    }

    fn is_applied(&self, idempotency_key: &str) -> Result<bool, PersonaStoreError> {
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM persona_transitions WHERE idempotency_key = ?1)",
            params![idempotency_key],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    /// 读取最近的转移记录，按时间倒序。
    ///
    /// # Errors
    ///
    /// 查询失败时返回 [`PersonaStoreError`]。
    pub fn recent_transitions(
        &self,
        owner_id: &str,
        limit: u32,
    ) -> Result<Vec<TransitionRecord>, PersonaStoreError> {
        let mut statement = self.connection.prepare(
            "SELECT to_revision, cause, occurred_at_ms FROM persona_transitions
             WHERE owner_id = ?1 ORDER BY occurred_at_ms DESC, transition_id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![owner_id, i64::from(limit)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (revision, cause, occurred_at_ms) = row?;
            records.push(TransitionRecord {
                revision: u64::try_from(revision).unwrap_or(0),
                cause,
                occurred_at_ms: u64::try_from(occurred_at_ms).unwrap_or(0),
            });
        }
        Ok(records)
    }

    /// 读取已发布的习得风格与发布版本。
    ///
    /// # Errors
    ///
    /// 查询失败时返回 [`PersonaStoreError`]。
    pub fn published_style(&self, owner_id: &str) -> Result<PublishedStyle, PersonaStoreError> {
        let mut statement = self.connection.prepare(
            "SELECT style_id, text, published_at_ms FROM persona_learned_style
             WHERE owner_id = ?1 AND status = 'published' ORDER BY created_at_ms DESC",
        )?;
        let rows = statement.query_map(params![owner_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            let (style_id, text, published_at_ms) = row?;
            entries.push(StyleEntry {
                style_id,
                text,
                published_at_ms: published_at_ms.and_then(|value| u64::try_from(value).ok()),
            });
        }
        let publication: Option<i64> = self
            .connection
            .query_row(
                "SELECT revision FROM persona_style_publication WHERE owner_id = ?1",
                params![owner_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(PublishedStyle {
            revision: publication.map_or(0, |value| u64::try_from(value).unwrap_or(0)),
            entries,
        })
    }

    /// 返回是否写入成功；`false` 表示 revision 冲突，需要重试。
    fn write_settlement(
        &mut self,
        owner_id: &str,
        base: &PersonaState,
        next: &PersonaState,
        cause: &str,
        event: &PersonaEvent,
        idempotency_key: &str,
    ) -> Result<bool, PersonaStoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: Option<i64> = transaction
            .query_row(
                "SELECT revision FROM persona_state WHERE owner_id = ?1",
                params![owner_id],
                |row| row.get(0),
            )
            .optional()?;
        let stored_revision = stored.map_or(0, |value| u64::try_from(value).unwrap_or(0));
        if stored_revision != base.revision {
            transaction.rollback()?;
            return Ok(false);
        }
        let event_json = serde_json::to_string(event)
            .map_err(|error| PersonaStoreError::Storage(error.to_string()))?;
        transaction.execute(
            "INSERT INTO persona_state (
                owner_id, revision, warmth_offset, familiarity, user_affect,
                failure_streak, drift_guard, last_cause, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(owner_id) DO UPDATE SET
                revision = excluded.revision,
                warmth_offset = excluded.warmth_offset,
                familiarity = excluded.familiarity,
                user_affect = excluded.user_affect,
                failure_streak = excluded.failure_streak,
                drift_guard = excluded.drift_guard,
                last_cause = excluded.last_cause,
                updated_at_ms = excluded.updated_at_ms",
            params![
                owner_id,
                to_i64(next.revision)?,
                i64::from(next.warmth_offset),
                i64::from(next.familiarity),
                next.user_affect.as_token(),
                i64::from(next.failure_streak),
                next.drift_guard,
                next.last_cause,
                to_i64(next.updated_at_ms)?,
            ],
        )?;
        let inserted = transaction.execute(
            "INSERT INTO persona_transitions (
                transition_id, owner_id, from_revision, to_revision,
                event_json, cause, idempotency_key, occurred_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(idempotency_key) DO NOTHING",
            params![
                format!("{owner_id}:{idempotency_key}"),
                owner_id,
                to_i64(base.revision)?,
                to_i64(next.revision)?,
                event_json,
                cause,
                idempotency_key,
                to_i64(next.updated_at_ms)?,
            ],
        )?;
        if inserted == 0 {
            transaction.rollback()?;
            return Ok(false);
        }
        prune_transitions(&transaction, owner_id, TRANSITION_RETENTION)?;
        transaction.commit()?;
        Ok(true)
    }
}

fn transition_prune_sql(limit: u32) -> String {
    format!(
        "DELETE FROM persona_transitions
         WHERE owner_id = ?1 AND transition_id NOT IN (
             SELECT transition_id FROM persona_transitions
             WHERE owner_id = ?1 ORDER BY occurred_at_ms DESC, transition_id DESC LIMIT {limit}
         )"
    )
}

fn prune_transitions(
    transaction: &rusqlite::Transaction<'_>,
    owner_id: &str,
    limit: u32,
) -> Result<(), PersonaStoreError> {
    transaction.execute(&transition_prune_sql(limit), params![owner_id])?;
    Ok(())
}

fn decode_state(
    raw: (i64, i64, i64, String, i64, Option<String>, String, i64),
) -> Result<PersonaState, PersonaStoreError> {
    let (revision, warmth_offset, familiarity, affect, failure_streak, drift_guard, cause, updated) =
        raw;
    Ok(PersonaState {
        revision: u64::try_from(revision)
            .map_err(|error| PersonaStoreError::Storage(error.to_string()))?,
        warmth_offset: i8::try_from(warmth_offset)
            .map_err(|error| PersonaStoreError::Storage(error.to_string()))?,
        familiarity: u8::try_from(familiarity)
            .map_err(|error| PersonaStoreError::Storage(error.to_string()))?,
        user_affect: crate::domain::persona_state::UserAffect::parse(&affect),
        failure_streak: u8::try_from(failure_streak)
            .map_err(|error| PersonaStoreError::Storage(error.to_string()))?,
        drift_guard,
        last_cause: cause,
        updated_at_ms: u64::try_from(updated)
            .map_err(|error| PersonaStoreError::Storage(error.to_string()))?,
    })
}

fn to_i64(value: u64) -> Result<i64, PersonaStoreError> {
    i64::try_from(value).map_err(|error| PersonaStoreError::Storage(error.to_string()))
}

fn event_now_ms(event: &PersonaEvent) -> u64 {
    match event {
        PersonaEvent::TurnSettled(input) => input.now_ms,
        PersonaEvent::Reset { now_ms, .. } => *now_ms,
    }
}

fn ensure_wal(connection: &Connection) -> Result<(), PersonaStoreError> {
    let current: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !current.eq_ignore_ascii_case("wal") {
        let _mode: String =
            connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    }
    Ok(())
}

fn verify_schema_identity(connection: &Connection) -> Result<(), PersonaStoreError> {
    let identity_table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='persona_schema_identity')",
        [],
        |row| row.get(0),
    )?;
    if !identity_table_exists {
        return Err(PersonaStoreError::SchemaMismatch {
            found: "缺少身份表".to_owned(),
        });
    }
    let identity: String = connection.query_row(
        "SELECT identity FROM persona_schema_identity LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    if identity != PERSONA_SCHEMA_IDENTITY {
        return Err(PersonaStoreError::SchemaMismatch { found: identity });
    }
    Ok(())
}

fn ensure_schema(connection: &mut Connection) -> Result<(), PersonaStoreError> {
    let identity_table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='persona_schema_identity')",
        [],
        |row| row.get(0),
    )?;
    if identity_table_exists {
        let identity: String = connection.query_row(
            "SELECT identity FROM persona_schema_identity LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        if identity != PERSONA_SCHEMA_IDENTITY {
            return Err(PersonaStoreError::SchemaMismatch { found: identity });
        }
    } else {
        let tables: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if tables > 0 {
            return Err(PersonaStoreError::SchemaMismatch {
                found: "未知库".to_owned(),
            });
        }
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(SCHEMA_DDL)?;
    transaction.execute(
        "INSERT OR IGNORE INTO persona_schema_identity(identity) VALUES(?1)",
        params![PERSONA_SCHEMA_IDENTITY],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "sqlite_persona_store_tests.rs"]
mod tests;
