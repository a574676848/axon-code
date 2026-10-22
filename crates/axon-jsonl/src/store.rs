//! JSONL shard 的路径、锁、写入、读取和压缩实现。

use std::{
    collections::HashMap,
    fs,
    io::{self, Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

/// 默认单个 shard 的大小上限。
pub const DEFAULT_SHARD_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// shard 序号的稳定显示宽度。
pub const SHARD_SEQUENCE_WIDTH: usize = 6;
/// 固定宽度 shard 序号的最大值。
const MAX_SHARD_SEQUENCE: u64 = 10_u64.pow(SHARD_SEQUENCE_WIDTH as u32) - 1;
/// 逻辑 JSONL 路径与物理 shard 共用的后缀。
const JSONL_SUFFIX: &str = ".jsonl";

/// 统一 JSONL 分片存储。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardedJsonlStore {
    max_shard_bytes: u64,
    sync_on_write: bool,
}

impl Default for ShardedJsonlStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ShardedJsonlStore {
    /// 使用默认 shard 大小上限创建存储。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_shard_bytes: DEFAULT_SHARD_MAX_BYTES,
            sync_on_write: false,
        }
    }

    /// 使用业务存储预算创建存储；单条行也不得超过该上限。
    #[must_use]
    pub const fn with_max_shard_bytes(max_shard_bytes: u64) -> Self {
        Self {
            max_shard_bytes,
            sync_on_write: false,
        }
    }

    /// 创建每次追加都同步数据的存储，用于恢复账本。
    #[must_use]
    pub const fn durable() -> Self {
        Self {
            max_shard_bytes: DEFAULT_SHARD_MAX_BYTES,
            sync_on_write: true,
        }
    }

    /// 列出逻辑路径对应的物理 shard，按序号稳定排序。
    pub fn shard_paths(&self, logical_path: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
        Ok(self
            .shard_entries(logical_path)?
            .into_iter()
            .map(|(_, path)| path)
            .collect())
    }

    /// 计算逻辑 JSONL 的全部物理字节数。
    pub fn total_bytes(&self, logical_path: impl AsRef<Path>) -> io::Result<u64> {
        self.shard_paths(logical_path)?
            .iter()
            .try_fold(0_u64, |total, path| {
                Ok(total.saturating_add(path.metadata()?.len()))
            })
    }

    /// 解析逻辑路径指定序号的物理 shard 路径；不做磁盘访问。
    #[must_use]
    pub fn physical_shard_path(logical_path: impl AsRef<Path>, sequence: u64) -> PathBuf {
        shard_path(logical_path.as_ref(), sequence).unwrap_or_default()
    }

    /// 读取逻辑 JSONL 的全部行；不包含空行。
    pub fn read_lines(&self, logical_path: impl AsRef<Path>) -> io::Result<Vec<Vec<u8>>> {
        self.with_lock(logical_path, |store, logical_path| {
            store.read_lines_unlocked(logical_path)
        })
    }

    /// 读取全部行；写入前先裁掉崩溃造成的未完成尾行。
    pub fn read_repaired_lines(&self, logical_path: impl AsRef<Path>) -> io::Result<Vec<Vec<u8>>> {
        self.with_lock(logical_path, |store, logical_path| {
            store.repair_tail(logical_path)?;
            store.read_lines_unlocked(logical_path)
        })
    }

    /// 追加一行；超过 shard 预算时滚动，单行超限时直接拒绝。
    pub fn append_line(
        &self,
        logical_path: impl AsRef<Path>,
        line: impl AsRef<[u8]>,
    ) -> io::Result<()> {
        let line = normalize_line(line.as_ref())?;
        self.validate_line_bytes(line.len() as u64)?;
        self.with_lock(logical_path, |store, logical_path| {
            store.repair_tail(logical_path)?;
            let path = store.append_target(logical_path, line.len())?;
            write_line(&path, &line, store.sync_on_write)
        })
    }

    /// 追加一行并在同一锁内校验逻辑 JSONL 的总容量预算。
    pub fn append_bounded_line(
        &self,
        logical_path: impl AsRef<Path>,
        line: impl AsRef<[u8]>,
        max_total_bytes: u64,
    ) -> io::Result<()> {
        let line = normalize_line(line.as_ref())?;
        self.validate_line_bytes(line.len() as u64)?;
        let line_bytes = line.len() as u64 + 1;
        self.with_lock(logical_path, |store, logical_path| {
            store.repair_tail(logical_path)?;
            if store.total_bytes(logical_path)?.saturating_add(line_bytes) > max_total_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::ResourceBusy,
                    "JSONL 总容量预算已耗尽",
                ));
            }
            let path = store.append_target(logical_path, line.len())?;
            write_line(&path, &line, store.sync_on_write)
        })
    }

    /// 追加一行并强制同步数据到磁盘。
    pub fn append_durable_line(
        &self,
        logical_path: impl AsRef<Path>,
        line: impl AsRef<[u8]>,
    ) -> io::Result<()> {
        let line = normalize_line(line.as_ref())?;
        self.validate_line_bytes(line.len() as u64)?;
        self.with_lock(logical_path, |store, logical_path| {
            store.repair_tail(logical_path)?;
            let path = store.append_target(logical_path, line.len())?;
            write_line(&path, &line, true)
        })
    }

    /// 在同一锁内读取快照并只追加业务闭包生成的新行。
    pub fn append_with<T, F>(&self, logical_path: impl AsRef<Path>, operation: F) -> io::Result<T>
    where
        F: FnOnce(&[Vec<u8>]) -> io::Result<(T, Vec<Vec<u8>>)>,
    {
        self.with_lock(logical_path, |store, logical_path| {
            store.repair_tail(logical_path)?;
            let input = store.read_lines_unlocked(logical_path)?;
            let (result, lines) = operation(&input)?;
            let normalized_lines = lines
                .iter()
                .map(|line| {
                    let normalized = normalize_line(line)?;
                    store.validate_line_bytes(normalized.len() as u64)?;
                    Ok(normalized)
                })
                .collect::<io::Result<Vec<_>>>()?;
            for normalized in normalized_lines {
                let path = store.append_target(logical_path, normalized.len())?;
                write_line(&path, &normalized, store.sync_on_write)?;
            }
            Ok(result)
        })
    }

    /// 追加一行并在同一锁内按记录时间淘汰过期数据。
    ///
    /// `recorded_at` 由业务提供，因此机制不需要理解具体 JSON 字段。
    pub fn append_retained_line<F>(
        &self,
        logical_path: impl AsRef<Path>,
        line: impl AsRef<[u8]>,
        recorded_at_ms: u64,
        max_age_ms: u64,
        recorded_at: F,
    ) -> io::Result<()>
    where
        F: Fn(&[u8]) -> io::Result<u64>,
    {
        let line = normalize_line(line.as_ref())?;
        self.validate_line_bytes(line.len() as u64)?;
        self.with_lock(logical_path, |store, logical_path| {
            store.repair_tail(logical_path)?;
            let lines = store.read_lines_unlocked(logical_path)?;
            let retained = retained_lines(&lines, recorded_at_ms, max_age_ms, &recorded_at)?;
            if retained.len() != lines.len() {
                store.rewrite_unlocked(logical_path, retained)?;
            }
            let path = store.append_target(logical_path, line.len())?;
            write_line(&path, &line, store.sync_on_write)
        })
    }

    /// 在同一锁内读取当前行并整体重写；返回值由业务闭包决定。
    pub fn rewrite_with<T, F>(&self, logical_path: impl AsRef<Path>, operation: F) -> io::Result<T>
    where
        F: FnOnce(&[Vec<u8>]) -> io::Result<(T, Vec<Vec<u8>>)>,
    {
        self.with_lock(logical_path, |store, logical_path| {
            store.repair_tail(logical_path)?;
            let input = store.read_lines_unlocked(logical_path)?;
            let (result, lines) = operation(&input)?;
            let normalized = lines
                .into_iter()
                .map(|line| {
                    let normalized = normalize_line(&line)?;
                    store.validate_line_bytes(normalized.len() as u64)?;
                    Ok(normalized)
                })
                .collect::<io::Result<Vec<_>>>()?;
            store.rewrite_unlocked(logical_path, normalized)?;
            Ok(result)
        })
    }

    /// 删除逻辑 JSONL 的全部物理 shard。
    pub fn remove_all(&self, logical_path: impl AsRef<Path>) -> io::Result<bool> {
        self.with_lock(logical_path, |store, logical_path| {
            let paths = store.shard_paths(logical_path)?;
            let mut removed = false;
            for path in paths {
                match fs::remove_file(&path) {
                    Ok(()) => removed = true,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(removed)
        })
    }

    fn append_target(&self, logical_path: &Path, line_bytes: usize) -> io::Result<PathBuf> {
        let line_bytes = u64::try_from(line_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "JSONL 行长度无效"))?;
        self.validate_line_bytes(line_bytes)?;
        let stored_bytes = line_bytes.saturating_add(1);
        let shards = self.shard_entries(logical_path)?;
        let Some((last_sequence, last_path)) = shards.last() else {
            return shard_path(logical_path, 0);
        };
        let last_bytes = last_path.metadata()?.len();
        if last_bytes == 0 || last_bytes.saturating_add(stored_bytes) <= self.max_shard_bytes {
            return Ok(last_path.clone());
        }
        let next_sequence = last_sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "JSONL shard 序号溢出"))?;
        if next_sequence > MAX_SHARD_SEQUENCE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSONL shard 序号超过固定宽度上限",
            ));
        }
        shard_path(logical_path, next_sequence)
    }

    fn validate_line_bytes(&self, line_bytes: u64) -> io::Result<()> {
        if line_bytes.saturating_add(1) > self.max_shard_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "单条 JSONL 记录超过 shard 大小上限",
            ));
        }
        Ok(())
    }

    fn shard_entries(&self, logical_path: impl AsRef<Path>) -> io::Result<Vec<(u64, PathBuf)>> {
        let logical_path = logical_path.as_ref();
        let prefix = logical_prefix(logical_path)?;
        let directory = logical_directory(logical_path)?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut shards = fs::read_dir(directory)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let suffix = name.strip_prefix(&prefix)?.strip_suffix(JSONL_SUFFIX);
                let sequence = suffix
                    .filter(|value| value.len() == SHARD_SEQUENCE_WIDTH)
                    .and_then(|value| value.parse::<u64>().ok())?;
                Some((sequence, entry.path()))
            })
            .collect::<Vec<_>>();
        shards.sort_by_key(|(sequence, _)| *sequence);
        Ok(shards)
    }

    fn read_lines_unlocked(&self, logical_path: &Path) -> io::Result<Vec<Vec<u8>>> {
        let mut lines = Vec::new();
        for path in self.shard_paths(logical_path)? {
            for bytes in fs::read(path)?.split(|byte| *byte == b'\n') {
                let line = bytes.strip_suffix(b"\r").unwrap_or(bytes);
                if !line.is_empty() {
                    lines.push(line.to_vec());
                }
            }
        }
        Ok(lines)
    }

    fn rewrite_unlocked(&self, logical_path: &Path, lines: Vec<Vec<u8>>) -> io::Result<()> {
        remove_shards(logical_path)?;
        if lines.is_empty() {
            return Ok(());
        }
        let mut target = shard_path(logical_path, 0)?;
        let mut file = open_append(&target)?;
        for line in lines {
            let current_bytes = target.metadata()?.len();
            if current_bytes > 0
                && current_bytes.saturating_add(line.len() as u64 + 1) > self.max_shard_bytes
            {
                file.flush()?;
                target = self.append_target(logical_path, line.len())?;
                file = open_append(&target)?;
            }
            file.write_all(&line)?;
            file.write_all(b"\n")?;
        }
        file.flush().and_then(|()| {
            if self.sync_on_write {
                file.sync_data()
            } else {
                Ok(())
            }
        })
    }

    fn repair_tail(&self, logical_path: &Path) -> io::Result<()> {
        let paths = self.shard_paths(logical_path)?;
        let Some((last_path, prior_paths)) = paths.split_last() else {
            return Ok(());
        };
        for path in prior_paths {
            let length = fs::metadata(path)?.len();
            if length == 0 {
                continue;
            }
            if !Self::path_ends_with_newline(path, length)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JSONL 历史 shard 缺少换行结尾",
                ));
            }
        }
        let length = fs::metadata(last_path)?.len();
        if length == 0 || Self::path_ends_with_newline(last_path, length)? {
            return Ok(());
        }
        let bytes = fs::read(last_path)?;
        let valid_length = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let file = fs::OpenOptions::new().write(true).open(last_path)?;
        file.set_len(valid_length as u64)?;
        file.sync_all()?;
        Ok(())
    }

    fn path_ends_with_newline(path: &Path, length: u64) -> io::Result<bool> {
        let mut last_byte = [0_u8; 1];
        let mut file = fs::File::open(path)?;
        file.seek(io::SeekFrom::Start(length - 1))?;
        file.read_exact(&mut last_byte)?;
        Ok(last_byte[0] == b'\n')
    }

    fn with_lock<T, F>(&self, logical_path: impl AsRef<Path>, operation: F) -> io::Result<T>
    where
        F: FnOnce(&Self, &Path) -> io::Result<T>,
    {
        let logical_path = logical_path.as_ref();
        if logical_path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "JSONL 逻辑路径必须以 .jsonl 结尾",
            ));
        }
        if let Some(parent) = logical_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let process_lock = process_lock_for(logical_path);
        let _process_guard = process_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path(logical_path))?;
        lock_file.lock()?;
        let result = operation(self, logical_path);
        let output = result?;
        lock_file.unlock()?;
        Ok(output)
    }
}

fn normalize_line(line: &[u8]) -> io::Result<Vec<u8>> {
    if line.iter().any(|byte| *byte == b'\n' || *byte == b'\r') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "JSONL 记录不能包含换行符",
        ));
    }
    if line.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "JSONL 记录不能为空",
        ));
    }
    Ok(line.to_vec())
}

fn retained_lines<F>(
    lines: &[Vec<u8>],
    now_ms: u64,
    max_age_ms: u64,
    recorded_at: &F,
) -> io::Result<Vec<Vec<u8>>>
where
    F: Fn(&[u8]) -> io::Result<u64>,
{
    let cutoff = now_ms.saturating_sub(max_age_ms);
    let mut retained = Vec::with_capacity(lines.len());
    for line in lines {
        if recorded_at(line)? >= cutoff {
            retained.push(line.clone());
        }
    }
    Ok(retained)
}

fn logical_prefix(logical_path: &Path) -> io::Result<String> {
    let file_name = logical_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "JSONL 逻辑路径无效"))?;
    let stem = file_name.strip_suffix(JSONL_SUFFIX).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "JSONL 逻辑路径缺少 .jsonl 后缀",
        )
    })?;
    Ok(format!("{stem}."))
}

fn logical_directory(logical_path: &Path) -> io::Result<&Path> {
    logical_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "JSONL 逻辑路径缺少父目录"))
}

fn shard_path(logical_path: &Path, sequence: u64) -> io::Result<PathBuf> {
    let prefix = logical_prefix(logical_path)?;
    Ok(logical_directory(logical_path)?.join(format!(
        "{prefix}{sequence:0width$}{JSONL_SUFFIX}",
        width = SHARD_SEQUENCE_WIDTH
    )))
}

fn remove_shards(logical_path: &Path) -> io::Result<()> {
    let shards = ShardedJsonlStore::new().shard_paths(logical_path)?;
    for path in shards {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_line(path: &Path, line: &[u8], sync_data: bool) -> io::Result<()> {
    let mut file = open_append(path)?;
    file.write_all(line)?;
    file.write_all(b"\n")?;
    file.flush()?;
    if sync_data {
        file.sync_data()?;
    }
    Ok(())
}

fn open_append(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().create(true).append(true).open(path)
}

fn lock_path(logical_path: &Path) -> PathBuf {
    logical_path.with_extension("lock")
}

fn process_lock_for(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}
