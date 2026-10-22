//! P0 差分 harness：在同一个临时 workspace 上对比完整扫描与候选索引路径。
//!
//! 对照规则：除 `files_scanned`
//! 之外，匹配文件、行号、文本、span、上下文、截断、状态、诊断路径和分页 token 都必须逐字段相等。
//! `files_scanned` 只统计真实进入 `grep-searcher` 的文件，索引跳过文件属于有意差异。

use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use super::{
    builder,
    manager::{IndexOutcome, IndexQuery, IndexTask, SearchIndexDecision, SearchIndexManager},
    model::{IndexKey, SearchIndexPolicy},
    overlay::LiveOverlay,
};
use crate::{
    builtin_search_engine::{SearchExecutionSummary, scan_workspace_with_index},
    builtin_search_tool::{RegexSearchMode, SearchToolExecutionInput, SearchToolFile},
    workspace_walker::{
        WorkspaceWalkDefaultScope, WorkspaceWalkEntry, WorkspaceWalkInclude, WorkspaceWalkInput,
        WorkspaceWalkReport, WorkspaceWalker,
    },
};

/// 夹具文件数：必须明显大于命中文件数，才能证明候选过滤真的排除了文件。
const DIFFERENTIAL_WORKSPACE_FILES: usize = 24;
/// 命中文件数：f00、f01、f02 三个文件包含查询词。
const DIFFERENTIAL_MATCHING_FILES: usize = 3;
const DIFFERENTIAL_QUERY: &str = "needle_xyz";
const DIFFERENTIAL_UNICODE_QUERY: &str = "ÄBC";

/// 随机差分轮数：每轮生成一个独立临时 workspace。
const FUZZ_ROUNDS: usize = 24;
/// 固定随机种子：任何失败都能原样复现，不使用外部随机源以保持确定性。
const FUZZ_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
/// 单轮 workspace 的文件数下限与上限。
const FUZZ_MIN_FILES: usize = 6;
const FUZZ_MAX_FILES: usize = 40;
/// 单轮每个文件随机片段数量的下限与上限。
const FUZZ_MIN_FRAGMENTS: usize = 8;
const FUZZ_MAX_FRAGMENTS: usize = 120;
/// 查询至少包含一个完整 trigram 才能进入候选过滤。
const FUZZ_MIN_QUERY_BYTES: usize = 3;
/// 生成内容使用的字母表：覆盖 ASCII、下划线、多字节 UTF-8、空白与正则元字符。
const FUZZ_ALPHABET: [&str; 10] = ["a", "b", "z", "9", "_", "中", "ä", "\n", " ", "."];
/// 注入查询词：保证随机 workspace 中确实存在可供对照的命中文件。
const FUZZ_PLANTED_TOKENS: [&str; 4] = ["needle_xyz", "中文片段", "abc_xyz", "ZZ9_token"];
/// 编辑阶段的文件数下限与上限。
const FUZZ_MIN_EDITS: usize = 1;
const FUZZ_MAX_EDITS: usize = 3;

/// 一次扫描的完整输出，用于两条路径逐字段对照；`files_scanned` 单独断言。
#[derive(Debug, PartialEq, Eq)]
struct ScanOutcome {
    files: Vec<SearchToolFile>,
    files_scanned: usize,
    next_file_offset: Option<usize>,
    missing_paths: Vec<String>,
    binary_paths: Vec<String>,
    unreadable_paths: Vec<String>,
    incomplete: bool,
    symlinks_skipped: usize,
    root_escapes_skipped: usize,
}

impl ScanOutcome {
    fn from_summary(summary: SearchExecutionSummary) -> Self {
        Self {
            files: summary.files,
            files_scanned: summary.files_scanned,
            next_file_offset: summary.next_file_offset,
            missing_paths: summary.missing_paths,
            binary_paths: summary.binary_paths,
            unreadable_paths: summary.unreadable_paths,
            incomplete: summary.incomplete,
            symlinks_skipped: summary.symlinks_skipped,
            root_escapes_skipped: summary.root_escapes_skipped,
        }
    }

    /// 除 `files_scanned` 外逐字段相等。
    fn assert_same_results(&self, oracle: &Self) {
        assert_eq!(
            self.files, oracle.files,
            "匹配文件、行、span 与 hashline 必须与完整扫描一致"
        );
        assert_eq!(
            self.next_file_offset, oracle.next_file_offset,
            "分页 continuation 必须与完整扫描一致"
        );
        assert_eq!(
            self.missing_paths, oracle.missing_paths,
            "missing 诊断必须与完整扫描一致"
        );
        assert_eq!(
            self.binary_paths, oracle.binary_paths,
            "binary 诊断必须与完整扫描一致"
        );
        assert_eq!(
            self.unreadable_paths, oracle.unreadable_paths,
            "unreadable 诊断必须与完整扫描一致"
        );
        assert_eq!(self.incomplete, oracle.incomplete, "incomplete 必须一致");
        assert_eq!(self.symlinks_skipped, oracle.symlinks_skipped);
        assert_eq!(self.root_escapes_skipped, oracle.root_escapes_skipped);
    }
}

/// 24 个文本文件：f00 有 3 处命中、f01/f02 各 1 处，其余只含公共文本。
fn literal_workspace() -> axon_test_support::TestTempDir {
    let directory = axon_test_support::test_temp_dir();
    for index in 0..DIFFERENTIAL_WORKSPACE_FILES {
        let content = match index {
            0 => format!(
                "{DIFFERENTIAL_QUERY}\nfiller\n{DIFFERENTIAL_QUERY}\n{DIFFERENTIAL_QUERY}\n"
            ),
            1 | 2 => format!("filler {index}\n{DIFFERENTIAL_QUERY}\n"),
            _ => format!("filler {index}\n"),
        };
        fs::write(directory.path().join(format!("f{index:02}.txt")), content)
            .expect("夹具文件必须可写");
    }
    directory
}

fn input(
    root: &Path,
    query: &str,
    mode: RegexSearchMode,
    case_sensitive: bool,
) -> SearchToolExecutionInput {
    SearchToolExecutionInput {
        query: query.to_owned(),
        root: root.to_path_buf(),
        snapshot_session: None,
        paths: Vec::new(),
        regex_mode: mode,
        case_sensitive,
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

fn walk_report(root: &Path) -> WorkspaceWalkReport {
    WorkspaceWalkReport {
        entries: walk_entries(root),
        ..WorkspaceWalkReport::default()
    }
}

/// 用真实 builder 构建并安装一份同代快照；不落盘，避免测试污染运行态索引目录。
fn warm_index(manager: &Arc<SearchIndexManager>, root: &Path) -> IndexKey {
    let key = IndexKey::new(root, false, true);
    let entries = walk_entries(root);
    let snapshot = builder::build_snapshot(
        &key,
        &entries,
        1,
        SearchIndexPolicy::default(),
        &AtomicBool::new(false),
    )
    .expect("未取消时必须产出快照");
    manager.install(
        key.clone(),
        Arc::new(snapshot),
        Arc::new(LiveOverlay::default()),
    );
    key
}

fn prepare(
    manager: &Arc<SearchIndexManager>,
    root: &Path,
    query: &str,
    is_literal: bool,
) -> SearchIndexDecision {
    manager.prepare(
        root,
        &walk_report(root),
        IndexQuery {
            query,
            is_literal,
            case_sensitive: true,
            hidden: false,
            gitignore: true,
            // `prepare` 只做判定，不启动线程；更新由用例通过 `run_task_now` 显式执行。
            allow_update: true,
        },
    )
}

/// 完整扫描 oracle：空管理器 + 禁止后台构建，完全不使用候选索引。
fn full_scan(input: &SearchToolExecutionInput, root: &Path) -> ScanOutcome {
    let manager = Arc::new(SearchIndexManager::default());
    let summary =
        scan_workspace_with_index(input, root, &manager, false).expect("完整扫描必须成功");
    ScanOutcome::from_summary(summary)
}

fn indexed_scan(
    input: &SearchToolExecutionInput,
    root: &Path,
    manager: &Arc<SearchIndexManager>,
) -> ScanOutcome {
    // 禁止后台构建：候选过滤照常生效，但更新由用例通过 `prepare` + `run_task_now` 显式驱动，
    // 避免测试与后台线程竞争造成不确定结果。
    let summary =
        scan_workspace_with_index(input, root, manager, false).expect("索引路径扫描必须成功");
    ScanOutcome::from_summary(summary)
}

#[test]
fn indexed_literal_scan_matches_full_scan_and_skips_files() {
    let workspace = literal_workspace();
    let root = workspace.path();
    let input = input(root, DIFFERENTIAL_QUERY, RegexSearchMode::Literal, true);
    let oracle = full_scan(&input, root);

    let manager = Arc::new(SearchIndexManager::default());
    warm_index(&manager, root);
    let decision = prepare(&manager, root, DIFFERENTIAL_QUERY, true);
    let actual = indexed_scan(&input, root, &manager);

    actual.assert_same_results(&oracle);
    assert_eq!(actual.files.len(), DIFFERENTIAL_MATCHING_FILES);
    assert_eq!(decision.diagnostics.outcome, IndexOutcome::CandidateFilter);
    assert_eq!(
        decision.diagnostics.excluded_count,
        DIFFERENTIAL_WORKSPACE_FILES - DIFFERENTIAL_MATCHING_FILES
    );
    assert!(
        actual.files_scanned < oracle.files_scanned,
        "索引路径必须少读文件"
    );
}

#[test]
fn regex_and_case_variants_fall_back_without_changing_results() {
    let workspace = literal_workspace();
    let root = workspace.path();
    let cases = [
        (RegexSearchMode::Regex, "needle_[a-z]+", true, true),
        (RegexSearchMode::Literal, "NEEDLE_XYZ", false, true),
        (
            RegexSearchMode::FixedString,
            DIFFERENTIAL_QUERY,
            true,
            false,
        ),
    ];

    for (mode, query, case_sensitive, expect_full_scan) in cases {
        let input = input(root, query, mode, case_sensitive);
        let oracle = full_scan(&input, root);

        let manager = Arc::new(SearchIndexManager::default());
        warm_index(&manager, root);
        let actual = indexed_scan(&input, root, &manager);

        actual.assert_same_results(&oracle);
        if expect_full_scan {
            assert_eq!(
                oracle.files_scanned, actual.files_scanned,
                "{query} 无法安全缩小候选时必须逐文件完整扫描"
            );
        } else {
            assert!(
                actual.files_scanned < oracle.files_scanned,
                "{query} 是字面量，必须走候选过滤"
            );
        }
    }
}

#[test]
fn unicode_case_insensitive_query_keeps_full_scan_results() {
    let directory = axon_test_support::test_temp_dir();
    fs::write(directory.path().join("a.txt"), "äbc\n").expect("夹具文件必须可写");
    fs::write(directory.path().join("b.txt"), "other\n").expect("夹具文件必须可写");
    let root = directory.path();
    let input = input(
        root,
        DIFFERENTIAL_UNICODE_QUERY,
        RegexSearchMode::Literal,
        false,
    );
    let oracle = full_scan(&input, root);

    let manager = Arc::new(SearchIndexManager::default());
    warm_index(&manager, root);
    let actual = indexed_scan(&input, root, &manager);

    actual.assert_same_results(&oracle);
    assert_eq!(oracle.files.len(), 1, "Unicode 大小写折叠必须仍然命中");
}

#[test]
fn pagination_context_and_truncation_stay_identical() {
    let workspace = literal_workspace();
    let root = workspace.path();
    let mut input = input(root, DIFFERENTIAL_QUERY, RegexSearchMode::Literal, true);
    input.context_before = 1;
    input.context_after = 1;
    input.max_files = Some(1);
    input.max_matches_per_file = Some(1);
    let oracle = full_scan(&input, root);

    let manager = Arc::new(SearchIndexManager::default());
    warm_index(&manager, root);
    let actual = indexed_scan(&input, root, &manager);

    actual.assert_same_results(&oracle);
    assert_eq!(actual.files.len(), 1, "max_files 必须同时约束两条路径");
    assert!(actual.files.iter().all(|file| file.matches_truncated));
    assert_eq!(actual.next_file_offset, Some(1));
}

#[test]
fn edited_file_stays_scannable_until_the_incremental_update_lands() {
    let workspace = literal_workspace();
    let root = workspace.path();
    let input = input(root, DIFFERENTIAL_QUERY, RegexSearchMode::Literal, true);
    let manager = Arc::new(SearchIndexManager::default());
    let key = warm_index(&manager, root);

    fs::write(root.join("f01.txt"), "filler 1\n").expect("编辑必须成功");
    fs::write(
        root.join("f04.txt"),
        format!("filler 4\n{DIFFERENTIAL_QUERY}\n"),
    )
    .expect("编辑必须成功");

    let stale = indexed_scan(&input, root, &manager);
    let oracle = full_scan(&input, root);
    stale.assert_same_results(&oracle);

    let decision = prepare(&manager, root, DIFFERENTIAL_QUERY, true);
    assert_eq!(decision.diagnostics.stale_count, 2);
    let task = decision.take_update().expect("编辑必须产生增量任务");
    assert!(matches!(task, IndexTask::Update { .. }));
    manager.run_task_now(task);

    let updated = indexed_scan(&input, root, &manager);
    updated.assert_same_results(&oracle);
    assert!(
        updated.files_scanned < oracle.files_scanned,
        "增量更新后必须重新获得候选缩小收益"
    );
    assert_eq!(
        manager
            .cached_state(&key)
            .expect("状态必须仍然常驻")
            .generation(),
        1,
        "小范围增量不得触发全量 merge 换代"
    );
}

#[test]
fn deleted_file_is_dropped_from_results_and_marked_as_removed() {
    let workspace = literal_workspace();
    let root = workspace.path();
    let input = input(root, DIFFERENTIAL_QUERY, RegexSearchMode::Literal, true);
    let manager = Arc::new(SearchIndexManager::default());
    warm_index(&manager, root);

    fs::remove_file(root.join("f01.txt")).expect("删除必须成功");
    let decision = prepare(&manager, root, DIFFERENTIAL_QUERY, true);
    assert_eq!(decision.diagnostics.stale_count, 1);
    let task = decision.take_update().expect("删除必须产生增量任务");
    manager.run_task_now(task);

    let actual = indexed_scan(&input, root, &manager);
    let oracle = full_scan(&input, root);

    actual.assert_same_results(&oracle);
    assert_eq!(actual.files.len(), DIFFERENTIAL_MATCHING_FILES - 1);
    assert!(
        !actual
            .files
            .iter()
            .any(|file| file.path.contains("f01.txt"))
    );
}

#[test]
fn binary_and_invalid_utf8_files_keep_their_diagnostics() {
    let workspace = literal_workspace();
    let root = workspace.path();
    fs::write(
        root.join("binary.dat"),
        [b'n', b'e', b'e', 0, b'd', b'l', b'e'],
    )
    .expect("二进制夹具必须可写");
    let mut invalid = vec![0xff, 0xfe];
    invalid.extend_from_slice(DIFFERENTIAL_QUERY.as_bytes());
    fs::write(root.join("invalid.txt"), invalid).expect("非法 UTF-8 夹具必须可写");

    let input = input(root, DIFFERENTIAL_QUERY, RegexSearchMode::Literal, true);
    let oracle = full_scan(&input, root);

    let manager = Arc::new(SearchIndexManager::default());
    warm_index(&manager, root);
    let actual = indexed_scan(&input, root, &manager);

    actual.assert_same_results(&oracle);
    assert_eq!(actual.binary_paths.len(), 1, "二进制文件必须仍然被报告");
    assert_eq!(
        actual.unreadable_paths.len(),
        1,
        "非法 UTF-8 文件必须仍然被报告"
    );
}

#[test]
fn explicit_paths_search_does_not_schedule_an_index_update() {
    let workspace = literal_workspace();
    let root = workspace.path();
    let manager = Arc::new(SearchIndexManager::default());
    warm_index(&manager, root);

    let mut input = input(root, DIFFERENTIAL_QUERY, RegexSearchMode::Literal, true);
    input.paths = vec!["f00.txt".to_owned()];
    let summary =
        scan_workspace_with_index(&input, root, &manager, true).expect("显式路径扫描必须成功");

    assert_eq!(
        summary.files.len(),
        1,
        "显式路径搜索必须仍然命中索引中的文件"
    );
    assert_eq!(
        manager.active_task_count(),
        0,
        "显式路径搜索不得登记后台索引任务"
    );
}

/// 确定性 xorshift64* 伪随机数发生器：固定种子即可复现任意失败用例，且不引入外部依赖。
#[derive(Debug)]
struct Randomized(u64);

impl Randomized {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 返回 `[0, upper)` 区间内的随机值；`upper` 必须大于 0。
    fn below(&mut self, upper: usize) -> usize {
        debug_assert!(upper > 0, "随机上界必须大于 0");
        usize::try_from(self.next_u64() % u64::try_from(upper).unwrap_or(u64::MAX))
            .unwrap_or_default()
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// 字符边界偏移（含末尾），保证随机子串切片不会切断多字节字符。
fn char_boundaries(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect()
}

/// 生成一段随机文本；约三分之一概率注入查询词，保证存在真实命中。
fn random_body(rng: &mut Randomized, planted: &str) -> String {
    let fragments = FUZZ_MIN_FRAGMENTS + rng.below(FUZZ_MAX_FRAGMENTS - FUZZ_MIN_FRAGMENTS + 1);
    let mut body = String::new();
    if rng.below(3) == 0 {
        body.push_str(planted);
        body.push('\n');
    }
    for _ in 0..fragments {
        body.push_str(rng.pick(&FUZZ_ALPHABET));
    }
    body
}

/// 随机查询：一半概率直接使用查询词，另一半从现有内容里截取字符边界对齐的字面量。
///
/// search tool 只支持单行查询：跨行字面量会被 grep-regex 的 `line_terminator` 判为无效，
/// `planner` 也因此把跨行查询归为 `MatchAll`。随机查询必须保持在单行内，否则只会测到
/// 引擎的输入校验，测不到候选缩小规则。
fn random_query(rng: &mut Randomized, contents: &[String], planted: &str) -> String {
    if contents.is_empty() || rng.below(2) == 0 {
        return planted.to_owned();
    }
    for _ in 0..8 {
        let text = &contents[rng.below(contents.len())];
        let boundaries = char_boundaries(text);
        let start = boundaries[rng.below(boundaries.len())];
        let ends = boundaries
            .iter()
            .copied()
            .filter(|end| *end >= start + FUZZ_MIN_QUERY_BYTES)
            .collect::<Vec<_>>();
        if ends.is_empty() {
            continue;
        }
        let candidate = &text[start..ends[rng.below(ends.len())]];
        if !candidate.contains(['\n', '\r']) {
            return candidate.to_owned();
        }
    }
    planted.to_owned()
}

/// 随机差分：任何 matcher 缩小规则都不得让「完整扫描会命中的文件」被排除。
///
/// 每轮随机生成 workspace、查询和编辑序列，并在三种状态下对照完整扫描 oracle：
/// 索引刚构建、编辑后尚未更新、增量任务落地后。只有 `files_scanned` 允许不同。
#[test]
fn randomized_literal_queries_never_lose_a_match() {
    let mut rng = Randomized::new(FUZZ_SEED);
    let mut narrowed_rounds = 0;

    for _ in 0..FUZZ_ROUNDS {
        let workspace = axon_test_support::test_temp_dir();
        let root = workspace.path();
        let file_count = FUZZ_MIN_FILES + rng.below(FUZZ_MAX_FILES - FUZZ_MIN_FILES + 1);
        let planted = *rng.pick(&FUZZ_PLANTED_TOKENS);
        let mut contents = Vec::with_capacity(file_count);
        for index in 0..file_count {
            let body = random_body(&mut rng, planted);
            contents.push(body.clone());
            fs::write(root.join(format!("f{index:02}.txt")), body).expect("夹具文件必须可写");
        }

        let query = random_query(&mut rng, &contents, planted);
        let mut input = input(root, &query, RegexSearchMode::Literal, true);
        if rng.below(4) == 0 {
            input.max_files = Some(1 + rng.below(2));
        }

        let manager = Arc::new(SearchIndexManager::default());
        warm_index(&manager, root);
        let oracle = full_scan(&input, root);
        let actual = indexed_scan(&input, root, &manager);
        actual.assert_same_results(&oracle);
        if actual.files_scanned < oracle.files_scanned {
            narrowed_rounds += 1;
        }

        if rng.below(2) == 0 {
            let edits = FUZZ_MIN_EDITS + rng.below(FUZZ_MAX_EDITS - FUZZ_MIN_EDITS + 1);
            for _ in 0..edits {
                let path = root.join(format!("f{:02}.txt", rng.below(file_count)));
                if rng.below(3) == 0 {
                    fs::remove_file(&path).expect("删除必须成功");
                    continue;
                }
                fs::write(&path, random_body(&mut rng, planted)).expect("编辑必须成功");
            }

            let stale_actual = indexed_scan(&input, root, &manager);
            let stale_oracle = full_scan(&input, root);
            stale_actual.assert_same_results(&stale_oracle);

            let decision = prepare(&manager, root, &query, true);
            if let Some(task) = decision.take_update() {
                manager.run_task_now(task);
            }
            let updated_actual = indexed_scan(&input, root, &manager);
            let updated_oracle = full_scan(&input, root);
            updated_actual.assert_same_results(&updated_oracle);
        }
    }

    assert!(
        narrowed_rounds > 0,
        "随机用例必须真正触发候选过滤，否则并没有覆盖 matcher 缩小规则"
    );
}
