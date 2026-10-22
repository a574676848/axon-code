//! 测试临时产物隔离设施。
//!
//! 测试里的临时目录与文件长期被直接建在系统临时目录根下（`std::env::temp_dir().join(...)`），
//! 测试结束无人回收，于是每次 `cargo test` 都往系统临时目录里堆一批**永不删除**的目录。本设施
//! 把分配权收归一处：每个测试线程拿到一个独占的临时根，线程结束（含 panic 展开）时随 `Drop`
//! 自动删除，测试不需要手写清理。
//!
//! 目录形状：
//!
//! ```text
//! <系统临时目录>/axon-test-<pid>/
//! ├── t-<序号>-<nonce>/        # 每个测试线程一个独占根，线程结束即删除
//! └── ...                      # 同一进程内其他测试线程的独占根
//! ```
//!
//! 进程根在最后一个线程根释放后变空并被顺手删除；句柄比测试线程活得更久时（进程内连接池、
//! 后台作业、LSP 子进程），删除会交给进程内后台回收线程继续重试。被杀死的测试进程（Ctrl-C、
//! 超时强杀）会留下进程根，因此每个进程根都持有一份存活租约（`.lease`）：活着的持有者独占
//! 锁住它，下一次测试运行据此回收**持有者已退出**的根（不受保留期限制），只有既拿不到锁又
//! 明显陈旧（24 小时）的根才继续保留。
//!
//! 本 crate 只作为 `dev-dependencies` 使用，不进入任何生产依赖图。

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

/// 进程级临时根目录前缀。
const PROCESS_ROOT_PREFIX: &str = "axon-test";
/// 进程根的存活租约文件名。
///
/// 活着的测试进程独占锁住该文件；回收方拿不到锁就说明持有者仍在运行，因此陈旧根可以在
/// **下一次测试运行**就回收，而不必等待固定保留期。
const LEASE_FILE_NAME: &str = ".lease";
/// 线程级子目录前缀。
const THREAD_DIR_PREFIX: &str = "t";
/// 线程序号与唯一后缀之间的连接符。
const NAME_SEPARATOR: char = '-';
/// 陈旧进程根的最小保留时长。
///
/// 只回收明显陈旧的根：正在运行的测试进程（包括并行工作区的门禁）随时可能在写自己的根，
/// 24 小时远大于任何一次测试运行的时长，因此不会误删活着的运行。
const STALE_ROOT_MIN_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// 删除线程根的重试次数。
///
/// Windows 上 SQLite 连接、WAL 文件等句柄可能比测试线程晚一步释放，首次删除会失败。
const REMOVE_RETRY_ATTEMPTS: u32 = 4;
/// 每次删除重试之间的等待时长。
const REMOVE_RETRY_DELAY: Duration = Duration::from_millis(50);
/// 后台回收线程的重试间隔。
const REAPER_INTERVAL: Duration = Duration::from_secs(2);
/// 后台回收线程的最大轮数；超过后放弃（残留交给下次运行的陈旧根回收）。
const REAPER_MAX_ROUNDS: u32 = 150;

thread_local! {
    /// 当前测试线程独占的临时根；只在 `ThreadRoot` 释放时才删除目录。
    static THREAD_ROOT: RefCell<Option<ThreadRoot>> = const { RefCell::new(None) };
}

/// 首次删除失败的线程根及其进程根，等待后台回收线程重试。
static PENDING_ROOTS: Mutex<Vec<(PathBuf, PathBuf)>> = Mutex::new(Vec::new());
/// 后台回收线程的启动开关；每进程最多一个。
static REAPER_STARTED: OnceLock<()> = OnceLock::new();
/// 本进程持有的存活租约；`OnceLock` 保证句柄活到进程结束，锁随之释放。
static PROCESS_LEASE: OnceLock<fs::File> = OnceLock::new();

/// 当前测试线程独占的临时根。
struct ThreadRoot {
    /// 线程根路径；`Drop` 时整体删除。
    path: PathBuf,
    /// 进程根路径；线程根删除后若已空则顺手删除。
    process_root: PathBuf,
}

impl Drop for ThreadRoot {
    fn drop(&mut self) {
        if remove_dir_all_with_retry(&self.path) {
            remove_process_root_if_empty(&self.process_root);
        } else {
            // 句柄比测试线程活得更久（后台作业、LSP 子进程、进程内连接池）：交给后台线程继续回收。
            schedule_retry(self.path.clone(), self.process_root.clone());
        }
    }
}

/// 删除目录；失败时短暂重试，等迟一步释放的文件句柄（SQLite 连接、WAL 文件等）。
///
/// 返回目录是否已被删除。
fn remove_dir_all_with_retry(path: &Path) -> bool {
    for attempt in 0..REMOVE_RETRY_ATTEMPTS {
        if fs::remove_dir_all(path).is_ok() || !path.exists() {
            return true;
        }
        if attempt + 1 < REMOVE_RETRY_ATTEMPTS {
            std::thread::sleep(REMOVE_RETRY_DELAY);
        }
    }
    !path.exists()
}

/// 进程根为空时删除它；非空说明同进程还有其他测试线程在用。
fn remove_process_root_if_empty(process_root: &Path) {
    let _ = fs::remove_dir(process_root);
}

/// 把首次删除失败的线程根登记到延迟回收队列，并按需启动后台回收线程。
fn schedule_retry(path: PathBuf, process_root: PathBuf) {
    PENDING_ROOTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((path, process_root));
    REAPER_STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("axon-test-temp-reaper".to_owned())
            .spawn(reap_pending_roots);
    });
}

/// 后台回收：周期性重试被登记失败的线程根，直到成功或超过最大轮数。
fn reap_pending_roots() {
    for _ in 0..REAPER_MAX_ROUNDS {
        std::thread::sleep(REAPER_INTERVAL);
        let mut pending = PENDING_ROOTS.lock().unwrap_or_else(PoisonError::into_inner);
        pending.retain(|(path, process_root)| {
            if remove_dir_all_with_retry(path) {
                remove_process_root_if_empty(process_root);
                false
            } else {
                true
            }
        });
        if pending.is_empty() {
            return;
        }
    }
}

impl ThreadRoot {
    /// 在当前进程根下创建本线程独占的根。
    fn create() -> Self {
        prune_stale_roots_once();
        let process_root = process_root_path();
        let path = process_root.join(format!(
            "{THREAD_DIR_PREFIX}{NAME_SEPARATOR}{}{NAME_SEPARATOR}{}",
            next_thread_seq(),
            nonce()
        ));
        fs::create_dir_all(&path).expect("测试隔离临时根应可创建");
        hold_process_lease(&process_root);
        Self { path, process_root }
    }
}

/// 持有进程根的存活租约，让后续运行能判定本进程是否仍在测试。
///
/// 失败（例如文件系统不支持文件锁）只降级为按修改时间回收，不影响测试本身。
fn hold_process_lease(process_root: &Path) {
    PROCESS_LEASE.get_or_init(|| {
        let lease_path = process_root.join(LEASE_FILE_NAME);
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lease_path)
            .expect("测试进程租约文件应可创建");
        file.lock().expect("测试进程租约应可加锁");
        file
    });
}

/// 返回当前测试线程独占的临时根目录；同一线程内多次调用返回同一路径。
///
/// 目录在首次调用时创建，随测试线程结束自动删除。测试**必须**把临时目录与文件建在该根之下，
/// 不要再直接使用 `std::env::temp_dir()`。
pub fn test_temp_root() -> PathBuf {
    THREAD_ROOT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(ThreadRoot::create());
        }
        slot.as_ref().expect("线程临时根刚被创建").path.clone()
    })
}

/// 返回当前测试线程临时根下名为 `name` 的路径；只拼路径，不创建任何东西。
///
/// 需要唯一目录时由调用方给出唯一名称（例如带上 `std::process::id()` 或非重复计数值）。
pub fn test_temp_path(name: &str) -> PathBuf {
    test_temp_root().join(name)
}

/// 测试临时目录句柄；`path()` 与 `tempfile::TempDir` 同形，便于既有测试直接替换。
///
/// 目录建在当前测试线程根之下：首次删除失败时登记到延迟回收队列，由后台回收线程继续重试，
/// 因此进程内连接池、后台作业等迟一步释放句柄的场景也不会留下残留。
#[derive(Debug)]
pub struct TestTempDir {
    /// 目录路径。
    path: PathBuf,
}

impl TestTempDir {
    /// 目录路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        if !remove_dir_all_with_retry(&self.path) {
            schedule_retry(self.path.clone(), process_root_path());
        }
    }
}

/// 在当前测试线程根下创建唯一临时目录。
pub fn test_temp_dir() -> TestTempDir {
    let path = test_temp_root().join(format!(
        "{THREAD_DIR_PREFIX}{NAME_SEPARATOR}{}{NAME_SEPARATOR}{}",
        next_thread_seq(),
        nonce()
    ));
    fs::create_dir_all(&path).expect("测试临时目录应可创建");
    TestTempDir { path }
}

/// 析构时执行清理动作的守卫。
///
/// 用来在**临时目录析构之前**释放被进程内缓存长期持有的资源：例如 SQLite 连接池按路径缓存
/// 连接，不先关闭就会一直锁着库文件，临时目录在 Windows 上删不掉。守卫必须声明在临时目录
/// **之后**（Rust 局部变量按声明逆序析构），这样它先于目录释放。
pub struct DeferredCleanup {
    /// 清理动作；`Option` 便于 `Drop` 中取走，保证只执行一次。
    action: Option<Box<dyn FnOnce() + Send>>,
}

impl DeferredCleanup {
    /// 注册一个在守卫析构时执行的清理动作。
    pub fn new(action: impl FnOnce() + Send + 'static) -> Self {
        Self {
            action: Some(Box::new(action)),
        }
    }
}

impl Drop for DeferredCleanup {
    fn drop(&mut self) {
        if let Some(action) = self.action.take() {
            action();
        }
    }
}

/// 当前进程的临时根路径：`<系统临时目录>/axon-test-<pid>`。
fn process_root_path() -> PathBuf {
    std::env::temp_dir().join(format!("{PROCESS_ROOT_PREFIX}-{}", std::process::id()))
}

/// 首次分配时回收一次陈旧根；同一进程只做一次，避免每次都扫目录。
fn prune_stale_roots_once() {
    static PRUNED: OnceLock<()> = OnceLock::new();
    PRUNED.get_or_init(|| {
        let own_root = process_root_path();
        prune_stale_roots_at(
            &std::env::temp_dir(),
            &own_root,
            SystemTime::now(),
            STALE_ROOT_MIN_AGE,
        );
    });
}

/// 回收 `scan_dir` 下陈旧的进程根；返回被删除的数量。
///
/// `now` 与 `min_age` 显式传入，使「陈旧判定」可以用注入时间做确定性验证。
fn prune_stale_roots_at(
    scan_dir: &Path,
    own_root: &Path,
    now: SystemTime,
    min_age: Duration,
) -> usize {
    let Ok(entries) = fs::read_dir(scan_dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !is_process_root_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        if path == own_root || !is_reclaimable(&path, now, min_age) {
            continue;
        }
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 判断进程根是否可以被回收。
///
/// 有存活租约时以锁为准：能拿到独占锁说明持有者已退出，根可以立刻回收；拿不到锁说明持有者
/// 仍在运行，无论多旧都必须保留。没有租约文件（更早版本或异常中断）才退回按修改时间判断。
fn is_reclaimable(path: &Path, now: SystemTime, min_age: Duration) -> bool {
    let Ok(lease) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join(LEASE_FILE_NAME))
    else {
        return is_stale(path, now, min_age);
    };
    match lease.try_lock() {
        Ok(()) => {
            let _ = lease.unlock();
            true
        }
        Err(_) => false,
    }
}

/// 判断目录名是否为进程根名（`axon-test-<pid>`，其后可带 `-` 分隔的附加段）。
fn is_process_root_name(name: &str) -> bool {
    name.strip_prefix(PROCESS_ROOT_PREFIX)
        .and_then(|rest| rest.strip_prefix(NAME_SEPARATOR))
        .and_then(|rest| rest.split(NAME_SEPARATOR).next())
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
}

/// 目录最后修改时间是否早于 `min_age`；读不到时间戳时按「不陈旧」处理，宁可不删。
fn is_stale(path: &Path, now: SystemTime, min_age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= min_age)
}

/// 线程序号：保证同进程内多个测试线程的目录名不冲突。
fn next_thread_seq() -> u64 {
    static NEXT_SEQ: AtomicU64 = AtomicU64::new(0);
    NEXT_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 进程内的唯一后缀；与线程序号共同构成目录名。
fn nonce() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicBool};

    use super::*;

    /// 释放守卫必须在析构时执行清理动作。
    #[test]
    fn deferred_cleanup_runs_on_drop() {
        let cleaned = Arc::new(AtomicBool::new(false));
        {
            let flag = Arc::clone(&cleaned);
            let _guard = DeferredCleanup::new(move || flag.store(true, Ordering::SeqCst));
            assert!(!cleaned.load(Ordering::SeqCst), "守卫存活期间不得执行清理");
        }
        assert!(cleaned.load(Ordering::SeqCst), "守卫析构必须执行清理动作");
    }

    /// 线程根必须落在临时目录的进程根之下、同线程复用，并在测试线程结束时自动删除。
    #[test]
    fn thread_root_is_temp_scoped_and_removed_with_its_thread() {
        let (root, config_path, process_root) = std::thread::spawn(|| {
            let root = test_temp_root();
            assert_eq!(root, test_temp_root(), "同一测试线程必须复用同一个临时根");
            assert!(root.is_dir(), "临时根应已创建：{}", root.display());
            assert_eq!(test_temp_path("config.toml"), root.join("config.toml"));
            (root, test_temp_path("config.toml"), process_root_path())
        })
        .join()
        .expect("临时根持有线程应正常结束");

        assert!(
            root.starts_with(&process_root),
            "临时根必须位于进程根之下：{}",
            root.display()
        );
        assert!(
            root.starts_with(std::env::temp_dir()),
            "临时根必须位于系统临时目录：{}",
            root.display()
        );
        assert!(
            !root.exists(),
            "临时根必须随测试线程结束自动删除：{}",
            root.display()
        );
        assert!(config_path.starts_with(&root), "派生路径必须落在临时根之下");
    }

    /// 不同测试线程必须拿到互不相同的根，避免并行测试互相看到对方的临时事实。
    #[test]
    fn parallel_test_threads_get_distinct_roots() {
        let roots: Vec<PathBuf> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    let root = test_temp_root();
                    assert!(root.is_dir());
                    root
                })
                .join()
                .expect("临时根持有线程应正常结束")
            })
            .collect();

        let mut unique = roots.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            roots.len(),
            "并行测试线程的临时根必须互不相同"
        );
        for root in roots {
            assert!(!root.exists(), "线程根必须自动删除：{}", root.display());
        }
    }

    /// 进程根名识别：只有 `axon-test-<数字>` 形态参与回收。
    #[test]
    fn process_root_name_matches_only_pid_shaped_names() {
        assert!(is_process_root_name("axon-test-1234"));
        assert!(is_process_root_name("axon-test-1234-extra"));
        assert!(!is_process_root_name("axon-test-"));
        assert!(!is_process_root_name("axon-test-abc"));
        assert!(!is_process_root_name("axon-test-logs-1234"));
        assert!(!is_process_root_name("other-1234"));
    }

    /// 陈旧判定与回收：保留期内一个都不删，越过保留期后删掉除当前进程根以外的全部。
    #[test]
    fn stale_roots_are_pruned_without_touching_fresh_ones() {
        let scan_dir = std::env::temp_dir().join(format!(
            "{PROCESS_ROOT_PREFIX}-scan-{}-{}",
            std::process::id(),
            nonce()
        ));
        let own_root = scan_dir.join("axon-test-0-own");
        let other_root = scan_dir.join("axon-test-1-other");
        let not_a_root = scan_dir.join("axon-rollout-test");
        for path in [&own_root, &other_root, &not_a_root] {
            fs::create_dir_all(path).expect("扫描目录应可创建");
        }

        let now = SystemTime::now();
        assert_eq!(
            prune_stale_roots_at(&scan_dir, &own_root, now, STALE_ROOT_MIN_AGE),
            0,
            "保留期内的根一个都不能删"
        );
        assert_eq!(
            prune_stale_roots_at(
                &scan_dir,
                &own_root,
                now + STALE_ROOT_MIN_AGE * 2,
                STALE_ROOT_MIN_AGE
            ),
            1,
            "越期后只删非当前进程的进程根"
        );
        assert!(own_root.is_dir(), "当前进程根不得被回收");
        assert!(!other_root.is_dir(), "越期的其他进程根必须被回收");
        assert!(not_a_root.is_dir(), "非本设施命名的目录不得被回收");
        let _ = fs::remove_dir_all(&scan_dir);
    }

    /// 存活租约决定回收：持有者已退出的根立刻回收，仍在运行的根无论多新都保留。
    #[test]
    fn lease_decides_reclaim_regardless_of_age() {
        let scan_dir = std::env::temp_dir().join(format!(
            "{PROCESS_ROOT_PREFIX}-lease-{}-{}",
            std::process::id(),
            nonce()
        ));
        let own_root = scan_dir.join("axon-test-0-own");
        let live_root = scan_dir.join("axon-test-1-live");
        let dead_root = scan_dir.join("axon-test-2-dead");
        for path in [&own_root, &live_root, &dead_root] {
            fs::create_dir_all(path).expect("扫描目录应可创建");
        }
        let live_lease = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(live_root.join(LEASE_FILE_NAME))
            .expect("存活租约应可创建");
        live_lease.lock().expect("存活租约应可加锁");
        fs::write(dead_root.join(LEASE_FILE_NAME), b"").expect("退出租约应可创建");

        let removed =
            prune_stale_roots_at(&scan_dir, &own_root, SystemTime::now(), STALE_ROOT_MIN_AGE);

        assert_eq!(removed, 1, "只应回收租约已释放的那个根");
        assert!(
            live_root.is_dir(),
            "持有者仍在运行时不得回收：{}",
            live_root.display()
        );
        assert!(!dead_root.is_dir(), "持有者已退出的根必须立刻回收");
        drop(live_lease);
        let _ = fs::remove_dir_all(&scan_dir);
    }
}
