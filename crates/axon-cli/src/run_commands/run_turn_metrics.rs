//! 轮次指标的进程内写入通道。
//!
//! 指标必须落库，但通道本身不能成为写放大来源：
//! - 共享 runtime 库按 data 根缓存，进程内只建一份连接池；
//! - 守护进程启动时把自己的 host 记录注册进来，写指标直接复用代际，不再抢租约；
//! - 独立进程按需申请一次长租约并缓存，只在租约将尽或围栏拒绝时刷新一次。
//!
//! 收口约束：正常路径每轮只产生一次 `append_events`，不新增连接建立、不新增租约写入。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use axon_domain::{DaemonSessionId, RuntimeHostId};
use axon_session::{DaemonHostRecord, EventLogStore as _, RuntimeStore as _, SqliteRuntimeStore};

use super::run_session_store::RunSessionRecord;

/// 独立进程申请的 host 租约时长：覆盖多轮，避免每轮续租写一次。
const HOST_LEASE_MS: u64 = 300_000;
/// 提前续租余量：剩余租约不足该值就重新申请，避免写事件时刚好过期。
const HOST_LEASE_MARGIN_MS: u64 = 60_000;
/// 缓存库句柄上限：进程内实际只会用到少数几个 runtime 根，超出即整体丢弃。
const STORE_CACHE_LIMIT: usize = 4;
/// 围栏失败后的刷新次数上限（含首次尝试）。
const APPEND_ATTEMPTS: u32 = 2;

/// 进程内缓存的共享库句柄，键为 runtime data 根。
fn store_cache() -> &'static Mutex<HashMap<PathBuf, SqliteRuntimeStore>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, SqliteRuntimeStore>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 进程级 host 记录。
struct ProcessHost {
    record: DaemonHostRecord,
    /// 由宿主注册（守护进程）时为真：租约由宿主心跳续期，本地不判过期。
    registered: bool,
}

fn process_host() -> &'static Mutex<Option<ProcessHost>> {
    static HOST: OnceLock<Mutex<Option<ProcessHost>>> = OnceLock::new();
    HOST.get_or_init(|| Mutex::new(None))
}

/// 登记本进程的 host 记录。
///
/// 只允许宿主（守护进程）在启动时调用一次：此后写指标复用该代际，不再申请租约。
pub(crate) fn register_process_host(record: DaemonHostRecord) {
    let mut slot = process_host()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = Some(ProcessHost {
        record,
        registered: true,
    });
}

/// 取按 data 根缓存的共享库句柄。
pub(super) fn shared_store(config_path: &Path) -> Result<SqliteRuntimeStore, String> {
    let data_root = crate::runtime_paths::runtime_axon_data_path_static(config_path)?;
    let mut cache = store_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(store) = cache.get(&data_root) {
        return Ok(store.clone());
    }
    if cache.len() >= STORE_CACHE_LIMIT {
        cache.clear();
    }
    let store = SqliteRuntimeStore::from_root(data_root.clone());
    cache.insert(data_root, store.clone());
    Ok(store)
}

/// 解析可用 host：注册的记录优先，其次未过期的申请记录，最后按需申请一次。
fn resolve_host(store: &SqliteRuntimeStore, now_ms: u64) -> Option<DaemonHostRecord> {
    {
        let slot = process_host()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = slot.as_ref() {
            let usable = current.registered
                || current.record.lease_expires_at_ms.saturating_sub(now_ms) > HOST_LEASE_MARGIN_MS;
            if usable {
                return Some(current.record.clone());
            }
        }
    }
    let host_id = RuntimeHostId::new(format!("cli-{}", std::process::id())).ok()?;
    let claimed = store
        .claim_host(&host_id, std::process::id(), None, now_ms, HOST_LEASE_MS)
        .ok()?;
    let mut slot = process_host()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = Some(ProcessHost {
        record: claimed.clone(),
        registered: false,
    });
    Some(claimed)
}

/// 丢弃缓存的 host 记录：围栏拒绝通常意味着代际已变或租约失效。
fn invalidate_host() {
    let mut slot = process_host()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = None;
}

/// 把本轮指标写入会话事件流。
///
/// 失败不阻断本轮执行：指标是投影事实，缺一条会在界面上表现为该轮缺数，不影响对话本身。
///
/// 会话行的模型事实也在这里落地：这是所有来源唯一的终态记账点，复用同一次 `store`，
/// 因此不必为它多开一次连接，也不需要另一处再补一遍。
pub(crate) fn publish_turn_metrics(config_path: &Path, record: &RunSessionRecord) {
    let Ok(store) = shared_store(config_path) else {
        return;
    };
    let Ok(session_id) = DaemonSessionId::new(record.session_id.clone()) else {
        return;
    };
    record_session_model(&store, &session_id, record);
    // 逐轮读数必须挂在具体某一轮上：没有归属标识就没有可匹配的轮次，宁可不写。
    let Some((interaction_id, payload)) = metrics_payload(record) else {
        return;
    };
    let Ok(interaction_id) = axon_domain::InteractionId::new(interaction_id) else {
        axon_logger::warning(&format!(
            "turn_metrics 归属标识非法，跳过写入: session_id={}",
            record.session_id
        ));
        return;
    };
    let now_ms = axon_time::unix_now_ms();
    for attempt in 0..APPEND_ATTEMPTS {
        let Some(host) = resolve_host(&store, now_ms) else {
            axon_logger::warning(&format!(
                "未能取得 host 代际，跳过 turn_metrics 写入: session_id={}",
                record.session_id
            ));
            return;
        };
        // 收口之后租约已释放：只写归属，不校验租约，读数才不会整轮丢失。
        match store.append_attributed_events(
            &session_id,
            &host,
            &interaction_id,
            &[("turn_metrics".to_owned(), payload.clone())],
            now_ms,
        ) {
            Ok(_events) => return,
            Err(error) => {
                invalidate_host();
                if attempt + 1 >= APPEND_ATTEMPTS {
                    axon_logger::warning(&format!(
                        "turn_metrics 写入失败: session_id={}, error={error:?}",
                        record.session_id
                    ));
                }
            }
        }
    }
}

/// 记录会话最近一轮实际使用的 provider 与模型。
///
/// 会话清单只读 `sessions` 三列，因此这份事实必须每轮落地；写入是条件单行 UPDATE，
/// 连续多轮同一模型时 0 行命中，不产生页写，也不新增查询。
fn record_session_model(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    record: &RunSessionRecord,
) {
    if let Err(error) = store.record_session_model(
        session_id,
        &record.provider,
        &record.model,
        record.model_name.as_deref(),
    ) {
        axon_logger::warning(&format!(
            "会话模型事实写入失败: session_id={}, error={error:?}",
            record.session_id
        ));
    }
}

/// 组装本轮指标载荷；返回它的归属交互标识。
///
/// `TurnMetrics.turn_id` 就是这枚 interaction 标识：客户端按 `turnId === turn.id`
/// 过滤收尾指标帧，`assistant_part_id` 只是消息分段标识，两者不可互换。
fn metrics_payload(record: &RunSessionRecord) -> Option<(String, String)> {
    if record.interaction_id.is_empty() {
        return None;
    }
    let usage = record.usage.unwrap_or_default();
    let active_ms = record
        .turn_events
        .iter()
        .filter_map(|event| event.duration_ms)
        .fold(0_u64, |total, value| total.saturating_add(value));
    let metrics = axon_eventlog::TurnMetrics {
        turn_id: record.interaction_id.clone(),
        ttft_ms: record.ttft_ms,
        active_ms,
        provider_duration_ms: active_ms,
        tool_ms: 0,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_output_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        cache_write_tokens: usage.cache_write_input_tokens,
        generation_speed_tps: record.generation_speed_tps,
        // 作业计数来自真实生产者：本模块只读订阅侧的结果。
        background_jobs_started: crate::run_commands::run_job_lifecycle_events::take_started_count(
            &record.session_id,
        ),
        background_jobs_running: crate::run_commands::run_job_lifecycle_events::running_count(
            &record.session_id,
        ),
        outcome: axon_eventlog::TurnOutcome::Succeeded,
        tool_calls: u32::try_from(record.tool_calls.len()).unwrap_or(u32::MAX),
    };
    serde_json::to_string(&metrics)
        .ok()
        .map(|payload| (record.interaction_id.clone(), payload))
}
