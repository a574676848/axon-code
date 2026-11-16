//! 跨进程独占写锁。

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::EventLogError;

/// 默认抢锁超时：超过即判「已被其他写入者占用」，不无限阻塞调用方。
///
/// 一次追加只是一帧写 + 一次 fsync，正常持锁在毫秒量级；5 秒足够覆盖磁盘抖动，
/// 又不会让卡死的写入方把整个会话的追加/清理链永久挂住。
pub(crate) const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// 抢锁轮询间隔。
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 日志级独占写锁。
///
/// 用标准库的文件锁实现：进程崩溃时由内核释放，因此不需要租约与过期时间。
/// 锁文件与日志文件分开，避免锁语义与数据追加互相干扰。
#[derive(Debug)]
pub(crate) struct ExclusiveLock {
    file: File,
}

impl ExclusiveLock {
    /// 获取指定路径的独占锁；超过默认超时仍被占用时返回 [`EventLogError::Locked`]。
    pub(crate) fn acquire(path: &Path) -> Result<Self, EventLogError> {
        Self::acquire_with_timeout(path, DEFAULT_LOCK_TIMEOUT)
    }

    /// 带显式超时的抢锁：超时后不再阻塞，交给调用方决定重试或放弃。
    pub(crate) fn acquire_with_timeout(
        path: &Path,
        timeout: Duration,
    ) -> Result<Self, EventLogError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(EventLogError::Locked);
                    }
                    std::thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }
}

impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_across_handles() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("session.lock");
        let first = ExclusiveLock::acquire(&path).expect("首次加锁");
        drop(first);
        let second = ExclusiveLock::acquire(&path).expect("释放后可再次加锁");
        drop(second);
    }

    #[test]
    fn contended_lock_times_out_instead_of_blocking_forever() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("session.lock");
        let held = ExclusiveLock::acquire(&path).expect("先占住锁");
        let started = Instant::now();
        let error = ExclusiveLock::acquire_with_timeout(&path, Duration::from_millis(120))
            .expect_err("被占用时必须超时失败");
        assert!(
            matches!(error, EventLogError::Locked),
            "实际错误：{error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "抢锁不得无限阻塞"
        );
        drop(held);
        ExclusiveLock::acquire(&path).expect("释放后应可再次加锁");
    }
}
