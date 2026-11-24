//! 会话清理后的 SQLite 安全维护。
//!
//! Cron 会递归维护 `.axon` 内已知本机数据库。只读检查先行，锁冲突跳过；空闲页为零的库
//! 不做 VACUUM；**有活动连接的库整库独占维护一律跳过**。
//!
//! 最后一条是硬规则：`VACUUM` 要把整库重写一遍，全程持有独占锁。运行态库（会话控制面、
//! 待消费队列）在这样的窗口里既不能读也不能写，逐帧追加与网关读请求会一起卡住并按
//! `busy_timeout` 报 `database is locked`。是否「活着」按 WAL 共享内存文件判定：它只在
//! 有连接打开该库时存在且非空，因此这条规则不依赖库名清单，也不依赖谁在跑。

use std::{fs, path::Path, time::Duration};

use rusqlite::Connection;

use super::session_governance::has_live_connection;

/// SQLite 文件后缀。
const SQLITE_FILE_SUFFIX: &str = ".sqlite3";
/// 清理维护使用的短锁等待时间。
const SQLITE_VACUUM_BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// 返回本轮安全完成 VACUUM 的数据库数量。
pub(super) fn vacuum_session_sqlite_files(root: &Path) -> Result<usize, String> {
    let mut vacuumed = 0usize;
    for path in sqlite_files(root)? {
        match vacuum_database_if_needed(&path) {
            Ok(true) => vacuumed = vacuumed.saturating_add(1),
            Ok(false) => {}
            Err(error) => {
                axon_logger::warning(&format!(
                    "跳过本轮 SQLite VACUUM：{}，原因={error}",
                    path.display()
                ));
            }
        }
    }
    Ok(vacuumed)
}

fn sqlite_files(root: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut files = Vec::new();
    collect_sqlite_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_sqlite_files(
    directory: &Path,
    files: &mut Vec<std::path::PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("读取 SQLite 维护目录 {} 失败：{error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取 SQLite 维护条目失败：{error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取 SQLite 维护条目类型失败：{error}"))?;
        if file_type.is_dir() {
            collect_sqlite_files(&path, files)?;
        } else if file_type.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(SQLITE_FILE_SUFFIX))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn vacuum_database_if_needed(path: &Path) -> Result<bool, String> {
    // 先判「有没有人在用」：整库独占维护不能落在活库上，锁等待预算救不了这件事。
    if has_live_connection(path) {
        axon_logger::debug(&format!(
            "跳过 SQLite VACUUM：库有活动连接，整库独占维护留给离线维护窗口 {}",
            path.display()
        ));
        return Ok(false);
    }
    let connection = open_database(path)?;
    let free_page_count: i64 = connection
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .map_err(|error| format!("读取 SQLite 空闲页失败：{error}"))?;
    if free_page_count == 0 {
        return Ok(false);
    }
    if !quick_check_passed(&connection, path)? {
        return Ok(false);
    }
    connection
        .execute_batch("VACUUM;")
        .map_err(|error| format!("SQLite VACUUM 失败 {}: {error}", path.display()))?;
    Ok(true)
}

fn quick_check_passed(connection: &Connection, path: &Path) -> Result<bool, String> {
    let result: String = connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|error| format!("SQLite 快速完整性检查失败 {}: {error}", path.display()))?;
    Ok(result.eq_ignore_ascii_case("ok"))
}

fn open_database(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open(path)
        .map_err(|error| format!("SQLite 打开失败 {}: {error}", path.display()))?;
    connection
        .busy_timeout(SQLITE_VACUUM_BUSY_TIMEOUT)
        .map_err(|error| format!("SQLite busy timeout 配置失败 {}: {error}", path.display()))?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vacuums_sqlite_with_free_pages_and_skips_empty_database() {
        let root = axon_test_support::test_temp_dir();
        let database = root.path().join("data").join("maintenance.sqlite3");
        std::fs::create_dir_all(database.parent().unwrap()).expect("测试目录应可创建");
        let connection = Connection::open(&database).expect("测试数据库应可打开");
        connection
            .execute_batch(
                "PRAGMA auto_vacuum = NONE;
                 CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
                 WITH RECURSIVE seq(n) AS (
                     SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 500
                 )
                 INSERT INTO records(value)
                 SELECT randomblob(10000) FROM seq;
                 DELETE FROM records;",
            )
            .expect("测试数据应可写入");
        let free_page_count: i64 = connection
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .expect("空闲页应可统计");
        assert!(
            free_page_count > 0,
            "fixture 应产生空闲页：{free_page_count}"
        );
        drop(connection);

        assert_eq!(vacuum_session_sqlite_files(root.path()).unwrap(), 1);
        assert_eq!(vacuum_session_sqlite_files(root.path()).unwrap(), 0);
    }
}
