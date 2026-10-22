//! Gateway 可恢复任务账本。

use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Gateway task SQLite 文件名。
pub const REMOTE_FEISHU_GATEWAY_TASK_STORE_FILE: &str = "remote_feishu_gateway_tasks.sqlite3";
/// Gateway task 账本 schema 版本。
const REMOTE_FEISHU_GATEWAY_TASK_STORE_SCHEMA_VERSION: u32 = 1;

/// Gateway task 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFeishuGatewayTaskStatus {
    /// 等待领取。
    Pending,
    /// 已被某个 Gateway 实例领取。
    Leased,
    /// 已持久化执行检查点，允许恢复时重新领取。
    Checkpointed,
    /// 已完成，不可再次领取。
    Completed,
}

impl RemoteFeishuGatewayTaskStatus {
    fn parse(value: String) -> AxonResult<Self> {
        match value.as_str() {
            "pending" => Ok(Self::Pending),
            "leased" => Ok(Self::Leased),
            "checkpointed" => Ok(Self::Checkpointed),
            "completed" => Ok(Self::Completed),
            _ => Err(AxonError::runtime_failure("Gateway task 状态无效")),
        }
    }
}

/// Gateway task 当前记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayTaskRecord {
    /// 稳定任务标识。
    pub task_id: String,
    /// 入站幂等键。
    pub idempotency_key: String,
    /// 当前状态。
    pub status: RemoteFeishuGatewayTaskStatus,
    /// 持久化输入引用。
    pub input_ref: String,
    /// 持久化检查点引用。
    pub checkpoint_ref: Option<String>,
    /// 持久化结果引用。
    pub result_ref: Option<String>,
    /// 当前租约持有者。
    pub lease_owner: Option<String>,
    /// 当前租约到期时间。
    pub lease_expires_at_ms: Option<u64>,
    /// 已领取次数。
    pub attempt_count: u32,
    /// 最后更新时间。
    pub updated_at_ms: u64,
}

/// Gateway task 账本 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuGatewayTaskStoreRuntime;

impl RemoteFeishuGatewayTaskStoreRuntime {
    /// 解析 task store 路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_FEISHU_GATEWAY_TASK_STORE_FILE)
    }

    /// 使用入站幂等键持久化待执行任务。
    pub fn enqueue(
        path: impl AsRef<Path>,
        task_id: &str,
        idempotency_key: &str,
        input_ref: &str,
        input_payload: &str,
        now_ms: u64,
    ) -> AxonResult<()> {
        validate(task_id, idempotency_key, input_ref, input_payload, now_ms)?;
        let connection = open(path.as_ref())?;
        connection.execute("INSERT INTO gateway_tasks(task_id,idempotency_key,status,input_ref,input_payload,attempt_count,updated_at_ms) VALUES(?1,?2,'pending',?3,?4,0,?5) ON CONFLICT(idempotency_key) DO NOTHING", params![task_id,idempotency_key,input_ref,input_payload,ms(now_ms)?]).map_err(db_error)?;
        Ok(())
    }

    /// 读取 task 的持久化输入。
    pub fn read_input(path: impl AsRef<Path>, task_id: &str) -> AxonResult<String> {
        if task_id.trim().is_empty() {
            return Err(AxonError::runtime_failure("Gateway task ID 无效"));
        }
        let connection = open(path.as_ref())?;
        connection
            .query_row(
                "SELECT input_payload FROM gateway_tasks WHERE task_id=?1",
                [task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| AxonError::runtime_failure("Gateway task 不存在"))
    }

    /// 原子恢复过期租约并领取一个可执行任务。
    pub fn claim(
        path: impl AsRef<Path>,
        owner: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> AxonResult<Option<RemoteFeishuGatewayTaskRecord>> {
        if owner.trim().is_empty() || now_ms == 0 || lease_ms == 0 {
            return Err(AxonError::runtime_failure("Gateway task claim 参数无效"));
        }
        let mut connection = open(path.as_ref())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction.execute("UPDATE gateway_tasks SET status='pending',lease_owner=NULL,lease_expires_at_ms=NULL,updated_at_ms=?1 WHERE status IN ('leased','checkpointed') AND lease_expires_at_ms<=?1", params![ms(now_ms)?]).map_err(db_error)?;
        let task_id = transaction.query_row("SELECT task_id FROM gateway_tasks WHERE status='pending' ORDER BY updated_at_ms LIMIT 1", [], |row| row.get::<_,String>(0)).optional().map_err(db_error)?;
        let Some(task_id) = task_id else {
            transaction.commit().map_err(db_error)?;
            return Ok(None);
        };
        let expires = now_ms.saturating_add(lease_ms);
        transaction.execute("UPDATE gateway_tasks SET status='leased',lease_owner=?2,lease_expires_at_ms=?3,attempt_count=attempt_count+1,updated_at_ms=?4 WHERE task_id=?1", params![task_id,owner,ms(expires)?,ms(now_ms)?]).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(Some(read_record(&connection, &task_id)?))
    }

    /// 仅允许当前租约持有者写入检查点。
    pub fn checkpoint(
        path: impl AsRef<Path>,
        task_id: &str,
        owner: &str,
        checkpoint_ref: &str,
        now_ms: u64,
    ) -> AxonResult<()> {
        validate_transition(task_id, owner, now_ms)?;
        if checkpoint_ref.trim().is_empty() {
            return Err(AxonError::runtime_failure("Gateway task 检查点引用无效"));
        }
        let changed = open(path.as_ref())?.execute("UPDATE gateway_tasks SET status='checkpointed',checkpoint_ref=?3,updated_at_ms=?4 WHERE task_id=?1 AND status='leased' AND lease_owner=?2", params![task_id,owner,checkpoint_ref,ms(now_ms)?]).map_err(db_error)?;
        require_current_lease(changed)
    }
    /// 仅允许当前租约持有者完成任务。
    pub fn complete(
        path: impl AsRef<Path>,
        task_id: &str,
        owner: &str,
        result_ref: &str,
        now_ms: u64,
    ) -> AxonResult<()> {
        validate_transition(task_id, owner, now_ms)?;
        if result_ref.trim().is_empty() {
            return Err(AxonError::runtime_failure("Gateway task 结果引用无效"));
        }
        let changed = open(path.as_ref())?.execute("UPDATE gateway_tasks SET status='completed',result_ref=?3,lease_owner=NULL,lease_expires_at_ms=NULL,updated_at_ms=?4 WHERE task_id=?1 AND status IN ('leased','checkpointed') AND lease_owner=?2", params![task_id,owner,result_ref,ms(now_ms)?]).map_err(db_error)?;
        require_current_lease(changed)
    }
}

fn require_current_lease(changed: usize) -> AxonResult<()> {
    (changed == 1)
        .then_some(())
        .ok_or_else(|| AxonError::runtime_failure("Gateway task 租约已失效或任务不存在"))
}

fn validate_transition(task_id: &str, owner: &str, now_ms: u64) -> AxonResult<()> {
    if task_id.trim().is_empty() || owner.trim().is_empty() || now_ms == 0 {
        Err(AxonError::runtime_failure("Gateway task 状态迁移参数无效"))
    } else {
        Ok(())
    }
}

fn open(path: &Path) -> AxonResult<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| AxonError::runtime_failure("Gateway task 目录创建失败"))?;
    }
    let connection = Connection::open(path).map_err(db_error)?;
    let schema = REMOTE_FEISHU_GATEWAY_TASK_STORE_SCHEMA_VERSION;
    connection.execute_batch(&format!("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA user_version={schema}; CREATE TABLE IF NOT EXISTS gateway_tasks(task_id TEXT PRIMARY KEY,idempotency_key TEXT UNIQUE NOT NULL,status TEXT NOT NULL CHECK(status IN ('pending','leased','checkpointed','completed')),input_ref TEXT NOT NULL,input_payload TEXT NOT NULL,checkpoint_ref TEXT,result_ref TEXT,lease_owner TEXT,lease_expires_at_ms INTEGER,attempt_count INTEGER NOT NULL,updated_at_ms INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS gateway_tasks_claim_idx ON gateway_tasks(status,updated_at_ms);")) .map_err(db_error)?;
    Ok(connection)
}

fn read_record(
    connection: &Connection,
    task_id: &str,
) -> AxonResult<RemoteFeishuGatewayTaskRecord> {
    connection.query_row("SELECT task_id,idempotency_key,status,input_ref,checkpoint_ref,result_ref,lease_owner,lease_expires_at_ms,attempt_count,updated_at_ms FROM gateway_tasks WHERE task_id=?1", [task_id], |row| Ok((row.get(0)?,row.get(1)?,row.get::<_,String>(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get::<_,Option<i64>>(7)?,row.get::<_,i64>(8)?,row.get::<_,i64>(9)?))).map_err(db_error).and_then(|(task_id,idempotency_key,status,input_ref,checkpoint_ref,result_ref,lease_owner,lease_expires_at_ms,attempt_count,updated_at_ms)| Ok(RemoteFeishuGatewayTaskRecord { task_id,idempotency_key,status:RemoteFeishuGatewayTaskStatus::parse(status)?,input_ref,checkpoint_ref,result_ref,lease_owner,lease_expires_at_ms:lease_expires_at_ms.map(u64::try_from).transpose().map_err(|_| AxonError::runtime_failure("Gateway task 租约时间无效"))?,attempt_count:u32::try_from(attempt_count).map_err(|_| AxonError::runtime_failure("Gateway task 尝试次数无效"))?,updated_at_ms:u64::try_from(updated_at_ms).map_err(|_| AxonError::runtime_failure("Gateway task 更新时间无效"))? }))
}

fn validate(
    task_id: &str,
    idempotency_key: &str,
    input_ref: &str,
    input_payload: &str,
    now_ms: u64,
) -> AxonResult<()> {
    if task_id.trim().is_empty()
        || idempotency_key.trim().is_empty()
        || input_ref.trim().is_empty()
        || input_payload.trim().is_empty()
        || now_ms == 0
    {
        Err(AxonError::runtime_failure("Gateway task 记录无效"))
    } else {
        Ok(())
    }
}
fn ms(value: u64) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::runtime_failure("Gateway task 时间超限"))
}
fn db_error(_: rusqlite::Error) -> AxonError {
    AxonError::runtime_failure("Gateway task SQLite 操作失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASK_ID: &str = "gateway-task-1";
    const IDEMPOTENCY_KEY: &str = "request-1";
    const INPUT_PAYLOAD: &str = r#"{\"requestId\":\"request-1\"}"#;
    const FIRST_OWNER: &str = "gateway:1:100";
    const SECOND_OWNER: &str = "gateway:2:200";
    const INITIAL_TIME_MS: u64 = 100;
    const LEASE_DURATION_MS: u64 = 50;

    #[test]
    fn enqueue_is_idempotent_and_retains_the_original_input() {
        let directory = axon_test_support::test_temp_dir();
        let path = RemoteFeishuGatewayTaskStoreRuntime::resolve_store_path(directory.path());

        enqueue(
            &path,
            TASK_ID,
            IDEMPOTENCY_KEY,
            INPUT_PAYLOAD,
            INITIAL_TIME_MS,
        );
        enqueue(
            &path,
            "other-task",
            IDEMPOTENCY_KEY,
            "other",
            INITIAL_TIME_MS + 1,
        );

        assert_eq!(
            RemoteFeishuGatewayTaskStoreRuntime::read_input(&path, TASK_ID).expect("读取输入"),
            INPUT_PAYLOAD
        );
        let claimed = RemoteFeishuGatewayTaskStoreRuntime::claim(
            &path,
            FIRST_OWNER,
            INITIAL_TIME_MS,
            LEASE_DURATION_MS,
        )
        .expect("领取任务")
        .expect("存在任务");
        assert_eq!(claimed.task_id, TASK_ID);
        assert_eq!(claimed.attempt_count, 1);
        assert!(
            RemoteFeishuGatewayTaskStoreRuntime::claim(
                &path,
                SECOND_OWNER,
                INITIAL_TIME_MS + 1,
                LEASE_DURATION_MS,
            )
            .expect("再次领取")
            .is_none()
        );
    }

    #[test]
    fn expired_lease_is_recovered_and_previous_owner_is_fenced() {
        let directory = axon_test_support::test_temp_dir();
        let path = RemoteFeishuGatewayTaskStoreRuntime::resolve_store_path(directory.path());
        enqueue(
            &path,
            TASK_ID,
            IDEMPOTENCY_KEY,
            INPUT_PAYLOAD,
            INITIAL_TIME_MS,
        );
        RemoteFeishuGatewayTaskStoreRuntime::claim(
            &path,
            FIRST_OWNER,
            INITIAL_TIME_MS,
            LEASE_DURATION_MS,
        )
        .expect("首次领取");

        let reclaimed = RemoteFeishuGatewayTaskStoreRuntime::claim(
            &path,
            SECOND_OWNER,
            INITIAL_TIME_MS + LEASE_DURATION_MS,
            LEASE_DURATION_MS,
        )
        .expect("恢复过期租约")
        .expect("恢复后的任务");

        assert_eq!(reclaimed.attempt_count, 2);
        assert_eq!(reclaimed.lease_owner.as_deref(), Some(SECOND_OWNER));
        assert!(
            RemoteFeishuGatewayTaskStoreRuntime::complete(
                &path,
                TASK_ID,
                FIRST_OWNER,
                "result:old",
                INITIAL_TIME_MS + LEASE_DURATION_MS,
            )
            .is_err()
        );
    }

    #[test]
    fn checkpointed_task_is_reclaimed_then_completed_once() {
        let directory = axon_test_support::test_temp_dir();
        let path = RemoteFeishuGatewayTaskStoreRuntime::resolve_store_path(directory.path());
        enqueue(
            &path,
            TASK_ID,
            IDEMPOTENCY_KEY,
            INPUT_PAYLOAD,
            INITIAL_TIME_MS,
        );
        RemoteFeishuGatewayTaskStoreRuntime::claim(
            &path,
            FIRST_OWNER,
            INITIAL_TIME_MS,
            LEASE_DURATION_MS,
        )
        .expect("领取任务");
        RemoteFeishuGatewayTaskStoreRuntime::checkpoint(
            &path,
            TASK_ID,
            FIRST_OWNER,
            "checkpoint:governed",
            INITIAL_TIME_MS + 1,
        )
        .expect("写入检查点");

        let reclaimed = RemoteFeishuGatewayTaskStoreRuntime::claim(
            &path,
            SECOND_OWNER,
            INITIAL_TIME_MS + LEASE_DURATION_MS,
            LEASE_DURATION_MS,
        )
        .expect("领取检查点任务")
        .expect("存在检查点任务");
        assert_eq!(reclaimed.status, RemoteFeishuGatewayTaskStatus::Leased);
        assert_eq!(
            reclaimed.checkpoint_ref.as_deref(),
            Some("checkpoint:governed")
        );
        RemoteFeishuGatewayTaskStoreRuntime::complete(
            &path,
            TASK_ID,
            SECOND_OWNER,
            "result:completed",
            INITIAL_TIME_MS + LEASE_DURATION_MS + 1,
        )
        .expect("完成任务");
        assert!(
            RemoteFeishuGatewayTaskStoreRuntime::claim(
                &path,
                FIRST_OWNER,
                INITIAL_TIME_MS + 4,
                LEASE_DURATION_MS,
            )
            .expect("完成任务不再领取")
            .is_none()
        );
    }

    fn enqueue(path: &Path, task_id: &str, idempotency_key: &str, payload: &str, now_ms: u64) {
        RemoteFeishuGatewayTaskStoreRuntime::enqueue(
            path,
            task_id,
            idempotency_key,
            &format!("gateway-task-input:{task_id}"),
            payload,
            now_ms,
        )
        .expect("入账任务");
    }
}
