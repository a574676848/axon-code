//! 远控 JSONL durable writer。

use std::path::Path;

use axon_domain::{AxonError, AxonResult};
use axon_jsonl::ShardedJsonlStore;

/// 远控恢复账本要求每次写入同步，并由统一机制提供跨进程锁。
const DURABLE_STORE: ShardedJsonlStore = ShardedJsonlStore::durable();

/// 远控 durable JSONL 存储运行时。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteDurableJsonlRuntime;

impl RemoteDurableJsonlRuntime {
    /// 以跨线程/跨进程锁保护的 append + fsync 写入一条记录。
    pub fn append_line(path: impl AsRef<Path>, line: &str) -> AxonResult<()> {
        DURABLE_STORE
            .append_line(path, line)
            .map_err(|_| AxonError::runtime_failure("无法持久化远控 durable store"))
    }

    /// 读取 durable store 文本；不接受半行，调用方负责逐行解析。
    pub fn read_text(path: impl AsRef<Path>) -> AxonResult<Option<String>> {
        let lines = DURABLE_STORE
            .read_repaired_lines(path)
            .map_err(|_| AxonError::runtime_failure("无法读取远控 durable store"))?;
        if lines.is_empty() {
            return Ok(None);
        }
        let mut text = Vec::new();
        for line in lines {
            text.extend_from_slice(&line);
            text.push(b'\n');
        }
        String::from_utf8(text)
            .map(Some)
            .map_err(|_| AxonError::runtime_failure("远控 durable store 不是有效 UTF-8"))
    }

    /// 在同一跨进程锁内读取当前快照并追加完整 JSONL 行。
    ///
    /// `operation` 只能依据快照生成新行，不能在回调中执行网络或阻塞 I/O。
    pub fn read_then_append<T>(
        path: impl AsRef<Path>,
        operation: impl FnOnce(Option<&str>) -> AxonResult<(T, Vec<String>)>,
    ) -> AxonResult<T> {
        DURABLE_STORE
            .append_with(path, |lines| {
                let text = join_snapshot(lines);
                operation(text.as_deref())
                    .map(|(result, lines)| {
                        (
                            result,
                            lines.into_iter().map(|line| line.into_bytes()).collect(),
                        )
                    })
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "远控 durable store 操作失败",
                        )
                    })
            })
            .map_err(|_| AxonError::runtime_failure("无法持久化远控 durable store"))
    }

    /// 在同一跨进程锁内读取当前快照并原子替换完整 JSONL 内容。
    ///
    /// `operation` 只能依据快照生成完整行，不能在回调中执行网络或阻塞 I/O。
    pub fn read_then_rewrite<T>(
        path: impl AsRef<Path>,
        operation: impl FnOnce(Option<&str>) -> AxonResult<(T, Option<Vec<String>>)>,
    ) -> AxonResult<T> {
        DURABLE_STORE
            .rewrite_with(path, |lines| {
                let text = join_snapshot(lines);
                operation(text.as_deref())
                    .map(|(result, output)| {
                        let output = output.unwrap_or_else(|| {
                            lines
                                .iter()
                                .map(|line| String::from_utf8_lossy(line).into_owned())
                                .collect()
                        });
                        (
                            result,
                            output.into_iter().map(|line| line.into_bytes()).collect(),
                        )
                    })
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "远控 durable store 操作失败",
                        )
                    })
            })
            .map_err(|_| AxonError::runtime_failure("无法持久化远控 durable store"))
    }
}
fn join_snapshot(lines: &[Vec<u8>]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend_from_slice(line);
        bytes.push(b'\n');
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc, thread, time::Duration};

    use super::{RemoteDurableJsonlRuntime, ShardedJsonlStore};

    const READ_BLOCK_ASSERTION_TIMEOUT: Duration = Duration::from_millis(500);
    const INDEPENDENT_STORE_ASSERTION_TIMEOUT: Duration = Duration::from_millis(500);

    #[test]
    fn read_waits_for_concurrent_writer_lock_before_reading_jsonl() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("durable.jsonl");
        let physical = ShardedJsonlStore::physical_shard_path(&path, 0);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(&physical, "{\"complete\":true}\n").expect("write fixture");
        let lock_path = path.with_extension("lock");
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .expect("open lock");
        lock.lock().expect("lock");
        let (sent, received) = mpsc::channel();
        let reader_path = path.clone();
        let reader = thread::spawn(move || {
            let result = RemoteDurableJsonlRuntime::read_text(reader_path);
            sent.send(result).expect("send result");
        });

        assert!(received.recv_timeout(READ_BLOCK_ASSERTION_TIMEOUT).is_err());
        lock.unlock().expect("unlock");
        assert_eq!(
            received
                .recv_timeout(READ_BLOCK_ASSERTION_TIMEOUT)
                .expect("read after unlock")
                .expect("read succeeds"),
            Some("{\"complete\":true}\n".to_owned())
        );
        reader.join().expect("reader join");
    }

    #[test]
    fn recovery_truncates_unterminated_crash_tail_before_the_next_append() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("durable.jsonl");
        let physical = ShardedJsonlStore::physical_shard_path(&path, 0);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(&physical, "{\"complete\":true}\n{\"partial\"").expect("write fixture");

        assert_eq!(
            RemoteDurableJsonlRuntime::read_text(&path).expect("read"),
            Some("{\"complete\":true}\n".to_owned())
        );
        RemoteDurableJsonlRuntime::append_line(&path, "{\"next\":true}").expect("append");
        assert_eq!(
            fs::read_to_string(ShardedJsonlStore::physical_shard_path(&path, 0))
                .expect("read repaired store"),
            "{\"complete\":true}\n{\"next\":true}\n"
        );
    }

    #[test]
    fn distinct_stores_do_not_share_the_process_lock() {
        let directory = axon_test_support::test_temp_dir();
        let blocked_path = directory.path().join("blocked.jsonl");
        let independent_path = directory.path().join("independent.jsonl");
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let blocked_writer = thread::spawn(move || {
            RemoteDurableJsonlRuntime::read_then_append(&blocked_path, |_| {
                entered_sender.send(()).expect("report lock");
                release_receiver.recv().expect("release lock");
                Ok(((), Vec::new()))
            })
        });

        entered_receiver
            .recv_timeout(READ_BLOCK_ASSERTION_TIMEOUT)
            .expect("blocked store lock");
        let (completed_sender, completed_receiver) = mpsc::channel();
        let independent_writer = thread::spawn(move || {
            completed_sender
                .send(RemoteDurableJsonlRuntime::append_line(
                    independent_path,
                    "{\"independent\":true}",
                ))
                .expect("report independent write");
        });

        completed_receiver
            .recv_timeout(INDEPENDENT_STORE_ASSERTION_TIMEOUT)
            .expect("independent store should not wait")
            .expect("independent write");
        release_sender.send(()).expect("release blocked store");
        blocked_writer
            .join()
            .expect("blocked writer join")
            .expect("blocked write");
        independent_writer.join().expect("independent writer join");
    }

    #[test]
    fn rewrite_replaces_snapshot_without_retaining_superseded_lines() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("durable.jsonl");
        let physical = ShardedJsonlStore::physical_shard_path(&path, 0);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(&physical, "{\"version\":1}\n{\"version\":2}\n").expect("write fixture");

        RemoteDurableJsonlRuntime::read_then_rewrite(&path, |text| {
            assert!(text.expect("snapshot").contains("\"version\":2"));
            Ok(((), Some(vec!["{\"version\":2}".to_owned()])))
        })
        .expect("rewrite");

        assert_eq!(
            RemoteDurableJsonlRuntime::read_text(path).expect("read"),
            Some("{\"version\":2}\n".to_owned())
        );
    }
}
