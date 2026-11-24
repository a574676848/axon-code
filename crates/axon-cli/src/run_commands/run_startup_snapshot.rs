//! Run 会话启动快照：统一承载可在用户提交前完成的静态运行时资源。

use std::{
    path::Path,
    sync::{Arc, RwLock},
    thread,
    time::Instant,
};

use axon_agent_runtime::{
    AgentModelMessage, AgentModelPayloadRuntime, AgentModelStablePrefix, ToolRegistry,
};
use axon_config::ConfigLoader;
use axon_domain::RunPrincipalContext;
use axon_memory_read::{PersonalMemorySnapshot, load_personal_memory_snapshot};
use axon_session_runtime::ProviderToolEnvironment;

use crate::{
    runtime_principal_context::load_run_principal,
    runtime_prompt_fragments::{RuntimePromptFragmentsInput, runtime_prompt_fragment_messages},
    runtime_tools::RuntimeSkillsSnapshot,
    runtime_user_agents::load_user_agents_message,
    runtime_workspace_instructions::{WorkspaceInstruction, load_workspace_instructions},
};

use super::{AgentMemoryWriteRuntime, run_session_store::resolved_axon_root};

const STARTUP_PRINCIPAL_CHANNEL_REF: &str = "local";
const STARTUP_STAGE_PRINCIPAL: &str = "principal";
const STARTUP_STAGE_PERSONAL_MEMORY: &str = "personal_memory";
const STARTUP_STAGE_USER_AGENTS: &str = "user_agents";
const STARTUP_STAGE_WORKSPACE: &str = "workspace";
const STARTUP_STAGE_SKILLS: &str = "skills";
const STARTUP_STAGE_MCP_REGISTRY: &str = "mcp_registry";
const STARTUP_STAGE_PROMPT_CONTEXT: &str = "prompt_context";
#[derive(Debug, Clone)]
pub(crate) struct RunPromptContextSnapshot {
    pub(crate) stable_prefix: AgentModelStablePrefix,
    pub(crate) subagent_stable_prefix: AgentModelStablePrefix,
    pub(crate) workspace_context_messages: Vec<AgentModelMessage>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStartupStageTiming {
    pub(crate) stage: String,
    pub(crate) duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStartupMcpCacheStatus {
    pub(crate) server_id: String,
    pub(crate) disposition: String,
    /// 该 server 在本次构建中是否探测失败；用于 TTL 到期后决定是否重跑 MCP 探测。
    #[serde(default)]
    pub(crate) failed: bool,
    /// 失败原因（已脱敏）；成功时为缺省。`/mcp` 的列表直接展示这一句。
    #[serde(default)]
    pub(crate) failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStartupTimeline {
    pub(crate) total_ms: u64,
    pub(crate) stages: Vec<RunStartupStageTiming>,
    pub(crate) mcp_schema_cache: Vec<RunStartupMcpCacheStatus>,
    pub(crate) skill_catalog_revision: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RunStartupSnapshot {
    /// 用户级 Axon Home 目录。
    pub(crate) home_root: String,
    /// 当前选中的 Workspace；位于 Home 时为空。
    pub(crate) workspace_root: Option<String>,
    pub(crate) personal_memory: Option<PersonalMemorySnapshot>,
    pub(crate) user_agents: Option<AgentModelMessage>,
    pub(crate) workspace_instructions: Vec<WorkspaceInstruction>,
    pub(crate) skills: RuntimeSkillsSnapshot,
    pub(crate) execution_tools: ToolRegistry,
    pub(crate) prompt_context: RunPromptContextSnapshot,
    pub(crate) timeline: RunStartupTimeline,
    pub(crate) memory_write_runtime: Arc<AgentMemoryWriteRuntime>,
}
impl RunStartupSnapshot {
    pub(super) fn resolve_for_run(
        config_path: &Path,
        cwd: Option<&Path>,
        preloaded: Option<Arc<Self>>,
    ) -> Result<Arc<Self>, String> {
        let cwd = super::run_request::resolve_run_cwd(cwd)?;
        if let Some(snapshot) = preloaded {
            let workspace = selected_workspace(&cwd, Path::new(&snapshot.home_root));
            let workspace_root = workspace.map(|path| path.to_string_lossy().into_owned());
            if snapshot.workspace_root == workspace_root {
                return Ok(snapshot);
            }
            return replace_workspace_snapshot(config_path, &cwd, workspace, &snapshot)
                .map(Arc::new);
        }
        Self::build(config_path, &cwd).map(Arc::new)
    }

    pub(crate) fn build(config_path: &Path, cwd: &Path) -> Result<Self, String> {
        Self::build_with_mcp_refresh(config_path, cwd, false)
    }

    /// 构建启动快照；`refresh_mcp_health=true` 时 MCP 探测绕过健康负缓存与 schema 缓存。
    pub(crate) fn build_with_mcp_refresh(
        config_path: &Path,
        cwd: &Path,
        refresh_mcp_health: bool,
    ) -> Result<Self, String> {
        let started = Instant::now();
        let cwd = cwd
            .canonicalize()
            .map_err(|_| format!("工作目录不可访问：{}", cwd.display()))?;
        let home_root = resolved_axon_root(config_path)?;
        let home_root = home_root.canonicalize().unwrap_or(home_root);
        let workspace = selected_workspace(&cwd, &home_root);
        thread::scope(|scope| {
            build_scoped_snapshot(
                scope,
                config_path,
                &cwd,
                workspace,
                started,
                home_root.to_string_lossy().into_owned(),
                refresh_mcp_health,
            )
        })
    }

    /// 上一次构建是否存在探测失败的 MCP server。
    #[must_use]
    pub(crate) fn has_failed_mcp_servers(&self) -> bool {
        self.timeline
            .mcp_schema_cache
            .iter()
            .any(|status| status.failed)
    }

    pub(crate) fn execution_tools_for_session(
        &self,
        _config_path: &Path,
        _session_id: &str,
    ) -> Arc<RwLock<ToolRegistry>> {
        Arc::new(RwLock::new(self.execution_tools.clone()))
    }

    pub(crate) fn tool_environment_for_session(
        &self,
        config_path: &Path,
        session_id: &str,
    ) -> ProviderToolEnvironment {
        let mcp_schema_revision = self
            .timeline
            .mcp_schema_cache
            .iter()
            .map(|status| format!("{}:{}", status.server_id, status.disposition))
            .collect::<Vec<_>>()
            .join("\n");
        ProviderToolEnvironment::new(
            self.execution_tools_for_session(config_path, session_id),
            self.skills.revision(),
            mcp_schema_revision,
        )
    }
}
struct TimedStage<T> {
    timing: RunStartupStageTiming,
    value: T,
}

struct StartupTaskResults {
    principal: TimedStage<StartupPrincipalLoad>,
    workspace: TimedStage<Result<Vec<WorkspaceInstruction>, String>>,
    user_agents: TimedStage<Result<Option<AgentModelMessage>, String>>,
    skills: TimedStage<Result<RuntimeSkillsSnapshot, String>>,
    execution_tools: TimedStage<crate::runtime_tools::RuntimeExecutionToolSnapshot>,
}

/// 主代理 principal 加载结果：身份上下文、诊断与首次 Personal 写入所需的 Axon 根。
struct StartupPrincipalLoad {
    context: Option<RunPrincipalContext>,
    diagnostic: Option<String>,
    identity_root: Option<std::path::PathBuf>,
}

fn build_scoped_snapshot<'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    config_path: &'scope Path,
    cwd: &'scope Path,
    workspace_root: Option<&'scope Path>,
    started: Instant,
    home_root: String,
    refresh_mcp_health: bool,
) -> Result<RunStartupSnapshot, String> {
    let principal = scope.spawn(|| {
        timed(STARTUP_STAGE_PRINCIPAL, || {
            load_startup_principal(config_path)
        })
    });
    let workspace = scope.spawn(move || {
        timed(STARTUP_STAGE_WORKSPACE, || {
            load_selected_workspace_instructions(workspace_root)
        })
    });
    let user_agents = scope.spawn(|| timed(STARTUP_STAGE_USER_AGENTS, load_user_agents_message));
    let skills = scope.spawn(move || {
        timed(STARTUP_STAGE_SKILLS, || {
            crate::runtime_tools::load_runtime_skills_snapshot(config_path, workspace_root)
        })
    });
    let execution_tools = scope.spawn(move || {
        timed(STARTUP_STAGE_MCP_REGISTRY, || {
            crate::runtime_tools::build_runtime_tool_registry_snapshot_with_refresh(
                config_path,
                workspace_root,
                refresh_mcp_health,
            )
        })
    });
    let principal = join(principal, "Principal")?;
    let personal_memory = timed(STARTUP_STAGE_PERSONAL_MEMORY, || {
        load_startup_personal_memory(config_path, principal.value.context.as_ref())
    });
    let workspace = join(workspace, "Workspace instruction")?;
    let user_agents = join(user_agents, "User Agents")?;
    let skills = join(skills, "Skill")?;
    let workspace_instructions = workspace.value.clone()?;
    let skills_snapshot = skills.value.clone()?;
    let prompt_context = timed(STARTUP_STAGE_PROMPT_CONTEXT, || {
        prompt_context_snapshot(cwd, &workspace_instructions, &skills_snapshot)
    });
    assemble_startup_snapshot(
        started,
        workspace_root,
        StartupTaskResults {
            principal,
            workspace,
            user_agents,
            skills,
            execution_tools: join(execution_tools, "MCP registry")?,
        },
        prompt_context,
        personal_memory,
        workspace_instructions,
        skills_snapshot,
        home_root,
    )
}

fn assemble_startup_snapshot(
    started: Instant,
    workspace_root: Option<&Path>,
    tasks: StartupTaskResults,
    prompt_context: TimedStage<RunPromptContextSnapshot>,
    personal_memory: TimedStage<Option<PersonalMemorySnapshot>>,
    workspace_instructions: Vec<WorkspaceInstruction>,
    skills_snapshot: RuntimeSkillsSnapshot,
    home_root: String,
) -> Result<RunStartupSnapshot, String> {
    let StartupPrincipalLoad {
        context: principal,
        diagnostic: principal_diagnostic,
        identity_root,
    } = tasks.principal.value;
    let user_agents = tasks.user_agents.value?;
    let memory_write_runtime = Arc::new(
        AgentMemoryWriteRuntime::new(
            principal.as_ref().map(|value| value.owner_id.clone()),
            principal_diagnostic,
        )
        .with_identity_root(identity_root),
    );
    let timeline = RunStartupTimeline {
        total_ms: elapsed_ms(started),
        stages: vec![
            tasks.principal.timing,
            personal_memory.timing,
            tasks.workspace.timing,
            tasks.user_agents.timing,
            tasks.skills.timing,
            tasks.execution_tools.timing,
            prompt_context.timing,
        ],
        mcp_schema_cache: mcp_cache_statuses(&tasks.execution_tools.value),
        skill_catalog_revision: skills_snapshot.revision().to_owned(),
    };
    Ok(RunStartupSnapshot {
        home_root,
        workspace_root: workspace_root.map(|path| path.to_string_lossy().into_owned()),
        personal_memory: personal_memory.value,
        user_agents,
        workspace_instructions,
        skills: skills_snapshot,
        execution_tools: tasks.execution_tools.value.registry,
        prompt_context: prompt_context.value,
        timeline,
        memory_write_runtime,
    })
}

fn load_startup_personal_memory(
    config_path: &Path,
    principal: Option<&RunPrincipalContext>,
) -> Option<PersonalMemorySnapshot> {
    let owner_id = principal?.owner_id.as_str();
    let loaded = ConfigLoader::load_from_path(config_path).ok()?;
    load_personal_memory_snapshot(
        config_path,
        &loaded.config,
        owner_id,
        axon_time::unix_now_ms(),
    )
    .ok()
}

fn load_startup_principal(config_path: &Path) -> StartupPrincipalLoad {
    let Ok(root) = resolved_axon_root(config_path) else {
        return StartupPrincipalLoad {
            context: None,
            diagnostic: Some("无法解析用户级 Axon 目录".to_owned()),
            identity_root: None,
        };
    };
    let identity_root = Some(root.clone());
    match load_run_principal(&root, STARTUP_PRINCIPAL_CHANNEL_REF) {
        Ok(context) => StartupPrincipalLoad {
            context,
            diagnostic: None,
            identity_root,
        },
        Err(diagnostic) => StartupPrincipalLoad {
            context: None,
            diagnostic: Some(diagnostic),
            identity_root,
        },
    }
}

fn timed<T>(stage: &str, operation: impl FnOnce() -> T) -> TimedStage<T> {
    let started = Instant::now();
    let value = operation();
    TimedStage {
        timing: RunStartupStageTiming {
            stage: stage.to_owned(),
            duration_ms: elapsed_ms(started),
        },
        value,
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
pub(crate) fn test_startup_timeline() -> RunStartupTimeline {
    RunStartupTimeline {
        total_ms: 0,
        stages: Vec::new(),
        mcp_schema_cache: Vec::new(),
        skill_catalog_revision: String::new(),
    }
}

fn prompt_context_snapshot(
    cwd: &Path,
    workspace_instructions: &[WorkspaceInstruction],
    skills: &RuntimeSkillsSnapshot,
) -> RunPromptContextSnapshot {
    let runtime = AgentModelPayloadRuntime;
    prompt_context_snapshot_with_stable_prefix(
        cwd,
        workspace_instructions,
        skills,
        runtime.compile_stable_prefix(),
        runtime.compile_subagent_stable_prefix(),
    )
}

fn prompt_context_snapshot_with_stable_prefix(
    cwd: &Path,
    workspace_instructions: &[WorkspaceInstruction],
    skills: &RuntimeSkillsSnapshot,
    stable_prefix: AgentModelStablePrefix,
    subagent_stable_prefix: AgentModelStablePrefix,
) -> RunPromptContextSnapshot {
    let available_skills = skills.render_available();
    let context_messages = runtime_prompt_fragment_messages(RuntimePromptFragmentsInput {
        cwd: &cwd.to_string_lossy(),
        workspace_instructions,
        available_skills_context: &available_skills,
        selected_skill_prompt: None,
    });
    RunPromptContextSnapshot {
        stable_prefix,
        subagent_stable_prefix,
        workspace_context_messages: context_messages,
    }
}

fn replace_workspace_snapshot(
    config_path: &Path,
    cwd: &Path,
    workspace_root: Option<&Path>,
    stable: &RunStartupSnapshot,
) -> Result<RunStartupSnapshot, String> {
    let started = Instant::now();
    thread::scope(|scope| {
        let workspace = scope.spawn(|| {
            timed(STARTUP_STAGE_WORKSPACE, || {
                load_selected_workspace_instructions(workspace_root)
            })
        });
        let skills = scope.spawn(|| {
            timed(STARTUP_STAGE_SKILLS, || {
                crate::runtime_tools::load_runtime_skills_snapshot(config_path, workspace_root)
            })
        });
        let execution_tools = scope.spawn(|| {
            timed(STARTUP_STAGE_MCP_REGISTRY, || {
                crate::runtime_tools::build_runtime_tool_registry_snapshot(
                    config_path,
                    workspace_root,
                )
            })
        });

        let workspace = join(workspace, "Workspace instruction")?;
        let skills = join(skills, "Skill")?;
        let execution_tools = join(execution_tools, "MCP registry")?;
        let workspace_instructions = workspace.value?;
        let skills_snapshot = skills.value?;
        let prompt_context = timed(STARTUP_STAGE_PROMPT_CONTEXT, || {
            prompt_context_snapshot_with_stable_prefix(
                cwd,
                &workspace_instructions,
                &skills_snapshot,
                stable.prompt_context.stable_prefix.clone(),
                stable.prompt_context.subagent_stable_prefix.clone(),
            )
        });
        let timeline = RunStartupTimeline {
            total_ms: elapsed_ms(started),
            stages: vec![
                workspace.timing,
                skills.timing,
                execution_tools.timing,
                prompt_context.timing,
            ],
            mcp_schema_cache: mcp_cache_statuses(&execution_tools.value),
            skill_catalog_revision: skills_snapshot.revision().to_owned(),
        };

        Ok(RunStartupSnapshot {
            home_root: stable.home_root.clone(),
            workspace_root: workspace_root.map(|path| path.to_string_lossy().into_owned()),
            personal_memory: stable.personal_memory.clone(),
            user_agents: stable.user_agents.clone(),
            workspace_instructions,
            skills: skills_snapshot,
            execution_tools: execution_tools.value.registry,
            prompt_context: prompt_context.value,
            timeline,
            memory_write_runtime: Arc::clone(&stable.memory_write_runtime),
        })
    })
}

fn mcp_cache_statuses(
    execution_tools: &crate::runtime_tools::RuntimeExecutionToolSnapshot,
) -> Vec<RunStartupMcpCacheStatus> {
    execution_tools
        .mcp_schema_cache
        .iter()
        .map(|report| RunStartupMcpCacheStatus {
            server_id: report.server_id.clone(),
            disposition: report.disposition.to_owned(),
            failed: report.failure.is_some(),
            failure: report.failure.clone(),
        })
        .collect()
}

fn selected_workspace<'a>(cwd: &'a Path, home_root: &Path) -> Option<&'a Path> {
    (!cwd.starts_with(home_root)).then_some(cwd)
}

fn load_selected_workspace_instructions(
    workspace_root: Option<&Path>,
) -> Result<Vec<WorkspaceInstruction>, String> {
    workspace_root.map_or_else(|| Ok(Vec::new()), load_workspace_instructions)
}

fn join<T>(handle: thread::ScopedJoinHandle<'_, T>, label: &str) -> Result<T, String> {
    handle
        .join()
        .map_err(|_| format!("{label} 启动预热任务异常退出"))
}

#[cfg(test)]
pub(crate) fn test_startup_runtime(
    config_path: &Path,
    cwd: &Path,
    session_id: &str,
) -> (Arc<RunStartupSnapshot>, Arc<RwLock<ToolRegistry>>) {
    let startup =
        Arc::new(RunStartupSnapshot::build(config_path, cwd).expect("测试启动快照应可构造"));
    let execution_tools = startup.execution_tools_for_session(config_path, session_id);
    // 断言的是「测试快照不注入个人信息」，而不是「本机没有 owner identity」：
    // 后者取决于宿主环境（打包安装或按需补齐都会让身份存在），不是本测试的事实。
    assert!(
        startup
            .personal_memory
            .as_ref()
            .is_none_or(|snapshot| snapshot.context.is_empty()),
        "测试启动快照不得注入个人信息"
    );
    (startup, execution_tools)
}

#[cfg(test)]
#[path = "run_startup_snapshot_tests.rs"]
mod tests;
