//! `resume_pointer` 的 SQLite 实现。
//!
//! 单行状态，主键固定，读写都走主键命中。

use super::SqliteRuntimeStore;
use super::helpers::i64_value;
use crate::ports::{ResumePointerStore, ResumePointerStoreError, RuntimeStore};
use axon_domain::DaemonSessionId;
use rusqlite::{OptionalExtension, params};

const RESUME_POINTER_KEY: &str = "current";

impl ResumePointerStore for SqliteRuntimeStore {
    fn read_resume_pointer(&self) -> Result<Option<DaemonSessionId>, ResumePointerStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let value: Option<String> = connection
            .query_row(
                "SELECT session_id FROM resume_pointer WHERE singleton_key=?1",
                params![RESUME_POINTER_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        value
            .map(|session_id| {
                DaemonSessionId::new(session_id).map_err(|_| {
                    ResumePointerStoreError::Storage("指针里的会话 ID 非法".to_owned())
                })
            })
            .transpose()
    }

    fn write_resume_pointer(
        &self,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<(), ResumePointerStoreError> {
        self.initialize().map_err(runtime_error)?;
        let now = i64_value(now_ms).map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        connection
            .execute(
                "INSERT INTO resume_pointer(singleton_key, session_id, updated_at_ms)
                 VALUES(?1, ?2, ?3)
                 ON CONFLICT(singleton_key) DO UPDATE SET
                     session_id = excluded.session_id,
                     updated_at_ms = excluded.updated_at_ms",
                params![RESUME_POINTER_KEY, session_id.as_str(), now],
            )
            .map(|_| ())
            .map_err(storage_error)
    }

    fn clear_resume_pointer(&self) -> Result<(), ResumePointerStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        connection
            .execute(
                "DELETE FROM resume_pointer WHERE singleton_key=?1",
                params![RESUME_POINTER_KEY],
            )
            .map(|_| ())
            .map_err(storage_error)
    }
}

fn storage_error(error: rusqlite::Error) -> ResumePointerStoreError {
    ResumePointerStoreError::Storage(error.to_string())
}

fn runtime_error(error: crate::ports::RuntimeStoreError) -> ResumePointerStoreError {
    ResumePointerStoreError::Storage(error.to_string())
}
