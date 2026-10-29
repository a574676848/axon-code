use std::path::{Path, PathBuf};

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig as AxonMemoryConfig, MemoryOrchestrator,
};
use axon_config::{AxonConfig, ConfigLoader};
pub(super) use axon_display::display_path;
use axon_time::unix_now_ms;
use clap::{Args, Subcommand, ValueEnum};

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_config_error, cli_error, resolved_config_path},
    run_commands::load_run_env_file,
    runtime_paths::runtime_axon_root_path,
    setup_commands::env_path_for_config,
};

mod compare;
mod conflicts;
mod consistency;
mod consolidate;
mod core_commands;
mod export;
mod governance;
mod import;
mod inspect;
mod quality;
mod recover_conflict;
mod render;
mod review;
mod skill_evolution_status;
mod sync_audit;
mod view;

pub use compare::MemoryCompareCommand;
use compare::run_memory_compare;
pub use conflicts::MemoryConflictsCommand;
use conflicts::run_memory_conflicts;
pub use consistency::MemoryConsistencyCommand;
use consistency::run_memory_consistency;
pub use consolidate::MemoryConsolidateCommand;
use consolidate::run_memory_consolidate;
use core_commands::{run_memory_doctor, run_memory_list, run_memory_search, run_memory_stats};
pub use export::MemoryExportCommand;
use export::run_memory_export;
pub use governance::{
    MemoryApprovePendingCommand, MemoryArchivePendingCommand, MemoryGovernanceCommand,
};
use governance::{
    MemoryGovernanceAction, run_memory_approve_pending, run_memory_archive_pending,
    run_memory_governance,
};
pub use import::MemoryImportCommand;
use import::run_memory_import;
pub use inspect::MemoryInspectCommand;
use inspect::run_memory_inspect;
pub use quality::MemoryQualityCommand;
use quality::run_memory_quality;
pub use recover_conflict::MemoryRecoverConflictCommand;
use recover_conflict::run_memory_recover_conflict;
pub use review::MemoryReviewCommand;
use review::run_memory_review;
pub use skill_evolution_status::{SkillEvolutionPromoteCommand, SkillEvolutionStatusCommand};
use skill_evolution_status::{run_skill_evolution_promote, run_skill_evolution_status};
pub use sync_audit::MemorySyncAuditCommand;
use sync_audit::run_memory_sync_audit;

const MEMORY_CLI_DEFAULT_LIMIT: usize = 16;
const MEMORY_STATUS_READY: &str = "memory_ready";

/// `axon memory` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryCommand {
    /// memory 子命令。
    #[command(subcommand)]
    pub command: Option<MemorySubcommand>,
}

/// `axon memory` 子命令。
#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum MemorySubcommand {
    /// 展示 Memory store 汇总。
    Stats,
    /// 运行 Memory doctor / drift audit。
    Doctor(MemoryDoctorCommand),
    /// 列出本机治理记录引用。
    List(MemoryListCommand),
    /// 通过 Hybrid Retrieval 搜索本机引用。
    Search(MemorySearchCommand),
    /// 执行 Memory 睡眠期巩固治理。
    Consolidate(MemoryConsolidateCommand),
    /// 检查并可选修复物理投影一致性。
    Consistency(MemoryConsistencyCommand),
    /// 只读列出 Memory 冲突组。
    Conflicts(MemoryConflictsCommand),
    /// 比较两条 Memory 的 ref-only 冲突证据。
    Compare(MemoryCompareCommand),
    /// 评估 Memory store 质量门禁。
    Quality(MemoryQualityCommand),
    /// 恢复被冲突治理归档的 Memory。
    RecoverConflict(MemoryRecoverConflictCommand),
    /// 列出需要本机复核的 Memory 治理记录。
    Review(MemoryReviewCommand),
    /// 导出 Memory ref-only 治理包。
    Export(MemoryExportCommand),
    /// 只读预检 Memory ref-only 导出包。
    Import(MemoryImportCommand),
    /// 查看 Memory 跨设备同步审计。
    SyncAudit(MemorySyncAuditCommand),
    /// 查看单条 Memory 的 ref-only 证据链。
    Inspect(MemoryInspectCommand),
    /// 查看指定 session 的 Skill Evolution 当前状态。
    SkillEvolutionStatus(SkillEvolutionStatusCommand),
    /// 将已验证的 Skill draft 晋升到用户 Skill 目录。
    SkillEvolutionPromote(SkillEvolutionPromoteCommand),
    /// 本机批准 weak 记忆，提升为 strong。
    Approve(MemoryGovernanceCommand),
    /// 批量批准安全的待复核 weak 记忆。
    ApprovePending(MemoryApprovePendingCommand),
    /// 批量归档待复核记忆，移出召回上下文。
    ArchivePending(MemoryArchivePendingCommand),
    /// 本机隔离记忆，移出召回上下文。
    Quarantine(MemoryGovernanceCommand),
    /// 本机遗忘记忆，写入 tombstone 阻断复活。
    Forget(MemoryGovernanceCommand),
}

/// `axon memory list` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryListCommand {
    /// 治理状态过滤。
    #[arg(long, value_enum)]
    pub kind: Option<MemoryKindFilter>,
    /// 稳定记忆类型过滤。
    #[arg(long = "memory-kind", value_enum)]
    pub memory_kind: Option<MemoryTypeFilter>,
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
}

/// `axon memory doctor` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryDoctorCommand {
    /// 最大物理投影探测记录数；省略时使用 runtime 默认值。
    #[arg(long)]
    pub max_probe_records: Option<usize>,
}

/// `axon memory search` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemorySearchCommand {
    /// 查询文本。
    pub query: String,
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
    /// 可选图片：https URL、data:image base64 或本地图片路径；带图时 embedding 走多模态接口。
    #[arg(long = "image", value_name = "URL_OR_PATH")]
    pub images: Vec<String>,
}

/// Memory list 状态过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MemoryKindFilter {
    /// 未归档 strong/weak 记录。
    Active,
    /// 已归档记录。
    Archived,
    /// strong 记录。
    Strong,
    /// weak 记录。
    Weak,
    /// quarantined 记录。
    Quarantined,
    /// tombstone 记录。
    Tombstone,
}

/// Memory list 类型过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MemoryTypeFilter {
    /// 用户画像。
    Profile,
    /// 用户偏好。
    Preference,
    /// 可复用经验。
    Experience,
    /// 稳定知识。
    Knowledge,
    /// 可执行程序或流程。
    Procedure,
}

pub(crate) fn run_memory(
    command: &MemoryCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    // Memory 子命令可能触发 embedding 等 provider 调用，必须先加载配置相邻 `.env` 到凭据仓库。
    if let Err(error) = load_run_env_file(&env_path_for_config(&path)) {
        return cli_error(error);
    }
    let loaded = match ConfigLoader::load_from_path(&path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    let subcommand = command.command.as_ref().unwrap_or(&MemorySubcommand::Stats);
    let context = match MemoryCliContext::open(&path, &loaded.config) {
        Ok(context) => context,
        Err(error) => return cli_error(error),
    };

    match subcommand {
        MemorySubcommand::Stats => run_memory_stats(format, &context),
        MemorySubcommand::Doctor(command) => run_memory_doctor(format, &context, command),
        MemorySubcommand::List(command) => run_memory_list(format, &context, command),
        MemorySubcommand::Search(command) => run_memory_search(format, &context, command),
        MemorySubcommand::Consolidate(command) => run_memory_consolidate(format, &context, command),
        MemorySubcommand::Consistency(command) => run_memory_consistency(format, &context, command),
        MemorySubcommand::Conflicts(command) => run_memory_conflicts(format, &context, command),
        MemorySubcommand::Compare(command) => run_memory_compare(format, &context, command),
        MemorySubcommand::Quality(command) => run_memory_quality(format, &context, command),
        MemorySubcommand::RecoverConflict(command) => {
            run_memory_recover_conflict(format, &context, command)
        }
        MemorySubcommand::Review(command) => run_memory_review(format, &context, command),
        MemorySubcommand::Export(command) => run_memory_export(format, &context, command),
        MemorySubcommand::Import(command) => run_memory_import(format, &context, command),
        MemorySubcommand::SyncAudit(command) => run_memory_sync_audit(format, &context, command),
        MemorySubcommand::Inspect(command) => run_memory_inspect(format, &context, command),
        MemorySubcommand::SkillEvolutionStatus(command) => {
            run_skill_evolution_status(format, &context, command)
        }
        MemorySubcommand::SkillEvolutionPromote(command) => {
            run_skill_evolution_promote(format, &context, command)
        }
        MemorySubcommand::Approve(command) => {
            run_memory_governance(format, &context, command, MemoryGovernanceAction::Approve)
        }
        MemorySubcommand::ApprovePending(command) => {
            run_memory_approve_pending(format, &context, command)
        }
        MemorySubcommand::ArchivePending(command) => {
            run_memory_archive_pending(format, &context, command)
        }
        MemorySubcommand::Quarantine(command) => run_memory_governance(
            format,
            &context,
            command,
            MemoryGovernanceAction::Quarantine,
        ),
        MemorySubcommand::Forget(command) => {
            run_memory_governance(format, &context, command, MemoryGovernanceAction::Forget)
        }
    }
}

pub(super) struct MemoryCliContext {
    pub(super) config_path: PathBuf,
    pub(super) axon_root: PathBuf,
    pub(super) orchestrator: MemoryOrchestrator,
}

impl MemoryCliContext {
    fn open(config_path: &Path, config: &AxonConfig) -> Result<Self, String> {
        let axon_root = runtime_axon_root_path(config_path)?;
        let memory_config = AxonMemoryConfig::from_config_dir(
            &axon_root,
            config.memory.embedding.enabled,
            memory_embedding_dimension(config),
        )
        .map_err(|error| format!("{error:?}"))?;
        let orchestrator =
            MemoryOrchestrator::init(memory_config).map_err(|error| format!("{error:?}"))?;
        Ok(Self {
            config_path: config_path.to_path_buf(),
            axon_root,
            orchestrator,
        })
    }
}

pub(super) fn bounded_limit(limit: Option<usize>) -> Result<usize, &'static str> {
    let limit = limit.unwrap_or(MEMORY_CLI_DEFAULT_LIMIT);
    if limit == 0 {
        return Err("memory limit 必须大于 0");
    }
    Ok(limit)
}

fn memory_embedding_dimension(config: &AxonConfig) -> usize {
    if config.memory.embedding.enabled {
        config.memory.embedding.dimension
    } else {
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
    }
}

pub(super) fn now_ms() -> u64 {
    unix_now_ms()
}

#[cfg(test)]
mod limit_tests {
    use super::bounded_limit;

    #[test]
    fn explicit_memory_limit_is_not_capped() {
        assert_eq!(bounded_limit(Some(128)), Ok(128));
    }
}
