//! 远控状态 SQLite 的进程内连接池。
//!
//! 投递路径（outbox、订阅索引、状态索引）逐操作重建连接时，Windows 上的文件打开
//! 与日志配置会占据绝大部分耗时，实测每次投递相差一个数量级。池按数据库路径复用
//! 连接：取用与归还都不阻塞其他线程，因此嵌套调用不会自死锁；`WAL` 让读取不被
//! 写入阻塞，`synchronous=FULL` 保证已落盘的投递结果不因断电丢失。
//!
//! 连接在池中存活期间，库文件仍可能被外部替换或删除（例如运维重建空库）。因此每个
//! 空闲连接都记录建连时的文件身份，取用时身份不一致就丢弃并重新打开，避免继续写入
//! 已被删除的旧文件。

use axon_domain::{AxonError, AxonResult};
use rusqlite::Connection;
use std::{
    collections::HashMap,
    fs,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError},
    time::{Duration, Instant},
};

/// 每个数据库路径保留的空闲连接上限；超出的归还连接直接关闭。
const MAX_IDLE_CONNECTIONS_PER_PATH: usize = 4;
/// 空闲连接的最长保留时长；超过即关闭，释放库文件句柄。
///
/// 池化连接会一直持有库文件句柄（Windows 上表现为文件无法删除）。运维重建空库、删除数据
/// 目录、以及测试清理临时目录都依赖句柄在空闲后自动释放，因此空闲连接必须有回收上限。
const MAX_IDLE_AGE: Duration = Duration::from_secs(20);
/// 空闲回收线程的检查间隔。
const JANITOR_INTERVAL: Duration = Duration::from_secs(5);
/// 单条连接等待写锁的毫秒数；超时返回错误交由上层可观测地失败。
const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
/// 每条连接共享的 PRAGMA 配置：已提交的外部副作用事实必须断电级落盘。
const CONNECTION_PRAGMAS: &str = "PRAGMA synchronous = FULL;";
/// 守卫在连接被取走后再次解引用时的显式失败信息。
const CONNECTION_TAKEN: &str = "远控状态连接在守卫释放前被取走";
/// 无法读取数据库文件身份时的失败信息。
const ERROR_FILE_IDENTITY: &str = "无法读取远控状态数据库文件信息";

static POOLS: LazyLock<Mutex<HashMap<PathBuf, Arc<ConnectionPool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// 空闲回收线程的启动开关；每进程最多一个。
static JANITOR_STARTED: OnceLock<()> = OnceLock::new();

/// 库文件的稳定身份；重建同一个路径的文件会得到新的身份，与库内容写入无关。
type FileIdentity = u128;

/// 取用指定数据库的连接；空闲池为空时新建并执行初始化。
pub(crate) fn acquire(
    path: &Path,
    initialize: &dyn Fn(&Path, &Connection) -> AxonResult<()>,
) -> AxonResult<PooledConnection> {
    start_janitor_once();
    let pool = {
        let mut pools = POOLS.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(
            pools
                .entry(path.to_path_buf())
                .or_insert_with(|| Arc::new(ConnectionPool::new(path.to_path_buf()))),
        )
    };
    pool.acquire(initialize)
}

/// 启动空闲回收线程：周期性关闭超时空闲连接，保证库文件句柄不会长期占用。
fn start_janitor_once() {
    JANITOR_STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("axon-remote-state-pool-janitor".to_owned())
            .spawn(|| {
                loop {
                    std::thread::sleep(JANITOR_INTERVAL);
                    evict_idle_connections(Instant::now());
                }
            });
    });
}

/// 关闭所有路径上超过空闲上限的连接。
fn evict_idle_connections(now: Instant) {
    let pools: Vec<Arc<ConnectionPool>> = POOLS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .cloned()
        .collect();
    for pool in pools {
        pool.evict_idle_older_than(now);
    }
}

/// 关闭并移除指定库路径的连接池。
///
/// 池化连接会一直持有库文件句柄（Windows 上表现为文件无法删除），删除或重建库文件之前必须
/// 先调用本函数；已在借用中的连接由 `PooledConnection` 归还后随池一起关闭。
pub(crate) fn close_path(path: &Path) {
    let mut pools = POOLS.lock().unwrap_or_else(PoisonError::into_inner);
    pools.remove(path);
}

/// 进程内可复用的 SQLite 连接池。
struct ConnectionPool {
    path: PathBuf,
    idle: Mutex<Vec<IdleConnection>>,
}

/// 池中空闲连接及其建连时的文件身份与归还时刻。
struct IdleConnection {
    connection: Connection,
    identity: FileIdentity,
    /// 归还进池的时刻；用于空闲上限回收。
    idle_since: Instant,
}

impl ConnectionPool {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            idle: Mutex::new(Vec::new()),
        }
    }

    fn acquire(
        self: &Arc<Self>,
        initialize: &dyn Fn(&Path, &Connection) -> AxonResult<()>,
    ) -> AxonResult<PooledConnection> {
        let now = Instant::now();
        let pooled = {
            let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
            idle.retain(|idle| now.duration_since(idle.idle_since) < MAX_IDLE_AGE);
            idle.pop()
        }
        .filter(|idle| is_current_file(&self.path, idle.identity));
        let idle = match pooled {
            Some(idle) => idle,
            None => self.open(initialize)?,
        };
        Ok(PooledConnection {
            pool: Arc::clone(self),
            idle: Some(idle),
        })
    }

    /// 关闭池中超过空闲上限的连接。
    fn evict_idle_older_than(&self, now: Instant) {
        let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        idle.retain(|idle| now.duration_since(idle.idle_since) < MAX_IDLE_AGE);
    }

    fn open(
        &self,
        initialize: &dyn Fn(&Path, &Connection) -> AxonResult<()>,
    ) -> AxonResult<IdleConnection> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| AxonError::runtime_failure("无法创建远控状态目录"))?;
        }
        let connection = Connection::open(&self.path)
            .map_err(|_| AxonError::runtime_failure("无法打开远控状态数据库"))?;
        connection
            .busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
            .map_err(|_| AxonError::runtime_failure("无法配置远控状态数据库"))?;
        connection
            .execute_batch(CONNECTION_PRAGMAS)
            .map_err(|_| AxonError::runtime_failure("无法配置远控状态数据库"))?;
        initialize(&self.path, &connection)?;
        Ok(IdleConnection {
            identity: file_identity(&self.path)?,
            connection,
            idle_since: Instant::now(),
        })
    }

    fn release(&self, mut idle: IdleConnection) {
        // 空闲计时从归还时刻起算，空闲超过上限即由回收线程关闭。
        idle.idle_since = Instant::now();
        let mut idle_connections = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        if idle_connections.len() < MAX_IDLE_CONNECTIONS_PER_PATH {
            idle_connections.push(idle);
        }
    }
}

/// 归还式连接守卫；离开作用域时把连接放回池中复用。
pub(crate) struct PooledConnection {
    pool: Arc<ConnectionPool>,
    idle: Option<IdleConnection>,
}

impl Deref for PooledConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.idle.as_ref().expect(CONNECTION_TAKEN).connection
    }
}

impl DerefMut for PooledConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.idle.as_mut().expect(CONNECTION_TAKEN).connection
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        if let Some(idle) = self.idle.take() {
            self.pool.release(idle);
        }
    }
}

/// 判断库文件是否仍是建连时的那一个；文件被删除或替换即为否。
fn is_current_file(path: &Path, identity: FileIdentity) -> bool {
    file_identity(path).is_ok_and(|current| current == identity)
}

/// 读取库文件身份，Windows 取创建时间。
#[cfg(windows)]
fn file_identity(path: &Path) -> AxonResult<FileIdentity> {
    use std::os::windows::fs::MetadataExt as _;

    let metadata =
        fs::metadata(path).map_err(|_| AxonError::runtime_failure(ERROR_FILE_IDENTITY))?;
    Ok(u128::from(metadata.creation_time()))
}

/// 读取库文件身份，Unix 取设备号与 inode 号。
#[cfg(unix)]
fn file_identity(path: &Path) -> AxonResult<FileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata =
        fs::metadata(path).map_err(|_| AxonError::runtime_failure(ERROR_FILE_IDENTITY))?;
    Ok((u128::from(metadata.dev()) << 64) | u128::from(metadata.ino()))
}
