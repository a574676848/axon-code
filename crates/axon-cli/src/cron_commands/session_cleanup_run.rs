//! 会话清理的阶段执行与 Memory 联动。

use std::{path::Path, time::Instant};

use axon_memory_ops::{MemoryConsistencyCommand, MemoryOpsOutputFormat};

use super::SessionCleanupReport;

const CLEANUP_STATUS_COMPLETED: &str = "completed";
const PHASE_COMPLETED: &str = "completed";

pub(super) fn run_session_cleanup(
    config_path: &Path,
    dry_run: bool,
) -> Result<SessionCleanupReport, String> {
    let started = Instant::now();
    let mut report = super::run_session_cleanup_data(config_path, dry_run)?;
    report.memory_consistency_report = Some(run_memory_consistency(config_path, dry_run)?);
    report.status = CLEANUP_STATUS_COMPLETED.to_owned();
    report.phase = PHASE_COMPLETED.to_owned();
    report.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(report)
}

fn run_memory_consistency(config_path: &Path, dry_run: bool) -> Result<serde_json::Value, String> {
    let loaded = axon_config::ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("加载配置失败：{error:?}"))?;
    let ops_context = axon_memory_ops::open_memory_ops_context(config_path, &loaded.config)
        .map_err(|error| format!("初始化 Memory ops context 失败：{error}"))?;
    let command = MemoryConsistencyCommand {
        max_probe_records: None,
        repair: !dry_run,
        max_repair_records: None,
    };
    let result = axon_memory_ops::run_memory_consistency(
        MemoryOpsOutputFormat::Json,
        &ops_context,
        &command,
    );
    if result.exit_code != 0 {
        return Err(result.stderr);
    }
    serde_json::from_str(&result.stdout)
        .or_else(|_| Ok(serde_json::json!({"output": result.stdout})))
}
