//! 回放夹具：把一轮轨迹冻结成可直接重放的输入，钉住四条不变量。
//!
//! 夹具不读本机数据库：`data/runtime` 在现场常常是空的，靠真实记录做门禁
//! 等于没有门禁。这里冻结的是「结构真实、来源可追」的一轮，回执条数、
//! 状态分布与指纹形态取自设计文档 §10.2 的基线，正文字段一律用占位，
//! 换机器、换时间跑都必须得到同一结果。

use crate::reflection_card::{BatchBlock, ReflectionCard, WhereBlock};
use crate::reflection_pool::card_meets_execution_floor;
use crate::turn_trace::{
    ExplicitSignals, HourBucket, MessageRef, TURN_TRACE_SCHEMA_VERSION, ToolReceiptRef, TurnTrace,
};
use crate::{ExecutionEvidenceStatus, SalienceVector, TurnFinalStatus};

/// 单卡硬上限，与 `reflection_card` 的实现保持一致。
const CARD_MAX_BYTES: usize = 4_096;
/// 夹具的冻结时刻，固定值而不是 `now()`：夹具必须与运行时间无关。
const FROZEN_AT_MS: u64 = 1_763_246_400_000;

fn receipt(index: usize, status: ExecutionEvidenceStatus) -> ToolReceiptRef {
    ToolReceiptRef {
        receipt_ref: format!("receipt:{index}"),
        tool_name: if index.is_multiple_of(3) {
            "builtin.shell_command".to_owned()
        } else {
            "builtin.read_file".to_owned()
        },
        status,
        runtime_verified: matches!(
            status,
            ExecutionEvidenceStatus::Verified | ExecutionEvidenceStatus::Executed
        ),
        args_digest: format!("{index:032x}"),
        output_digest: format!("{:032x}", index * 7),
        byte_len: u32::try_from(index).unwrap_or(u32::MAX) * 64,
    }
}

/// 按「每 3 条一次反向信号」的分布造回执：既要覆盖 Verified，
/// 也要覆盖 Attempted 与 Contradicted，否则裁剪顺序根本没被压到。
fn receipts(count: usize) -> Vec<ToolReceiptRef> {
    (1..=count)
        .map(|index| match index % 3 {
            0 => receipt(index, ExecutionEvidenceStatus::Contradicted),
            1 => receipt(index, ExecutionEvidenceStatus::Attempted),
            _ => receipt(index, ExecutionEvidenceStatus::Verified),
        })
        .collect()
}

fn frozen_signals() -> ExplicitSignals {
    ExplicitSignals {
        hour_bucket: HourBucket::LateNight,
        consecutive_turns: 12,
        turns_last_30min: 5,
        input_len_delta: -180,
        affect_sequence: vec!["neutral".to_owned(), "frustrated".to_owned()],
        failure_streak: 1,
    }
}

fn frozen_trace(count: usize) -> TurnTrace {
    TurnTrace {
        schema_version: TURN_TRACE_SCHEMA_VERSION,
        trace_id: format!("trace:session-replay:{count}"),
        session_id: "session-replay".to_owned(),
        turn_id: "turn-replay".to_owned(),
        agent_path: "/root".to_owned(),
        recorded_at_ms: FROZEN_AT_MS,
        user: MessageRef {
            message_ref: "message:user:replay".to_owned(),
            chars: 42,
            digest: MessageRef::digest_text("重放夹具：把失败的那一轮排查完"),
        },
        assistant: MessageRef {
            message_ref: "turn:turn-replay:assistant".to_owned(),
            chars: 480,
            digest: MessageRef::digest_text("定位到分片上限判定缺失，已补测试"),
        },
        tool_receipts: receipts(count),
        final_status: TurnFinalStatus::Completed,
        signals: frozen_signals(),
    }
}

fn frozen_where() -> WhereBlock {
    WhereBlock {
        cwd_ref: "E:/zbg/axon-code".to_owned(),
        workspace_kind: "project".to_owned(),
    }
}

fn frozen_batch() -> BatchBlock {
    BatchBlock {
        window_started_at_ms: FROZEN_AT_MS,
        window_ended_at_ms: FROZEN_AT_MS,
        card_count: 1,
        accumulated_salience: 0,
    }
}

fn frozen_signals_vector() -> SalienceVector {
    SalienceVector {
        utility: 7_200,
        persistence: 6_800,
        verification: 8_400,
        specificity: 6_800,
        novelty: 5_400,
    }
}

fn cards(count: usize) -> Vec<ReflectionCard> {
    ReflectionCard::from_trace(
        &frozen_trace(count),
        frozen_signals_vector(),
        frozen_where(),
        12_000,
        frozen_batch(),
    )
}

#[test]
fn every_receipt_is_covered_by_the_evidence_range() {
    let cards = cards(37);
    let total: u32 = cards.iter().map(|card| card.evidence_range.count).sum();

    assert_eq!(total, 37, "证据区间必须覆盖全部回执，不得静默丢证据");
    assert_eq!(cards[0].evidence_range.first_ref, "receipt:1");
    assert_eq!(
        cards
            .last()
            .map(|card| card.evidence_range.last_ref.as_str()),
        Some("receipt:37")
    );
}

#[test]
fn card_bytes_never_grow_with_receipt_count() {
    let small = cards(20);
    let large = cards(200);
    let small_bytes = small.first().map_or(0, ReflectionCard::encoded_len);
    let large_bytes = large.first().map_or(0, ReflectionCard::encoded_len);

    for card in small.iter().chain(large.iter()) {
        assert!(
            card.encoded_len() <= CARD_MAX_BYTES,
            "单卡必须落在 {CARD_MAX_BYTES} 字节内"
        );
    }
    let growth = large_bytes.saturating_sub(small_bytes);
    assert!(
        growth * 10 <= small_bytes,
        "首卡字节数不应随回执总数线性增长：{small_bytes} -> {large_bytes}"
    );
}

#[test]
fn replaying_the_same_fixture_is_idempotent() {
    let first = cards(37);
    let second = cards(37);

    assert_eq!(first, second, "同一夹具重放两次，卡片必须逐字段相等");
}

#[test]
fn admission_is_stable_regardless_of_timestamps_and_pool_state() {
    let signals = frozen_signals_vector();

    // 同一信号向量重复判定必须一致；这是「准入不看时间戳、不看池状态」的最小证据。
    assert_eq!(
        card_meets_execution_floor(&signals),
        card_meets_execution_floor(&signals)
    );
    assert!(
        card_meets_execution_floor(&signals),
        "夹具三维都在执行类底线之上，应判为可单跑的强卡"
    );

    let weak = SalienceVector {
        verification: 1_000,
        ..signals
    };
    assert!(
        !card_meets_execution_floor(&weak),
        "任一维跌破底线即不得判为强卡"
    );
}
