//! 单轮证据轨迹契约。
//!
//! 轨迹只保存引用与指纹，不保存工具输出原文：正文留在轮次记录里，需要时按引用回取。
//! 这样轨迹本身与任务长度解耦，可以随反思卡片一起流转而不会把上下文吃回去。

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{ExecutionEvidenceStatus, TurnFinalStatus, TurnOutcomeEnvelope};

/// 证据轨迹 schema 版本。
pub const TURN_TRACE_SCHEMA_VERSION: u32 = 1;
/// 指纹尚未采集时的占位值；采集层补齐真实指纹后不再使用。
pub const TURN_TRACE_DIGEST_PLACEHOLDER: &str = "";
/// 指纹保留的十六进制字符数；短指纹足够去重与漂移比对，也不会撑大卡片。
pub const TURN_TRACE_DIGEST_HEX_CHARS: usize = 32;

/// 单条消息的引用与指纹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    /// 运行时消息引用。
    pub message_ref: String,
    /// 正文字符数。
    pub chars: u32,
    /// 正文指纹；不可逆，只用于去重与漂移比对。
    pub digest: String,
}

impl MessageRef {
    /// 用一段有界摘要投影出消息引用；指纹留给采集层补齐。
    #[must_use]
    pub fn from_summary(message_ref: impl Into<String>, summary: &str) -> Self {
        Self {
            message_ref: message_ref.into(),
            chars: u32::try_from(summary.chars().count()).unwrap_or(u32::MAX),
            digest: TURN_TRACE_DIGEST_PLACEHOLDER.to_owned(),
        }
    }

    /// 用正文计算不可逆指纹；只取前若干十六进制字符，够去重、不占地。
    #[must_use]
    pub fn digest_text(text: &str) -> String {
        hex::encode(Sha256::digest(text.as_bytes()))
            .chars()
            .take(TURN_TRACE_DIGEST_HEX_CHARS)
            .collect()
    }

    /// 就地补齐正文指纹。
    #[must_use]
    pub fn with_digest(mut self, text: &str) -> Self {
        self.digest = Self::digest_text(text);
        self
    }
}

/// 单条工具回执的引用与指纹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReceiptRef {
    /// 回执引用，可据此回取原文。
    pub receipt_ref: String,
    /// 工具完整名称。
    pub tool_name: String,
    /// 工具执行状态。
    pub status: ExecutionEvidenceStatus,
    /// 是否由运行时回执确认执行。
    pub runtime_verified: bool,
    /// 参数指纹；未采集时为空串。
    pub args_digest: String,
    /// 输出指纹；未采集时为空串。
    pub output_digest: String,
    /// 输出字节数；未采集时为 0。
    pub byte_len: u32,
}

impl ToolReceiptRef {
    /// 是否带反向信号：`Contradicted`，或 `Attempted` 且未被运行时确认。
    ///
    /// 两者都不计入有效证据，卡片与轨迹共用同一判定，避免两处口径分叉。
    #[must_use]
    pub const fn is_rejected(&self) -> bool {
        matches!(self.status, ExecutionEvidenceStatus::Contradicted)
            || (matches!(self.status, ExecutionEvidenceStatus::Attempted) && !self.runtime_verified)
    }
}

/// 采集层回填的单条回执指纹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReceiptFingerprint {
    /// 对应的回执引用。
    pub receipt_ref: String,
    /// 参数指纹。
    pub args_digest: String,
    /// 输出指纹。
    pub output_digest: String,
    /// 输出字节数。
    pub byte_len: u32,
}

/// 时段分桶，由调用方按本地时区判定后传入，避免运行层依赖时区库。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HourBucket {
    /// 深夜。
    LateNight,
    /// 上午。
    Morning,
    /// 下午。
    Afternoon,
    /// 晚间。
    Evening,
}

/// 时段分桶的默认时区偏移（东八区），单位分钟。
///
/// `axon_time` 只提供 UTC 毫秒，也不引入时区库：这里以一个常量换掉一个依赖，
/// 需要别的时区时调用 [`hour_bucket_of`] 显式传偏移。
pub const AXON_DEFAULT_UTC_OFFSET_MINUTES: i32 = 480;
/// 一天与一小时的毫秒数，供时段分桶换算。
const TURN_TRACE_DAY_MS: i64 = 86_400_000;
const TURN_TRACE_HOUR_MS: i64 = 3_600_000;

/// 按给定时区偏移把 Unix 毫秒折算成时段分桶。
///
/// 按 `rem_euclid` 回绕，负偏移跨日不会折出负数小时。
#[must_use]
pub fn hour_bucket_of(now_ms: u64, utc_offset_minutes: i32) -> HourBucket {
    let offset_ms = i64::from(utc_offset_minutes).saturating_mul(60_000);
    let shifted = i64::try_from(now_ms)
        .unwrap_or(i64::MAX)
        .saturating_add(offset_ms);
    let hour = shifted.rem_euclid(TURN_TRACE_DAY_MS) / TURN_TRACE_HOUR_MS;
    match hour {
        0..=5 => HourBucket::LateNight,
        6..=11 => HourBucket::Morning,
        12..=17 => HourBucket::Afternoon,
        _ => HourBucket::Evening,
    }
}

/// 零模型的客观信号，供人格结算与反思触发共同使用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplicitSignals {
    /// 时段分桶。
    pub hour_bucket: HourBucket,
    /// 同一会话连续轮数。
    pub consecutive_turns: u32,
    /// 近 30 分钟轮次密度。
    pub turns_last_30min: u32,
    /// 本轮用户输入相对上一轮的长度变化，正数为变长。
    pub input_len_delta: i32,
    /// 最近若干轮的用户状态序列。
    pub affect_sequence: Vec<String>,
    /// 连续失败轮数；只统计本轮之前已落盘的轮次，不含本轮。
    pub failure_streak: u32,
}

impl Default for ExplicitSignals {
    fn default() -> Self {
        Self {
            hour_bucket: HourBucket::Afternoon,
            consecutive_turns: 0,
            turns_last_30min: 0,
            input_len_delta: 0,
            affect_sequence: Vec::new(),
            failure_streak: 0,
        }
    }
}

/// 单轮证据轨迹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTrace {
    /// schema 版本。
    pub schema_version: u32,
    /// 轨迹 ID。
    pub trace_id: String,
    /// 会话 ID。
    pub session_id: String,
    /// 轮次 ID。
    pub turn_id: String,
    /// canonical 主代理路径。
    pub agent_path: String,
    /// 记录时间，单位毫秒。
    pub recorded_at_ms: u64,
    /// 用户消息引用。
    pub user: MessageRef,
    /// 助手消息引用。
    pub assistant: MessageRef,
    /// 工具回执引用，按运行顺序排列。
    pub tool_receipts: Vec<ToolReceiptRef>,
    /// 本轮终态。
    pub final_status: TurnFinalStatus,
    /// 零模型客观信号。
    pub signals: ExplicitSignals,
}

impl TurnTrace {
    /// 由有界事实包投影出轨迹。
    ///
    /// 回执引用与状态来自运行时证据索引，指纹与字节数留空由采集层补齐；
    /// 助手消息引用按轮次派生，因为事实包只携带用户消息引用。
    #[must_use]
    pub fn from_outcome(
        outcome: &TurnOutcomeEnvelope,
        trace_id: impl Into<String>,
        recorded_at_ms: u64,
        signals: ExplicitSignals,
    ) -> Self {
        let tool_receipts = outcome
            .execution_evidence
            .records
            .iter()
            .map(|record| ToolReceiptRef {
                receipt_ref: record.evidence_ref.clone(),
                tool_name: record.tool_name.clone(),
                status: record.status,
                runtime_verified: record.runtime_verified,
                args_digest: TURN_TRACE_DIGEST_PLACEHOLDER.to_owned(),
                output_digest: TURN_TRACE_DIGEST_PLACEHOLDER.to_owned(),
                byte_len: 0,
            })
            .collect();
        Self {
            schema_version: TURN_TRACE_SCHEMA_VERSION,
            trace_id: trace_id.into(),
            session_id: outcome.conversation_id.clone(),
            turn_id: outcome.turn_id.clone(),
            agent_path: outcome.agent_path.clone(),
            recorded_at_ms,
            user: MessageRef::from_summary(
                outcome.user_message_ref.clone(),
                &outcome.user_intent_summary,
            ),
            assistant: MessageRef::from_summary(
                format!("turn:{}:assistant", outcome.turn_id),
                &outcome.assistant_outcome_summary,
            ),
            tool_receipts,
            final_status: outcome.final_status,
            signals,
        }
    }

    /// 证据区间：首尾引用与总数，尺寸与工具调用数无关。
    #[must_use]
    pub fn evidence_range(&self) -> Option<EvidenceRange> {
        let first = self.tool_receipts.first()?;
        let last = self.tool_receipts.last()?;
        Some(EvidenceRange {
            first_ref: first.receipt_ref.clone(),
            last_ref: last.receipt_ref.clone(),
            count: u32::try_from(self.tool_receipts.len()).unwrap_or(u32::MAX),
            verified_count: u32::try_from(
                self.tool_receipts
                    .iter()
                    // 与卡片侧同一口径：被判否的回执、以及只加载技能的调用，
                    // 都不算有效执行证据。
                    .filter(|receipt| is_verified_execution(receipt))
                    .count(),
            )
            .unwrap_or(u32::MAX),
            rejected_count: u32::try_from(
                self.tool_receipts
                    .iter()
                    .filter(|receipt| evidence_rejected(receipt))
                    .count(),
            )
            .unwrap_or(u32::MAX),
        })
    }

    /// 按回执引用就地补齐指纹；未命中的回执保持占位值。
    ///
    /// 参数与输出原文在采集层手里，运行层拿不到，因此指纹只能从外部注入。
    /// 返回实际补齐条数，便于采集层核对是否全部命中。
    pub fn apply_receipt_fingerprints(&mut self, fingerprints: &[ToolReceiptFingerprint]) -> u32 {
        let mut applied = 0_u32;
        for receipt in &mut self.tool_receipts {
            let Some(fingerprint) = fingerprints
                .iter()
                .find(|candidate| candidate.receipt_ref == receipt.receipt_ref)
            else {
                continue;
            };
            receipt.args_digest.clone_from(&fingerprint.args_digest);
            receipt.output_digest.clone_from(&fingerprint.output_digest);
            receipt.byte_len = fingerprint.byte_len;
            applied = applied.saturating_add(1);
        }
        applied
    }
}

/// 证据区间指针。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRange {
    /// 区间内第一条回执引用。
    pub first_ref: String,
    /// 区间内最后一条回执引用。
    pub last_ref: String,
    /// 区间内回执总数。
    pub count: u32,
    /// 其中被运行时确认执行的数量。
    pub verified_count: u32,
    /// 其中带反向信号的数量：派发未执行，或验证结论与完成结论矛盾。
    pub rejected_count: u32,
}

/// 判定一条证据是否带反向信号。
///
/// `Attempted` 且未被运行时确认，说明工具派发了却没有任何执行回执；
/// `Contradicted` 说明执行后的验证与完成结论相反。两者都不计入有效证据。
pub(crate) const fn evidence_rejected(receipt: &ToolReceiptRef) -> bool {
    receipt.is_rejected()
}

/// 加载技能提示的工具名。
const SKILL_INVOCATION_TOOL: &str = "invoke_skill";

/// 判定一条回执是否计入「运行时已验证的执行证据」。
///
/// 三个条件缺一不可：运行时确认过、没有被判否、且不是技能加载。`invoke_skill`
/// 只是把技能提示读进上下文，没有改动任何东西；把它算成执行证据，等于让「读过
/// 说明书」冒充「做过事」，执行类准入会被系统性放宽。
#[must_use]
pub(crate) fn is_verified_execution(receipt: &ToolReceiptRef) -> bool {
    receipt.runtime_verified
        && !evidence_rejected(receipt)
        && !receipt
            .tool_name
            .eq_ignore_ascii_case(SKILL_INVOCATION_TOOL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExecutionEvidenceIndex, ExecutionEvidenceKind, ExecutionEvidenceRecord,
        ExecutionEvidenceStatus, TurnFinalStatus, TurnOutcomeEnvelope,
    };

    fn outcome(records: Vec<ExecutionEvidenceRecord>) -> TurnOutcomeEnvelope {
        TurnOutcomeEnvelope {
            conversation_id: "session-1".to_owned(),
            turn_id: "turn-7".to_owned(),
            agent_path: "/root".to_owned(),
            user_intent_summary: "排查构建失败".to_owned(),
            assistant_outcome_summary: "定位到缺失依赖".to_owned(),
            user_message_ref: "message:user:1".to_owned(),
            execution_evidence: ExecutionEvidenceIndex { records },
            selected_skill: None,
            final_status: TurnFinalStatus::Completed,
        }
    }

    fn record(reference: &str, verified: bool) -> ExecutionEvidenceRecord {
        ExecutionEvidenceRecord {
            evidence_ref: reference.to_owned(),
            tool_name: "builtin.shell_command".to_owned(),
            status: ExecutionEvidenceStatus::Verified,
            kind: ExecutionEvidenceKind::Validation,
            runtime_verified: verified,
            verification_refs: Vec::new(),
        }
    }

    #[test]
    fn trace_projects_every_evidence_record_without_loss() {
        let envelope = outcome(vec![
            record("receipt:1", true),
            record("receipt:2", false),
            record("receipt:3", true),
        ]);
        let trace =
            TurnTrace::from_outcome(&envelope, "trace:7", 1_000, ExplicitSignals::default());

        assert_eq!(trace.tool_receipts.len(), 3);
        assert_eq!(trace.tool_receipts[0].receipt_ref, "receipt:1");
        assert_eq!(trace.tool_receipts[2].receipt_ref, "receipt:3");
        assert_eq!(trace.schema_version, TURN_TRACE_SCHEMA_VERSION);
    }

    #[test]
    fn trace_evidence_range_is_size_independent() {
        let small = TurnTrace::from_outcome(
            &outcome(vec![record("receipt:1", true)]),
            "trace:small",
            1,
            ExplicitSignals::default(),
        );
        let large = TurnTrace::from_outcome(
            &outcome(
                (1..=200)
                    .map(|index| record(&format!("receipt:{index}"), true))
                    .collect(),
            ),
            "trace:large",
            1,
            ExplicitSignals::default(),
        );

        let small_range = small.evidence_range().expect("非空证据");
        let large_range = large.evidence_range().expect("非空证据");
        assert_eq!(small_range.count, 1);
        assert_eq!(large_range.count, 200);
        assert_eq!(large_range.first_ref, "receipt:1");
        assert_eq!(large_range.last_ref, "receipt:200");
        assert_eq!(large_range.verified_count, 200);
        assert_eq!(large_range.rejected_count, 0);
    }

    #[test]
    fn trace_keeps_message_lengths_and_survives_round_trip() {
        let trace = TurnTrace::from_outcome(
            &outcome(Vec::new()),
            "trace:9",
            2_000,
            ExplicitSignals {
                hour_bucket: HourBucket::LateNight,
                consecutive_turns: 4,
                turns_last_30min: 6,
                input_len_delta: -12,
                affect_sequence: vec!["neutral".to_owned(), "urgent".to_owned()],
                failure_streak: 1,
            },
        );

        assert_eq!(trace.user.message_ref, "message:user:1");
        assert_eq!(trace.user.chars, "排查构建失败".chars().count() as u32);
        assert_eq!(trace.assistant.message_ref, "turn:turn-7:assistant");
        assert!(trace.evidence_range().is_none());

        let json = serde_json::to_string(&trace).expect("序列化");
        let restored: TurnTrace = serde_json::from_str(&json).expect("反序列化");
        assert_eq!(restored, trace);
    }

    #[test]
    fn evidence_range_counts_reverse_signals() {
        let mut trace = TurnTrace::from_outcome(
            &outcome(vec![
                record("receipt:1", true),
                record("receipt:2", false),
                record("receipt:3", false),
            ]),
            "trace:rejected",
            1,
            ExplicitSignals::default(),
        );
        trace.tool_receipts[1].status = ExecutionEvidenceStatus::Attempted;
        trace.tool_receipts[2].status = ExecutionEvidenceStatus::Contradicted;

        let range = trace.evidence_range().expect("非空证据");
        assert_eq!(range.count, 3);
        assert_eq!(range.verified_count, 1);
        assert_eq!(range.rejected_count, 2);
    }

    #[test]
    fn fingerprints_are_injected_by_receipt_ref() {
        let mut trace = TurnTrace::from_outcome(
            &outcome(vec![record("receipt:1", true), record("receipt:2", true)]),
            "trace:fingerprints",
            1,
            ExplicitSignals::default(),
        );
        let applied = trace.apply_receipt_fingerprints(&[
            ToolReceiptFingerprint {
                receipt_ref: "receipt:2".to_owned(),
                args_digest: "args-2".to_owned(),
                output_digest: "out-2".to_owned(),
                byte_len: 1_024,
            },
            ToolReceiptFingerprint {
                receipt_ref: "receipt:9".to_owned(),
                args_digest: "args-9".to_owned(),
                output_digest: "out-9".to_owned(),
                byte_len: 9,
            },
        ]);

        assert_eq!(applied, 1);
        assert_eq!(
            trace.tool_receipts[0].args_digest,
            TURN_TRACE_DIGEST_PLACEHOLDER
        );
        assert_eq!(trace.tool_receipts[1].output_digest, "out-2");
        assert_eq!(trace.tool_receipts[1].byte_len, 1_024);
    }

    #[test]
    fn message_digest_is_stable_and_bounded() {
        let digest = MessageRef::digest_text("排查构建失败");

        assert_eq!(digest.chars().count(), TURN_TRACE_DIGEST_HEX_CHARS);
        assert_eq!(digest, MessageRef::digest_text("排查构建失败"));
        assert_ne!(digest, MessageRef::digest_text("排查构建通过"));
    }
    #[test]
    fn hour_bucket_follows_the_given_offset() {
        // 2026-11-13T00:30:00Z：东八区已是上午，UTC 仍在深夜。
        let midnight_utc = 1_794_529_800_000;

        assert_eq!(
            hour_bucket_of(midnight_utc, AXON_DEFAULT_UTC_OFFSET_MINUTES),
            HourBucket::Morning
        );
        assert_eq!(hour_bucket_of(midnight_utc, 0), HourBucket::LateNight);
    }

    #[test]
    fn hour_bucket_wraps_across_the_day_boundary() {
        // 2026-11-13T20:00:00Z：加八小时后已是次日凌晨，必须回绕而不是折出负数小时。
        let evening_utc = 1_794_600_000_000;

        assert_eq!(
            hour_bucket_of(evening_utc, AXON_DEFAULT_UTC_OFFSET_MINUTES),
            HourBucket::LateNight
        );
        assert_eq!(hour_bucket_of(evening_utc, 0), HourBucket::Evening);
        assert_eq!(hour_bucket_of(evening_utc, -480), HourBucket::Afternoon);
    }
}
