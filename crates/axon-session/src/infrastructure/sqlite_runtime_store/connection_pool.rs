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
///
/// 预算按「最长合法写事务」定：一次 `PASSIVE` 回写（体量上限 64 MiB）叠上杀毒扫描可以
/// 吃掉数百毫秒，突发写入再排几拍，1 秒会给出误报。这里放宽到 2 秒，配合
/// `with_busy_retry` 吃掉的瞬时争用，实测里的 `database is locked` 属于「等不到」而不是
/// 「库坏了」。
const BUSY_TIMEOUT_MS: u64 = 2_000;
/// 生产默认的写锁等待上限。
const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_millis(BUSY_TIMEOUT_MS);
/// 每条连接共享的 PRAGMA 配置。
///
/// `synchronous=NORMAL` 是 WAL 下的推荐档位：`COMMIT` 不再逐事务 `fsync`，只在
/// checkpoint 与显式同步点落盘。代价是断电可能丢掉最近一小段已提交事务，但库结构
/// 不会损坏；对运行态事实（会话状态、队列、事件水位）这是可接受的语义弱化，崩溃恢复
/// 本就按「以日志为准回退水位、从结算记录重建叙述」设计。原先的 `FULL` 让每批流式帧
/// 都承担一次持久性代价，实测直接决定了单帧延迟上限。
///
/// WAL **不在此批次内**：它是库级持久属性，且切换到 WAL 需要独占锁，逐连接重复切换
/// 会在并发读写下反复争抢该锁并以 `SQLITE_BUSY` 失败（实测 busy_timeout 到期仍失败）。
/// WAL 由建库路径按需确保一次，见 `schema` 模块。
///
/// `journal_size_limit` 是 WAL 的硬上限：没有它，WAL 会随着每次写入无限增长
/// （实测 743 MB，与主库同量级），而一次 checkpoint 要把这些页全部回写主库，
/// 期间独占写锁数十秒，所有写者按 busy_timeout 排队失败。上限一旦生效，
/// checkpoint 只在有界体量上发生，写锁抖动不再波及无关写入方。
///
/// `wal_autocheckpoint = 0` 关闭 SQLite 的自动回写。自动回写是提交路径上的同步动作：
/// 触发阈值（旧值 1000 页，约 4 MB）的那次 `COMMIT` 必须等回写做完才返回，重写负载下
/// 每个写者都在替别人的回写排队，单帧延迟因此被回写时长直接决定。关掉之后回写改由
/// Daemon 的独立线程驱动（`axon-cli` 的 `daemon_host_runner_workers`）：每 30 秒比较
/// 一次「自上次回写以来的写批次数」，攒到 4096 批立即 `PASSIVE` 回写，未攒够则按
/// 5 分钟周期兜底。
///
/// 关掉自动回写不会让 WAL 无界增长，三处机制共同兜住体量：
/// 1. 上一条 `journal_size_limit`：每次成功回写重置 WAL 之后，第一次提交把文件截到
///    64 MiB，文件长度此后是「高水位」而不是当前帧量，同一轮里反复回写不再叠加。
///    这同时意味着文件长度不能当判据用：长度恰好等于 64 MiB 时 WAL 里可能一帧都没有，
///    而任意一次写入又会让长度严格大于 64 MiB；按长度触发会在「刚好被截到高水位」与
///    「回写长期不完整」两种情况下退化成每个 tick 都回写一次。判据因此取写批次数
///    （回写后归零），文件长度只作为占用空间的观察量；
/// 2. Daemon 回写线程：4096 个写批次与 5 分钟周期双触发。批次由写者在提交后自行累加，
///    不依赖回写线程去比对文件变化，因此不存在「写了但没被算到」的窗口；回写动作本身
///    的执行延迟上界是一个 tick（30 秒）；
/// 3. 进程退出：最后一个连接关闭时 SQLite 会做一次完整回写并删除 WAL，`axon run` 这类
///    短命进程不会留下没回写的日志。
///
/// 自动回写页数按进程分档：默认保留 SQLite 的 1000 页兜底（CLI 直跑的长轮次与批量修复
/// 没有别的回写驱动），Daemon 在启动时把它设为 0，改由独立线程按写批次与周期驱动。
static WAL_AUTOCHECKPOINT_PAGES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(1000);

/// 设定本进程连接的自动回写页数；必须在建立连接之前调用。
pub(super) fn set_wal_autocheckpoint_pages(pages: usize) {
    WAL_AUTOCHECKPOINT_PAGES.store(pages, std::sync::atomic::Ordering::Relaxed);
}

/// 连接建立时执行的 PRAGMA 批次。
fn connection_pragmas() -> String {
    format!(
        "PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL; \
         PRAGMA journal_size_limit = 67108864; PRAGMA wal_autocheckpoint = {};",
        WAL_AUTOCHECKPOINT_PAGES.load(std::sync::atomic::Ordering::Relaxed)
    )
}
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
            .execute_batch(&connection_pragmas())
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

#[cfg(test)]
mod wal_autocheckpoint_tests {
    use super::*;

    /// 自动回写落在提交线程里：连接建立后必须读回 0，证明它已经被关掉。
    ///
    /// 断言必须来自 SQLite 的读回值，不能只看常量字符串：pragma 名字写错不会编译失败，
    /// 只会静默沿用 SQLite 的编译期默认（1000 页），自动回写就这样留在提交路径上。
    #[test]
    fn wal_autocheckpoint_is_disabled_on_pooled_connections() {
        let directory = axon_test_support::test_temp_dir();
        let pool = ConnectionPool::new(directory.path().join("control.sqlite3"));
        let connection = pool.acquire().expect("连接池应能建立连接");
        let pages: i64 = connection
            .query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))
            .expect("读取 wal_autocheckpoint");

        assert_eq!(
            pages, 1000,
            "默认连接必须保留 SQLite 的兜底回写，否则纯 CLI 进程的 WAL 会随写入量无界增长"
        );
    }
}
