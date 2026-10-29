//! CLI debug 报告生成。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axon_agent_runtime::{
    AgentControlRuntime, AgentPath, AgentToolRegistryRuntime, LocalAgentGraphStore,
    LspCatalogRuntime, ProviderToolCall, SubagentSessionRuntime, ToolRegistry,
};
use axon_config::ConfigLoader;
use axon_domain::{
    CellId, ModelProfileId, PermissionSnapshotId, SessionId, ToolCallInput, ToolInputFormat,
};
use serde::Serialize;

use crate::run_commands::{
    AgentMemoryWriteRuntime, load_run_env_file,
    run_channel_affinity::{
        ChannelConnectionMetrics, channel_metrics_path, load_channel_connection_metrics,
        load_channel_metrics,
    },
    run_subagent_executor::NoopRunSubagentExecutor,
};
use crate::tool_orchestrator::{
    NON_PROVIDER_TURN_ID, ToolOrchestratorContext, ToolOutputStream, ToolRuntimeEvent,
    execute_tool_call_with_observer, now_ms, schedule_tool_calls,
};
use crate::{
    CliRunResult, DebugBuiltinCommand, DebugBuiltinRunCommand, DebugBuiltinSubcommand,
    DebugChannelMetricsCommand, DebugCommand, DebugSubcommand, OutputFormat,
    registered_command_names,
};

const DEBUG_BUILTIN_CALL_ID: &str = "debug-builtin-call";
/// debug 入口的 principal 渠道引用。
const DEBUG_PRINCIPAL_CHANNEL_REF: &str = "local-debug";
const DEBUG_BUILTIN_TIMEOUT_MIN_SECS: u64 = 1;
const DEBUG_CHANNEL_METRICS_MIN_SAMPLES: u32 = 3;
const DEBUG_CHANNEL_METRICS_SUCCESS_WARNING_PCT: f64 = 95.0;
const DEBUG_CHANNEL_METRICS_P95_WARNING_MS: f64 = 2_000.0;
const DEBUG_CHANNEL_METRICS_CACHE_WARNING_BPS: u16 = 5_000;
const DEBUG_CHANNEL_METRICS_MAX_SAMPLES_PER_CHANNEL: usize = 256;
const DEBUG_MILLISECONDS_PER_HOUR: u64 = 3_600_000;

/// debug 报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugReport {
    /// axon-cli 版本。
    pub version: String,
    /// 已注册命令。
    pub registered_commands: Vec<String>,
    /// 配置来源键。
    pub config_keys: Vec<String>,
    /// OpenCode 对标的 LSP server 总数。
    pub lsp_server_count: usize,
    /// P1-3 第一阶段核心 diagnostics server。
    pub lsp_core_servers: Vec<String>,
    /// 当前机器 PATH 中已探测到的 LSP server。
    pub lsp_available_servers: Vec<String>,
    /// OpenCode LSP 对外暴露的能力。
    pub lsp_exposed_capabilities: Vec<String>,
}

/// builtin 工具 debug 执行报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugBuiltinRunReport {
    /// 工具名。
    pub tool_name: String,
    /// 执行状态。
    pub status: String,
    /// 回执摘要。
    pub summary: String,
    /// 工具 metadata。
    pub metadata: Option<serde_json::Value>,
    /// 错误类型。
    pub error_type: Option<String>,
    /// 是否发生文件写入。
    pub filesystem_write_performed: bool,
    /// progress/output 事件。
    pub events: Vec<DebugBuiltinRuntimeEvent>,
}

/// builtin 工具 debug 事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugBuiltinRuntimeEvent {
    /// 事件类型。
    pub kind: String,
    /// 事件阶段或 stream。
    pub phase: String,
    /// 当前条目。
    pub current_item: Option<String>,
    /// 文本内容。
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMetricsDebugReport {
    /// 指标生成时间。
    pub generated_at_ms: u64,
    /// 指标范围；来自本地持久化分片。
    pub scope: &'static str,
    /// 指标持久化文件路径。
    pub data_path: String,
    /// 按动态评分从优到劣排列的渠道。
    pub channels: Vec<ChannelMetricsDebugEntry>,
}

/// 单个渠道的可读指标和解读。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMetricsDebugEntry {
    pub channel_key: String,
    pub sample_count: u32,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub success_rate_pct: f64,
    pub cache_hit_rate_pct: f64,
    pub adjusted_score: f64,
    pub metrics_applied: bool,
    pub interpretation: String,
    pub connection_observation_count: u32,
    pub connection_reuse_rate_pct: f64,
    pub fallback_count: u32,
    pub avg_connection_age_ms: Option<f64>,
}

impl DebugReport {
    pub(crate) fn collect() -> Result<Self, String> {
        let loaded = ConfigLoader::load_defaults()
            .map_err(|error| format!("默认配置加载失败：{error:?}"))?;
        Ok(Self {
            lsp_server_count: LspCatalogRuntime::catalog().len(),
            lsp_core_servers: LspCatalogRuntime::core_diagnostics_servers(),
            lsp_available_servers: LspCatalogRuntime::summary()
                .available_servers
                .into_iter()
                .map(|server| server.id)
                .collect(),
            lsp_exposed_capabilities: LspCatalogRuntime::summary().exposed_capabilities,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            registered_commands: registered_command_names(),
            config_keys: loaded
                .report
                .entries
                .iter()
                .map(|entry| entry.key.to_owned())
                .collect(),
        })
    }

    pub(crate) fn to_text(&self) -> String {
        let mut output = format!("Axon debug {}\n", self.version);
        let _ = writeln!(output, "- commands: {}", self.registered_commands.join(","));
        let _ = writeln!(output, "- config_keys: {}", self.config_keys.join(","));
        let _ = writeln!(output, "- lsp_server_count: {}", self.lsp_server_count);
        let _ = writeln!(
            output,
            "- lsp_core_servers: {}",
            self.lsp_core_servers.join(",")
        );
        let _ = writeln!(
            output,
            "- lsp_available_servers: {}",
            self.lsp_available_servers.join(",")
        );
        let _ = writeln!(
            output,
            "- lsp_exposed_capabilities: {}",
            self.lsp_exposed_capabilities.join(",")
        );
        output
    }

    pub(crate) fn to_json_line(&self) -> String {
        format!(
            "{}\n",
            // Invariant: DebugReport contains only JSON-serializable fields.
            serde_json::to_string(self).expect("DebugReport 序列化不应失败")
        )
    }
}

pub(crate) fn run_debug(
    command: &DebugCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> CliRunResult {
    match command.command.as_ref() {
        None => run_overview_debug(format),
        Some(DebugSubcommand::Builtin(command)) => {
            run_builtin_debug(command, format, config_path, cwd)
        }
        Some(DebugSubcommand::ChannelMetrics(command)) => {
            run_channel_metrics_debug(command, format, config_path)
        }
        Some(DebugSubcommand::Mailbox(command)) => {
            super::management_debug_mailbox::run_mailbox_debug(command, format, config_path)
        }
        Some(DebugSubcommand::Session(command)) => {
            crate::session_commands::run_session(command, format, config_path)
        }
        Some(DebugSubcommand::RealtimeMetrics(command)) => {
            super::management_debug_realtime::run_realtime_metrics_debug(
                command,
                format,
                config_path,
            )
        }
    }
}

fn run_overview_debug(format: OutputFormat) -> CliRunResult {
    let report = match DebugReport::collect() {
        Ok(report) => report,
        Err(message) => return failed(message),
    };
    let stdout = match format {
        OutputFormat::Text => report.to_text(),
        OutputFormat::Json => report.to_json_line(),
    };
    ok(stdout)
}

fn run_builtin_debug(
    command: &DebugBuiltinCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> CliRunResult {
    match &command.command {
        DebugBuiltinSubcommand::List => run_builtin_list(format),
        DebugBuiltinSubcommand::Run(command) => run_builtin_tool(command, format, config_path, cwd),
    }
}

fn run_builtin_list(format: OutputFormat) -> CliRunResult {
    let tools = match builtin_tool_names() {
        Ok(tools) => tools,
        Err(message) => return failed(message),
    };
    let stdout = match format {
        OutputFormat::Text => format!("Axon debug builtin tools\n- {}\n", tools.join("\n- ")),
        OutputFormat::Json => {
            format!("{}\n", serde_json::json!({ "tools": tools }))
        }
    };
    ok(stdout)
}

fn run_channel_metrics_debug(
    command: &DebugChannelMetricsCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
) -> CliRunResult {
    let resolved_config_path = match resolved_debug_config_path(config_path) {
        Ok(path) => path,
        Err(message) => return failed(message),
    };
    let data_path = match channel_metrics_path(&resolved_config_path) {
        Ok(path) => path,
        Err(message) => return failed(message),
    };
    let since_ms = (command.since_hours > 0).then(|| {
        now_ms().saturating_sub(
            command
                .since_hours
                .saturating_mul(DEBUG_MILLISECONDS_PER_HOUR),
        )
    });
    let router = match load_channel_metrics(
        &resolved_config_path,
        since_ms,
        command.model.as_deref(),
        command.provider.as_deref(),
        DEBUG_CHANNEL_METRICS_MAX_SAMPLES_PER_CHANNEL,
        command.limit,
    ) {
        Ok(router) => router,
        Err(message) => return failed(message),
    };
    let connection_metrics = match load_channel_connection_metrics(
        &resolved_config_path,
        since_ms,
        command.model.as_deref(),
        command.provider.as_deref(),
        command.limit,
    ) {
        Ok(metrics) => metrics,
        Err(message) => return failed(message),
    };
    let mut channels: Vec<ChannelMetricsDebugEntry> = Vec::with_capacity(command.limit);
    for metrics in router.all_metrics() {
        let score = router.compute_score(&metrics.channel_key, 0);
        let interpretation = channel_metrics_interpretation(metrics);
        let connection = connection_metrics
            .get(&metrics.channel_key)
            .cloned()
            .unwrap_or_default();
        let entry = ChannelMetricsDebugEntry {
            channel_key: metrics.channel_key.clone(),
            sample_count: metrics.sample_count,
            avg_latency_ms: metrics.avg_latency_ms,
            p95_latency_ms: metrics.p95_latency_ms,
            success_rate_pct: metrics.success_rate_pct,
            cache_hit_rate_pct: f64::from(metrics.cache_hit_rate_basis_points) / 100.0,
            adjusted_score: score.adjusted_score,
            metrics_applied: score.metrics_applied,
            interpretation,
            connection_observation_count: connection.observation_count,
            connection_reuse_rate_pct: connection_reuse_rate_pct(&connection),
            fallback_count: connection.fallback_count,
            avg_connection_age_ms: average_connection_age_ms(&connection),
        };
        if command.limit == 0 {
            continue;
        }
        let insert_at =
            channels.partition_point(|current| current.adjusted_score <= entry.adjusted_score);
        channels.insert(insert_at, entry);
        if channels.len() > command.limit {
            channels.pop();
        }
    }
    let report = ChannelMetricsDebugReport {
        generated_at_ms: now_ms(),
        scope: "persisted",
        data_path: data_path.display().to_string(),
        channels,
    };
    match format {
        OutputFormat::Text => ok(channel_metrics_debug_text(&report)),
        OutputFormat::Json => ok(format!(
            "{}\n",
            serde_json::to_string(&report).expect("渠道指标报告序列化不应失败")
        )),
    }
}

fn channel_metrics_interpretation(metrics: &axon_agent_runtime::ChannelMetrics) -> String {
    if metrics.sample_count < DEBUG_CHANNEL_METRICS_MIN_SAMPLES {
        return "样本不足，当前仅沿用静态优先级和权重".to_owned();
    }
    if metrics.success_rate_pct < DEBUG_CHANNEL_METRICS_SUCCESS_WARNING_PCT {
        return "成功率偏低，动态评分已增加失败惩罚".to_owned();
    }
    if metrics.p95_latency_ms >= DEBUG_CHANNEL_METRICS_P95_WARNING_MS {
        return "p95 延迟偏高，建议检查 Provider 排队或限流".to_owned();
    }
    if metrics.cache_hit_rate_basis_points < DEBUG_CHANNEL_METRICS_CACHE_WARNING_BPS {
        return "缓存命中率偏低，建议检查稳定前缀、工具集合或渠道粘性".to_owned();
    }
    "指标稳定，可参与新会话动态选路".to_owned()
}

fn channel_metrics_debug_text(report: &ChannelMetricsDebugReport) -> String {
    let mut output = format!(
        "Axon debug channel-metrics (scope={}, data_path={}, generated_at_ms={})\n",
        report.scope, report.data_path, report.generated_at_ms
    );
    if report.channels.is_empty() {
        output.push_str("- 没有匹配的已持久化渠道指标\n");
        return output;
    }
    for channel in &report.channels {
        let _ = writeln!(
            output,
            "- {}: samples={} avg_ms={:.1} p95_ms={:.1} success_pct={:.2} cache_hit_pct={:.2} score={:.3} metrics_applied={} connections={} reuse_pct={:.2} fallback_count={} avg_connection_age_ms={} interpretation={}",
            channel.channel_key,
            channel.sample_count,
            channel.avg_latency_ms,
            channel.p95_latency_ms,
            channel.success_rate_pct,
            channel.cache_hit_rate_pct,
            channel.adjusted_score,
            channel.metrics_applied,
            channel.connection_observation_count,
            channel.connection_reuse_rate_pct,
            channel.fallback_count,
            channel
                .avg_connection_age_ms
                .map_or_else(|| "-".to_owned(), |value| format!("{value:.1}")),
            channel.interpretation
        );
    }
    output
}

fn connection_reuse_rate_pct(metrics: &ChannelConnectionMetrics) -> f64 {
    if metrics.reusable_observation_count == 0 {
        return 0.0;
    }
    f64::from(metrics.reused_connection_count) * 100.0
        / f64::from(metrics.reusable_observation_count)
}

fn average_connection_age_ms(metrics: &ChannelConnectionMetrics) -> Option<f64> {
    (metrics.connection_age_count > 0)
        .then(|| metrics.connection_age_total_ms as f64 / f64::from(metrics.connection_age_count))
}

fn run_builtin_tool(
    command: &DebugBuiltinRunCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> CliRunResult {
    let tool_name = command.tool.trim().to_owned();
    let config_path = match resolved_debug_config_path(config_path) {
        Ok(path) => path,
        Err(message) => return failed(message),
    };
    if let Err(message) =
        load_run_env_file(&crate::setup_commands::env_path_for_config(&config_path))
    {
        return failed(message);
    }
    match ConfigLoader::load_from_path(&config_path) {
        Ok(_) => {}
        Err(error) => return failed(format!("{error:?}")),
    }
    let cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let context = debug_tool_context(config_path, cwd, debug_timeout(command.timeout_secs));
    let registry = context
        .execution_tools
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let input = match parse_debug_tool_input(&registry, &tool_name, &command.args) {
        Ok(input) => input,
        Err(message) => return failed(message),
    };
    let tool_call = ProviderToolCall::new(DEBUG_BUILTIN_CALL_ID, tool_name, input);
    let request = {
        match schedule_tool_calls(
            &registry,
            std::slice::from_ref(&tool_call),
            NON_PROVIDER_TURN_ID,
            &context.cwd,
            context.session_id,
            context.cell_id,
            now_ms(),
        ) {
            Ok(mut requests) => requests.remove(0),
            Err(message) => return failed(message),
        }
    };
    let mut events = Vec::new();
    let mut observer = |event| events.push(project_runtime_event(event));
    let receipt = execute_tool_call_with_observer(&context, &request, Some(&mut observer));
    let report = DebugBuiltinRunReport {
        tool_name: receipt.tool_name,
        status: receipt.status,
        summary: receipt.summary,
        metadata: receipt.metadata,
        error_type: receipt.error_type,
        filesystem_write_performed: receipt.filesystem_write_performed,
        events,
    };
    let stdout = match format {
        OutputFormat::Text => debug_builtin_report_text(&report),
        OutputFormat::Json => {
            format!(
                "{}\n",
                // Invariant: DebugBuiltinRunReport contains only JSON-serializable fields.
                serde_json::to_string(&report).expect("DebugBuiltinRunReport 序列化不应失败")
            )
        }
    };
    CliRunResult {
        exit_code: u8::from(report.status != "completed"),
        stdout,
        stderr: String::new(),
    }
}

fn debug_tool_context(
    config_path: PathBuf,
    cwd: PathBuf,
    timeout: Duration,
) -> ToolOrchestratorContext {
    let execution_tools = Arc::new(std::sync::RwLock::new(
        crate::runtime_tools::build_execution_tool_registry_for_session(
            Some(&config_path),
            Some(&cwd),
            "debug-session",
        ),
    ));
    let memory_write_runtime = Arc::new(debug_memory_write_runtime(&config_path));
    ToolOrchestratorContext {
        config_path: Some(config_path),
        cwd,
        session_id: SessionId(1),
        thread_session_id: "debug-session".to_owned(),
        cell_id: CellId(1),
        model_messages: Vec::new(),
        execution_tools,
        memory_write_runtime,
        timeout,
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        run_cancel_token: None,
        model_profile_id: ModelProfileId(0),
        supports_vision: true,
        permission_snapshot_id: PermissionSnapshotId(0),
        current_agent_path: AgentPath::root(),
        agent_control: Arc::new(Mutex::new(AgentControlRuntime::default())),
        agent_graph_store: Arc::new(LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(SubagentSessionRuntime::default())),
        subagent_executor: Arc::new(NoopRunSubagentExecutor),
        subagent_cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
        workspace_files: Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        user_question_port: None,
    }
}

/// 构造 debug 入口的 Memory 写入 runtime。
///
/// 与 `axon run` 共用同一条 principal 加载路径与同一个按需补齐点：debug 入口曾是唯一
/// 不加载 principal 的入口，导致同一台机器在 run 可用、debug 被 fail-closed 拒绝。
fn debug_memory_write_runtime(config_path: &Path) -> AgentMemoryWriteRuntime {
    let Ok(root) = crate::run_commands::resolved_axon_root(config_path) else {
        return AgentMemoryWriteRuntime::new(None, Some("无法解析用户级 Axon 目录".to_owned()));
    };
    let (owner_id, diagnostic) = match crate::runtime_principal_context::load_run_principal(
        &root,
        DEBUG_PRINCIPAL_CHANNEL_REF,
    ) {
        Ok(principal) => (principal.map(|value| value.owner_id), None),
        Err(error) => (None, Some(error)),
    };
    AgentMemoryWriteRuntime::new(owner_id, diagnostic).with_identity_root(Some(root))
}

fn project_runtime_event(event: ToolRuntimeEvent) -> DebugBuiltinRuntimeEvent {
    match event {
        ToolRuntimeEvent::Progress(progress) => DebugBuiltinRuntimeEvent {
            kind: "progress".to_owned(),
            phase: progress.phase,
            current_item: progress.current_item,
            text: progress.message,
        },
        ToolRuntimeEvent::Output(output) => DebugBuiltinRuntimeEvent {
            kind: "output".to_owned(),
            phase: match output.stream {
                ToolOutputStream::Stdout => "stdout",
                ToolOutputStream::Stderr => "stderr",
            }
            .to_owned(),
            current_item: None,
            text: Some(output.text),
        },
    }
}

fn debug_builtin_report_text(report: &DebugBuiltinRunReport) -> String {
    let mut output = format!("Axon debug builtin {}\n", report.tool_name);
    let _ = writeln!(output, "- status: {}", report.status);
    let _ = writeln!(
        output,
        "- filesystem_write_performed: {}",
        report.filesystem_write_performed
    );
    // 会话/作业 ID 只存在于 metadata 中；文本输出此前不投影，导致经本通道触发的
    // Running 会话无法被续接（`write_stdin` 需要 session_id）。JSON 输出一直包含完整
    // metadata，这里补齐文本通道，与工具的 `always 返回 session_id` 契约对齐。
    if let Some(metadata) = report.metadata.as_ref() {
        for (label, key) in [("session_id", "sessionId"), ("job_id", "jobId")] {
            if let Some(value) = metadata.get(key).and_then(serde_json::Value::as_str) {
                let _ = writeln!(output, "- {label}: {value}");
            }
        }
    }
    let _ = writeln!(output, "- summary:\n{}", report.summary);
    if !report.events.is_empty() {
        let _ = writeln!(output, "- events:");
        for event in &report.events {
            let text = event.text.as_deref().unwrap_or_default();
            let _ = writeln!(output, "  - {}:{} {}", event.kind, event.phase, text);
        }
    }
    output
}

fn builtin_tool_names() -> Result<Vec<String>, String> {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry)
        .map_err(|error| format!("{error:?}"))?;
    let mut tools = registry
        .tools()
        .iter()
        .map(|tool| format!("{}.{}", tool.namespace, tool.name))
        .collect::<Vec<_>>();
    tools.sort();
    Ok(tools)
}

fn parse_debug_tool_input(
    registry: &ToolRegistry,
    tool_name: &str,
    arguments: &str,
) -> Result<ToolCallInput, String> {
    let tool = registry
        .tool_by_qualified_name(tool_name)
        .ok_or_else(|| format!("工具 {tool_name} 未注册"))?;
    match &tool.schema.input_format {
        ToolInputFormat::JsonSchema => {
            let value = serde_json::from_str::<serde_json::Value>(arguments)
                .map_err(|_| "builtin debug JSON Schema 工具参数必须是 JSON object".to_owned())?;
            if !value.is_object() {
                return Err("builtin debug JSON Schema 工具参数必须是 JSON object".to_owned());
            }
            Ok(ToolCallInput::json(value))
        }
        ToolInputFormat::Freeform { .. } => serde_json::from_str::<String>(arguments)
            .map(ToolCallInput::text)
            .map_err(|_| "builtin debug freeform 工具参数必须是 JSON string".to_owned()),
    }
}

fn resolved_debug_config_path(config_path: Option<&Path>) -> Result<PathBuf, String> {
    config_path.map(Path::to_path_buf).map_or_else(
        || ConfigLoader::default_user_config_path().map_err(|_| "无法解析用户配置路径".to_owned()),
        Ok,
    )
}

fn debug_timeout(seconds: u64) -> Duration {
    Duration::from_secs(seconds.max(DEBUG_BUILTIN_TIMEOUT_MIN_SECS))
}

fn ok(stdout: String) -> CliRunResult {
    CliRunResult {
        exit_code: 0,
        stdout,
        stderr: String::new(),
    }
}

fn failed(message: impl Into<String>) -> CliRunResult {
    CliRunResult {
        exit_code: 1,
        stdout: format!("{}\n", message.into()),
        stderr: String::new(),
    }
}
