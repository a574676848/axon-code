//! 自我迭代 subagent 回执本机持久化。

use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult};
use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

use crate::{
    PROMPT_CACHE_ANCHOR_HASH_LEN, SELF_ITERATION_MEMORY_PROMPT_MODE,
    SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE, SelfIterationHookKind,
    SelfIterationSubagentStatus,
    runtime_store_path::{append_retained_jsonl, runtime_store_file_path},
};

/// 自我迭代回执文件名。
pub const SELF_ITERATION_RECEIPT_STORE_FILE: &str = "self_iteration_receipts.jsonl";
/// 自我迭代回执 schema 版本。
pub const SELF_ITERATION_RECEIPT_STORE_SCHEMA_VERSION: u32 = 1;
/// 自我迭代事实回执保留时长。
pub const SELF_ITERATION_RECEIPT_RETENTION_MS: u64 = 180 * 24 * 60 * 60 * 1_000;
/// 自我迭代回执使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// 自我迭代写入决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelfIterationWriteDecision {
    /// 未尝试写入。
    NotAttempted,
    /// 已写入持久化内容。
    Persisted,
    /// subagent 判断无需写入。
    SkippedNoChange,
    /// 写入失败但不遮蔽父任务。
    FailedNonFatal,
}

/// 自我迭代回执输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationReceiptInput {
    /// 父会话 ID。
    pub parent_session_id: String,
    /// 父 RuntimeCell ID。
    pub parent_cell_id: Option<u64>,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// hook 类型。
    pub hook: SelfIterationHookKind,
    /// source trace。
    pub source_trace: String,
    /// subagent 运行状态。
    pub status: SelfIterationSubagentStatus,
    /// subagent 成本，单位微美元。
    pub cost_micros: u64,
    /// 写入决策。
    pub write_decision: SelfIterationWriteDecision,
    /// 写入或跳过原因。
    pub reason: String,
    /// 写入摘要 hash。
    pub summary_hash: Option<String>,
    /// skill 名称。
    pub skill_name: Option<String>,
    /// hook 内迭代数。
    pub hook_iterations: u64,
}

/// 可持久化自我迭代回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfIterationReceiptRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// 父会话 ID。
    pub parent_session_id: String,
    /// 父 RuntimeCell ID。
    pub parent_cell_id: Option<u64>,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// hook 类型。
    pub hook: SelfIterationHookKind,
    /// prompt mode。
    pub prompt_mode: String,
    /// source trace。
    pub source_trace: String,
    /// subagent 运行状态。
    pub status: SelfIterationSubagentStatus,
    /// subagent 成本，单位微美元。
    pub cost_micros: u64,
    /// 写入决策。
    pub write_decision: SelfIterationWriteDecision,
    /// 写入或跳过原因。
    pub reason: String,
    /// 写入摘要 hash。
    pub summary_hash: Option<String>,
    /// skill 名称。
    pub skill_name: Option<String>,
    /// hook 内迭代数。
    pub hook_iterations: u64,
    /// 是否持久化 prompt 原文。
    pub prompt_text_persisted: bool,
    /// 是否持久化 subagent 原始 transcript。
    pub raw_transcript_persisted: bool,
    /// 是否允许远控触发或读取。
    pub remote_trigger_allowed: bool,
}

/// 父 session 维度的自我迭代回执汇总。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIterationReceiptSessionSummary {
    /// 父会话 ID。
    pub parent_session_id: String,
    /// 回执数量。
    pub total_receipts: usize,
    /// memory update 回执数量。
    pub memory_update_receipts: usize,
    /// skill reflection 回执数量。
    pub skill_reflection_receipts: usize,
    /// 产生持久写入的数量。
    pub persisted_write_receipts: usize,
    /// subagent 成本合计。
    pub total_cost_micros: u64,
    /// 最新状态。
    pub latest_status: SelfIterationSubagentStatus,
    /// 最新 source trace。
    pub latest_source_trace: String,
    /// 是否持久化 prompt 原文。
    pub prompt_text_persisted: bool,
    /// 是否持久化 subagent 原始 transcript。
    pub raw_transcript_persisted: bool,
    /// 是否允许远控触发或读取。
    pub remote_trigger_allowed: bool,
}

/// 自我迭代回执 store runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SelfIterationReceiptStoreRuntime;

impl SelfIterationReceiptRecord {
    /// 从输入生成回执记录。
    pub fn from_input(input: SelfIterationReceiptInput) -> AxonResult<Self> {
        validate_input(&input)?;
        Ok(Self {
            schema_version: SELF_ITERATION_RECEIPT_STORE_SCHEMA_VERSION,
            parent_session_id: input.parent_session_id,
            parent_cell_id: input.parent_cell_id,
            recorded_at_ms: input.recorded_at_ms,
            hook: input.hook,
            prompt_mode: prompt_mode_for_hook(input.hook).to_owned(),
            source_trace: input.source_trace,
            status: input.status,
            cost_micros: input.cost_micros,
            write_decision: input.write_decision,
            reason: input.reason,
            summary_hash: input.summary_hash,
            skill_name: input.skill_name,
            hook_iterations: input.hook_iterations,
            prompt_text_persisted: false,
            raw_transcript_persisted: false,
            remote_trigger_allowed: false,
        })
    }
}

impl SelfIterationReceiptStoreRuntime {
    /// 解析默认回执 store 路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        runtime_store_file_path(root, SELF_ITERATION_RECEIPT_STORE_FILE)
    }

    /// 追加写入回执记录。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &SelfIterationReceiptRecord,
    ) -> AxonResult<()> {
        validate_record(record)?;
        let path = path.as_ref();
        let line = serde_json::to_string(record)
            .map_err(|_| AxonError::runtime_failure("自我迭代回执序列化失败"))?;
        append_retained_jsonl(
            path,
            &line,
            record.recorded_at_ms,
            SELF_ITERATION_RECEIPT_RETENTION_MS,
        )
        .map_err(|_| AxonError::runtime_failure("无法写入自我迭代回执"))
    }

    /// 读取全部回执记录。
    pub fn read_records(path: impl AsRef<Path>) -> AxonResult<Vec<SelfIterationReceiptRecord>> {
        let path = path.as_ref();
        SHARD_STORE
            .read_lines(path)
            .map_err(|_| AxonError::runtime_failure("无法读取自我迭代回执"))?
            .iter()
            .map(|line| {
                let record = serde_json::from_slice::<SelfIterationReceiptRecord>(line)
                    .map_err(|_| AxonError::runtime_failure("自我迭代回执 JSONL 无效"))?;
                validate_record(&record)?;
                Ok(record)
            })
            .collect()
    }

    /// 读取父 session 的全部回执，按记录时间稳定排序。
    pub fn receipts_for_parent_session(
        path: impl AsRef<Path>,
        parent_session_id: &str,
    ) -> AxonResult<Vec<SelfIterationReceiptRecord>> {
        let mut records = Self::read_records(path)?
            .into_iter()
            .filter(|record| record.parent_session_id == parent_session_id)
            .collect::<Vec<_>>();
        records.sort_by_key(|record| (record.recorded_at_ms, hook_rank(record.hook)));
        Ok(records)
    }

    /// 读取父 session 最新回执。
    pub fn latest_for_parent_session(
        path: impl AsRef<Path>,
        parent_session_id: &str,
    ) -> AxonResult<Option<SelfIterationReceiptRecord>> {
        Ok(Self::receipts_for_parent_session(path, parent_session_id)?
            .into_iter()
            .max_by_key(|record| (record.recorded_at_ms, hook_rank(record.hook))))
    }

    /// 汇总父 session 回执。
    pub fn summary_for_parent_session(
        path: impl AsRef<Path>,
        parent_session_id: &str,
    ) -> AxonResult<Option<SelfIterationReceiptSessionSummary>> {
        let records = Self::receipts_for_parent_session(path, parent_session_id)?;
        Ok(SelfIterationReceiptSessionSummary::from_records(
            parent_session_id,
            &records,
        ))
    }
}

impl SelfIterationReceiptSessionSummary {
    fn from_records(
        parent_session_id: &str,
        records: &[SelfIterationReceiptRecord],
    ) -> Option<Self> {
        let latest = records.last()?;
        Some(Self {
            parent_session_id: parent_session_id.to_owned(),
            total_receipts: records.len(),
            memory_update_receipts: records
                .iter()
                .filter(|record| record.hook == SelfIterationHookKind::MemoryUpdate)
                .count(),
            skill_reflection_receipts: records
                .iter()
                .filter(|record| record.hook == SelfIterationHookKind::SkillReflection)
                .count(),
            persisted_write_receipts: records
                .iter()
                .filter(|record| record.write_decision == SelfIterationWriteDecision::Persisted)
                .count(),
            total_cost_micros: records.iter().map(|record| record.cost_micros).sum(),
            latest_status: latest.status,
            latest_source_trace: latest.source_trace.clone(),
            prompt_text_persisted: records.iter().any(|record| record.prompt_text_persisted),
            raw_transcript_persisted: records.iter().any(|record| record.raw_transcript_persisted),
            remote_trigger_allowed: records.iter().any(|record| record.remote_trigger_allowed),
        })
    }
}

fn validate_input(input: &SelfIterationReceiptInput) -> AxonResult<()> {
    validate_common(
        &input.parent_session_id,
        input.hook,
        prompt_mode_for_hook(input.hook),
        &input.source_trace,
        &input.reason,
        input.summary_hash.as_deref(),
        input.skill_name.as_deref(),
    )
}

fn validate_record(record: &SelfIterationReceiptRecord) -> AxonResult<()> {
    if record.schema_version != SELF_ITERATION_RECEIPT_STORE_SCHEMA_VERSION {
        return Err(AxonError::runtime_failure("自我迭代回执 schema 版本无效"));
    }
    if record.prompt_text_persisted
        || record.raw_transcript_persisted
        || record.remote_trigger_allowed
    {
        return Err(AxonError::runtime_failure(
            "自我迭代回执不能暴露 prompt 原文、transcript 或远控触发",
        ));
    }
    validate_common(
        &record.parent_session_id,
        record.hook,
        &record.prompt_mode,
        &record.source_trace,
        &record.reason,
        record.summary_hash.as_deref(),
        record.skill_name.as_deref(),
    )
}

fn validate_common(
    parent_session_id: &str,
    hook: SelfIterationHookKind,
    prompt_mode: &str,
    source_trace: &str,
    reason: &str,
    summary_hash: Option<&str>,
    skill_name: Option<&str>,
) -> AxonResult<()> {
    validate_non_empty(parent_session_id, "自我迭代父 session 不能为空")?;
    if prompt_mode != prompt_mode_for_hook(hook) {
        return Err(AxonError::runtime_failure(
            "自我迭代回执 prompt mode 与 hook 不匹配",
        ));
    }
    validate_non_empty(source_trace, "自我迭代 source trace 无效")?;
    validate_non_empty(reason, "自我迭代回执原因无效")?;
    if let Some(hash) = summary_hash {
        validate_bounded(hash, PROMPT_CACHE_ANCHOR_HASH_LEN, "自我迭代摘要 hash 无效")?;
    }
    if let Some(name) = skill_name {
        validate_non_empty(name, "自我迭代 skill 名称无效")?;
    }
    Ok(())
}

fn validate_non_empty(value: &str, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

fn validate_bounded(value: &str, max_chars: usize, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

const fn prompt_mode_for_hook(hook: SelfIterationHookKind) -> &'static str {
    match hook {
        SelfIterationHookKind::MemoryUpdate => SELF_ITERATION_MEMORY_PROMPT_MODE,
        SelfIterationHookKind::SkillReflection => SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE,
    }
}

const fn hook_rank(hook: SelfIterationHookKind) -> u8 {
    match hook {
        SelfIterationHookKind::MemoryUpdate => 0,
        SelfIterationHookKind::SkillReflection => 1,
    }
}
