//! 存量事件日志修复：序号重排与归属回填。
//!
//! 两条子能力各对应一条不变式的**存量入口**，写侧不变式本身已在运行期生效，这里处理的
//! 是它们落地之前就已经写坏的日志：
//!
//! - `--reorder`（INV-1）：写侧现在会核对文件尾并拒绝不一致的追加，但旧日志里仍留着
//!   「序号与落点不对应」的逆序帧。本能力按会话扫日志，列出读到的序号不再严格递增的
//!   位置，并在执行态按序号重排重写。
//! - `--attribute`（INV-4）：子代理会话的旧帧没有归属列。归属由派发行 join 判定
//!   （`inbox.session_id = 子会话` 且 `source_message_id` 以派发前缀开头），能唯一确定
//!   就回填，没有派发行或多义则如实报「无法确定」，不做猜测。归属回填只覆盖子代理
//!   会话：主、定时与目标会话的会话级事件按 INV-4 白名单本来就不带归属。
//!
//! 两条修复共守同一条写纪律：**默认 dry-run** 只出清单，真写必须再给 `--apply`；
//! 写之前先打印目标路径与备份路径，先备份原日志再原子替换。重排只改落点、回填只改
//! 归属列，序号集合不变，因此重复执行不产生二次变化（幂等）。
//!
//! 读路径复用事件日志层自身的解码：逆序日志被 `EventLogReader` 以「序号必须连续」判为
//! 损坏而整体拒读，所以清单与重排统一走 `read_tail_records`，它按帧顺序解码、只校验帧内
//! 连续，正是「先读出坏在哪」需要的那一级宽松度。

use std::path::{Path, PathBuf};

use axon_domain::{AGENT_DISPATCH_SOURCE_PREFIX, DaemonSessionId, InteractionId};
use axon_eventlog::{EventLog, RuntimeEventRecord};
use axon_session::{
    InteractionQueryStore as _, RuntimeStore as _, SessionKind, SqliteRuntimeStore,
};
use clap::Args;

use crate::cron_commands::session_log_files;
use crate::management_commands::cli_error;
use crate::runtime_paths::runtime_axon_data_path_static;
use crate::{CliRunResult, OutputFormat};

use super::session_repair_report::{
    AttributeReport, AttributeSessionReport, InversionEntry, ReorderReport, ReorderSessionReport,
    RepairReport, report_status,
};

/// 派发 join 的扫描上限：与网关历史页同一口径，一页足够判断归属是否唯一。
const DISPATCH_JOIN_SCAN_LIMIT: u32 = 256;
/// 单条会话最多点名多少处证据；超出只计数不再展开，避免报告体量失控。
pub(super) const REPAIR_EVIDENCE_LIMIT: usize = 200;
/// 备份文件的时间戳后缀前缀。
const BACKUP_SUFFIX_PREFIX: &str = ".bak-";

/// `axon debug session repair` 参数。
///
/// 默认 dry-run：只出清单，不改库也不改文件。执行修复必须再显式给 `--apply`。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SessionRepairCommand {
    /// 只修这一个会话（会话 ID）；省略时扫描数据目录里的全部会话日志。
    #[arg(long)]
    pub session: Option<String>,
    /// 序号重排（INV-1 存量入口）：列出并修复事件日志里序号逆序的位置。
    #[arg(long)]
    pub reorder: bool,
    /// 归属回填（INV-4 存量入口）：按派发 join 给子代理会话的无归属帧补写归属。
    #[arg(long)]
    pub attribute: bool,
    /// 只出清单，不写任何文件（默认行为）；与 --apply 互斥。
    #[arg(long)]
    pub dry_run: bool,
    /// 明确执行修复：先备份原日志再原子替换；省略时等价于 --dry-run。
    #[arg(long)]
    pub apply: bool,
}

/// 一次修复要跑的子能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepairMode {
    /// 序号重排。
    Reorder,
    /// 归属回填。
    Attribute,
}

impl RepairMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Reorder => "reorder",
            Self::Attribute => "attribute",
        }
    }
}

/// 一条待处理的事件日志。
struct LogTarget {
    /// 会话标识（以日志 header 为权威）。
    session_id: String,
    /// 日志句柄：同时给出目标路径与重写入参。
    log: EventLog,
}

impl LogTarget {
    fn path(&self) -> PathBuf {
        self.log.path()
    }
}

/// `axon debug session repair` 入口。
pub(crate) fn run_session_repair(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionRepairCommand,
) -> CliRunResult {
    let mode = match resolve_mode(command) {
        Ok(mode) => mode,
        Err(message) => return cli_error(message),
    };
    let data_root = match runtime_axon_data_path_static(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let store = SqliteRuntimeStore::from_root(&data_root);
    let session = command
        .session
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let dry_run = !command.apply;
    let report = match build_report(&store, mode, session, dry_run) {
        Ok(report) => report,
        Err(message) => return cli_error(message),
    };
    let exit_code = u8::from(report.failed());
    let stdout = match format {
        OutputFormat::Json => match report.to_json_line() {
            Ok(line) => line,
            Err(message) => return cli_error(message),
        },
        OutputFormat::Text => report.to_text(),
    };
    CliRunResult {
        exit_code,
        stdout,
        stderr: String::new(),
    }
}

/// 按子能力组装报告。
fn build_report(
    store: &SqliteRuntimeStore,
    mode: RepairMode,
    session: Option<&str>,
    dry_run: bool,
) -> Result<RepairReport, String> {
    match mode {
        RepairMode::Reorder => reorder_report(store, session, dry_run)
            .map(Box::new)
            .map(RepairReport::Reorder)
            .map_err(|message| format!("序号重排无法执行：{message}")),
        RepairMode::Attribute => attribute_report(store, session, dry_run)
            .map(Box::new)
            .map(RepairReport::Attribute)
            .map_err(|message| format!("归属回填无法执行：{message}")),
    }
}

/// 校验参数组合：一次只跑一条子能力，且 dry-run 与 apply 互斥。
fn resolve_mode(command: &SessionRepairCommand) -> Result<RepairMode, String> {
    let mode = match (command.reorder, command.attribute) {
        (true, true) => {
            return Err(
                "--reorder 与 --attribute 一次只能选一个：先重排序号，再回填归属".to_owned(),
            );
        }
        (true, false) => RepairMode::Reorder,
        (false, true) => RepairMode::Attribute,
        (false, false) => {
            return Err(
                "需要指定 --reorder 或 --attribute；默认 dry-run 只出清单，加 --apply 才写文件"
                    .to_owned(),
            );
        }
    };
    if command.dry_run && command.apply {
        return Err("--dry-run 与 --apply 互斥：前者只出清单，后者才写文件".to_owned());
    }
    Ok(mode)
}

/// 收集要扫的会话日志：显式给出的会话只扫它，否则扫数据目录下的全部会话日志。
fn target_logs(
    store: &SqliteRuntimeStore,
    session: Option<&str>,
) -> Result<(Vec<LogTarget>, Vec<String>), String> {
    if let Some(session_id) = session {
        let id = DaemonSessionId::new(session_id.to_owned())
            .map_err(|_| format!("会话 ID 非法：{session_id}"))?;
        let log = store.event_log(&id);
        if !log.path().is_file() {
            return Err(format!(
                "未找到会话 {session_id} 的事件日志：{}",
                log.path().display()
            ));
        }
        return Ok((
            vec![LogTarget {
                session_id: id.as_str().to_owned(),
                log,
            }],
            Vec::new(),
        ));
    }
    let events_root = store.events_root();
    if !events_root.is_dir() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut logs = Vec::new();
    let mut warnings = Vec::new();
    for path in session_log_files(events_root)? {
        // 目录名是转义片段、不可逆；会话标识以日志 header 为准。
        match axon_eventlog::read_header(&path) {
            Ok(Some(header)) => match DaemonSessionId::new(header.session_id) {
                Ok(id) => logs.push(LogTarget {
                    session_id: id.as_str().to_owned(),
                    log: store.event_log(&id),
                }),
                Err(_) => warnings.push(format!("事件日志头部会话标识非法：{}", path.display())),
            },
            Ok(None) => {}
            Err(error) => {
                warnings.push(format!("读取事件日志头部失败：{}：{error}", path.display()))
            }
        }
    }
    Ok((logs, warnings))
}

/// 按文件顺序读出整条日志的全部记录。
///
/// 逆序日志会被 `EventLogReader` 以「序号必须连续」整体拒读，因此这里走
/// `read_tail_records`：它按帧顺序解码、只校验帧内连续，能如实暴露跨帧的逆序。
pub(super) fn read_log_records(path: &Path) -> Result<Vec<RuntimeEventRecord>, String> {
    axon_eventlog::read_tail_records(path, usize::MAX)
        .map_err(|error| format!("读取事件日志失败：{}：{error}", path.display()))
}

/// 序号重排：先出清单，执行态再按序号重写。
fn reorder_report(
    store: &SqliteRuntimeStore,
    session: Option<&str>,
    dry_run: bool,
) -> Result<ReorderReport, String> {
    let (logs, mut warnings) = target_logs(store, session)?;
    let mut sessions = Vec::with_capacity(logs.len());
    let mut inversion_count = 0usize;
    for target in logs {
        let path = target.path();
        let mut report = ReorderSessionReport {
            session_id: target.session_id,
            log_path: path.display().to_string(),
            backup_path: None,
            event_count: 0,
            inversions: Vec::new(),
            inversions_truncated: false,
            repaired: false,
            error: None,
        };
        let records = match read_log_records(&path) {
            Ok(records) => records,
            Err(message) => {
                warnings.push(message);
                report.error = Some("事件日志读不出来，无法判定逆序".to_owned());
                sessions.push(report);
                continue;
            }
        };
        report.event_count = records.len();
        let all = collect_inversions(&records);
        inversion_count = inversion_count.saturating_add(all.len());
        report.inversions_truncated = all.len() > REPAIR_EVIDENCE_LIMIT;
        report.inversions = all.into_iter().take(REPAIR_EVIDENCE_LIMIT).collect();
        if !report.inversions.is_empty() && !dry_run {
            match rewrite_sorted_log(&target.log, &records) {
                Ok(backup) => {
                    report.backup_path = Some(backup.display().to_string());
                    report.repaired = true;
                }
                Err(message) => report.error = Some(message),
            }
        }
        sessions.push(report);
    }
    let dirty = sessions
        .iter()
        .filter(|item| !item.inversions.is_empty())
        .count();
    Ok(ReorderReport {
        status: report_status(dry_run, sessions.iter().any(|item| item.error.is_some())),
        mode: RepairMode::Reorder.as_str(),
        dry_run,
        events_root: store.events_root().display().to_string(),
        scanned_sessions: sessions.len(),
        dirty_sessions: dirty,
        inversion_count,
        sessions,
        warnings,
    })
}

/// 列出「读到的序号不再严格递增」的位置。
fn collect_inversions(records: &[RuntimeEventRecord]) -> Vec<InversionEntry> {
    let mut inversions = Vec::new();
    for (index, pair) in records.windows(2).enumerate() {
        let [previous, current] = pair else {
            continue;
        };
        if current.sequence > previous.sequence {
            continue;
        }
        inversions.push(InversionEntry {
            position: index.saturating_add(1),
            previous_sequence: previous.sequence,
            current_sequence: current.sequence,
            previous_event_type: previous.event_type.as_str().to_owned(),
            current_event_type: current.event_type.as_str().to_owned(),
            previous_emitted_at_ms: previous.emitted_at_ms,
            current_emitted_at_ms: current.emitted_at_ms,
            duplicate: current.sequence == previous.sequence,
        });
    }
    inversions
}

/// 按序号重排重写：先备份，再原子替换；序号集合保持不变。
fn rewrite_sorted_log(log: &EventLog, records: &[RuntimeEventRecord]) -> Result<PathBuf, String> {
    if has_duplicate_sequences(records) {
        return Err(
            "事件日志存在同号序号，重排无法消除；需要人工确认后单独处理，本次未改动文件".to_owned(),
        );
    }
    let Some(expected_tail) = records.last().map(|record| record.sequence) else {
        return Err("事件日志没有记录，无需重排".to_owned());
    };
    let mut sorted = records.to_vec();
    sorted.sort_by_key(|record| record.sequence);
    let backup = backup_log(&log.path())?;
    match axon_eventlog::rewrite_log_if_unchanged(log, expected_tail, &read_header(log)?, &sorted) {
        Ok(Some(_)) => Ok(backup),
        Ok(None) => {
            // 读快照之后有人追加过：本次重写会盖掉那些帧，放弃并清掉刚建的备份。
            let _ = std::fs::remove_file(&backup);
            Err("事件日志在读取之后被追加，本轮放弃；原文件未改动".to_owned())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&backup);
            Err(format!("事件日志重写失败，原文件未改动：{error}"))
        }
    }
}

/// 是否存在同号序号：同号是重复写，不是顺序错位。
fn has_duplicate_sequences(records: &[RuntimeEventRecord]) -> bool {
    records
        .windows(2)
        .any(|pair| pair.len() == 2 && pair[0].sequence == pair[1].sequence)
}

/// 读取日志头部；缺失时给出显式原因。
fn read_header(log: &EventLog) -> Result<axon_eventlog::Header, String> {
    match axon_eventlog::read_header(&log.path()) {
        Ok(Some(header)) => Ok(header),
        Ok(None) => Err(format!(
            "事件日志缺少 header 帧，无法重写：{}",
            log.path().display()
        )),
        Err(error) => Err(format!("读取事件日志头部失败：{error}")),
    }
}

/// 备份原日志：同目录同名加时间戳后缀，与日志同卷，便于人工回滚。
fn backup_log(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("事件日志路径不可用：{}", path.display()))?;
    let stamp = axon_time::unix_now_ms();
    let backup = path.with_file_name(format!("{name}{BACKUP_SUFFIX_PREFIX}{stamp}"));
    std::fs::copy(path, &backup)
        .map_err(|error| format!("备份事件日志失败：{}：{error}", backup.display()))?;
    Ok(backup)
}

/// 归属回填：先判定能不能唯一确定，执行态再给可确定的那批帧补写归属列。
fn attribute_report(
    store: &SqliteRuntimeStore,
    session: Option<&str>,
    dry_run: bool,
) -> Result<AttributeReport, String> {
    let (logs, mut warnings) = target_logs(store, session)?;
    let mut sessions = Vec::with_capacity(logs.len());
    let mut backfillable_frames = 0usize;
    let mut undetermined_frames = 0usize;
    for target in logs {
        let explicit = session.is_some();
        if !explicit && !is_subagent_session(store, &target.session_id) {
            continue;
        }
        let path = target.path();
        let mut report = AttributeSessionReport {
            session_id: target.session_id,
            log_path: path.display().to_string(),
            event_count: 0,
            unattributed_frames: 0,
            verdict: "clean",
            interaction_id: None,
            reason: None,
            backup_path: None,
            repaired: false,
            error: None,
        };
        let mut records = match read_log_records(&path) {
            Ok(records) => records,
            Err(message) => {
                warnings.push(message);
                report.error = Some("事件日志读不出来，无法判定归属".to_owned());
                sessions.push(report);
                continue;
            }
        };
        report.event_count = records.len();
        let unattributed = records
            .iter()
            .filter(|record| record.interaction_id.is_none())
            .count();
        report.unattributed_frames = unattributed;
        if unattributed == 0 {
            sessions.push(report);
            continue;
        }
        match resolve_dispatch(store, &report.session_id) {
            Ok(DispatchResolution::Unique(interaction_id)) => {
                backfillable_frames = backfillable_frames.saturating_add(unattributed);
                report.verdict = "backfillable";
                report.interaction_id = Some(interaction_id.as_str().to_owned());
                if !dry_run {
                    match rewrite_attributed_log(&target.log, &mut records, &interaction_id) {
                        Ok(backup) => {
                            report.backup_path = Some(backup.display().to_string());
                            report.repaired = true;
                        }
                        Err(message) => report.error = Some(message),
                    }
                }
            }
            Ok(DispatchResolution::Undetermined(reason)) => {
                undetermined_frames = undetermined_frames.saturating_add(unattributed);
                report.verdict = "undetermined";
                report.reason = Some(reason);
            }
            Err(message) => report.error = Some(message),
        }
        sessions.push(report);
    }
    Ok(AttributeReport {
        status: report_status(dry_run, sessions.iter().any(|item| item.error.is_some())),
        mode: RepairMode::Attribute.as_str(),
        dry_run,
        events_root: store.events_root().display().to_string(),
        scanned_sessions: sessions.len(),
        backfillable_frames,
        undetermined_frames,
        sessions,
        warnings,
    })
}

/// 会话是不是子代理会话：只有子代理会话的整段帧属于同一条派发轮次。
///
/// 会话行读不出来时按「不在范围内」处理：归属回填只覆盖能证明归属的会话，读不到类型
/// 就不该往里写。显式给出 `--session` 时不受这一层过滤，由派发 join 自己判定。
pub(super) fn is_subagent_session(store: &SqliteRuntimeStore, session_id: &str) -> bool {
    let Ok(id) = DaemonSessionId::new(session_id.to_owned()) else {
        return false;
    };
    match store.get_session(&id) {
        Ok(record) => record.kind == SessionKind::Subagent,
        Err(_) => false,
    }
}

/// 派发 join 的结论。
pub(super) enum DispatchResolution {
    /// 派发唯一：整段会话帧都属于这一轮。
    Unique(InteractionId),
    /// 无法唯一确定，附原因。
    Undetermined(String),
}

/// 按派发行 join 判定子会话的归属。
///
/// 判据与网关历史页的第二级读取同源：`inbox.session_id = 子会话` 且 `source_message_id`
/// 以派发前缀开头。窗口满时宁可如实报「无法证明唯一」，也不把整段会话挂到一条轮次上。
pub(super) fn resolve_dispatch(
    store: &SqliteRuntimeStore,
    session_id: &str,
) -> Result<DispatchResolution, String> {
    let rows = store
        .interactions_for_session(session_id, DISPATCH_JOIN_SCAN_LIMIT, None)
        .map_err(|error| format!("读取子会话 {session_id} 的派发行失败：{error}"))?;
    let window_full = rows.len() >= usize::try_from(DISPATCH_JOIN_SCAN_LIMIT).unwrap_or(usize::MAX);
    if window_full {
        return Ok(DispatchResolution::Undetermined(format!(
            "子会话 {session_id} 的轮次窗口已满（{} 条），无法证明这条派发唯一",
            rows.len()
        )));
    }
    let dispatches = rows
        .iter()
        .filter(|row| {
            row.source
                .source_message_id()
                .starts_with(AGENT_DISPATCH_SOURCE_PREFIX)
        })
        .collect::<Vec<_>>();
    match dispatches.len() {
        0 => Ok(DispatchResolution::Undetermined(format!(
            "子会话 {session_id} 没有派发行（inbox 里没有 source_message_id 以 {AGENT_DISPATCH_SOURCE_PREFIX} 开头的轮次）"
        ))),
        1 => match dispatches.first() {
            Some(row) => Ok(DispatchResolution::Unique(row.interaction_id.clone())),
            None => Ok(DispatchResolution::Undetermined(format!(
                "子会话 {session_id} 的派发行读取结果为空，无法确定归属"
            ))),
        },
        count => Ok(DispatchResolution::Undetermined(format!(
            "子会话 {session_id} 有 {count} 条派发轮次，无法唯一确定这批帧属于哪一轮"
        ))),
    }
}

/// 给无归属帧补写归属列并原子重写：先备份，再替换。
fn rewrite_attributed_log(
    log: &EventLog,
    records: &mut [RuntimeEventRecord],
    interaction_id: &InteractionId,
) -> Result<PathBuf, String> {
    if !collect_inversions(records).is_empty() {
        return Err(
            "事件日志序号存在逆序，回填会一并改写落点；先用 --reorder 修复序号，本次未改动文件"
                .to_owned(),
        );
    }
    let Some(expected_tail) = records.last().map(|record| record.sequence) else {
        return Err("事件日志没有记录，无需回填".to_owned());
    };
    let mut changed = 0usize;
    for record in records.iter_mut() {
        if record.interaction_id.is_some() {
            continue;
        }
        record.interaction_id = Some(interaction_id.as_str().to_owned());
        changed = changed.saturating_add(1);
    }
    if changed == 0 {
        return Err("本次没有任何无归属帧可回填，未改动文件".to_owned());
    }
    let backup = backup_log(&log.path())?;
    match axon_eventlog::rewrite_log_if_unchanged(log, expected_tail, &read_header(log)?, records) {
        Ok(Some(_)) => Ok(backup),
        Ok(None) => {
            let _ = std::fs::remove_file(&backup);
            Err("事件日志在读取之后被追加，本轮放弃；原文件未改动".to_owned())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&backup);
            Err(format!("事件日志重写失败，原文件未改动：{error}"))
        }
    }
}

#[cfg(test)]
#[path = "session_repair_tests.rs"]
mod tests;
