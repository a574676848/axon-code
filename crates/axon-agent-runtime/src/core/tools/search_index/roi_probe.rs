//! 端到端 ROI 探针（默认 `#[ignore]`，按需手动执行）。
//!
//! 在真实仓库 workspace 上对比「完整扫描」与「候选索引 + 精确扫描」的端到端 p50/p95，
//! 并给出首次构建成本、候选缩小率与回本次数，作为发布门禁的实测入口。
//! 因为它依赖仓库自身内容而不是临时夹具，所以不进入常规测试门禁：
//!
//! ```powershell
//! cargo test --release -p axon-agent-runtime --lib roi_probe -- --ignored --nocapture
//! ```

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use super::{
    builder,
    manager::SearchIndexManager,
    model::{IndexKey, SearchIndexPolicy},
    overlay::LiveOverlay,
};
use crate::{
    builtin_search_engine::scan_workspace_with_index,
    builtin_search_tool::{RegexSearchMode, SearchToolExecutionInput},
    workspace_walker::{
        WorkspaceWalkDefaultScope, WorkspaceWalkEntry, WorkspaceWalkInclude, WorkspaceWalkInput,
        WorkspaceWalker,
    },
};

/// 探针查询：大小写敏感的单行字面量且不短于 3 字节，才能进入候选过滤。
const ROI_QUERY: &str = "search_index";
/// 两条路径各自重复的次数。
const ROI_ROUNDS: usize = 5;
/// 分位计算使用的百分比刻度。
const ROI_PERCENT_SCALE: usize = 100;
const ROI_P95_PERCENT: usize = 95;
/// 字节到 KiB 的换算基数。
const BYTES_PER_KIB: usize = 1024;
/// 探针只安装一份快照，代际固定为 1。
const ROI_SNAPSHOT_GENERATION: u64 = 1;

fn repository_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| manifest.to_path_buf(), Path::to_path_buf)
}

fn probe_input(root: &Path) -> SearchToolExecutionInput {
    SearchToolExecutionInput {
        query: ROI_QUERY.to_owned(),
        root: root.to_path_buf(),
        snapshot_session: None,
        paths: Vec::new(),
        regex_mode: RegexSearchMode::Literal,
        case_sensitive: true,
        gitignore: true,
        hidden: false,
        context_before: 0,
        context_after: 0,
        file_offset: 0,
        max_files: None,
        max_matches_per_file: None,
        cancel_token: None,
        remote_requested: false,
    }
}

fn walk_entries(root: &Path) -> Vec<WorkspaceWalkEntry> {
    WorkspaceWalker::walk(WorkspaceWalkInput {
        root: root.to_path_buf(),
        paths: Vec::new(),
        hidden: false,
        gitignore: true,
        include: WorkspaceWalkInclude::Files,
        default_scope: WorkspaceWalkDefaultScope::RecursiveFiles,
        cancel_token: None,
    })
    .expect("walker 必须成功")
    .entries
}

fn percentile(samples: &mut [Duration], percent: usize) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    samples.sort_unstable();
    let rank = samples
        .len()
        .saturating_mul(percent)
        .saturating_div(ROI_PERCENT_SCALE);
    samples[rank.min(samples.len().saturating_sub(1))]
}

fn scan(
    input: &SearchToolExecutionInput,
    root: &Path,
    manager: &Arc<SearchIndexManager>,
    allow_index_build: bool,
) -> (Duration, usize, usize) {
    let started = Instant::now();
    let summary = scan_workspace_with_index(input, root, manager, allow_index_build)
        .expect("探针扫描必须成功");
    (
        started.elapsed(),
        summary.files_scanned,
        summary.files.len(),
    )
}

#[test]
#[ignore = "依赖真实仓库内容，按需手动执行"]
fn roi_probe_on_repository_workspace() {
    let root = repository_root();
    let input = probe_input(&root);

    let mut full_samples = Vec::with_capacity(ROI_ROUNDS);
    let mut indexed_samples = Vec::with_capacity(ROI_ROUNDS);
    let mut full_scanned = 0usize;
    let mut indexed_scanned = 0usize;
    let mut matched_files = 0usize;

    let cold = Arc::new(SearchIndexManager::default());
    for _ in 0..ROI_ROUNDS {
        let (elapsed, scanned, matched) = scan(&input, &root, &cold, false);
        full_samples.push(elapsed);
        full_scanned = scanned;
        matched_files = matched;
    }

    // 首次构建：与本轮 walker 共用同一份 entries，不额外读取目录树。
    let build_started = Instant::now();
    let entries = walk_entries(&root);
    let key = IndexKey::new(&root, false, true);
    let snapshot = builder::build_snapshot(
        &key,
        &entries,
        ROI_SNAPSHOT_GENERATION,
        SearchIndexPolicy::default(),
        &AtomicBool::new(false),
    )
    .expect("未取消时必须产出快照");
    let build_elapsed = build_started.elapsed();
    // 持久化格式是 checksum 头部加 payload，序列化长度即为索引文件字节数。
    let index_bytes = serde_json::to_vec(&snapshot).map_or(0, |payload| payload.len());

    let manager = Arc::new(SearchIndexManager::default());
    manager.install(key, Arc::new(snapshot), Arc::new(LiveOverlay::default()));
    for _ in 0..ROI_ROUNDS {
        let (elapsed, scanned, _) = scan(&input, &root, &manager, false);
        indexed_samples.push(elapsed);
        indexed_scanned = scanned;
    }

    let full_p50 = percentile(&mut full_samples, ROI_PERCENT_SCALE / 2);
    let full_p95 = percentile(&mut full_samples, ROI_P95_PERCENT);
    let indexed_p50 = percentile(&mut indexed_samples, ROI_PERCENT_SCALE / 2);
    let indexed_p95 = percentile(&mut indexed_samples, ROI_P95_PERCENT);
    let saved = full_p50.saturating_sub(indexed_p50);

    println!("== ROI 探针 ==");
    println!("workspace: {}", root.display());
    println!("query: {ROI_QUERY}");
    println!(
        "entries: {} 个文本文件，命中 {matched_files} 个",
        entries.len()
    );
    println!(
        "完整扫描: p50 {:?} / p95 {:?}，读取 {full_scanned} 个文件",
        full_p50, full_p95
    );
    println!(
        "候选索引: p50 {:?} / p95 {:?}，读取 {indexed_scanned} 个文件",
        indexed_p50, indexed_p95
    );
    println!("首次构建（含 walk）: {build_elapsed:?}");
    println!(
        "索引磁盘占用: {:.1} KiB",
        index_bytes as f64 / BYTES_PER_KIB as f64
    );
    println!(
        "候选缩小率: {:.1}%",
        ROI_PERCENT_SCALE as f64 * (1.0 - ratio(indexed_scanned, full_scanned))
    );
    if saved.is_zero() {
        println!("回本次数: 无可证明的时间 ROI");
    } else {
        println!(
            "回本次数 ≈ {:.1}",
            build_elapsed.as_secs_f64() / saved.as_secs_f64()
        );
    }
}

fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    part as f64 / whole as f64
}
