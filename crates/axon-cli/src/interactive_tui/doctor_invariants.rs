//! doctor 的三条存储不变式检查：INV-2、INV-7a、INV-7b。
//!
//! 三条都是「运行态收敛过程」的不变式，因此有一个共同前提：**Daemon 正在接管运行态**。
//! 没有 Daemon 时 `recovering` 本来就没人收敛、在途工作项也没人结算，读出来的零行只代表
//! 「没人干活」，不能当作通过。这种情况一律如实标注「无法检查」，不给绿灯。
//!
//! 判定 SQL 各只有一处来源：
//!
//! - INV-2 直接调用 `axon_session::conclusive_work_items_without_terminal_kind_sql()`，
//!   状态词表由领域枚举派生，本模块不复制任何状态字面量；
//! - INV-7a / INV-7b 的谓词在下方各定义一次，计数与点名查询共用同一份谓词，不会分叉。
//!
//! 计数是权威结论，点名查询只为给出可执行的定位信息（最多 [`INVARIANT_SAMPLE_LIMIT`] 条）。

use axon_domain::ExecutionStatus;
use axon_session::SqliteRuntimeStore;

use super::doctor_commands::DoctorCheck;
use crate::constants::{DOCTOR_CHECK_INV2, DOCTOR_CHECK_INV7A, DOCTOR_CHECK_INV7B};

/// 一条不变式最多点名多少行；超出只报总数，避免报告体量随库变大。
const INVARIANT_SAMPLE_LIMIT: usize = 3;
/// `recovering` 的过期缓冲：300s 明显大于 120s 的恢复 TTL，留出收敛与租约续期的余量。
const RECOVERING_TTL_BUFFER_MS: i64 = 300_000;

/// INV-7a 谓词：会话停在 `recovering`，名下却没有任何在途工作项。
///
/// 这种会话已经无事可做，收敛路径只需把它送回 `idle`；停在原地就是「回不来的态」。
const INV7A_PREDICATE: &str = "s.status = 'recovering' AND NOT EXISTS (\
     SELECT 1 FROM work_items AS w \
     WHERE w.session_id = s.session_id AND w.status IN ('queued','running'))";

/// INV-7b 谓词：`recovering` 停留超过 300s 缓冲（`?1` = 缓冲毫秒，`?2` = now_ms）。
///
/// 缓冲必须大于恢复 TTL（120s），否则正常的恢复窗口也会被判成过期。
const INV7B_PREDICATE: &str = "s.status = 'recovering' AND s.last_activity_at_ms + ?1 < ?2";

/// 一次检查的可用性。
pub(super) enum InvariantProbe {
    /// 运行态库存在且 Daemon 正在接管：可以查。
    Ready {
        /// 运行态存储句柄。
        store: SqliteRuntimeStore,
        /// 判定 INV-7b 用的「现在」。
        now_ms: i64,
    },
    /// 前提缺失：三条都标「无法检查」，并带上具体原因。
    Unavailable(String),
}

/// 默认 doctor 运行里的三条不变式检查。
pub(super) fn doctor_invariant_checks() -> Vec<DoctorCheck> {
    invariant_checks_for(invariant_probe())
}

/// 只跑其中一条（`--check inv-2` 这类定向诊断）。
pub(super) fn doctor_invariant_check(name: &str) -> DoctorCheck {
    let checks = invariant_checks_for(invariant_probe());
    match checks.into_iter().find(|check| check.name == name) {
        Some(check) => check,
        None => DoctorCheck::unverifiable(name.to_owned(), format!("未知存储不变式检查项：{name}")),
    }
}

/// 按可用性给出三条检查结论。
pub(super) fn invariant_checks_for(probe: InvariantProbe) -> Vec<DoctorCheck> {
    match probe {
        InvariantProbe::Ready { store, now_ms } => vec![
            inv2_check(&store),
            inv7a_check(&store),
            inv7b_check(&store, now_ms),
        ],
        InvariantProbe::Unavailable(reason) => {
            let message = format!(
                "无法检查：{reason}。三条不变式都要求 Daemon 正在接管运行态，没有 Daemon 时读到的零行只代表没有人在收敛"
            );
            vec![
                unverifiable(DOCTOR_CHECK_INV2, &message),
                unverifiable(DOCTOR_CHECK_INV7A, &message),
                unverifiable(DOCTOR_CHECK_INV7B, &message),
            ]
        }
    }
}

/// 组装检查前提：运行态库必须已经建出来，Daemon 必须能应答健康查询。
fn invariant_probe() -> InvariantProbe {
    let Ok(config_path) = crate::setup_commands::resolved_setup_config_path(None) else {
        return InvariantProbe::Unavailable("无法解析用户配置路径".to_owned());
    };
    let Ok(data_root) = crate::runtime_paths::runtime_axon_data_path_static(&config_path) else {
        return InvariantProbe::Unavailable("无法解析 Axon 运行目录".to_owned());
    };
    let store = SqliteRuntimeStore::from_root(&data_root);
    if !store.path().is_file() {
        return InvariantProbe::Unavailable(format!(
            "运行态库尚未创建：{}",
            store.path().display()
        ));
    }
    if let Err(reason) =
        crate::daemon_commands::probe_health(crate::daemon_commands::DEFAULT_ENDPOINT)
    {
        return InvariantProbe::Unavailable(format!("Daemon 未运行或 IPC 不可达（{reason}）"));
    }
    InvariantProbe::Ready {
        store,
        now_ms: now_ms(),
    }
}

/// 当前时刻（毫秒），按 SQLite 的整数口径给出。
fn now_ms() -> i64 {
    i64::try_from(axon_time::unix_now_ms()).unwrap_or(i64::MAX)
}

/// INV-2：结论状态的工作项必须同时写 `terminal_kind`，违规恒为零行。
fn inv2_check(store: &SqliteRuntimeStore) -> DoctorCheck {
    let count = match scalar_count(
        store,
        &axon_session::conclusive_work_items_without_terminal_kind_sql(),
        [],
    ) {
        Ok(count) => count,
        Err(message) => return DoctorCheck::failed(DOCTOR_CHECK_INV2, message),
    };
    if count <= 0 {
        return DoctorCheck::ok(
            DOCTOR_CHECK_INV2,
            "结论状态的工作项都带 terminal_kind（违规 0 行）",
        );
    }
    let samples = inv2_offenders(store);
    DoctorCheck::failed(
        DOCTOR_CHECK_INV2,
        format!(
            "违规 {count} 行：结论状态的工作项缺 terminal_kind{}{}。执行 `axon daemon restart` 让收口路径重跑；仍不消失说明有绕过唯一结算出口（settle_work_item）的写入方，按上面的标识定位",
            sample_suffix(&samples),
            overflow_suffix(count, samples.len())
        ),
    )
}

/// INV-2 的违规定位：谓词与权威计数 SQL 同源（结论状态词表由领域枚举派生）。
fn inv2_offenders(store: &SqliteRuntimeStore) -> Vec<String> {
    let sql = format!(
        "SELECT execution_id, session_id FROM work_items_all \
         WHERE status IN ({}) AND terminal_kind IS NULL \
         ORDER BY updated_at_ms DESC LIMIT {INVARIANT_SAMPLE_LIMIT}",
        conclusive_status_list()
    );
    store
        .with_read_connection(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map([], |row| {
                let execution_id: String = row.get(0)?;
                let session_id: Option<String> = row.get(1)?;
                Ok(format!(
                    "execution_id={execution_id} session_id={}",
                    session_id.unwrap_or_else(|| "-".to_owned())
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// 结论状态的 SQL 词表：由领域枚举派生，不复制状态字面量。
fn conclusive_status_list() -> String {
    axon_session::domain::interaction_status::CONCLUSIVE_EXECUTION_STATUSES
        .iter()
        .map(|status| format!("'{}'", execution_status_token(*status)))
        .collect::<Vec<_>>()
        .join(",")
}

/// 落库取值与 serde 的 snake_case 契约同源（`sessions.status` 的 CHECK 词表也由它生成）。
fn execution_status_token(status: ExecutionStatus) -> String {
    let value = serde_json::to_value(status).expect("ExecutionStatus 必须可序列化");
    value
        .as_str()
        .map(str::to_owned)
        .expect("ExecutionStatus 的序列化取值必须是字符串")
}

/// INV-7a：`recovering` 必须有归宿，没有在途工作项就该回 `idle`。
fn inv7a_check(store: &SqliteRuntimeStore) -> DoctorCheck {
    let sql = format!("SELECT COUNT(*) FROM sessions AS s WHERE {INV7A_PREDICATE}");
    let count = match scalar_count(store, &sql, []) {
        Ok(count) => count,
        Err(message) => return DoctorCheck::failed(DOCTOR_CHECK_INV7A, message),
    };
    if count <= 0 {
        return DoctorCheck::ok(
            DOCTOR_CHECK_INV7A,
            "没有停在 recovering 却无在途工作项的会话",
        );
    }
    let samples = inv7_offenders(store, INV7A_PREDICATE);
    DoctorCheck::failed(
        DOCTOR_CHECK_INV7A,
        format!(
            "违规 {count} 个会话：停在 recovering 但名下没有在途工作项{}{}。执行 `axon daemon restart` 让收敛路径把它们送回 idle",
            sample_suffix(&samples),
            overflow_suffix(count, samples.len())
        ),
    )
}

/// INV-7b：`recovering` 超过 300s 缓冲仍未派发，说明收敛路径没有跑到。
fn inv7b_check(store: &SqliteRuntimeStore, now_ms: i64) -> DoctorCheck {
    let sql = format!("SELECT COUNT(*) FROM sessions AS s WHERE {INV7B_PREDICATE}");
    let count = match scalar_count(
        store,
        &sql,
        rusqlite::params![RECOVERING_TTL_BUFFER_MS, now_ms],
    ) {
        Ok(count) => count,
        Err(message) => return DoctorCheck::failed(DOCTOR_CHECK_INV7B, message),
    };
    if count <= 0 {
        return DoctorCheck::ok(
            DOCTOR_CHECK_INV7B,
            format!(
                "没有停在 recovering 超过 {RECOVERING_TTL_BUFFER_MS}ms 缓冲的会话（缓冲大于 120s 恢复 TTL）"
            ),
        );
    }
    let samples = inv7b_offenders(store, now_ms);
    DoctorCheck::failed(
        DOCTOR_CHECK_INV7B,
        format!(
            "违规 {count} 个会话：停在 recovering 超过 {RECOVERING_TTL_BUFFER_MS}ms 仍未收口{}{}。执行 `axon daemon restart`；复现说明恢复路径没有跑到，按上面的会话逐条复核",
            sample_suffix(&samples),
            overflow_suffix(count, samples.len())
        ),
    )
}

/// INV-7a 的点名查询：与计数共用同一份谓词。
fn inv7_offenders(store: &SqliteRuntimeStore, predicate: &str) -> Vec<String> {
    let sql = offender_sql(predicate);
    store
        .with_read_connection(move |connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map([], |row| {
                let session_id: String = row.get(0)?;
                let last_activity_at_ms: i64 = row.get(1)?;
                Ok(format!(
                    "session_id={session_id} last_activity_at_ms={last_activity_at_ms}"
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// INV-7b 的点名查询：与计数共用同一份谓词，两个占位符与计数查询同序。
fn inv7b_offenders(store: &SqliteRuntimeStore, now_ms: i64) -> Vec<String> {
    let sql = offender_sql(INV7B_PREDICATE);
    store
        .with_read_connection(move |connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(
                rusqlite::params![RECOVERING_TTL_BUFFER_MS, now_ms],
                |row| {
                    let session_id: String = row.get(0)?;
                    let last_activity_at_ms: i64 = row.get(1)?;
                    Ok(format!(
                        "session_id={session_id} last_activity_at_ms={last_activity_at_ms}"
                    ))
                },
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
}

/// 点名查询的公共形状：谓词来自调用方，列与排序固定。
fn offender_sql(predicate: &str) -> String {
    format!(
        "SELECT s.session_id, s.last_activity_at_ms FROM sessions AS s \
         WHERE {predicate} ORDER BY s.last_activity_at_ms ASC LIMIT {INVARIANT_SAMPLE_LIMIT}"
    )
}

/// 执行一次标量计数查询。
fn scalar_count<P>(store: &SqliteRuntimeStore, sql: &str, params: P) -> Result<i64, String>
where
    P: rusqlite::Params,
{
    store
        .with_read_connection(move |connection| {
            connection.query_row(sql, params, |row| row.get::<_, i64>(0))
        })
        .map_err(|error| format!("查询不变式失败：{error}"))
}

/// 点名后缀：空清单不追加任何内容。
fn sample_suffix(samples: &[String]) -> String {
    if samples.is_empty() {
        String::new()
    } else {
        format!("（{}）", samples.join("；"))
    }
}

/// 超出点名上限时的补充说明。
fn overflow_suffix(count: i64, sampled: usize) -> String {
    let sampled = i64::try_from(sampled).unwrap_or(i64::MAX);
    if count > sampled {
        format!("，另有 {} 行未在此列出", count.saturating_sub(sampled))
    } else {
        String::new()
    }
}

/// 前提缺失时的检查结论。
fn unverifiable(name: impl Into<String>, message: &str) -> DoctorCheck {
    DoctorCheck::unverifiable(name, message)
}

#[cfg(test)]
#[path = "doctor_invariants_tests.rs"]
mod tests;
