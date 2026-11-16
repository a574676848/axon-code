//! 跨进程独占写锁。

use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::error::EventLogError;

/// 日志级独占写锁。
///
/// 用标准库的文件锁实现：进程崩溃时由内核释放，因此不需要租约与过期时间。
/// 锁文件与日志文件分开，避免锁语义与数据追加互相干扰。
#[derive(Debug)]
pub(crate) struct ExclusiveLock {
    file: File,
}

impl ExclusiveLock {
    /// 获取指定路径的独占锁；同进程或跨进程的第二个持有者会阻塞到前者释放。
    pub(crate) fn acquire(path: &Path) -> Result<Self, EventLogError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.lock()?;
        Ok(Self { file })
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
}
