//! 投影水位：同一批事件只投影一次。
//!
//! `projection_state` 记录每个会话「已投影到哪一批」。累加型投影（会话累计指标）没有
//! 幂等语义：同一批事件重放一次，轮次数与累计耗时就会翻倍。闸门按「批首序号」判断，
//! 批首序号不大于水位就整批跳过。
//!
//! 闸门放在投影入口（`apply_projections`）这一层，不放进任何一个投影内部：三个投影是
//! 同一次提交的派生结果，只跳过其中一个会留下彼此矛盾的行；放在入口还能让累加投影保持
//! 原签名，现有调用点一处都不用改。

use rusqlite::{OptionalExtension, params};

use super::super::SqliteRuntimeStore;
use super::super::helpers::{i64_value, storage};
use axon_domain::DaemonSessionId;

/// 这一批是否已经投影过。
///
/// 判据是「批首序号不大于水位」：整批是同一次提交，已投影过就整批跳过，不能只跳过累加
/// 那一个投影。
pub(super) fn already_projected(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
) -> bool {
    read_watermark(store, session_id).is_some_and(|last| start_sequence <= last)
}

/// 投影跑完后把水位推进到本批的批首序号。
///
/// 取 `MAX`：迟到的低序号批次不得把水位倒退回去。写失败只记日志：水位缺失最多让下一批
/// 多投影一次，投影本身是可重建的派生数据。
pub(super) fn advance(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
    now_ms: u64,
) {
    let written = store.connection().and_then(|connection| {
        connection
            .execute(
                "INSERT INTO projection_state(session_id,last_projected_seq,updated_at_ms) \
                 VALUES(?1,?2,?3) \
                 ON CONFLICT(session_id) DO UPDATE SET \
                     last_projected_seq=MAX(projection_state.last_projected_seq,\
                         excluded.last_projected_seq), \
                     updated_at_ms=excluded.updated_at_ms",
                params![
                    session_id.as_str(),
                    i64_value(start_sequence)?,
                    i64_value(now_ms)?
                ],
            )
            .map_err(storage)
    });
    if let Err(error) = written {
        report(&format!(
            "投影水位写入失败，下一批可能重复累加: session={} start={start_sequence} error={error}",
            session_id.as_str()
        ));
    }
}

/// 读取水位；没有这一行（从未投影过）时返回 `None`。
///
/// 读失败按「未投影」返回 `None`：宁可多算一次，也不能因为一次读失败就静默丢掉一整批
/// 投影。失败事实同时进日志与 stderr，与事件面写失败同款。
fn read_watermark(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> Option<u64> {
    let read = store.connection().and_then(|connection| {
        connection
            .query_row(
                "SELECT last_projected_seq FROM projection_state WHERE session_id=?1",
                params![session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage)
    });
    match read {
        Ok(found) => found.and_then(|value| u64::try_from(value).ok()),
        Err(error) => {
            report(&format!(
                "投影水位读取失败，本批按未投影处理: session={} error={error}",
                session_id.as_str()
            ));
            None
        }
    }
}

/// 失败事实同时写日志文件与 stderr：日志文件是线上定位依据，stderr 是不依赖日志初始化的
/// 兜底通道（CLI 直跑时也能立刻看到）。
fn report(message: &str) {
    axon_logger::error(message);
    eprintln!("{message}");
}
