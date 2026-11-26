//! 投影水位：同一批事件只投影一次。
//!
//! `projection_state` 记录每个会话「已投影到的最后一条序号」。累加型投影（会话累计
//! 指标）没有幂等语义：同一批事件重放一次，轮次数与累计耗时就会翻倍。闸门按批的
//! 首序号判断，首序号不大于水位就说明这一批已经完整投影过，整批跳过。
//!
//! 闸门放在投影执行入口（`projection_worker` 里的 `project`）这一层，不放进任何一个投影
//! 内部：三个投影是同一次提交的派生结果，只跳过其中一个会留下彼此矛盾的行；放在入口还能
//! 让累加投影保持原签名。

use rusqlite::{OptionalExtension, params};

use super::super::SqliteRuntimeStore;
use super::super::helpers::{i64_value, storage};
use axon_domain::DaemonSessionId;

/// 这一批是否已经投影过；已投影过时返回挡住它的水位。
///
/// 判据是「批首序号不大于水位」：整批是同一次提交，已投影过就整批跳过，不能只跳过累加
/// 那一个投影。把水位一并返回是为了留痕：整批跳过是静默丢投影的高危路径，调用方要能说清
/// 这一批被哪一档水位挡掉了。
pub(super) fn already_projected(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
) -> Option<u64> {
    watermark(store, session_id).filter(|last| start_sequence <= *last)
}

/// 投影跑完后把水位推进到本批的最后一条序号。
///
/// 调用方只在本批三个投影全部成功后才调用：失败批次不推进水位，重放时才不会被整批跳过。
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

/// 事件日志序号回退后清空水位。
///
/// 闸门只认「批首序号不大于水位即已投影」，它假设序号单调。事件日志被清空、修复重排或
/// 截断后，新一批的序号会落回水位之下，闸门会把这一批以及之后每一批都当成重复而整批
/// 跳过，该会话的投影就此永久停摆。调用方在检测到「落盘序号小于分配序号」时先调用它，
/// 再交给闸门判定。
pub(super) fn reset(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) {
    let deleted = store.connection().and_then(|connection| {
        connection
            .execute(
                "DELETE FROM projection_state WHERE session_id=?1",
                params![session_id.as_str()],
            )
            .map_err(storage)
    });
    match deleted {
        Ok(_) => report(&format!(
            "事件日志序号回退，已重置投影水位: session={}",
            session_id.as_str()
        )),
        Err(error) => report(&format!(
            "投影水位重置失败，该会话投影可能整批被跳过: session={} error={error}",
            session_id.as_str()
        )),
    }
}

/// 日志重排把批首序号压到水位以下时清空水位。
///
/// 与 [`reset`] 的差别在条件：只有「水位不低于重排后的批首序号」才清。事件面写失败的
/// 正常补偿（另一个写者已推进文件尾）也会重排，那种情形水位仍低于新批首序号，清水位会
/// 让已投影的历史批被重复累加。这一条挂在 `realign_sequences` 上，三条事件写路径因此
/// 共用同一处判定。
///
/// 返回是否真的清掉了水位，便于调用方留痕。
pub(super) fn reset_if_rewound(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    start_sequence: u64,
) -> bool {
    let cleared = store.connection().and_then(|connection| {
        connection
            .execute(
                "DELETE FROM projection_state \
                 WHERE session_id=?1 AND last_projected_seq>=?2",
                params![session_id.as_str(), i64_value(start_sequence)?],
            )
            .map_err(storage)
    });
    match cleared {
        Ok(count) if count > 0 => {
            report(&format!(
                "事件日志重排把序号压到水位之下，已重置投影水位: session={} start={start_sequence}",
                session_id.as_str()
            ));
            true
        }
        Ok(_) => false,
        Err(error) => {
            report(&format!(
                "投影水位重置失败，该会话投影可能整批被跳过: session={} start={start_sequence} \
                 error={error}",
                session_id.as_str()
            ));
            false
        }
    }
}

/// 读取水位；没有这一行（从未投影过）时返回 `None`。
///
/// 读失败按「未投影」返回 `None`：宁可多算一次，也不能因为一次读失败就静默丢掉一整批
/// 投影。失败事实同时进日志与 stderr，与事件面写失败同款。
pub(super) fn watermark(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> Option<u64> {
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
