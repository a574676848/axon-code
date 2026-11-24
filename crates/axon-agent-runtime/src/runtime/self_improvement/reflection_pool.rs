//! 反思候选池：攒批触发判定与批组装。
//!
//! 触发判定是纯函数，八条入口各自独立，任一命中即跑：
//! 只靠按量触发会漏掉低频场景，一天两三轮的深度任务永远攒不满五张卡。
//! 池按地点分桶，不跨桶合并，避免把两个项目的线索攒到一起。

use serde::{Deserialize, Serialize};

use crate::{
    EXECUTION_SPECIFICITY_MIN_BP, EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS,
    EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS, REFLECTION_BATCH_MAX_BYTES, ReflectionCard,
    SalienceVector,
};

/// 条数入口：攒够这么多卡片。
pub const BATCH_MIN_CANDIDATES: u32 = 5;
/// 执行类准入的三维下限之和：verification 7000 + utility 6500 + specificity 6500。
///
/// 它是「一张卡刚够被写进记忆」的最低五维和，因此拿它当单卡强档的锚点：
/// 阈值不是拍的，是由准入规则反推出来的，事后审得出依据。
pub const REFLECTION_EXECUTION_FLOOR_BP: u16 = 20_000;
/// 单卡强档入口：达到执行类准入底线的单卡立即单跑。
///
/// 取底线而非更大的整数：达标卡片与改造前同频单跑，攒批不会把该当场记住的轮次拖后；
/// 低于底线的弱卡才进池，由条数、累计与池龄三条入口接管。
pub const SINGLE_CARD_MIN_BP: u16 = REFLECTION_EXECUTION_FLOOR_BP;
/// 累计显著度入口：池内权重达到两张执行底线即成批。
///
/// 一张刚过线的卡不足以撑起一次批处理，两张才够；再多的弱卡交给条数入口。
pub const BATCH_MIN_ACCUMULATED_BP: u64 = (SINGLE_CARD_MIN_BP as u64) * 2;
/// 显式纠正入口：用户当场指出错误时立即单跑。
pub const CORRECTION_TRIGGER_BP: u16 = 8_000;
/// 池年龄入口：最老卡片满 30 分钟。
pub const BATCH_MAX_AGE_MS: u64 = 1_800_000;
/// 惰性补漏入口：新会话启动时扫描，最老卡片超 2 小时。
pub const STALE_POOL_MS: u64 = 7_200_000;
/// 同簇复现入口：同桶内语义相似卡片满这么多张。
pub const RECURRENCE_MIN_CARDS: u32 = 3;
/// 触发本次记忆线的入口。八条各自独立，命中顺序即优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BatchTrigger {
    /// 会话收口强制 flush；池非空即跑。
    SessionClose,
    /// 条数越线。
    CountThreshold,
    /// 累计显著度越线。
    AccumulatedSalience,
    /// 单卡强档。
    SingleStrongCard,
    /// 显式纠正。
    ExplicitCorrection,
    /// 池年龄越线。
    PoolAge,
    /// 惰性补漏。
    StalePool,
    /// 同簇复现。
    RecurrenceCluster,
}

impl BatchTrigger {
    /// 稳定的机器标识，用于回执的 `reason` 字段。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SessionClose => "session_close",
            Self::CountThreshold => "count_threshold",
            Self::AccumulatedSalience => "accumulated_salience",
            Self::SingleStrongCard => "single_strong_card",
            Self::ExplicitCorrection => "explicit_correction",
            Self::PoolAge => "pool_age",
            Self::StalePool => "stale_pool",
            Self::RecurrenceCluster => "recurrence_cluster",
        }
    }
}

/// 触发判定输入；全部由池的当前状态投影而来，判定本身不碰 IO。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchTriggerInput {
    /// 池内卡片数。
    pub candidate_count: u32,
    /// 池内显著度累计。
    pub accumulated_salience_bp: u64,
    /// 池内是否存在「三维同时达到执行类准入底线」的卡片。
    ///
    /// 不能拿五维之和去比 `SINGLE_CARD_MIN_BP`：那是三维门槛之和，量纲不同，
    /// 五维均值只要到 4000 就会被误判成强卡，池会被每轮抽干、攒批永不生效。
    pub strongest_card_meets_execution_floor: bool,
    /// 池内最强纠正强度。
    pub strongest_correction_bp: u16,
    /// 池内最老卡片的年龄，单位毫秒。
    pub oldest_age_ms: u64,
    /// 本轮是否为会话收口；当前没有显式 close 事件，调用方恒传 false。
    pub session_closing: bool,
    /// 本次判定是否来自进程启动后的首次扫描（冷启动补漏专用）。
    pub is_cold_start: bool,
    /// 同簇内相似卡片数。
    pub same_cluster_cards: u32,
    /// 惰性补漏时读取的最老卡片年龄，单位毫秒。
    pub stale_pool_age_ms: u64,
}

/// 判定是否触发一次记忆线；返回命中的第一条入口。
///
/// 顺序即优先级：收口是硬 flush，其余按「越确定越先」。同一条输入只返回一个入口，
/// 便于回执给出唯一原因，而不是一串并行命中的噪声。
#[must_use]
pub fn evaluate_batch_trigger(input: &BatchTriggerInput) -> Option<BatchTrigger> {
    if input.session_closing && input.candidate_count > 0 {
        return Some(BatchTrigger::SessionClose);
    }
    if input.candidate_count >= BATCH_MIN_CANDIDATES {
        return Some(BatchTrigger::CountThreshold);
    }
    if input.accumulated_salience_bp >= BATCH_MIN_ACCUMULATED_BP {
        return Some(BatchTrigger::AccumulatedSalience);
    }
    if input.strongest_card_meets_execution_floor {
        return Some(BatchTrigger::SingleStrongCard);
    }
    if input.strongest_correction_bp >= CORRECTION_TRIGGER_BP {
        return Some(BatchTrigger::ExplicitCorrection);
    }
    // 冷启动补漏必须排在池龄入口之前：两者都以池龄为输入，池龄 >= 2 小时必然也
    // >= 30 分钟，排在后面就永不命中。非冷启动轮次不判这一条：常规轮次由 30 分钟
    // 池龄接管，否则每次加载都会强行跑批。
    if input.is_cold_start && input.stale_pool_age_ms >= STALE_POOL_MS {
        return Some(BatchTrigger::StalePool);
    }
    if input.oldest_age_ms >= BATCH_MAX_AGE_MS {
        return Some(BatchTrigger::PoolAge);
    }
    if input.same_cluster_cards >= RECURRENCE_MIN_CARDS {
        return Some(BatchTrigger::RecurrenceCluster);
    }
    None
}

/// 池内一张待处理卡片。
///
/// 以 `turn_id` 加 `trace_id` 为幂等键：重复写入、崩溃重放与补跑都按它去重，
/// 保证同一轮不会产出两次记忆。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionPoolEntry {
    /// 轮次标识。
    pub turn_id: String,
    /// 轨迹标识。
    pub trace_id: String,
    /// 入池时刻，单位毫秒。
    pub enqueued_at_ms: u64,
    /// 卡片本体。
    pub card: ReflectionCard,
    /// 地点桶键：由 `pool_bucket_key(cwd, branch)` 生成。
    ///
    /// 池文件是全局一个，桶键让不同工作区的卡片互不成批；空值代表旧版本写入的条目，
    /// 读侧按「归当前桶」兼容，避免升级后旧卡片永远出不来。
    #[serde(default)]
    pub bucket: String,
    /// 本轮语义评估给出的纠正强度，单位 basis point。
    ///
    /// 它不进卡片：卡片只表达五维显著度，纠正强度是触发入口专用的信号，
    /// 放在队列表述上，触发判定与卡片契约各自单一职责。
    #[serde(default)]
    pub correction_bp: u16,
}

impl ReflectionPoolEntry {
    /// 幂等键。
    #[must_use]
    pub fn idempotency_key(&self) -> String {
        // 单轮超长时会切成多段卡，各段共用同一个 trace_id，只用 turn+trace 会把
        // 第 2 段起判成重复而静默丢弃，所以段号必须进键。
        format!(
            "{}:{}:{}",
            self.turn_id, self.trace_id, self.card.segment_index
        )
    }
}

/// 单个地点桶的内存队列。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReflectionPool {
    entries: Vec<ReflectionPoolEntry>,
}

impl ReflectionPool {
    /// 入池；同一幂等键只保留最早一次，重复入池返回 `false`。
    pub fn enqueue(&mut self, entry: ReflectionPoolEntry) -> bool {
        let key = entry.idempotency_key();
        if self
            .entries
            .iter()
            .any(|existing| existing.idempotency_key() == key)
        {
            return false;
        }
        self.entries.push(entry);
        true
    }

    /// 池内卡片数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 池是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 扣掉会话收口的触发输入：只填与池状态有关的字段，其余由调用方补齐。
    #[must_use]
    pub fn trigger_input(
        &self,
        now_ms: u64,
        session_closing: bool,
        is_cold_start: bool,
    ) -> BatchTriggerInput {
        let strongest_card_meets_execution_floor = self
            .entries
            .iter()
            .any(|entry| card_meets_execution_floor(&entry.card.signals));
        BatchTriggerInput {
            candidate_count: u32::try_from(self.entries.len()).unwrap_or(u32::MAX),
            accumulated_salience_bp: self.accumulated_salience(),
            strongest_card_meets_execution_floor,
            strongest_correction_bp: self
                .entries
                .iter()
                .map(|entry| entry.correction_bp)
                .max()
                .unwrap_or(0),
            oldest_age_ms: self.oldest_age_ms(now_ms),
            session_closing,
            is_cold_start,
            same_cluster_cards: largest_cluster_size(&self.entries),
            // 补漏入口量的是「池里最老卡片有多久」：池龄入口按 30 分钟先命中，
            // 补漏只在「本轮档位被判 Skip、本来不该跑」时用来强行兜一次底。
            stale_pool_age_ms: self.oldest_age_ms(now_ms),
        }
    }

    /// 池内最大同簇卡片数。卡片不含正文，拿不到嵌入，因此簇键用
    /// 「工作区类别 + 目录 + 主导工具」这个确定性组合：同一地点同一类工具
    /// 反复出现三次，说明它是该地点的常规工作流。这是可解释的启发式，
    /// 宁可漏簇也不乱并簇。
    #[must_use]
    pub fn largest_cluster_size(&self) -> u32 {
        largest_cluster_size(&self.entries)
    }

    /// 池内显著度累计。
    #[must_use]
    pub fn accumulated_salience(&self) -> u64 {
        self.entries
            .iter()
            .map(|entry| u64::from(entry.card.signals.total()))
            .sum()
    }

    /// 最老卡片的年龄；空池返回 0。
    #[must_use]
    pub fn oldest_age_ms(&self, now_ms: u64) -> u64 {
        self.entries
            .iter()
            .map(|entry| now_ms.saturating_sub(entry.enqueued_at_ms))
            .max()
            .unwrap_or(0)
    }

    /// 取出一批卡片：按入池顺序取，直到触到条数或字节预算。
    ///
    /// 至少取一张：单卡超预算时也要跑，否则空转永远不前进。
    /// 取出时回填批块，`evidence_range` 一律保持原样。
    pub fn drain_batch(&mut self, now_ms: u64) -> Vec<ReflectionCard> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let window_started_at_ms = self
            .entries
            .first()
            .map_or(now_ms, |entry| entry.enqueued_at_ms);
        let accumulated = self.accumulated_salience();

        let mut bytes = 0_usize;
        let mut taken = 0_usize;
        for entry in &self.entries {
            let card_bytes = entry.card.encoded_len();
            if taken > 0 && bytes.saturating_add(card_bytes) > REFLECTION_BATCH_MAX_BYTES {
                break;
            }
            bytes = bytes.saturating_add(card_bytes);
            taken += 1;
        }
        let taken = taken.max(1).min(self.entries.len());
        let mut drained: Vec<ReflectionPoolEntry> = self.entries.drain(..taken).collect();
        let card_count = u16::try_from(drained.len()).unwrap_or(u16::MAX);

        drained
            .drain(..)
            .map(|mut entry| {
                entry.card.batch.card_count = card_count;
                entry.card.batch.window_started_at_ms = window_started_at_ms;
                entry.card.batch.window_ended_at_ms = now_ms;
                entry.card.batch.accumulated_salience =
                    u32::try_from(accumulated).unwrap_or(u32::MAX);
                entry.card
            })
            .collect()
    }

    /// 只读快照，供持久化层写盘。
    #[must_use]
    pub fn entries(&self) -> &[ReflectionPoolEntry] {
        &self.entries
    }
}

/// 单卡是否同时达到执行类准入的三维底线。
///
/// 与 `ReflectionCandidate::admission` 用同一组常量：卡片只有三维都过线，才可能
/// 在后面的准入里被写进记忆。用五维之和去比门槛会把「一维极高、其余极低」的卡
/// 误判成强卡，攒批就永远攒不起来。
#[must_use]
pub fn card_meets_execution_floor(signals: &SalienceVector) -> bool {
    signals.verification >= EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS
        && signals.utility >= EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS
        && signals.specificity >= EXECUTION_SPECIFICITY_MIN_BP
}

/// 按地点桶切分池条目：返回 `(本桶, 其他桶)`。
///
/// 空桶键是升级前写入的旧条目，按「归当前桶」兼容，否则它们会永远留在文件里出不来。
/// 回写时调用方必须把 `其他桶` 原样并回去，只替换本桶剩余队列。
#[must_use]
pub fn split_pool_by_bucket(
    entries: Vec<ReflectionPoolEntry>,
    bucket: &str,
) -> (Vec<ReflectionPoolEntry>, Vec<ReflectionPoolEntry>) {
    entries
        .into_iter()
        .partition(|entry| entry.bucket.is_empty() || entry.bucket == bucket)
}

/// 地点桶键：普通 Workspace 与其分支各成一桶，Home 单独一桶。
///
/// 桶键只为分目录用，不参与语义判断，因此不做规范化猜测：
/// 调用方给什么就拼什么，避免同一地点因路径写法差异裂成两个桶。
#[must_use]
pub fn pool_bucket_key(cwd: &str, git_branch: Option<&str>) -> String {
    match git_branch {
        Some(branch) if !branch.trim().is_empty() => format!("{cwd}@{branch}"),
        _ => cwd.to_owned(),
    }
}

/// 单卡五维之和，供调用方构造触发输入。
#[must_use]
pub fn card_salience_total(signals: &SalienceVector) -> u16 {
    u16::try_from(signals.total()).unwrap_or(u16::MAX)
}

/// 池内最大同簇卡片数。
///
/// 簇键是「工作区类别 + 目录 + 主导工具名」：卡片不含正文，语义相似度无从算起，
/// 但「同一地点反复用同一类工具」已经足以说明这是常规工作流。
#[must_use]
pub fn largest_cluster_size(entries: &[ReflectionPoolEntry]) -> u32 {
    let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for entry in entries {
        let slot = counts.entry(cluster_key(&entry.card)).or_default();
        *slot = slot.saturating_add(1);
    }
    counts.values().copied().max().unwrap_or(0)
}

/// 单卡的簇键：`工作区类别|目录|主导工具名`。
#[must_use]
pub fn cluster_key(card: &ReflectionCard) -> String {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for pointer in &card.top_evidence {
        *counts.entry(pointer.tool_name.as_str()).or_default() += 1;
    }
    let dominant = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(name, _)| name)
        .unwrap_or_default();
    format!(
        "{}|{}|{dominant}",
        card.location.workspace_kind, card.location.cwd_ref
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BatchBlock, EXECUTION_SPECIFICITY_MIN_BP, EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS,
        EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS, EvidenceRange, HourBucket, WhatBlock, WhenBlock,
        WhereBlock,
    };

    fn card(index: u16) -> ReflectionCard {
        ReflectionCard {
            card_id: format!("card-{index}"),
            trace_id: format!("trace:session-1:{index}"),
            segment_index: 0,
            segment_count: 1,
            when: WhenBlock {
                started_at_ms: 0,
                duration_ms: 100,
                hour_bucket: HourBucket::Afternoon,
            },
            location: WhereBlock {
                cwd_ref: "E:/zbg/axon-code".to_owned(),
                workspace_kind: "project".to_owned(),
            },
            what: WhatBlock {
                intent_ref: "message:user:1".to_owned(),
                intent_chars: 8,
                intent_digest: "d".to_owned(),
                outcome_ref: "turn:1:assistant".to_owned(),
                outcome_chars: 8,
                outcome_digest: "d".to_owned(),
                final_status: "completed".to_owned(),
            },
            evidence_range: EvidenceRange {
                first_ref: "receipt:1".to_owned(),
                last_ref: "receipt:1".to_owned(),
                count: 1,
                verified_count: 1,
                rejected_count: 0,
            },
            top_evidence: Vec::new(),
            signals: SalienceVector {
                utility: 1_000,
                persistence: 1_000,
                verification: 1_000,
                specificity: 1_000,
                novelty: 1_000,
            },
            batch: BatchBlock {
                window_started_at_ms: 0,
                window_ended_at_ms: 0,
                card_count: 0,
                accumulated_salience: 0,
            },
            hints: Vec::new(),
        }
    }

    fn entry(index: u16, enqueued_at_ms: u64) -> ReflectionPoolEntry {
        ReflectionPoolEntry {
            turn_id: format!("turn-{index}"),
            trace_id: format!("trace:session-1:{index}"),
            enqueued_at_ms,
            card: card(index),
            bucket: String::new(),
            correction_bp: 0,
        }
    }

    #[test]
    fn every_entry_fires_on_its_own() {
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                session_closing: true,
                candidate_count: 1,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::SessionClose)
        );
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                candidate_count: BATCH_MIN_CANDIDATES,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::CountThreshold)
        );
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                accumulated_salience_bp: BATCH_MIN_ACCUMULATED_BP,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::AccumulatedSalience)
        );
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                strongest_card_meets_execution_floor: true,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::SingleStrongCard)
        );
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                strongest_correction_bp: CORRECTION_TRIGGER_BP,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::ExplicitCorrection)
        );
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                oldest_age_ms: BATCH_MAX_AGE_MS,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::PoolAge)
        );
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                is_cold_start: true,
                stale_pool_age_ms: STALE_POOL_MS,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::StalePool)
        );
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                same_cluster_cards: RECURRENCE_MIN_CARDS,
                ..BatchTriggerInput::default()
            }),
            Some(BatchTrigger::RecurrenceCluster)
        );
    }

    #[test]
    fn empty_pool_never_fires_even_on_session_close() {
        assert_eq!(
            evaluate_batch_trigger(&BatchTriggerInput {
                session_closing: true,
                ..BatchTriggerInput::default()
            }),
            None
        );
    }

    #[test]
    fn session_close_wins_over_quantity_entries() {
        let trigger = evaluate_batch_trigger(&BatchTriggerInput {
            session_closing: true,
            candidate_count: BATCH_MIN_CANDIDATES,
            accumulated_salience_bp: BATCH_MIN_ACCUMULATED_BP,
            ..BatchTriggerInput::default()
        });

        assert_eq!(trigger, Some(BatchTrigger::SessionClose));
    }

    #[test]
    fn enqueue_is_idempotent_by_turn_and_trace() {
        let mut pool = ReflectionPool::default();

        assert!(pool.enqueue(entry(1, 1_000)));
        assert!(!pool.enqueue(entry(1, 1_000)));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn drain_keeps_order_and_backfills_batch_block() {
        let mut pool = ReflectionPool::default();
        pool.enqueue(entry(1, 1_000));
        pool.enqueue(entry(2, 2_000));

        let batch = pool.drain_batch(9_000);

        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].card_id, "card-1");
        assert_eq!(batch[0].batch.card_count, 2);
        assert_eq!(batch[0].batch.window_started_at_ms, 1_000);
        assert_eq!(batch[0].batch.window_ended_at_ms, 9_000);
        assert_eq!(batch[0].batch.accumulated_salience, 10_000);
        assert!(pool.is_empty());
    }

    #[test]
    fn drain_always_takes_at_least_one_card() {
        let mut pool = ReflectionPool::default();
        pool.enqueue(entry(1, 0));

        let batch = pool.drain_batch(1_000);

        assert_eq!(batch.len(), 1);
    }

    #[test]
    fn trigger_input_projects_pool_state() {
        let mut pool = ReflectionPool::default();
        pool.enqueue(entry(1, 1_000));
        pool.enqueue(entry(2, 2_000));

        let input = pool.trigger_input(5_000, false, false);

        assert_eq!(input.candidate_count, 2);
        assert_eq!(input.accumulated_salience_bp, 10_000);
        assert_eq!(
            input.strongest_card_meets_execution_floor,
            card_meets_execution_floor(&pool.entries()[0].card.signals),
        );
        assert_eq!(input.oldest_age_ms, 4_000);
        assert!(!input.session_closing);
        assert!(!input.is_cold_start);
    }

    #[test]
    fn buckets_never_cross_workspaces() {
        assert_eq!(
            pool_bucket_key("E:/zbg/axon-code", Some("master")),
            "E:/zbg/axon-code@master"
        );
        assert_eq!(
            pool_bucket_key("C:/Users/<user>/.axon", None),
            "C:/Users/<user>/.axon"
        );
        assert_ne!(
            pool_bucket_key("E:/zbg/axon-code", Some("master")),
            pool_bucket_key("E:/zbg/axon-code", Some("feat/x"))
        );
    }

    #[test]
    fn split_by_bucket_keeps_foreign_entries_out_and_adopts_legacy() {
        let mut mine = entry(1, 1_000);
        mine.bucket = "E:/a".to_owned();
        let mut foreign = entry(2, 2_000);
        foreign.bucket = "E:/b".to_owned();
        let legacy = entry(3, 3_000);

        let (current, others) = split_pool_by_bucket(vec![mine, foreign, legacy], "E:/a");

        assert_eq!(current.len(), 2, "本桶条目与旧条目都应归当前桶");
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].turn_id, "turn-2");
    }

    #[test]
    fn entry_correction_strength_fires_explicit_correction() {
        let mut pool = ReflectionPool::default();
        let mut corrected = entry(1, 0);
        corrected.correction_bp = CORRECTION_TRIGGER_BP;
        assert!(pool.enqueue(corrected));

        assert_eq!(
            evaluate_batch_trigger(&pool.trigger_input(0, false, false)),
            Some(BatchTrigger::ExplicitCorrection),
            "纠正强度必须从队列表述投影进触发输入，否则这条入口永远死着"
        );
    }

    #[test]
    fn single_card_floor_is_derived_from_admission_thresholds() {
        let admission_floor = u32::from(EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS)
            + u32::from(EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS)
            + u32::from(EXECUTION_SPECIFICITY_MIN_BP);

        assert_eq!(
            u32::from(REFLECTION_EXECUTION_FLOOR_BP),
            admission_floor,
            "单卡底线必须等于执行类准入的三维下限之和，改准入没同步改它就该红"
        );
        assert_eq!(SINGLE_CARD_MIN_BP, REFLECTION_EXECUTION_FLOOR_BP);
        assert_eq!(BATCH_MIN_ACCUMULATED_BP, u64::from(SINGLE_CARD_MIN_BP) * 2);
    }
    #[test]
    fn same_location_cards_cluster_into_a_recurrence_trigger() {
        let mut pool = ReflectionPool::default();
        pool.enqueue(entry(1, 0));
        pool.enqueue(entry(2, 0));
        pool.enqueue(entry(3, 0));

        assert_eq!(pool.largest_cluster_size(), 3);
        assert_eq!(
            evaluate_batch_trigger(&pool.trigger_input(0, false, false)),
            Some(BatchTrigger::RecurrenceCluster),
            "同地点同工具链第三次出现必须能自己触发，这条入口不再恒 0"
        );
    }
}
