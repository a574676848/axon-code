//! 单前台线索（M3-2）。
//!
//! 设计定稿：只维护**一条**命名空间内的前台线索，不做多线索列表、不做温度三态、
//! 不做 mergeThreads、不做结论压缩。`signature` 字段预留给后续语义去重，本期只写入。

mod commitment;
mod p2_render;
mod reference;

pub use commitment::{close_commitments, detects_closure, detects_commitment, record_commitment};
pub use p2_render::{
    P2_COMMITMENT_MAX_BYTES, P2_MAX_COMMITMENTS, P2_THREAD_MAX_BYTES, format_local_minute,
    render_p2_context,
};
pub use reference::{Reference, classify_reference, is_one_off_leaf};

use crate::store_thread::{MemoryThread, THREAD_STATUS_OPEN};
use crate::{MemorySqliteStore, store_thread::MemoryCommitment};
use axon_domain::AxonResult;

/// 线索与承诺的归属范围。
///
/// 会话是必须的归属维度：同一条对话只维护自己会话内的前台线索，
/// 定时任务、子代理与其它客户端不会复用或覆盖彼此的线索。
#[derive(Debug, Clone, Copy)]
pub struct ThreadScope<'a> {
    /// 命名空间引用。
    pub namespace_ref: &'a str,
    /// owner principal。
    pub user_ref: &'a str,
    /// 归属会话引用。
    pub session_ref: &'a str,
}

/// 线索与承诺的默认状态：默认开启。
///
/// 需要停用时把该常量改为 `false` 并重编，或在调用点显式传入 `false`。
pub const THREADS_ENABLED_DEFAULT: bool = true;

/// 线索话题的最大字符数。
const THREAD_TOPIC_MAX_CHARS: usize = 40;

/// 取或建「前台线索」，返回线索 ID。
///
/// * `enabled = false` 时零读写，返回 `None`，行为与基线一致；
/// * 一次性输入（天气/汇率/几点/寒暄）不建线索，前台线索保持不变；
/// * 指代（这个/它/继续）沿用现有前台线索并累加 `hit_count`，**不新建**；
/// * 其余情况同样沿用；仅当不存在前台线索（或已归档）时才新建一条。
pub fn ensure_foreground(
    store: &MemorySqliteStore,
    scope: &ThreadScope<'_>,
    text: &str,
    now_ms: i64,
    enabled: bool,
) -> AxonResult<Option<String>> {
    if !enabled || is_one_off_leaf(text) {
        return Ok(None);
    }
    if let Some(current) =
        store.foreground_thread(scope.namespace_ref, scope.user_ref, scope.session_ref)?
    {
        store.touch_thread(&current.thread_id, now_ms)?;
        return Ok(Some(current.thread_id));
    }
    // 没有前台线索时，纯指代没有可指代对象，不凭空新建。
    if matches!(classify_reference(text), Reference::AnaphoraRecent) {
        return Ok(None);
    }
    let signature = signature_of(scope, text);
    let thread_id = thread_id_for(&signature);
    let thread = MemoryThread {
        thread_id: thread_id.clone(),
        namespace_ref: scope.namespace_ref.to_owned(),
        user_ref: scope.user_ref.to_owned(),
        session_ref: scope.session_ref.to_owned(),
        topic: topic_of(text),
        signature,
        label: String::new(),
        summary: String::new(),
        status: THREAD_STATUS_OPEN.to_owned(),
        hit_count: 1,
        last_event_at_ms: now_ms,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    store.upsert_thread(&thread)?;
    Ok(Some(thread_id))
}

/// 生成承诺 ID：`commitment-<sha256 前 16 位>`。
#[must_use]
pub fn commitment_id_for(scope: &ThreadScope<'_>, text: &str, now_ms: i64) -> String {
    let digest = axon_crypto::sha256_hex(format!(
        "{}|{}|{}|{}",
        scope.namespace_ref, scope.user_ref, now_ms, text
    ));
    format!("commitment-{}", &digest[..16])
}

/// 构造一条待写入的承诺；`thread_id` 为当前前台线索。
#[must_use]
pub fn open_commitment(
    scope: &ThreadScope<'_>,
    thread_id: Option<&str>,
    text: &str,
    channel: &str,
    now_ms: i64,
) -> MemoryCommitment {
    MemoryCommitment {
        commitment_id: commitment_id_for(scope, text, now_ms),
        thread_id: thread_id.map(str::to_owned),
        namespace_ref: scope.namespace_ref.to_owned(),
        user_ref: scope.user_ref.to_owned(),
        text: text.trim().to_owned(),
        status: crate::store_thread::COMMITMENT_STATUS_OPEN.to_owned(),
        channel: channel.to_owned(),
        created_at_ms: now_ms,
        closed_at_ms: None,
    }
}

fn signature_of(scope: &ThreadScope<'_>, text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{}|{}|{}|{}",
        scope.namespace_ref, scope.user_ref, scope.session_ref, normalized
    )
}

fn thread_id_for(signature: &str) -> String {
    let digest = axon_crypto::sha256_hex(signature);
    format!("thread-{}", &digest[..16])
}

fn topic_of(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(THREAD_TOPIC_MAX_CHARS)
        .collect()
}

#[cfg(test)]
#[path = "thread_tests.rs"]
mod tests;
