//! 反思索引卡：记忆反思线的唯一输入形态。
//!
//! 卡片是索引，不是摘要：只携带引用、指纹、长度与显著度，
//! 任何原文都要靠 `builtin.turn_trace` 按引用回取。
//! 这样卡片尺寸与任务长度解耦，长任务不会把上下文吃回去。

use serde::{Deserialize, Serialize};

use crate::{EvidenceRange, HourBucket, ToolReceiptRef, TurnTrace};

/// 单卡硬上限，单位字节。超出按 `trim_to_budget` 收敛。
pub const REFLECTION_CARD_MAX_BYTES: usize = 4096;
/// 单批总量上限，单位字节；一批五张卡常态应落在 800 至 1200 字节每卡。
pub const REFLECTION_BATCH_MAX_BYTES: usize = 8192;
/// 常态证据指针条数上限。
pub const REFLECTION_CARD_TOP_EVIDENCE_LIMIT: usize = 8;
/// 超限裁剪后保留的证据指针条数。
pub const REFLECTION_CARD_TRIMMED_EVIDENCE_LIMIT: usize = 4;
/// 单段回执条数上限；超长轮次据此切段。
pub const REFLECTION_SEGMENT_MAX_RECEIPTS: u16 = 16;
/// 单轮段数上限；超出合并为一张 merged 卡。
pub const REFLECTION_MAX_SEGMENTS: u16 = 8;

/// 运行时确认执行的回执，作为证据指针时的显著度基准。
pub const REFLECTION_EVIDENCE_VERIFIED_SALIENCE_BP: u16 = 9_000;
/// 未获运行时确认的回执，作为证据指针时的显著度基准。
pub const REFLECTION_EVIDENCE_UNVERIFIED_SALIENCE_BP: u16 = 3_000;

/// 五维显著度，全部 basis point（0 至 10000）。
///
/// 刻意不合成单一标量：留档理由必须可解释，事后才审得出「凭什么记住这条」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalienceVector {
    /// 对未来任务的复用价值。
    pub utility: u16,
    /// 结论跨时间保持成立的概率。
    pub persistence: u16,
    /// 运行时执行证据的闭环强度。
    pub verification: u16,
    /// 是否指向具体对象（文件、命令、符号、字段），而非泛泛而谈。
    pub specificity: u16,
    /// 相对既有认知的新颖度。
    pub novelty: u16,
}

impl SalienceVector {
    /// 五维之和，用于批内排序；不作为准入门槛。
    #[must_use]
    pub fn total(&self) -> u32 {
        u32::from(self.utility)
            .saturating_add(u32::from(self.persistence))
            .saturating_add(u32::from(self.verification))
            .saturating_add(u32::from(self.specificity))
            .saturating_add(u32::from(self.novelty))
    }
    /// 逐维取大，用于把一批卡片的显著度折成「批次显著度」。
    ///
    /// 批次里只要有一张强卡把它拉进来，候选就该按强卡的量级过准入；若一律沿用
    /// 「最后一张卡（往往是弱卡）」的向量，早期强卡的候选会被判成不达标而丢掉。
    #[must_use]
    pub fn merge_max(self, other: Self) -> Self {
        Self {
            utility: self.utility.max(other.utility),
            persistence: self.persistence.max(other.persistence),
            verification: self.verification.max(other.verification),
            specificity: self.specificity.max(other.specificity),
            novelty: self.novelty.max(other.novelty),
        }
    }
}

/// 时间块：只存时刻、时长与分桶，不存自然语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhenBlock {
    /// 本轮起始时刻，单位毫秒。
    pub started_at_ms: u64,
    /// 本轮持续时长，单位毫秒。
    pub duration_ms: u64,
    /// 时段分桶，由调用方按本地时区判定。
    pub hour_bucket: HourBucket,
}

/// 位置块：cwd 引用与工作区类别。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhereBlock {
    /// 工作目录引用；只存路径，不存其内容。
    pub cwd_ref: String,
    /// 工作区类别标签，由调用方判定（如 project / home / other）。
    pub workspace_kind: String,
}

/// 内容块：意图与结果的长度与指纹，不含原文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatBlock {
    /// 用户意图的消息引用。
    pub intent_ref: String,
    /// 意图正文字符数。
    pub intent_chars: u32,
    /// 意图正文指纹。
    pub intent_digest: String,
    /// 助手结论的消息引用。
    pub outcome_ref: String,
    /// 结论正文字符数。
    pub outcome_chars: u32,
    /// 结论正文指纹。
    pub outcome_digest: String,
    /// 本轮终态标签。
    pub final_status: String,
}

/// 批块：窗口与批内累计显著度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchBlock {
    /// 窗口起始时刻，单位毫秒。
    pub window_started_at_ms: u64,
    /// 窗口结束时刻，单位毫秒。
    pub window_ended_at_ms: u64,
    /// 窗口内卡片数。
    pub card_count: u16,
    /// 窗口内显著度累计。
    pub accumulated_salience: u32,
}

/// 一条代表性证据指针；正文仍在归档里。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidencePointer {
    /// 回执引用，可据此回取原文。
    pub receipt_ref: String,
    /// 工具完整名称。
    pub tool_name: String,
    /// 是否由运行时确认执行。
    pub verified: bool,
    /// 该条证据的显著度，用于裁剪排序。
    pub salience: u16,
}

/// 反思索引卡。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionCard {
    /// 卡片标识。
    pub card_id: String,
    /// 来源轨迹标识。
    pub trace_id: String,
    /// 段序号，从 0 开始。
    pub segment_index: u16,
    /// 本轮的段总数。
    pub segment_count: u16,
    /// 时间块。
    pub when: WhenBlock,
    /// 位置块。
    pub location: WhereBlock,
    /// 内容块。
    pub what: WhatBlock,
    /// 证据区间指针；尺寸与工具调用数无关，永不裁剪。
    pub evidence_range: EvidenceRange,
    /// 代表证据，常态至多 8 条，超限裁到 4 条。
    pub top_evidence: Vec<EvidencePointer>,
    /// 五维显著度。
    pub signals: SalienceVector,
    /// 批块。
    pub batch: BatchBlock,
    /// 检索抓手：文件、符号、命令首段；只作检索起点，不作事实断言。
    pub hints: Vec<String>,
}

impl ReflectionCard {
    /// 序列化后的字节数，用于预算判定。
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len())
    }

    /// 是否落在单卡硬上限内。
    #[must_use]
    pub fn fits_card_budget(&self) -> bool {
        self.encoded_len() <= REFLECTION_CARD_MAX_BYTES
    }

    /// 按文档约定的顺序收敛到单卡预算内。
    ///
    /// 顺序固定：先截 `hints`，再裁 `top_evidence` 到 4 条并优先保 verified 与高显著度。
    /// `evidence_range` 永不裁剪：它的 `count` 与 `verified_count` 必须等于真实值，
    /// 否则卡片就成了失真摘要。
    ///
    /// 返回是否最终落在预算内。
    pub fn trim_to_budget(&mut self) -> bool {
        if self.fits_card_budget() {
            return true;
        }
        self.hints.clear();
        if self.fits_card_budget() {
            return true;
        }
        self.trim_top_evidence(REFLECTION_CARD_TRIMMED_EVIDENCE_LIMIT);
        self.fits_card_budget()
    }

    /// 按「verified 优先、显著度降序」保留前 `limit` 条证据。
    pub fn trim_top_evidence(&mut self, limit: usize) {
        if self.top_evidence.len() <= limit {
            return;
        }
        self.top_evidence.sort_by(|left, right| {
            right
                .verified
                .cmp(&left.verified)
                .then_with(|| right.salience.cmp(&left.salience))
        });
        self.top_evidence.truncate(limit);
    }

    /// 单轮按工具边界切段：每段至多 `REFLECTION_SEGMENT_MAX_RECEIPTS` 条回执。
    ///
    /// 返回值是 (段序号, 段内回执条数)；段数超过 `REFLECTION_MAX_SEGMENTS` 时，
    /// 调用方应把剩余段合并为一张 merged 卡，`evidence_range` 仍覆盖全区间。
    #[must_use]
    pub fn segment_receipt_counts(total_receipts: u32) -> Vec<(u16, u16)> {
        let per_segment = u32::from(REFLECTION_SEGMENT_MAX_RECEIPTS);
        if total_receipts == 0 {
            return Vec::new();
        }
        let segments = total_receipts.div_ceil(per_segment);
        (0..segments)
            .map(|index| {
                let remaining = total_receipts.saturating_sub(index * per_segment);
                let count = remaining.min(per_segment);
                (
                    u16::try_from(index).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                )
            })
            .collect()
    }

    /// 从单轮证据轨迹投影出索引卡。
    ///
    /// 切段按工具边界：每段至多 `REFLECTION_SEGMENT_MAX_RECEIPTS` 条回执，每段一张卡。
    /// 段数超过 `REFLECTION_MAX_SEGMENTS` 时，最后一张保留卡吞掉剩余全部回执：
    /// `segment_count` 仍报真实段数，`evidence_range` 始终覆盖该卡应有的全部回执，
    /// 不因切段丢证据。
    ///
    /// `hints` 留空：轨迹只存参数指纹、不存参数原文，凭空生成检索抓手等于伪造证据。
    #[must_use]
    pub fn from_trace(
        trace: &TurnTrace,
        signals: SalienceVector,
        location: WhereBlock,
        duration_ms: u64,
        batch: BatchBlock,
    ) -> Vec<Self> {
        let total = u32::try_from(trace.tool_receipts.len()).unwrap_or(u32::MAX);
        let segments = Self::segment_receipt_counts(total);
        let when = WhenBlock {
            started_at_ms: trace.recorded_at_ms,
            duration_ms,
            hour_bucket: trace.signals.hour_bucket,
        };
        let what = WhatBlock {
            intent_ref: trace.user.message_ref.clone(),
            intent_chars: trace.user.chars,
            intent_digest: trace.user.digest.clone(),
            outcome_ref: trace.assistant.message_ref.clone(),
            outcome_chars: trace.assistant.chars,
            outcome_digest: trace.assistant.digest.clone(),
            final_status: trace.final_status.label().to_owned(),
        };
        if segments.is_empty() {
            return vec![Self {
                card_id: format!("card:{}:0", trace.trace_id),
                trace_id: trace.trace_id.clone(),
                segment_index: 0,
                segment_count: 1,
                when,
                location,
                what,
                evidence_range: EvidenceRange {
                    first_ref: String::new(),
                    last_ref: String::new(),
                    count: 0,
                    verified_count: 0,
                    rejected_count: 0,
                },
                top_evidence: Vec::new(),
                signals,
                batch,
                hints: Vec::new(),
            }];
        }

        let segment_count = u16::try_from(segments.len()).unwrap_or(u16::MAX);
        let per_segment = u32::from(REFLECTION_SEGMENT_MAX_RECEIPTS);
        let keep = segments.len().min(usize::from(REFLECTION_MAX_SEGMENTS));
        let mut cards = Vec::with_capacity(keep);
        for index in 0..keep {
            let start = u32::try_from(index)
                .unwrap_or(u32::MAX)
                .saturating_mul(per_segment);
            let count = if index + 1 == keep {
                total.saturating_sub(start)
            } else {
                per_segment.min(total.saturating_sub(start))
            };
            let slice = receipt_slice(&trace.tool_receipts, start, count);
            let mut card = Self {
                card_id: format!("card:{}:{index}", trace.trace_id),
                trace_id: trace.trace_id.clone(),
                segment_index: u16::try_from(index).unwrap_or(u16::MAX),
                segment_count,
                when,
                location: location.clone(),
                what: what.clone(),
                evidence_range: segment_range(slice, count),
                top_evidence: slice
                    .iter()
                    .map(|receipt| EvidencePointer {
                        receipt_ref: receipt.receipt_ref.clone(),
                        tool_name: receipt.tool_name.clone(),
                        verified: receipt.runtime_verified,
                        salience: if receipt.runtime_verified {
                            REFLECTION_EVIDENCE_VERIFIED_SALIENCE_BP
                        } else {
                            REFLECTION_EVIDENCE_UNVERIFIED_SALIENCE_BP
                        },
                    })
                    .collect(),
                signals,
                batch,
                hints: Vec::new(),
            };
            card.trim_top_evidence(REFLECTION_CARD_TOP_EVIDENCE_LIMIT);
            card.trim_to_budget();
            cards.push(card);
        }
        cards
    }
}

/// 取 `[start, start + count)` 区间的回执；越界时返回空切片，不 panic。
fn receipt_slice(receipts: &[ToolReceiptRef], start: u32, count: u32) -> &[ToolReceiptRef] {
    let begin = usize::try_from(start).unwrap_or(usize::MAX);
    let end = usize::try_from(start.saturating_add(count)).unwrap_or(usize::MAX);
    receipts.get(begin..end).unwrap_or(&[])
}

/// 一段回执的 O(1) 证据区间指针；计数取真实值，不随裁剪变化。
fn segment_range(receipts: &[ToolReceiptRef], count: u32) -> EvidenceRange {
    EvidenceRange {
        first_ref: receipts
            .first()
            .map_or_else(String::new, |receipt| receipt.receipt_ref.clone()),
        last_ref: receipts
            .last()
            .map_or_else(String::new, |receipt| receipt.receipt_ref.clone()),
        count,
        // 矛盾证据会保留 runtime_verified=true（只有通过过运行时校验的记录才可能
        // 被判为 Contradicted），所以这里必须再排掉 rejected，否则同一条回执会被
        // 同时算进 verified 与 rejected，`count == verified + rejected` 不成立。
        verified_count: u32::try_from(
            receipts
                .iter()
                .filter(|receipt| super::turn_trace::is_verified_execution(receipt))
                .count(),
        )
        .unwrap_or(u32::MAX),
        rejected_count: u32::try_from(
            receipts
                .iter()
                .filter(|receipt| receipt.is_rejected())
                .count(),
        )
        .unwrap_or(u32::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionEvidenceStatus, ExplicitSignals, MessageRef, TurnFinalStatus};

    fn card(top_evidence: Vec<EvidencePointer>, hints: Vec<String>) -> ReflectionCard {
        ReflectionCard {
            card_id: "card-1".to_owned(),
            trace_id: "trace:session-1:7".to_owned(),
            segment_index: 0,
            segment_count: 1,
            when: WhenBlock {
                started_at_ms: 1_000,
                duration_ms: 2_000,
                hour_bucket: HourBucket::Afternoon,
            },
            location: WhereBlock {
                cwd_ref: "E:/zbg/axon-code".to_owned(),
                workspace_kind: "project".to_owned(),
            },
            what: WhatBlock {
                intent_ref: "message:user:1".to_owned(),
                intent_chars: 12,
                intent_digest: "digest-intent".to_owned(),
                outcome_ref: "turn:turn-7:assistant".to_owned(),
                outcome_chars: 40,
                outcome_digest: "digest-outcome".to_owned(),
                final_status: "completed".to_owned(),
            },
            evidence_range: EvidenceRange {
                first_ref: "receipt:1".to_owned(),
                last_ref: "receipt:3".to_owned(),
                count: 3,
                verified_count: 2,
                rejected_count: 1,
            },
            top_evidence,
            signals: SalienceVector {
                utility: 8_000,
                persistence: 7_000,
                verification: 9_000,
                specificity: 6_000,
                novelty: 5_000,
            },
            batch: BatchBlock {
                window_started_at_ms: 0,
                window_ended_at_ms: 1_000,
                card_count: 3,
                accumulated_salience: 18_000,
            },
            hints,
        }
    }

    fn pointer(reference: &str, verified: bool, salience: u16) -> EvidencePointer {
        EvidencePointer {
            receipt_ref: reference.to_owned(),
            tool_name: "builtin.shell_command".to_owned(),
            verified,
            salience,
        }
    }

    #[test]
    fn salience_never_collapses_to_single_scalar_for_admission() {
        let signals = SalienceVector {
            utility: 1_000,
            persistence: 2_000,
            verification: 3_000,
            specificity: 4_000,
            novelty: 5_000,
        };

        assert_eq!(signals.total(), 15_000);
        assert_eq!(SalienceVector::default().total(), 0);
    }

    #[test]
    fn trim_keeps_evidence_range_intact() {
        let mut target = card(
            (1..=12)
                .map(|index| pointer(&format!("receipt:{index}"), index % 2 == 0, 9_000 - index))
                .collect(),
            vec!["hint-a".to_owned(); 4],
        );
        let range_before = target.evidence_range.clone();

        target.trim_top_evidence(REFLECTION_CARD_TRIMMED_EVIDENCE_LIMIT);

        assert_eq!(target.evidence_range, range_before);
        assert_eq!(
            target.top_evidence.len(),
            REFLECTION_CARD_TRIMMED_EVIDENCE_LIMIT
        );
        assert!(target.top_evidence[0].verified, "verified 必须优先保留");
    }

    #[test]
    fn trim_to_budget_drops_hints_before_evidence() {
        let mut target = card(
            vec![pointer("receipt:1", true, 9_000)],
            (0..40).map(|index| format!("hint-{index}")).collect(),
        );
        target.trim_to_budget();

        assert!(target.fits_card_budget());
    }

    #[test]
    fn small_card_survives_round_trip_without_loss() {
        let original = card(vec![pointer("receipt:1", true, 9_000)], Vec::new());
        let json = serde_json::to_string(&original).expect("序列化");
        let restored: ReflectionCard = serde_json::from_str(&json).expect("反序列化");

        assert_eq!(restored, original);
        assert!(original.fits_card_budget());
    }

    #[test]
    fn long_turn_is_split_by_tool_boundary() {
        let segments = ReflectionCard::segment_receipt_counts(37);

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], (0, 16));
        assert_eq!(segments[2], (2, 5));
        assert!(ReflectionCard::segment_receipt_counts(0).is_empty());
    }

    fn trace_with_receipts(count: u32, verified_every: u32) -> TurnTrace {
        let tool_receipts = (0..count)
            .map(|index| {
                let ordinal = index + 1;
                let verified = verified_every != 0 && ordinal % verified_every == 0;
                ToolReceiptRef {
                    receipt_ref: format!("receipt:{ordinal}"),
                    tool_name: "builtin.shell_command".to_owned(),
                    status: if verified {
                        ExecutionEvidenceStatus::Verified
                    } else {
                        ExecutionEvidenceStatus::Executed
                    },
                    runtime_verified: verified,
                    args_digest: String::new(),
                    output_digest: String::new(),
                    byte_len: 0,
                }
            })
            .collect();
        TurnTrace {
            schema_version: 1,
            trace_id: "trace:session-1:7".to_owned(),
            session_id: "session-1".to_owned(),
            turn_id: "turn-7".to_owned(),
            agent_path: "/root".to_owned(),
            recorded_at_ms: 1_000,
            user: MessageRef {
                message_ref: "message:user:7".to_owned(),
                chars: 12,
                digest: "digest-intent".to_owned(),
            },
            assistant: MessageRef {
                message_ref: "turn:turn-7:assistant".to_owned(),
                chars: 40,
                digest: "digest-outcome".to_owned(),
            },
            tool_receipts,
            final_status: TurnFinalStatus::Completed,
            signals: ExplicitSignals::default(),
        }
    }

    fn batch_block() -> BatchBlock {
        BatchBlock {
            window_started_at_ms: 1_000,
            window_ended_at_ms: 1_000,
            card_count: 1,
            accumulated_salience: 6_000,
        }
    }

    #[test]
    fn from_trace_splits_by_tool_boundary_and_keeps_real_counts() {
        let trace = trace_with_receipts(37, 2);
        let cards = ReflectionCard::from_trace(
            &trace,
            SalienceVector::default(),
            WhereBlock {
                cwd_ref: "E:/zbg/axon-code".to_owned(),
                workspace_kind: "project".to_owned(),
            },
            2_000,
            batch_block(),
        );

        assert_eq!(cards.len(), 3);
        assert!(cards.iter().all(|card| card.segment_count == 3));
        assert_eq!(
            cards
                .iter()
                .map(|card| card.evidence_range.count)
                .sum::<u32>(),
            37,
            "切段不得丢回执"
        );
        assert_eq!(cards[0].evidence_range.first_ref, "receipt:1");
        assert_eq!(cards[2].evidence_range.last_ref, "receipt:37");
        assert_eq!(cards[0].evidence_range.verified_count, 8);
        assert_eq!(cards[0].what.final_status, "completed");
        assert_eq!(cards[0].when.started_at_ms, 1_000);
        assert_eq!(cards[0].when.duration_ms, 2_000);
    }

    #[test]
    fn from_trace_merges_overflow_segments_into_last_card() {
        let trace = trace_with_receipts(200, 3);
        let cards = ReflectionCard::from_trace(
            &trace,
            SalienceVector::default(),
            WhereBlock {
                cwd_ref: "E:/zbg/axon-code".to_owned(),
                workspace_kind: "project".to_owned(),
            },
            0,
            batch_block(),
        );

        assert_eq!(cards.len(), usize::from(REFLECTION_MAX_SEGMENTS));
        assert!(cards.iter().all(|card| card.segment_count == 13));
        assert_eq!(
            cards
                .iter()
                .map(|card| card.evidence_range.count)
                .sum::<u32>(),
            200,
            "溢出段并入最后一张卡，证据区间仍覆盖全部回执"
        );
        assert_eq!(
            cards.last().expect("末卡").evidence_range.last_ref,
            "receipt:200"
        );
        assert!(cards.iter().all(ReflectionCard::fits_card_budget));
    }

    #[test]
    fn from_trace_without_receipts_yields_one_index_card() {
        let trace = trace_with_receipts(0, 0);
        let cards = ReflectionCard::from_trace(
            &trace,
            SalienceVector::default(),
            WhereBlock {
                cwd_ref: "C:/Users/<user>/.axon".to_owned(),
                workspace_kind: "home".to_owned(),
            },
            0,
            batch_block(),
        );

        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].evidence_range.count, 0);
        assert!(cards[0].top_evidence.is_empty());
        assert_eq!(cards[0].segment_count, 1);
        assert!(cards[0].fits_card_budget());
    }
}
