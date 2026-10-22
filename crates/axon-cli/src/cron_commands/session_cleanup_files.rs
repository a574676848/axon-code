//! 会话保留清理的文件面：日志、MCP Schema 缓存与运行态临时目录。
//!
//! 目标态要求 `~/.axon` 下所有数据与日志都有界：SQLite 事实表由
//! `runtime_cleanup` 清理，日志与缓存文件由本模块按同一保留期回收。

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use rusqlite::Connection;

/// 日志目录，位于运行态 data 根之下。
const LOG_DIRECTORY: &str = "logs";
const LOG_FILE_PREFIX: &str = "axon-";
const LOG_FILE_SUFFIX: &str = ".log";
/// 缓存目录，位于运行态根之下。
const CACHE_DIRECTORY: &str = "cache";
const MCP_SCHEMA_CACHE_FILE: &str = "mcp_tool_schemas.sqlite3";
/// TUI 运行态临时目录，位于运行态 data 根之下。
const TMP_DIRECTORY: &str = "tmp";
/// 内容归档目录，位于运行态 data 根之下；复用 agent runtime 的归档目录名，避免两处漂移。
const CONTENT_ARCHIVE_DIRECTORY: &str = axon_agent_runtime::CONTENT_ARCHIVE_DIR_NAME;
/// Shell 完整输出归档子目录（位于日志根之下）；复用 agent runtime 常量避免两处漂移。
const SHELL_ARTIFACT_DIRECTORY: &str = axon_agent_runtime::SHELL_ARTIFACT_DIR_NAME;
/// Shell 归档文件后缀；清理时据此识别本工具产物，不触碰同目录其他内容。
const SHELL_ARTIFACT_FILE_SUFFIX: &str = axon_agent_runtime::SHELL_ARTIFACT_FILE_SUFFIX;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 文件面清理计数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FileCleanupCounts {
    pub(crate) log_files: usize,
    pub(crate) temp_entries: usize,
    pub(crate) mcp_schema_rows: usize,
    pub(crate) archive_files: usize,
    pub(crate) shell_artifact_files: usize,
}

/// 按保留期回收日志、临时文件与陈旧缓存。
pub(crate) fn cleanup_runtime_files(
    root: &Path,
    cutoff_ms: u64,
) -> Result<FileCleanupCounts, String> {
    run_file_cleanup(root, cutoff_ms, false, &std::env::temp_dir())
}

/// 只统计将要回收的文件与缓存行，不产生任何写入。
pub(crate) fn count_runtime_files(
    root: &Path,
    cutoff_ms: u64,
) -> Result<FileCleanupCounts, String> {
    run_file_cleanup(root, cutoff_ms, true, &std::env::temp_dir())
}

fn run_file_cleanup(
    root: &Path,
    cutoff_ms: u64,
    dry_run: bool,
    fallback_root: &Path,
) -> Result<FileCleanupCounts, String> {
    let data_root = root.join(axon_config::AXON_RUNTIME_DATA_DIR);
    // Shell 归档可能落在两处：注入了日志根时在 `<data>/logs/tee`，否则回退到进程
    // 临时目录下的同名子目录（见 `create_artifact`）。两处都按同一保留期回收：
    // 归档每会话一个文件且**再无其他删除点**，不回收则文件数随调用次数无界增长。
    //
    // `fallback_root` 显式传入而非在函数内取 `std::env::temp_dir()`：否则任何调用方
    // （含测试）都会以真实进程临时目录为作用域，把无关产物一并删除。
    let shell_artifact_files = cleanup_expired_shell_artifacts(
        &data_root.join(LOG_DIRECTORY).join(SHELL_ARTIFACT_DIRECTORY),
        cutoff_ms,
        dry_run,
    )?
    .saturating_add(cleanup_expired_shell_artifacts(
        &fallback_root.join(SHELL_ARTIFACT_DIRECTORY),
        cutoff_ms,
        dry_run,
    )?);
    Ok(FileCleanupCounts {
        log_files: cleanup_expired_logs(&data_root.join(LOG_DIRECTORY), cutoff_ms, dry_run)?,
        temp_entries: cleanup_expired_temp_entries(
            &data_root.join(TMP_DIRECTORY),
            cutoff_ms,
            dry_run,
        )?,
        mcp_schema_rows: cleanup_mcp_schema_cache(
            &root.join(CACHE_DIRECTORY).join(MCP_SCHEMA_CACHE_FILE),
            cutoff_ms,
            dry_run,
        )?,
        archive_files: cleanup_expired_archive_files(
            &data_root.join(CONTENT_ARCHIVE_DIRECTORY),
            cutoff_ms,
            dry_run,
        )?,
        shell_artifact_files,
    })
}

fn cleanup_expired_logs(log_dir: &Path, cutoff_ms: u64, dry_run: bool) -> Result<usize, String> {
    if !log_dir.is_dir() {
        return Ok(0);
    }
    let entries = fs::read_dir(log_dir).map_err(|error| format!("读取日志目录失败：{error}"))?;
    let mut deleted = 0usize;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取日志目录失败：{error}"))?;
        let path = entry.path();
        if !path.is_file() || !is_axon_log_file(&path) || !is_expired(&path, cutoff_ms) {
            continue;
        }
        if !dry_run {
            fs::remove_file(&path).map_err(|error| format!("删除过期日志失败：{error}"))?;
        }
        deleted = deleted.saturating_add(1);
    }
    Ok(deleted)
}

fn is_axon_log_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(LOG_FILE_PREFIX) && name.ends_with(LOG_FILE_SUFFIX))
}

fn cleanup_expired_temp_entries(
    tmp_dir: &Path,
    cutoff_ms: u64,
    dry_run: bool,
) -> Result<usize, String> {
    if !tmp_dir.is_dir() {
        return Ok(0);
    }
    let mut files = Vec::new();
    collect_expired_files(tmp_dir, cutoff_ms, &mut files)?;
    let mut directories = Vec::new();
    collect_expired_directories(tmp_dir, cutoff_ms, &mut directories)?;
    // 目录按深度倒序回收，保证父目录在子目录之后删除。
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut deleted = 0usize;
    for path in files {
        if dry_run {
            deleted = deleted.saturating_add(1);
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            deleted = deleted.saturating_add(1);
        }
    }
    for path in directories {
        if dry_run {
            deleted = deleted.saturating_add(1);
            continue;
        }
        if fs::remove_dir(&path).is_ok() {
            deleted = deleted.saturating_add(1);
        }
    }
    Ok(deleted)
}

fn collect_expired_files(
    dir: &Path,
    cutoff_ms: u64,
    expired: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("读取临时目录失败：{error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取临时目录失败：{error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取临时目录项失败：{error}"))?;
        if file_type.is_dir() {
            collect_expired_files(&path, cutoff_ms, expired)?;
            continue;
        }
        if is_expired(&path, cutoff_ms) {
            expired.push(path);
        }
    }
    Ok(())
}

fn collect_expired_directories(
    dir: &Path,
    cutoff_ms: u64,
    expired: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("读取临时目录失败：{error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取临时目录失败：{error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取临时目录项失败：{error}"))?;
        if !file_type.is_dir() {
            continue;
        }
        collect_expired_directories(&path, cutoff_ms, expired)?;
        if is_expired(&path, cutoff_ms) {
            expired.push(path);
        }
    }
    Ok(())
}

fn cleanup_mcp_schema_cache(path: &Path, cutoff_ms: u64, dry_run: bool) -> Result<usize, String> {
    if !path.is_file() {
        return Ok(0);
    }
    let cutoff_ms = i64::try_from(cutoff_ms).unwrap_or(i64::MAX);
    let connection = Connection::open(path).map_err(|_| "MCP Schema 缓存打开失败".to_owned())?;
    connection
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(|_| "MCP Schema 缓存 busy timeout 配置失败".to_owned())?;
    if dry_run {
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM mcp_schema_cache WHERE refreshed_at_ms < ?1",
                [cutoff_ms],
                |row| row.get(0),
            )
            .map_err(|_| "MCP Schema 缓存统计失败".to_owned())?;
        return Ok(usize::try_from(count).unwrap_or(usize::MAX));
    }
    connection
        .execute(
            "DELETE FROM mcp_schema_cache WHERE refreshed_at_ms < ?1",
            [cutoff_ms],
        )
        .map_err(|_| "MCP Schema 缓存清理失败".to_owned())
}

fn cleanup_expired_archive_files(
    archive_dir: &Path,
    cutoff_ms: u64,
    dry_run: bool,
) -> Result<usize, String> {
    if !archive_dir.is_dir() {
        return Ok(0);
    }
    let mut files = Vec::new();
    collect_expired_files(archive_dir, cutoff_ms, &mut files)?;
    let mut deleted = 0usize;
    for path in files {
        if dry_run {
            deleted = deleted.saturating_add(1);
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            deleted = deleted.saturating_add(1);
        }
    }
    Ok(deleted)
}

/// 回收超出保留期的 shell 完整输出归档。
///
/// 归档为**每个会话（或后台作业）一个文件**，而 `reap_once` 只从注册表移除会话、
/// 墓碑也不携带路径：本函数是这些文件**唯一的删除点**。缺了它，文件数会随
/// 调用次数无界增长（实测单机 3 天累积 1131 个），而单文件 64 MiB 的上限只约束
/// 单文件，对聚合维度毫无作用。
///
/// 只处理后缀匹配的文件，同目录下的其他内容一律不动。
fn cleanup_expired_shell_artifacts(
    artifact_dir: &Path,
    cutoff_ms: u64,
    dry_run: bool,
) -> Result<usize, String> {
    if !artifact_dir.is_dir() {
        return Ok(0);
    }
    let entries =
        fs::read_dir(artifact_dir).map_err(|error| format!("读取归档目录失败：{error}"))?;
    let mut deleted = 0usize;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取归档目录失败：{error}"))?;
        let path = entry.path();
        if !path.is_file() || !is_shell_artifact_file(&path) || !is_expired(&path, cutoff_ms) {
            continue;
        }
        // 单个文件删除失败不阻断整轮清理（可能被并发持锁），与归档目录的处理口径一致。
        if dry_run || fs::remove_file(&path).is_ok() {
            deleted = deleted.saturating_add(1);
        }
    }
    Ok(deleted)
}

fn is_shell_artifact_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(SHELL_ARTIFACT_FILE_SUFFIX))
}

fn is_expired(path: &Path, cutoff_ms: u64) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    let Ok(modified_ms) = modified.duration_since(UNIX_EPOCH) else {
        return false;
    };
    modified_ms.as_millis() < u128::from(cutoff_ms)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rusqlite::Connection;

    use super::*;

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("路径应含父目录")).expect("测试目录应可创建");
        fs::write(path, content).expect("测试文件应可写入");
    }

    /// 测试专用：把 shell 归档的**临时回退根**隔离到 `root` 之下。
    ///
    /// 直接调用公开入口会以真实进程临时目录为作用域，把无关产物一并删除：
    /// 那是测试污染真实环境，必须隔离。
    fn cleanup_runtime_files_isolated(
        root: &Path,
        cutoff_ms: u64,
    ) -> Result<FileCleanupCounts, String> {
        run_file_cleanup(root, cutoff_ms, false, &root.join("isolated-temp"))
    }

    fn count_runtime_files_isolated(
        root: &Path,
        cutoff_ms: u64,
    ) -> Result<FileCleanupCounts, String> {
        run_file_cleanup(root, cutoff_ms, true, &root.join("isolated-temp"))
    }

    /// 把文件修改时间回拨到纪元初，模拟超出保留期的历史文件。
    fn age_file(path: &Path) {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("测试文件应可打开");
        file.set_modified(UNIX_EPOCH + Duration::from_millis(1))
            .expect("测试文件应可回拨修改时间");
    }

    #[test]
    fn expired_logs_and_temp_files_are_removed() {
        let root = axon_test_support::test_temp_dir();
        let logs = root.path().join("data").join("logs");
        write_file(&logs.join("axon-2020-01-01.log"), "旧日志");
        age_file(&logs.join("axon-2020-01-01.log"));
        write_file(&logs.join("axon-2999-01-01.log"), "新日志");
        write_file(&logs.join("other.log"), "无关文件");
        let tmp = root.path().join("data").join("tmp");
        write_file(&tmp.join("stale.bin"), "旧临时文件");
        age_file(&tmp.join("stale.bin"));
        let archive = root.path().join("data").join("archive");
        write_file(&archive.join("old.log"), "旧工具输出");
        age_file(&archive.join("old.log"));
        write_file(&archive.join("new.log"), "新工具输出");

        let counts = cleanup_runtime_files_isolated(root.path(), 100).expect("清理应成功");

        assert_eq!(counts.log_files, 1);
        assert_eq!(counts.temp_entries, 1);
        assert_eq!(counts.archive_files, 1);
        assert!(!logs.join("axon-2020-01-01.log").exists());
        assert!(logs.join("axon-2999-01-01.log").exists());
        assert!(logs.join("other.log").exists());
        assert!(!tmp.join("stale.bin").exists());
        assert!(!archive.join("old.log").exists());
        assert!(archive.join("new.log").exists());
    }

    #[test]
    fn dry_run_reports_without_deleting() {
        let root = axon_test_support::test_temp_dir();
        let logs = root.path().join("data").join("logs");
        write_file(&logs.join("axon-2020-01-01.log"), "旧日志");
        age_file(&logs.join("axon-2020-01-01.log"));
        let archive = root.path().join("data").join("archive");
        write_file(&archive.join("old.log"), "旧工具输出");
        age_file(&archive.join("old.log"));

        let counts = count_runtime_files_isolated(root.path(), 100).expect("dry-run 应成功");

        assert_eq!(counts.log_files, 1);
        assert_eq!(counts.archive_files, 1);
        assert!(logs.join("axon-2020-01-01.log").exists());
        assert!(archive.join("old.log").exists());
    }

    #[test]
    fn stale_mcp_schema_cache_rows_are_removed() {
        let root = axon_test_support::test_temp_dir();
        let cache = root.path().join("cache").join("mcp_tool_schemas.sqlite3");
        fs::create_dir_all(cache.parent().expect("路径应含父目录")).expect("测试目录应可创建");
        let connection = Connection::open(&cache).expect("测试缓存应可打开");
        connection
            .execute_batch(
                "CREATE TABLE mcp_schema_cache (
                     server_id TEXT PRIMARY KEY NOT NULL,
                     config_ref TEXT NOT NULL,
                     config_fingerprint TEXT NOT NULL,
                     refreshed_at_ms INTEGER NOT NULL,
                     schemas_json TEXT NOT NULL
                 );
                 INSERT INTO mcp_schema_cache VALUES('old','ref','fp',1,'{}');
                 INSERT INTO mcp_schema_cache VALUES('new','ref','fp',9999,'{}');",
            )
            .expect("测试缓存应可写入");
        drop(connection);

        let counts = cleanup_runtime_files_isolated(root.path(), 100).expect("清理应成功");

        assert_eq!(counts.mcp_schema_rows, 1);
        let connection = Connection::open(&cache).expect("测试缓存应可重开");
        let remaining: i64 = connection
            .query_row("SELECT COUNT(*) FROM mcp_schema_cache", [], |row| {
                row.get(0)
            })
            .expect("统计应成功");
        assert_eq!(remaining, 1);
    }

    /// R-25 回归：超出保留期的 shell 归档必须回收，且不误伤保留期内文件与其他内容。
    ///
    /// 归档此前**没有任何删除点**：`reap_once` 只清注册表，墓碑不携带路径；
    /// 而 P0-01 的 64 MiB 上限只约束单文件，对文件数无作用。实测单机 3 天累积 1131 个。
    #[test]
    fn expired_shell_artifacts_are_reclaimed_without_touching_other_files() {
        let root = axon_test_support::test_temp_dir();
        let tee = root
            .path()
            .join(axon_config::AXON_RUNTIME_DATA_DIR)
            .join("logs")
            .join(SHELL_ARTIFACT_DIRECTORY);
        let expired_session = tee.join("11111111-1111-1111-1111-111111111111.shell.log");
        let expired_job = tee.join("job-7.shell.log");
        let fresh = tee.join("22222222-2222-2222-2222-222222222222.shell.log");
        let unrelated = tee.join("keep-me.txt");
        write_file(&expired_session, "old session");
        write_file(&expired_job, "old job");
        write_file(&fresh, "fresh");
        write_file(&unrelated, "other");
        age_file(&expired_session);
        age_file(&expired_job);

        let counts = cleanup_runtime_files_isolated(root.path(), 100).expect("清理应成功");

        assert_eq!(counts.shell_artifact_files, 2, "应回收两个过期归档");
        assert!(!expired_session.exists(), "过期的会话归档必须被删除");
        assert!(!expired_job.exists(), "过期的作业归档必须被删除");
        assert!(fresh.exists(), "保留期内的归档不得被删除");
        assert!(unrelated.exists(), "同目录下非本工具产生的文件不得被触碰");
    }

    /// R-25 回归（二）：未注入日志根时归档会回退到进程临时目录，该处同样必须回收。
    ///
    /// 实测泄漏正发生在这一分支（真实 `%TEMP%/tee` 累积 1131 个文件），
    /// 因此只覆盖 `<data>/logs/tee` 是不完整的。
    #[test]
    fn expired_shell_artifacts_in_the_fallback_root_are_reclaimed() {
        let root = axon_test_support::test_temp_dir();
        let fallback = root
            .path()
            .join("isolated-temp")
            .join(SHELL_ARTIFACT_DIRECTORY);
        let expired = fallback.join("33333333-3333-3333-3333-333333333333.shell.log");
        let fresh = fallback.join("44444444-4444-4444-4444-444444444444.shell.log");
        write_file(&expired, "old");
        write_file(&fresh, "fresh");
        age_file(&expired);

        let counts = cleanup_runtime_files_isolated(root.path(), 100).expect("清理应成功");

        assert_eq!(
            counts.shell_artifact_files, 1,
            "回退分支的过期归档也必须回收"
        );
        assert!(!expired.exists(), "回退根的过期归档必须被删除");
        assert!(fresh.exists(), "保留期内的归档不得被删除");
    }

    /// 真机性能验收：整条清理链路的只读统计必须在有界时间内完成。
    #[test]
    #[ignore = "真机验收：需设置 AXON_CLEANUP_BENCH_ROOT 指向 ~/.axon"]
    fn real_device_cleanup_dry_run_is_fast() {
        let root = std::path::PathBuf::from(
            std::env::var("AXON_CLEANUP_BENCH_ROOT").expect("需要 AXON_CLEANUP_BENCH_ROOT"),
        );
        let cutoff_ms = axon_time::unix_now_ms().saturating_sub(7 * 24 * 60 * 60 * 1_000);
        let started = std::time::Instant::now();
        let runtime =
            crate::cron_commands::runtime_cleanup::count_runtime_cleanup(&root, cutoff_ms)
                .expect("运行态统计应成功");
        let files = count_runtime_files(&root, cutoff_ms).expect("文件统计应成功");
        let elapsed = started.elapsed();
        println!("cleanup dry-run：runtime={runtime:?} files={files:?} elapsed={elapsed:?}");
        assert!(
            elapsed.as_millis() < 1_000,
            "清理 dry-run 统计必须在一秒内完成"
        );
    }
}
