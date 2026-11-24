//! 索引管理器：按 workspace 维护基础快照与增量覆盖层，并调度后台重建与增量更新。
//!
//! 查询只读取已发布的 `Arc<IndexSnapshot>` 与覆盖层；构建、合并与磁盘写入全部发生在
//! 后台线程，取消信号可以终止尚未完成的任务，任何失败都不会让候选过滤变成漏检。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    builder, candidate_filter,
    candidate_filter::CandidateSelection,
    model::{FileFingerprint, IndexKey, IndexSnapshot, SearchIndexPolicy, relative_path},
    overlay::LiveOverlay,
    planner::build_query_plan,
    storage,
};
use crate::workspace_walker::{WorkspaceWalkEntry, WorkspaceWalkReport};

/// 索引在本轮搜索中的实际作用。
///
/// 该枚举与 `IndexDiagnostics` 只在测试与基准构建中存在：当前 crate 没有可用的日志汇，
/// 生产路径既不采集也不上报诊断，因此不为此付出任何统计开销。
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum IndexOutcome {
    /// 没有可用索引状态，本轮完整扫描。
    #[default]
    Unavailable,
    /// 查询计划无法安全缩小候选，本轮完整扫描。
    MatchAll,
    /// 索引生效，已排除部分文件。
    CandidateFilter,
}

/// 内部诊断指标；只服务于测试与基准，不进入 provider-visible 回执。
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct IndexDiagnostics {
    pub(crate) outcome: IndexOutcome,
    pub(crate) generation: u64,
    /// 本轮 walker 发现的文件条目数。
    pub(crate) entry_count: usize,
    /// 被基础快照或覆盖层有效覆盖的文件数。
    pub(crate) indexed_count: usize,
    /// 被候选过滤安全排除、无需精确读取的文件数。
    pub(crate) excluded_count: usize,
    /// 本轮需要增量更新（重读或标记删除）的文件数。
    pub(crate) stale_count: usize,
}

#[derive(Debug, Default)]
pub(crate) struct SearchIndexDecision {
    pub(crate) selection: CandidateSelection,
    #[cfg(test)]
    pub(crate) diagnostics: IndexDiagnostics,
    update: Option<IndexTask>,
}

impl SearchIndexDecision {
    pub(crate) fn should_scan(&self, path: &Path) -> bool {
        self.selection.should_scan(path)
    }

    pub(crate) fn take_update(self) -> Option<IndexTask> {
        self.update
    }
}

/// 后台索引任务：冷启动或大面积变化时全量重建，小范围变化时只重读变化文件。
#[derive(Debug, Clone)]
pub(crate) enum IndexTask {
    Rebuild {
        key: IndexKey,
        entries: Vec<WorkspaceWalkEntry>,
    },
    Update {
        key: IndexKey,
        upserts: Vec<WorkspaceWalkEntry>,
        removals: Vec<String>,
    },
}

impl IndexTask {
    fn key(&self) -> &IndexKey {
        match self {
            Self::Rebuild { key, .. } | Self::Update { key, .. } => key,
        }
    }
}

/// 单个 workspace 的内存状态：基础快照加增量覆盖层。
#[derive(Debug)]
pub(crate) struct IndexState {
    snapshot: Arc<IndexSnapshot>,
    overlay: Arc<LiveOverlay>,
    last_used_tick: AtomicU64,
}

impl IndexState {
    pub(crate) fn new(snapshot: Arc<IndexSnapshot>, overlay: Arc<LiveOverlay>, tick: u64) -> Self {
        Self {
            snapshot,
            overlay,
            last_used_tick: AtomicU64::new(tick),
        }
    }

    fn touch(&self, tick: u64) {
        self.last_used_tick.store(tick, Ordering::Relaxed);
    }

    /// 当前基础快照的 generation；只用于验证增量更新没有误触发全量换代。
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.snapshot.generation
    }
}

/// 单次查询的索引判定输入。
///
/// 把查询语义与「本轮扫描结束后是否允许调度后台更新」分开表达：显式 `paths` 搜索只覆盖
/// 被点名的文件，不能据此收敛快照，因此只读不写；测试与基准也可以只读不写以获得确定性。
#[derive(Debug, Clone, Copy)]
pub(crate) struct IndexQuery<'a> {
    pub(crate) query: &'a str,
    pub(crate) is_literal: bool,
    pub(crate) case_sensitive: bool,
    pub(crate) hidden: bool,
    pub(crate) gitignore: bool,
    /// 本轮是否允许把重建或增量更新交给后台线程。
    pub(crate) allow_update: bool,
}

#[derive(Debug, Default)]
pub(crate) struct SearchIndexManager {
    states: RwLock<BTreeMap<IndexKey, Arc<IndexState>>>,
    building: Mutex<BTreeMap<IndexKey, Arc<AtomicBool>>>,
    last_build_at: Mutex<BTreeMap<IndexKey, Instant>>,
    next_generation: AtomicU64,
    access_tick: AtomicU64,
}

impl SearchIndexManager {
    pub(crate) fn prepare(
        &self,
        root: &Path,
        walk: &WorkspaceWalkReport,
        query: IndexQuery<'_>,
    ) -> SearchIndexDecision {
        let IndexQuery {
            query: text,
            is_literal,
            case_sensitive,
            hidden,
            gitignore,
            allow_update,
        } = query;
        let key = IndexKey::new(root, hidden, gitignore);
        let policy = SearchIndexPolicy::default();
        let plan = build_query_plan(text, is_literal, case_sensitive);
        let entry_count = walk.entries.iter().filter(|entry| !entry.is_dir).count();

        let Some(state) = self.state_for(&key) else {
            let should_build = allow_update && policy.should_build(entry_count);
            return SearchIndexDecision {
                selection: CandidateSelection::default(),
                #[cfg(test)]
                diagnostics: IndexDiagnostics {
                    outcome: IndexOutcome::Unavailable,
                    entry_count,
                    ..IndexDiagnostics::default()
                },
                update: should_build.then(|| IndexTask::Rebuild {
                    key,
                    entries: walk.entries.clone(),
                }),
            };
        };

        let selection = candidate_filter::select_candidates(
            &state.snapshot,
            &state.overlay,
            &key.root,
            &walk.entries,
            &plan,
        );
        let (upserts, removals) = stale_files(&state, &walk.entries, &key.root);
        let stale_count = upserts.len().saturating_add(removals.len());

        let update = if !allow_update || stale_count == 0 {
            None
        } else if stale_count > policy.max_overlay_files {
            // 变化规模超过覆盖层预算时改为整体重建，避免增量层无限膨胀。
            Some(IndexTask::Rebuild {
                key,
                entries: walk.entries.clone(),
            })
        } else {
            Some(IndexTask::Update {
                key,
                upserts,
                removals,
            })
        };

        #[cfg(test)]
        let diagnostics = {
            let excluded_count = selection.excluded_count();
            let outcome = if excluded_count == 0 {
                IndexOutcome::MatchAll
            } else {
                IndexOutcome::CandidateFilter
            };
            IndexDiagnostics {
                outcome,
                generation: state.snapshot.generation,
                entry_count,
                indexed_count: entry_count.saturating_sub(stale_count),
                excluded_count,
                stale_count,
            }
        };

        SearchIndexDecision {
            selection,
            #[cfg(test)]
            diagnostics,
            update,
        }
    }

    pub(crate) fn schedule(self: &Arc<Self>, task: Option<IndexTask>) {
        let Some(task) = task else {
            return;
        };
        let policy = SearchIndexPolicy::default();
        let key = task.key().clone();
        let Some(cancel) = self.begin_build(&key, policy.rebuild_cooldown) else {
            return;
        };

        let manager = Arc::clone(self);
        let spawn_result = thread::Builder::new()
            .name("axon-search-index-build".to_owned())
            .spawn(move || manager.run_task(task, policy, cancel));
        if spawn_result.is_err() {
            self.finish_build(&key);
        }
    }

    fn run_task(
        self: &Arc<Self>,
        task: IndexTask,
        policy: SearchIndexPolicy,
        cancel: Arc<AtomicBool>,
    ) {
        match task {
            IndexTask::Rebuild { key, entries } => self.run_rebuild(key, &entries, policy, &cancel),
            IndexTask::Update {
                key,
                upserts,
                removals,
            } => self.run_update(key, &upserts, &removals, policy, &cancel),
        }
    }

    /// 同步执行一次索引任务；只服务于测试与基准，避免后台线程带来的不确定性。
    #[cfg(test)]
    pub(crate) fn run_task_now(self: &Arc<Self>, task: IndexTask) {
        self.run_task(
            task,
            SearchIndexPolicy::default(),
            Arc::new(AtomicBool::new(false)),
        );
    }

    pub(crate) fn run_rebuild(
        self: &Arc<Self>,
        key: IndexKey,
        entries: &[WorkspaceWalkEntry],
        policy: SearchIndexPolicy,
        cancel: &Arc<AtomicBool>,
    ) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let Some(snapshot) = builder::build_snapshot(&key, entries, generation, policy, cancel)
        else {
            self.finish_build(&key);
            return;
        };
        let snapshot = Arc::new(snapshot);
        let _ = storage::persist(&snapshot, &key);
        self.install(key.clone(), snapshot, Arc::new(LiveOverlay::default()));
        self.finish_build(&key);
    }

    fn run_update(
        self: &Arc<Self>,
        key: IndexKey,
        upserts: &[WorkspaceWalkEntry],
        removals: &[String],
        policy: SearchIndexPolicy,
        cancel: &Arc<AtomicBool>,
    ) {
        let Some(state) = self.cached_state(&key) else {
            // 状态已被淘汰：放弃本次增量，下一轮搜索会重新检测并按需重建。
            self.finish_build(&key);
            return;
        };

        let mut overlay = (*state.overlay).clone();
        for entry in upserts {
            if cancel.load(Ordering::Relaxed) {
                self.finish_build(&key);
                return;
            }
            overlay.ingest(&key.root, entry, policy);
        }
        for path in removals {
            overlay.remove(path);
        }

        if overlay.exceeds_budget(policy) {
            let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
            let merged = Arc::new(overlay.merge_into(&state.snapshot, generation));
            let _ = storage::persist(&merged, &key);
            self.install(key.clone(), merged, Arc::new(LiveOverlay::default()));
        } else {
            self.install(key.clone(), Arc::clone(&state.snapshot), Arc::new(overlay));
        }
        self.finish_build(&key);
    }

    fn state_for(&self, key: &IndexKey) -> Option<Arc<IndexState>> {
        if let Some(state) = self.cached_state(key) {
            state.touch(self.access_tick.fetch_add(1, Ordering::Relaxed));
            return Some(state);
        }

        // storage::load 已在解码阶段完成结构校验与派生路径重建。
        let snapshot = storage::load(key)?;
        let state = Arc::new(IndexState::new(
            Arc::new(snapshot),
            Arc::new(LiveOverlay::default()),
            self.access_tick.fetch_add(1, Ordering::Relaxed),
        ));
        self.insert_state(key.clone(), Arc::clone(&state));
        Some(state)
    }

    pub(crate) fn cached_state(&self, key: &IndexKey) -> Option<Arc<IndexState>> {
        self.states
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    /// 写入 workspace 状态；超过保留上限时按最近使用淘汰，并取消被淘汰 workspace 的后台任务。
    pub(crate) fn insert_state(&self, key: IndexKey, state: Arc<IndexState>) {
        let policy = SearchIndexPolicy::default();
        let mut victims = Vec::new();
        {
            let mut states = self
                .states
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            states.insert(key.clone(), state);
            while states.len() > policy.max_tracked_workspaces {
                let mut victim: Option<(IndexKey, u64)> = None;
                for (candidate, tracked) in states.iter() {
                    if candidate == &key {
                        continue;
                    }
                    let tick = tracked.last_used_tick.load(Ordering::Relaxed);
                    if victim.as_ref().is_none_or(|(_, best)| tick < *best) {
                        victim = Some((candidate.clone(), tick));
                    }
                }
                let Some((victim_key, _)) = victim else {
                    break;
                };
                states.remove(&victim_key);
                victims.push(victim_key);
            }
        }
        for victim in victims {
            self.cancel_build(&victim);
        }
    }

    pub(crate) fn install(
        &self,
        key: IndexKey,
        snapshot: Arc<IndexSnapshot>,
        overlay: Arc<LiveOverlay>,
    ) {
        let tick = self.access_tick.fetch_add(1, Ordering::Relaxed);
        self.insert_state(key, Arc::new(IndexState::new(snapshot, overlay, tick)));
    }

    /// 登记一次后台任务；同一 key 已在运行或处于冷却窗口内时返回 `None`。
    fn begin_build(&self, key: &IndexKey, cooldown: Duration) -> Option<Arc<AtomicBool>> {
        let now = Instant::now();
        let mut building = self
            .building
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if building.contains_key(key) {
            return None;
        }
        let mut last_build_at = self
            .last_build_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !rebuild_allowed(last_build_at.get(key).copied(), cooldown, now) {
            return None;
        }
        last_build_at.insert(key.clone(), now);
        let cancel = Arc::new(AtomicBool::new(false));
        building.insert(key.clone(), Arc::clone(&cancel));
        Some(cancel)
    }

    fn finish_build(&self, key: &IndexKey) {
        self.building
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
    }

    fn cancel_build(&self, key: &IndexKey) {
        let cancel = self
            .building
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .map(Arc::clone);
        if let Some(cancel) = cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(crate) fn active_task_count(&self) -> usize {
        self.building
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 当前常驻内存的 workspace 状态数；只用于验证 LRU 淘汰边界。
    #[cfg(test)]
    pub(crate) fn tracked_state_count(&self) -> usize {
        self.states
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

/// 找出本轮 walker 观察到、但基础快照与覆盖层都无法可信覆盖的文件，以及已经消失的文件。
pub(crate) fn stale_files(
    state: &IndexState,
    entries: &[WorkspaceWalkEntry],
    root: &Path,
) -> (Vec<WorkspaceWalkEntry>, Vec<String>) {
    let mut seen = BTreeSet::new();
    let mut upserts = Vec::new();
    for entry in entries.iter().filter(|entry| !entry.is_dir) {
        let Some(relative) = relative_path(root, &entry.path) else {
            continue;
        };
        seen.insert(relative.clone());
        let fingerprint = FileFingerprint {
            length: entry.file_len,
            modified_at_ms: entry.modified_at_ms,
        };
        let covered = state
            .overlay
            .effective_entry(&relative, &fingerprint)
            .is_some()
            || (state.overlay.entry(&relative).is_none()
                && !state.overlay.is_removed(&relative)
                && state
                    .snapshot
                    .files
                    .get(&relative)
                    .is_some_and(|file| file.fingerprint == fingerprint));
        if !covered {
            upserts.push(entry.clone());
        }
    }

    let mut removals = Vec::new();
    for path in state
        .snapshot
        .files
        .keys()
        .chain(state.overlay.entry_paths())
    {
        if !seen.contains(path) && !state.overlay.is_removed(path) {
            removals.push(path.clone());
        }
    }
    removals.sort();
    removals.dedup();
    (upserts, removals)
}

/// 冷却窗口内不重复启动同一 workspace 的后台构建。
pub(crate) fn rebuild_allowed(
    last_started: Option<Instant>,
    cooldown: Duration,
    now: Instant,
) -> bool {
    last_started.is_none_or(|last| now.duration_since(last) >= cooldown)
}
