//! 运行态事件的归档与磁盘水位淘汰。
//!
//! 删除历史事件之前先把它们写成 `.jsonl.zst`：行没了、事实还在，按需解压即可回放。
//! 归档按 lane 分文件并追加写（zstd 允许把多个帧首尾相接，因此清理是分批、可中断的），
//! 文件名与事件正文外置目录并列，运维只需要看 `data/archive/session-events` 一个目录。

use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

use rusqlite::{Connection, params};

use super::runtime_cleanup::session_event_expiry_filter;

/// 运行态库的磁盘硬上限。
///
/// 取值必须明显高于「保留窗口内的正常体量」。实测口径按高负载重算：
/// desktop 车道单日可写入 30 万到 45 万帧、约 400 MiB/天，7 天保留期对应
/// 约 2.8 GiB。旧值 2 GiB 依据的是 55 MiB/天的旧基线，会让水位淘汰先于
/// 保留期触发，把 7 天窗口悄悄压到 2 至 3 天，因此提高到 4 GiB。
pub(super) const RUNTIME_MAX_DISK_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 高水位：越过即按「最旧优先」开始淘汰。
pub(super) const RUNTIME_HIGH_WATER_BYTES: u64 = RUNTIME_MAX_DISK_BYTES / 10 * 8;
/// 低水位：淘汰到这里停止，避免每轮都贴着上限抖动。
pub(super) const RUNTIME_LOW_WATER_BYTES: u64 = RUNTIME_MAX_DISK_BYTES / 2;
/// 水位淘汰的最小事件年龄：正在跑的轮次刚写入的事件不参与。
pub(super) const WATERMARK_MIN_AGE_MS: u64 = 60 * 60 * 1_000;
/// 单批归档并删除的行数。
const ARCHIVE_BATCH_ROWS: i64 = 2_000;
/// 归档压缩级别：清理是后台动作，选速度优先的档位。
const ARCHIVE_ZSTD_LEVEL: i32 = 3;

/// 一次归档/淘汰的结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ArchiveReport {
    /// 已归档并删除的事件行数。
    pub rows: usize,
    /// 本次触碰的归档文件数。
    pub files: usize,
}

impl ArchiveReport {
    fn merge(&mut self, other: Self) {
        self.rows = self.rows.saturating_add(other.rows);
        self.files = self.files.saturating_add(other.files);
    }

    /// 本次是否真的归档了内容。
    pub(super) fn is_empty(&self) -> bool {
        self.rows == 0 && self.files == 0
    }
}

/// 归档根目录：与事件正文外置共用 `data/archive/session-events`。
pub(super) fn event_archive_root(database_path: &Path) -> PathBuf {
    let parent = database_path.parent().unwrap_or(Path::new("."));
    parent.join("archive").join("session-events")
}

/// 把超出保留期的事件打成 `.jsonl.zst` 再删除行。
pub(super) fn archive_expired_session_events(
    connection: &mut Connection,
    archive_root: &Path,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<ArchiveReport, String> {
    let selection = format!(
        "SELECT rowid, session_id, sequence, event_type, payload_json, payload_ref, emitted_at_ms {} ORDER BY emitted_at_ms ASC LIMIT ?2",
        session_event_expiry_filter()
    );
    let mut report = ArchiveReport::default();
    while Instant::now() < deadline {
        let batch = read_archive_batch(connection, &selection, cutoff_ms)?;
        if batch.is_empty() {
            break;
        }
        report.merge(write_archives(archive_root, &batch)?);
        let deleted = delete_rows_by_rowid(connection, &batch)?;
        if !deleted.is_empty() {
            reclaim_external_payload_files(archive_root, &batch, &deleted);
            // 行已提交删除，才允许按新的保底线回收整片正文。
            let sessions: Vec<String> = batch.iter().map(|row| row.session_id.clone()).collect();
            reclaim_dead_payload_shards(connection, archive_root, &sessions);
        }
        if deleted.len() < batch.len() {
            // 行在归档期间被并发删除：本轮到此为止，下一轮重新取候选。
            break;
        }
    }
    Ok(report)
}

/// 磁盘水位淘汰：越过高水位时按最旧优先删到低水位。
///
/// 只淘汰一小时以前的、且不属于运行中 lane 的事件；每批仍然先归档再删。
pub(super) fn enforce_disk_budget(
    connection: &mut Connection,
    database_path: &Path,
    archive_root: &Path,
    now_ms: u64,
    deadline: Instant,
) -> Result<ArchiveReport, String> {
    if database_footprint(connection, database_path)? <= RUNTIME_HIGH_WATER_BYTES {
        return Ok(ArchiveReport::default());
    }
    let cutoff_ms = i64::try_from(now_ms.saturating_sub(WATERMARK_MIN_AGE_MS))
        .map_err(|_| "水位淘汰时间戳超出范围".to_owned())?;
    let selection = "SELECT rowid, session_id, sequence, event_type, payload_json, payload_ref, emitted_at_ms \
         FROM session_events \
         WHERE emitted_at_ms < ?1 \
           AND session_id NOT IN (SELECT session_id FROM session_runtimes WHERE status IN ('running','recovering')) \
         ORDER BY emitted_at_ms ASC LIMIT ?2";
    let mut report = ArchiveReport::default();
    while Instant::now() < deadline {
        if database_footprint(connection, database_path)? <= RUNTIME_LOW_WATER_BYTES {
            break;
        }
        let batch = read_archive_batch(connection, selection, cutoff_ms)?;
        if batch.is_empty() {
            break;
        }
        report.merge(write_archives(archive_root, &batch)?);
        let deleted = delete_rows_by_rowid(connection, &batch)?;
        if !deleted.is_empty() {
            reclaim_external_payload_files(archive_root, &batch, &deleted);
            // 行已提交删除，才允许按新的保底线回收整片正文。
            let sessions: Vec<String> = batch.iter().map(|row| row.session_id.clone()).collect();
            reclaim_dead_payload_shards(connection, archive_root, &sessions);
        }
        if deleted.len() < batch.len() {
            break;
        }
    }
    Ok(report)
}

/// 单条待归档事件。
#[derive(Debug)]
struct ArchiveRow {
    rowid: i64,
    session_id: String,
    sequence: i64,
    event_type: String,
    payload_json: String,
    payload_ref: Option<String>,
    emitted_at_ms: i64,
}

fn read_archive_batch(
    connection: &Connection,
    selection_sql: &str,
    cutoff_ms: i64,
) -> Result<Vec<ArchiveRow>, String> {
    let mut statement = connection
        .prepare(selection_sql)
        .map_err(|error| format!("归档候选查询准备失败：{error}"))?;
    let rows = statement
        .query_map(params![cutoff_ms, ARCHIVE_BATCH_ROWS], |row| {
            Ok(ArchiveRow {
                rowid: row.get(0)?,
                session_id: row.get(1)?,
                sequence: row.get(2)?,
                event_type: row.get(3)?,
                payload_json: row.get(4)?,
                payload_ref: row.get(5)?,
                emitted_at_ms: row.get(6)?,
            })
        })
        .map_err(|error| format!("归档候选查询失败：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("归档候选读取失败：{error}"))
}

/// 按 lane 追加写入 `.jsonl.zst`，并回收已外置的正文文件。
fn write_archives(archive_root: &Path, batch: &[ArchiveRow]) -> Result<ArchiveReport, String> {
    std::fs::create_dir_all(archive_root).map_err(|error| format!("归档目录创建失败：{error}"))?;
    let mut report = ArchiveReport::default();
    let mut index = 0;
    while index < batch.len() {
        let session_id = batch[index].session_id.clone();
        let start = index;
        while index < batch.len() && batch[index].session_id == session_id {
            index = index.saturating_add(1);
        }
        let lane_rows = &batch[start..index];
        let path = archive_root.join(format!("{session_id}.jsonl.zst"));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("归档文件打开失败 {}: {error}", path.display()))?;
        let mut encoder = zstd::Encoder::new(file, ARCHIVE_ZSTD_LEVEL)
            .map_err(|error| format!("归档压缩器创建失败：{error}"))?;
        for row in lane_rows {
            let payload = resolve_payload(archive_root, row);
            let line = serde_json::json!({
                "sessionId": row.session_id,
                "sequence": row.sequence,
                "eventType": row.event_type,
                "emittedAtMs": row.emitted_at_ms,
                "payload": payload,
            });
            writeln!(encoder, "{line}").map_err(|error| format!("归档写入失败：{error}"))?;
        }
        encoder
            .finish()
            .map_err(|error| format!("归档收口失败：{error}"))?;
        report.rows = report.rows.saturating_add(lane_rows.len());
        report.files = report.files.saturating_add(1);
    }
    Ok(report)
}

/// 取回事件正文：外置正文读文件，读不到时留一条可诊断的标记而不是静默丢行。
fn resolve_payload(archive_root: &Path, row: &ArchiveRow) -> String {
    match row.payload_ref.as_deref() {
        Some(reference) => axon_session::read_event_payload(archive_root, reference)
            .unwrap_or_else(|_| format!("<正文归档不可读：{reference}>")),
        None => row.payload_json.clone(),
    }
}

/// 在数据库行确认删除之后，回收这一批里**确实被删掉**的行所引用的外置正文文件。
///
/// 只按实际删除集合回收是关键：`batch` 里的 rowid 可能在读取候选之后被并发删除，
/// 整批回收会把仍被其他行引用的文件误删，制造「行还在、文件没了」的孤儿引用。
/// 只有独占一个文件的引用才随行回收；分片正文与同会话其他事件共用文件，
/// 单条行过期就删整片会把仍有效的正文一起毁掉，这类文件交给文件保留期统一清理。
fn reclaim_external_payload_files(archive_root: &Path, batch: &[ArchiveRow], deleted: &[i64]) {
    let deleted_set: std::collections::HashSet<i64> = deleted.iter().copied().collect();
    let references: Vec<&str> = batch
        .iter()
        .filter(|row| deleted_set.contains(&row.rowid))
        .filter_map(|row| row.payload_ref.as_deref())
        .collect();
    reclaim_exclusive_payloads(archive_root, &references);
}

/// 回收一批已确认删除的独占外置正文，以及因此变空的车道子目录。
///
/// 事务提交之后才允许调用：行还没删就先删文件，一旦回滚正文就永久没了。
/// 只认一对一映射的引用（`is_exclusive_payload_reference`）；分片正文与同会话
/// 其他事件共用文件，交给文件保留期统一清理。
pub(super) fn reclaim_exclusive_payloads(archive_root: &Path, references: &[&str]) {
    for reference in references {
        if !axon_session::is_exclusive_payload_reference(reference) {
            continue;
        }
        let path = axon_session::payload_storage_path(archive_root, reference);
        let _ = std::fs::remove_file(&path);
        if let Some(parent) = path.parent()
            && parent != archive_root
        {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// 整条会话家族已被删除后，回收该会话的外置事件正文目录（保留 `.jsonl.zst` 离线归档）。
pub(super) fn reclaim_session_archive_dir(archive_root: &Path, session_id: &str) {
    if session_id.is_empty()
        || session_id == "."
        || session_id == ".."
        || session_id.contains(['/', '\\'])
    {
        return;
    }
    let dir = archive_root.join(session_id);
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 按会话回收「整片都已过期」的外置正文分片，返回回收的文件数。
///
/// 分片正文被同一会话的多条事件共用，单条行过期时不能删整片；但一片只覆盖固定序号
/// 区间，只要整段区间都早于该会话仍存活的最小序号，片内就不可能有活着的行，此时
/// 整片回收才安全。没有这条路径时，行按保留期删掉了、正文却永久留在磁盘上，主库
/// 不再膨胀的代价被转移成了无上限的归档目录增长。
///
/// 必须在删除事务提交之后调用：行还在时算出的保底线不成立，会删掉仍被引用的正文。
pub(super) fn reclaim_dead_payload_shards(
    connection: &Connection,
    archive_root: &Path,
    session_ids: &[String],
) -> usize {
    let mut reclaimed = 0;
    for session_id in session_ids.iter().collect::<BTreeSet<_>>() {
        let min_live = connection
            .query_row(
                "SELECT MIN(sequence) FROM session_events WHERE session_id=?1",
                params![session_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .and_then(|value| u64::try_from(value).ok());
        reclaimed += axon_session::reclaim_shards_before(archive_root, session_id, min_live);
    }
    reclaimed
}

/// 删除一批已归档事件行，返回**确实被删掉**的 rowid 列表。
///
/// 用单条 `DELETE ... RETURNING` 同时完成删除与回执：只有拿到真实删除集合，调用方
/// 才能精确回收外置正文。先查后删会在两条语句之间留出并发窗口，窗口内被别的清理
/// 删掉的行仍留在候选集合里，调用方就分不清谁真的被删了，只能整批回收，误删仍被
/// 其他行引用的文件。
fn delete_rows_by_rowid(
    connection: &mut Connection,
    batch: &[ArchiveRow],
) -> Result<Vec<i64>, String> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; batch.len()].join(",");
    let sql = format!("DELETE FROM session_events WHERE rowid IN ({placeholders}) RETURNING rowid");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("归档删除语句准备失败：{error}"))?;
    let deleted = statement
        .query_map(
            rusqlite::params_from_iter(batch.iter().map(|row| row.rowid)),
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("归档删除失败：{error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("归档删除回执读取失败：{error}"))?;
    Ok(deleted)
}

/// 数据库当前占用的有效字节数：主库 + WAL 减去可复用空闲页。
///
/// 只按文件大小判定会让删完的行在 `incremental_vacuum` 之前一直「看起来还在」，
/// 于是水位淘汰每一轮都重复踩同一个高水位。
fn database_footprint(connection: &Connection, database_path: &Path) -> Result<u64, String> {
    let file_bytes = file_size(database_path);
    let wal_path = PathBuf::from(format!("{}-wal", database_path.display()));
    let wal_bytes = file_size(&wal_path);
    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| format!("读取页大小失败：{error}"))?;
    let freelist: i64 = connection
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .map_err(|error| format!("读取空闲页失败：{error}"))?;
    let free_bytes = u64::try_from(page_size.saturating_mul(freelist)).unwrap_or(0);
    Ok(file_bytes
        .saturating_add(wal_bytes)
        .saturating_sub(free_bytes))
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// 造一个只有清理相关两张表的最小运行态库。
    fn fixture() -> (Connection, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("临时目录");
        let connection =
            Connection::open(directory.path().join("runtime.sqlite3")).expect("打开临时库");
        connection
            .execute_batch(
                "CREATE TABLE session_runtimes (
                     session_id TEXT PRIMARY KEY,
                     last_event_sequence INTEGER NOT NULL,
                     last_activity_at_ms INTEGER NOT NULL,
                     status TEXT NOT NULL
                 );
                 CREATE TABLE session_events (
                     session_id TEXT NOT NULL,
                     sequence INTEGER NOT NULL,
                     generation TEXT NOT NULL,
                     event_type TEXT NOT NULL,
                     payload_json TEXT NOT NULL,
                     emitted_at_ms INTEGER NOT NULL,
                     payload_ref TEXT,
                     PRIMARY KEY(session_id, sequence)
                 );",
            )
            .expect("建表");
        (connection, directory)
    }

    /// 活动时间一律写 0：归档判定只看事件年龄与序号水位，车道不因「最近活动」被保留。
    fn insert_lane(connection: &Connection, session_id: &str, status: &str, last_sequence: i64) {
        connection
            .execute(
                "INSERT INTO session_runtimes(session_id,last_event_sequence,last_activity_at_ms,status) VALUES(?1,?2,0,?3)",
                params![session_id, last_sequence, status],
            )
            .expect("insert runtime");
    }

    fn insert_event(connection: &Connection, session_id: &str, sequence: i64, emitted_at_ms: i64) {
        connection
            .execute(
                "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms) \
                 VALUES(?1,?2,'g','wire_event',?3,?4)",
                params![
                    session_id,
                    sequence,
                    format!("{{\"sequence\":{sequence}}}"),
                    emitted_at_ms
                ],
            )
            .expect("insert event");
    }

    fn count_events(connection: &Connection, session_id: &str) -> i64 {
        connection
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .expect("count")
    }

    /// 删除之前必须先把行写进 `.jsonl.zst`：行没了，事实还要能解压回来。
    #[test]
    fn expired_events_are_archived_before_they_are_deleted() {
        let (mut connection, directory) = fixture();
        let lane = "session-lane-archive";
        insert_lane(&connection, lane, "idle", 1_001);
        insert_event(&connection, lane, 1, 1);

        let archive_root = directory.path().join("archive");
        let report = archive_expired_session_events(
            &mut connection,
            &archive_root,
            i64::MAX,
            Instant::now() + std::time::Duration::from_secs(5),
        )
        .expect("归档过期事件");

        assert_eq!(report.rows, 1);
        assert_eq!(count_events(&connection, lane), 0, "归档之后必须删行");

        let path = archive_root.join(format!("{lane}.jsonl.zst"));
        let bytes = std::fs::read(&path).expect("归档文件应存在");
        let decoded = zstd::stream::decode_all(bytes.as_slice()).expect("归档必须可解压");
        let text = String::from_utf8(decoded).expect("归档正文是 UTF-8");
        assert!(text.contains("\"sequence\":1"), "归档行必须带序号: {text}");
        assert!(
            text.contains("\"payload\""),
            "归档行必须带正文而不是只留引用: {text}"
        );
    }

    /// 水位淘汰只看「一小时以前的、且不在运行中的 lane」的事件。
    #[test]
    fn watermark_selection_skips_running_lanes_and_fresh_events() {
        let (connection, _directory) = fixture();
        insert_lane(&connection, "session-running", "running", 10);
        insert_lane(&connection, "session-idle", "idle", 10);
        let now_ms = 10_000_000_u64;
        let fresh = i64::try_from(now_ms - 1_000).expect("fresh");
        insert_event(&connection, "session-running", 1, 1);
        insert_event(&connection, "session-idle", 1, 1);
        insert_event(&connection, "session-idle", 2, fresh);

        let selection = "SELECT rowid, session_id, sequence, event_type, payload_json, payload_ref, emitted_at_ms \
             FROM session_events \
             WHERE emitted_at_ms < ?1 \
               AND session_id NOT IN (SELECT session_id FROM session_runtimes WHERE status IN ('running','recovering')) \
             ORDER BY emitted_at_ms ASC LIMIT ?2";
        let cutoff = i64::try_from(now_ms - WATERMARK_MIN_AGE_MS).expect("cutoff");
        let batch = read_archive_batch(&connection, selection, cutoff).expect("读取候选");

        assert_eq!(batch.len(), 1, "只应选中空闲 lane 的旧事件: {batch:?}");
        assert_eq!(batch[0].session_id, "session-idle");
    }

    #[test]
    fn archiving_expired_events_reclaims_external_payload_files_and_empty_dirs() {
        let (mut connection, directory) = fixture();
        let lane = "session-lane-external";
        insert_lane(&connection, lane, "completed", 1);
        let archive_root = directory.path().join("archive");
        let ref_rel = format!("{lane}/1.json");
        let ext_path = archive_root.join(&ref_rel);
        std::fs::create_dir_all(ext_path.parent().expect("parent")).expect("create dir");
        std::fs::write(&ext_path, "{\"big\":true}").expect("write external payload");
        connection
            .execute(
                "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms,payload_ref) \
                 VALUES(?1,1,'g','wire_event','',1,?2)",
                params![lane, ref_rel],
            )
            .expect("insert event");

        let report = archive_expired_session_events(
            &mut connection,
            &archive_root,
            i64::MAX,
            Instant::now() + std::time::Duration::from_secs(5),
        )
        .expect("归档应成功");

        assert_eq!(report.rows, 1);
        assert!(!ext_path.exists(), "归档并删行后必须回收外置正文文件");
        assert!(
            !archive_root.join(lane).exists(),
            "外置文件全部回收后应清理空的 session 子目录"
        );
        assert!(
            archive_root.join(format!("{lane}.jsonl.zst")).exists(),
            "离线归档压缩文件必须保留"
        );
    }
}
