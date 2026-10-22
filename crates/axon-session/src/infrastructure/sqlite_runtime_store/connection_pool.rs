//! 运行时事实源的进程内连接池。
//!
//! SQLite 单写者模型下逐操作重建连接会在 Windows 上引入可观的打开与日志配置
//! 开销（实测 500 次提交相差两个数量级）。池以「取用 / 归还」替代全局互斥：
//! 同一线程嵌套取用会拿到另一条连接，因此不会自死锁；多条连接在 WAL 下可并行
//! 读取，写事务仍由 SQLite 自身的写锁串行化。

use super::{RuntimeStoreError, storage};
use rusqlite::Connection;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// 空闲连接保留上限；超出的归还连接直接关闭，避免长尾线程堆积文件句柄。
const MAX_IDLE_CONNECTIONS: usize = 8;
/// 单条连接等待写锁的毫秒数；超时返回 Busy 交由上层可观测地失败，而不是无限阻塞。
const BUSY_TIMEOUT_MS: u64 = 5_000;
/// 生产默认的写锁等待上限。
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_millis(BUSY_TIMEOUT_MS);
/// 每条连接共享的 PRAGMA 配置。
///
/// `synchronous=FULL` 保证已提交的外部副作用事实在断电级故障前落盘，崩溃恢复只依赖
/// 已提交记录，不依赖重放猜测。WAL **不在此批次内**：它是库级持久属性，且切换到 WAL
/// 需要独占锁，逐连接重复切换会在并发读写下反复争抢该锁并以 `SQLITE_BUSY` 失败
/// （实测 busy_timeout 到期仍失败）。WAL 由建库路径按需确保一次，见 `schema` 模块。
const CONNECTION_PRAGMAS: &str = "PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;";
/// 守卫在连接被取走后再解引用时的显式失败信息。
const CONNECTION_TAKEN: &str = "运行时连接在守卫释放前被取走";

/// 进程内可复用的 SQLite 连接池。
pub(crate) struct ConnectionPool {
    path: PathBuf,
    busy_timeout: Duration,
    idle: Mutex<Vec<Connection>>,
}

impl std::fmt::Debug for ConnectionPool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectionPool")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ConnectionPool {
    /// 创建指向同一数据库文件路径的连接池。
    pub(crate) fn new(path: PathBuf) -> Self {
        Self::with_busy_timeout(path, DEFAULT_BUSY_TIMEOUT)
    }

    /// 使用自定义写锁等待上限创建连接池。
    pub(crate) fn with_busy_timeout(path: PathBuf, busy_timeout: Duration) -> Self {
        Self {
            path,
            busy_timeout,
            idle: Mutex::new(Vec::new()),
        }
    }

    /// 取出一条可用连接；空闲池为空时新建一条。
    pub(crate) fn acquire(&self) -> Result<PooledConnection<'_>, RuntimeStoreError> {
        let pooled = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let connection = match pooled {
            Some(connection) => connection,
            None => self.open()?,
        };
        Ok(PooledConnection {
            pool: self,
            connection: Some(connection),
        })
    }

    fn open(&self) -> Result<Connection, RuntimeStoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| RuntimeStoreError::Storage(error.to_string()))?;
        }
        let connection = Connection::open(&self.path)
            .map_err(|error| RuntimeStoreError::Storage(error.to_string()))?;
        // 写锁等待上限先于任何语句：连接上的 PRAGMA 与初始化语句都可能需要等锁，
        // 未设上限时会以默认的零等待撞上并发写者。
        connection
            .busy_timeout(self.busy_timeout)
            .map_err(storage)?;
        connection
            .execute_batch(CONNECTION_PRAGMAS)
            .map_err(storage)?;
        Ok(connection)
    }

    fn release(&self, connection: Connection) {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        if idle.len() < MAX_IDLE_CONNECTIONS {
            idle.push(connection);
        }
    }
}

/// 归还式连接守卫；离开作用域时把连接放回池中复用。
pub(crate) struct PooledConnection<'a> {
    pool: &'a ConnectionPool,
    connection: Option<Connection>,
}

impl Deref for PooledConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().expect(CONNECTION_TAKEN)
    }
}

impl DerefMut for PooledConnection<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().expect(CONNECTION_TAKEN)
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.pool.release(connection);
        }
    }
}
