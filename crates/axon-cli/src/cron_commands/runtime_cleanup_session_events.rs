//! Session Event 的定向清理与悬空正文引用修复。
//!
//! 从 `runtime_cleanup_families` 拆出：这一组直接操作 `session_events` 的正文
//! 生命周期，与其余按族删除的逻辑没有交集，独立成文后单文件硬限也不会被挤爆。

use std::path::Path;
use std::time::Instant;

use rusqlite::Connection;

use super::super::runtime_cleanup::{
    RUNTIME_CLEANUP_BATCH_LIMIT, RuntimeCleanupCounts, delete_ids_in, placeholders,
};

/// 删除 retained Session 中超出保留期且位于 sequence 保底线之前的旧事件。
///
/// 与归档共用同一份保留谓词：只要预算还有余量就按批推进，但**绝不越过预算**
/// 强推第一批。归档耗尽预算时这里直接返回，剩余数据由下一轮 Cron 重新走
/// 「先归档、再删行」的完整链路，外置正文因此不会被无归档地删成孤儿。
///
/// 每批在事务里同时取 rowid 与外置正文引用，提交成功之后才回收独占文件：
/// 行还没删就先删文件，一旦回滚正文就永久没了。
pub(in crate::cron_commands) fn delete_expired_session_events(
    connection: &mut Connection,
    archive_root: &Path,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<RuntimeCleanupCounts, String> {
    let sql = super::expired_session_events_selection_sql();
    let batch = i64::try_from(RUNTIME_CLEANUP_BATCH_LIMIT).unwrap_or(i64::MAX);
    let mut counts = RuntimeCleanupCounts::default();
    loop {
        if Instant::now() >= deadline {
            break;
        }
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "Daemon runtime Session Event 清理事务启动失败".to_owned())?;
        let rows = {
            let mut statement = transaction
                .prepare_cached(&sql)
                .map_err(|_| "Daemon runtime 过期 Session Event 查询失败".to_owned())?;
            statement
                .query_map([cutoff_ms, batch], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|_| "Daemon runtime 过期 Session Event 查询失败".to_owned())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| "Daemon runtime 过期 Session Event 读取失败".to_owned())?
        };
        if rows.is_empty() {
            transaction
                .rollback()
                .map_err(|_| "Daemon runtime Session Event 清理事务回滚失败".to_owned())?;
            break;
        }
        let rowids: Vec<String> = rows.iter().map(|(rowid, _, _)| rowid.to_owned()).collect();
        let payload_refs: Vec<&str> = rows
            .iter()
            .filter_map(|(_, _, reference)| reference.as_deref())
            .collect();
        let sessions: Vec<String> = rows
            .iter()
            .map(|(_, session_id, _)| session_id.clone())
            .collect();
        let placeholders = placeholders(rowids.len());
        let deleted = delete_ids_in(
            &transaction,
            "session_events",
            "rowid",
            &placeholders,
            &rowids,
        )?;
        transaction
            .commit()
            .map_err(|_| "Daemon runtime Session Event 清理事务提交失败".to_owned())?;
        /*
         * 只有整批都确认删除才回收外置正文：`IMMEDIATE` 事务持有写锁时这是常态，
         * 一旦出现差额说明这一批里有 rowid 已经不在库里。此时回收整批会把仍被
         * 其他行引用的文件误删，制造「行还在、文件没了」的孤儿引用；宁可留一个
         * 孤儿文件交给文件保留期处理，也不制造悬空引用。
         */
        if deleted == rowids.len() {
            super::super::runtime_event_archive::reclaim_exclusive_payloads(
                archive_root,
                &payload_refs,
            );
        }
        if deleted > 0 {
            // 行已提交删除：按各会话新的保底线回收整片已被清空的分片正文。
            // 缺了这一步，正文会永远留在磁盘上，把主库膨胀换成归档目录无界增长。
            super::super::runtime_event_archive::reclaim_dead_payload_shards(
                connection,
                archive_root,
                &sessions,
            );
        }
        counts.session_events = counts.session_events.saturating_add(deleted);
        if deleted < rows.len() {
            break;
        }
    }
    Ok(counts)
}

/// 修复指向不存在文件的外置正文引用。
///
/// 读取侧已经把缺失正文降级为占位帧，这里做的是收口：把悬空引用改写成内联
/// 占位并清空 `payload_ref`，下一次读取不再为同一行重复做一次失败的文件 I/O。
///
/// 只处理最近活跃的会话：正文的价值与「这条会话还可能被打开」强相关，冷会话的
/// 悬空引用会随保留期整族回收，不值得每轮全表扫描。每轮还受墙上时钟预算约束，
/// 剩下的会话由下一轮继续。
pub(in crate::cron_commands) fn repair_dangling_payload_refs(
    connection: &mut Connection,
    archive_root: &Path,
    cutoff_ms: i64,
    deadline: Instant,
) -> Result<usize, String> {
    /// 单轮最多检查的活跃会话数。
    const REPAIR_SESSION_LIMIT: i64 = 64;
    /// 单个会话单轮最多检查的事件数。
    const REPAIR_EVENT_LIMIT: i64 = 512;
    let sessions: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT session_id FROM session_runtimes
                 WHERE last_activity_at_ms >= ?1
                 ORDER BY last_activity_at_ms DESC LIMIT ?2",
            )
            .map_err(|_| "Daemon runtime 悬空正文候选查询失败".to_owned())?;
        statement
            .query_map(rusqlite::params![cutoff_ms, REPAIR_SESSION_LIMIT], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|_| "Daemon runtime 悬空正文候选查询失败".to_owned())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| "Daemon runtime 悬空正文候选读取失败".to_owned())?
    };
    let mut repaired = 0usize;
    for session_id in sessions {
        if Instant::now() >= deadline {
            break;
        }
        let rows: Vec<(i64, String, String)> = {
            let mut statement = connection
                .prepare(
                    "SELECT rowid, event_type, payload_ref FROM session_events
                     WHERE session_id = ?1 AND payload_ref IS NOT NULL AND emitted_at_ms >= ?2
                     ORDER BY rowid ASC LIMIT ?3",
                )
                .map_err(|_| "Daemon runtime 悬空正文扫描失败".to_owned())?;
            statement
                .query_map(
                    rusqlite::params![session_id, cutoff_ms, REPAIR_EVENT_LIMIT],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(|_| "Daemon runtime 悬空正文扫描失败".to_owned())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| "Daemon runtime 悬空正文扫描失败".to_owned())?
        };
        for (rowid, event_type, reference) in rows {
            if Instant::now() >= deadline {
                break;
            }
            /*
             * 只有「文件确实不存在」才允许改写：这条 UPDATE 会清空 payload_ref，
             * 不可逆。文件被临时占用、路径过长、编码不符这类可恢复或可诊断的
             * 故障只留给读取侧降级，绝不在这里把还能救的正文判成永久丢失。
             */
            match axon_session::read_event_payload(archive_root, &reference) {
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => continue,
            }
            let placeholder = serde_json::json!({
                "archiveMissing": true,
                "payloadRef": reference,
                "originalEventType": event_type,
            })
            .to_string();
            connection
                .execute(
                    "UPDATE session_events SET payload_json = ?1, payload_ref = NULL WHERE rowid = ?2",
                    rusqlite::params![placeholder, rowid],
                )
                .map_err(|_| "Daemon runtime 悬空正文修复失败".to_owned())?;
            repaired = repaired.saturating_add(1);
        }
    }
    Ok(repaired)
}
