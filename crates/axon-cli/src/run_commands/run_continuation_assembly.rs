//! 子代理终态的摘要级恢复装配（合同 §5.3.3）。
//!
//! 「恢复子代理上下文」不是把子代理历史复制进父上下文，而是把**终态事实**接回来：
//! 摘要 + artifact 引用 + 真实 token 用量。为此本模块在三执行面共用的注入通道上
//! 落一段有界 runtime note，落在下一个 provider 请求里。
//!
//! 结算顺序刻意是**先注入、后 CAS**：CAS 之前崩溃只会让同一段终态被重复注入一次，
//! 而反过来（先 CAS 后注入）会在注入前崩溃时永久丢掉这段事实。两害相权取其轻：
//! 重复注入没有副作用，丢失不可恢复。

use std::path::Path;

use axon_agent_runtime::{
    AgentRuntimeNoteKind, CONTINUATION_ASSEMBLY_BATCH_LIMIT, CONTINUATION_ASSEMBLY_MAX_CHARS,
};
use axon_session::{AgentContinuationRecord, AgentContinuationStore};

use super::run_steer::try_send_deferred_runtime_note;

/// 装配注入正文的截断标记；截断只丢摘要正文，不丢终态与引用。
const ASSEMBLY_TRUNCATION_MARKER: &str = "\n…（装配正文已截断，完整结果见上方引用）";

/// 一次恢复装配的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ContinuationAssemblyReport {
    /// 本次取出的待装配条数。
    pub candidate_count: usize,
    /// 本次真正完成 `pending -> consumed` 迁移的条数。
    pub assembled_count: usize,
}

impl ContinuationAssemblyReport {
    /// 是否发生了实际装配。
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub(crate) const fn assembled_any(&self) -> bool {
        self.assembled_count > 0
    }
}

/// 把当前会话待装配的子代理终态注入主代理的下一轮请求。
///
/// `session_key` 同时是 continuation 的 owner 作用域、父会话 ID 与注入键：
/// 三者同源，因此「谁产生的终态」与「注入给谁」天然一致。
pub(crate) fn assemble_pending_continuations(
    config_path: Option<&Path>,
    session_key: &str,
    now_ms: u64,
) -> Result<ContinuationAssemblyReport, String> {
    let Some(config_path) = config_path else {
        return Ok(ContinuationAssemblyReport::default());
    };
    let store = super::run_agent_store::shared_runtime_store(config_path)?;
    let pending = store
        .pending_continuations(
            session_key,
            session_key,
            now_ms,
            CONTINUATION_ASSEMBLY_BATCH_LIMIT,
        )
        .map_err(|error| format!("读取待装配 continuation 失败：{error}"))?;
    if pending.is_empty() {
        return Ok(ContinuationAssemblyReport::default());
    }
    let content = render_assembly(&pending);
    if let Err(error) = try_send_deferred_runtime_note(
        session_key,
        AgentRuntimeNoteKind::ContinuationRestore,
        content,
    ) {
        // 注入通道不可用：不消费任何记录，留待下一次装配重试：终态事实不能丢。
        return Err(format!("continuation 装配注入失败：{error:?}"));
    }
    let consumed_by = format!("{ASSEMBLY_CONSUMER_PREFIX}{session_key}");
    let mut assembled_count = 0_usize;
    for record in &pending {
        match store.consume_continuation(&record.continuation_id, &consumed_by, now_ms) {
            Ok(true) => assembled_count = assembled_count.saturating_add(1),
            // CAS 未命中说明另一执行面已经完成了这次有效执行；放弃即可。
            Ok(false) => {}
            Err(error) => return Err(format!("结算 continuation 失败：{error}")),
        }
    }
    Ok(ContinuationAssemblyReport {
        candidate_count: pending.len(),
        assembled_count,
    })
}

/// 装配消费方标识前缀；后缀是执行面与会话，便于审计。
const ASSEMBLY_CONSUMER_PREFIX: &str = "run:";

/// 渲染一批待装配终态为有界注入正文。
///
/// 引用与终态写在摘要之前：正文超限时先丢摘要尾部，
/// **宁可丢摘要正文，不可丢 artifact 引用与终态**。
fn render_assembly(records: &[AgentContinuationRecord]) -> String {
    let mut text = String::from("子代理终态（来自本会话此前的执行，已不再运行）：\n");
    for record in records {
        text.push_str(&format!(
            "\n- 路径：{}\n- 终态：{}\n- 退出原因：{}\n",
            record.agent_path, record.terminal_kind, record.exit_reason
        ));
        if let Some(artifact) = record.artifact_path.as_deref() {
            text.push_str(&format!("- 完整结果：{artifact}\n"));
        }
        if let Some(tokens) = token_summary(record) {
            text.push_str(&format!("- Token 用量：{tokens}\n"));
        }
        text.push_str(&format!("- 摘要：{}", record.summary));
    }
    bounded_assembly_text(&text)
}

/// 真实 token 用量的一行摘要；三项全无时返回 `None`。
fn token_summary(record: &AgentContinuationRecord) -> Option<String> {
    let input = record.token_input?;
    let output = record.token_output?;
    let reasoning = record.token_reasoning.unwrap_or(0);
    Some(format!("输入 {input}，输出 {output}，推理 {reasoning}"))
}

fn bounded_assembly_text(text: &str) -> String {
    if text.chars().count() <= CONTINUATION_ASSEMBLY_MAX_CHARS {
        return text.to_owned();
    }
    let head_chars =
        CONTINUATION_ASSEMBLY_MAX_CHARS.saturating_sub(ASSEMBLY_TRUNCATION_MARKER.chars().count());
    let head = text.chars().take(head_chars).collect::<String>();
    format!("{head}{ASSEMBLY_TRUNCATION_MARKER}")
}

#[cfg(test)]
#[path = "run_continuation_assembly_tests.rs"]
mod tests;
