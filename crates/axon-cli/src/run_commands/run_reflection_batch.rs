//! 反思卡片的投影、入池与批处理触发判定。
//!
//! 卡片只是索引：它带证据区间与显著度，不带正文。入池后由纯函数判定「现在跑不跑」，
//! 未触发的卡片留在池里等下一次治理，不会每轮都起一个子代理。

use axon_agent_runtime::{
    BatchBlock, ReflectionCard, ReflectionLedgerRuntime, ReflectionPool, ReflectionPoolEntry,
    ReflectionPoolLock, ReflectionPoolStoreRuntime, ReflectionTier, SalienceVector,
    SemanticSalienceAssessment, TurnOutcomeEnvelope, TurnTrace, WhereBlock, evaluate_batch_trigger,
    pool_bucket_key, split_pool_by_bucket, take_cold_start,
};

use super::{RunRequest, run_execution::now_ms};

/// 批处理准入结果。
///
/// 三态必须分清：`Deferred` 是「卡片已入池、等下一次触发」，
/// 与「本轮压根不该反思」（`NotApplicable`）在账面上必须能区分开，
/// 否则待跑与判否又会被合并成同一个 attempts=0。
pub(super) enum BatchAdmission {
    /// 立即跑这一批；可能是本轮一张，也可能是池里攒下的多张。
    Run(Vec<ReflectionCard>),
    /// 卡片已入池，等下一次治理或惰性补漏。
    Deferred,
    /// 没有卡片、没有评估，或档位判定为 Skip。
    NotApplicable,
}

/// 把本轮轨迹与显著度投影成反思索引卡。
///
/// 没有语义评估就没有显著度，凭空的卡片只会污染批次，因此直接返回空。
pub(super) fn reflection_cards(
    trace: &TurnTrace,
    assessment: Option<&SemanticSalienceAssessment>,
    cwd: &std::path::Path,
) -> Vec<ReflectionCard> {
    let Some(assessment) = assessment else {
        return Vec::new();
    };
    let signals: SalienceVector = assessment
        .signals
        .to_salience_vector(assessment.schema_version);
    let location = WhereBlock {
        cwd_ref: cwd.to_string_lossy().into_owned(),
        workspace_kind: workspace_kind(cwd).to_owned(),
    };
    let batch = BatchBlock {
        window_started_at_ms: trace.recorded_at_ms,
        window_ended_at_ms: trace.recorded_at_ms,
        card_count: 1,
        accumulated_salience: signals.total(),
    };
    ReflectionCard::from_trace(trace, signals, location, 0, batch)
}

/// 工作区类别：Home 单独一桶，含 `.git` 的算项目，其余归为 other。
///
/// 只做一次目录探测，不读文件内容，避免后置流程产生额外 IO 抖动。
fn workspace_kind(cwd: &std::path::Path) -> &'static str {
    if cwd.ends_with(".axon") {
        return "home";
    }
    if cwd.join(".git").exists() {
        return "project";
    }
    "other"
}

/// 读当前目录的 git 分支名；非 git 目录、detached HEAD、worktree 都返回 `None`。
///
/// 只读 `.git/HEAD` 这一个文件：桶键是热路径上的一次计算，为它拉一个 git 子进程
/// 不值得。读不到就按目录兜底：宁可少分一个桶，也不要猜一个错分支。
fn current_git_branch(cwd: &std::path::Path) -> Option<String> {
    let head = std::fs::read_to_string(cwd.join(".git").join("HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?.trim();
    (!branch.is_empty()).then(|| branch.to_owned())
}

/// 把本轮卡片放进反思池，并按触发入口判定是否现在就跑一批。
///
/// 返回 `Run` 表示本次要跑：可能是本轮这一张，也可能是池里攒下的多张。
/// 返回 `Deferred` 表示卡片已入池但未触发，留到下一次治理或惰性补漏。
///
/// 兜底优先级明确：解析不到运行态根、写盘失败时退回单轮卡片，
/// 宁可多跑一次反思，也不让存储故障把反思线整条停掉。
pub(super) fn admit_reflection_batch(
    request: &RunRequest,
    trace: &TurnTrace,
    cards: Vec<ReflectionCard>,
    assessment: Option<&SemanticSalienceAssessment>,
    envelope: &TurnOutcomeEnvelope,
) -> BatchAdmission {
    let Some(assessment) = assessment else {
        return BatchAdmission::NotApplicable;
    };
    // 档位判 Skip 表示「本轮不值得为此起一次反思」，但这不等于「池里的旧卡片可以
    // 一直烂着」：补漏入口专门处理这种情况，所以 Skip 不能在这里提前 return。
    let tier_skip = assessment.reflection_tier(envelope) == ReflectionTier::Skip;
    if cards.is_empty() && tier_skip {
        return BatchAdmission::NotApplicable;
    }
    let Ok(root) = super::run_session_store::resolved_axon_root(&request.config_path) else {
        return BatchAdmission::Run(cards);
    };
    let path = ReflectionPoolStoreRuntime::resolve_store_path(&root);
    // 池文件跨进程互斥：每轮治理都派生独立 worker，共用同一个池文件，
    // 无锁的 load → append → rewrite 会让两个 worker 互相吞卡。
    let Some(_lock) = ReflectionPoolLock::acquire(&path) else {
        axon_logger::warning("反思池被另一个 worker 占用，本轮推迟");
        return BatchAdmission::Deferred;
    };
    let now = now_ms();
    let is_cold_start = take_cold_start();
    // 上一轮若在反思之前被杀，这批卡片已经从池里取出、还没跑完；这里原样放回队列，
    // 避免整批静默丢失。只恢复一次：`take_inflight` 读到就删。
    let inflight_path = ReflectionLedgerRuntime::inflight_path(&root);
    for entry in ReflectionLedgerRuntime::take_inflight(&inflight_path) {
        if let Err(error) = ReflectionPoolStoreRuntime::append_entry(&path, &entry) {
            axon_logger::warning(&format!("反思池恢复未完成批次失败: {error:?}"));
        }
    }
    // 池文件是全局一个，卡片必须带地点桶键：否则 A 项目的待跑线索会在 B 项目里
    // 被一起 drain 出来反思，写进 B 的 Memory 作用域，成就跨项目串味。
    let branch = current_git_branch(&request.cwd);
    let bucket = pool_bucket_key(&request.cwd.to_string_lossy(), branch.as_deref());
    if !tier_skip {
        for card in &cards {
            let entry = ReflectionPoolEntry {
                correction_bp: assessment.signals.correction_strength,
                turn_id: trace.turn_id.clone(),
                trace_id: card.trace_id.clone(),
                enqueued_at_ms: now,
                card: card.clone(),
                bucket: bucket.clone(),
            };
            if let Err(error) = ReflectionPoolStoreRuntime::append_entry(&path, &entry) {
                axon_logger::warning(&format!("反思卡片入池失败: {error:?}"));
            }
        }
    }
    // 只把本桶条目装进判定用的池；空桶键是旧版本写入的条目，按本桶兼容。
    let (mine, others) =
        split_pool_by_bucket(ReflectionPoolStoreRuntime::load_entries(&path), &bucket);
    let mut pool = ReflectionPool::default();
    for entry in mine {
        pool.enqueue(entry);
    }
    let trigger = evaluate_batch_trigger(&pool.trigger_input(now, false, is_cold_start));
    // Skip 轮次本来不该跑：只要有一个入口命中（池龄、冷启动补漏、条数…）就兜底跑，
    // 一个都没命中才判「本轮不反思」。否则池里的卡片会在 Skip 轮次上永久搁浅。
    if tier_skip && trigger.is_none() {
        return BatchAdmission::NotApplicable;
    }
    if trigger.is_none() {
        return BatchAdmission::Deferred;
    }
    let before: Vec<ReflectionPoolEntry> = pool.entries().to_vec();
    let batch = pool.drain_batch(now);
    if batch.is_empty() {
        return BatchAdmission::Deferred;
    }
    // 回写时把别的桶原样并回去，只替换本桶剩余队列。
    let remaining: Vec<ReflectionPoolEntry> = others
        .into_iter()
        .chain(pool.entries().iter().cloned())
        .collect();
    // 先记 inflight 再回写池：反过来的话，崩溃在两步之间会让这批卡片既不在池里
    // 也不在 inflight 里，整批静默消失。先记则最坏是多恢复一次，由幂等键去重。
    let drained = before.len().saturating_sub(pool.entries().len());
    let inflight_entries: Vec<ReflectionPoolEntry> = before.into_iter().take(drained).collect();
    if let Err(error) = ReflectionLedgerRuntime::write_inflight(&inflight_path, &inflight_entries) {
        axon_logger::warning(&format!("反思批次 inflight 写入失败，本轮放弃: {error}"));
        return BatchAdmission::Deferred;
    }
    // 回写失败必须放弃本轮：盘上仍是旧队列，若照跑这批，下一轮会把同一批再取一次，
    // 等于重复反思与重复写入。宁可等到下一轮重试，也不重复产出记忆。
    if let Err(error) = ReflectionPoolStoreRuntime::rewrite_entries(&path, &remaining) {
        axon_logger::warning(&format!("反思池回写失败，本轮放弃执行以免重放: {error:?}"));
        return BatchAdmission::Deferred;
    }
    BatchAdmission::Run(batch)
}
