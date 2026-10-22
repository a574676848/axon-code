use axon_agent_runtime::{
    AgentRunShellToolArgs, ContentArchiveRuntime, LspBackgroundRuntime, MaybeApplyPatch,
    ProviderToolCall, ShellCommandExecutionInput, ShellCommandOutputChunk,
    ShellCommandOutputStream, ShellCommandPolicyEvaluator, ShellCommandToolRuntime,
    agent_run_job_started_tool_receipt, agent_run_shell_tool_args_from_provider_input,
    agent_run_shell_tool_receipt, agent_run_write_stdin_tool_input_from_provider_args,
    maybe_parse_apply_patch,
};
use axon_domain::OutputRef;
use serde_json::json;
use std::borrow::ToOwned;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};

use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolOrchestratorContext, ToolOutputChunk, ToolOutputStream, ToolReceipt,
    ToolRuntimeEvent, ToolRuntimeObserver, blocked_tool_receipt, failed_tool_input_receipt,
    failed_tool_receipt, failed_tool_receipt_from_axon_error, tool_receipt_from_runtime,
};
use crate::tool_orchestrator_hashline::execute_apply_patch_strategy;

const TOOL_METADATA_ARCHIVE_REF_KEY: &str = "archiveRef";

pub(crate) fn execute_shell(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    output_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    match request.tool_call.dispatch_name() {
        "builtin.shell_command" => execute_shell_command(request, context, output_observer),
        "builtin.write_stdin" => execute_write_stdin(request, context, output_observer),
        name => failed_tool_receipt(&request.tool_call, format!("未知 Shell 工具：{name}")),
    }
}

fn execute_shell_command(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    output_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    let tool_call = &request.tool_call;
    let args = match agent_run_shell_tool_args_from_provider_input(&tool_call.input, &context.cwd) {
        Ok(args) => args,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let evaluation = ShellCommandPolicyEvaluator::evaluate(&tool_call.input, &context.cwd);
    execute_shell_args(request, context, output_observer, args, evaluation, true)
}

fn execute_shell_args(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    mut output_observer: Option<&mut ToolRuntimeObserver<'_>>,
    args: AgentRunShellToolArgs,
    evaluation: axon_agent_runtime::ShellCommandPolicyEvaluation,
    allow_apply_patch: bool,
) -> ToolReceipt {
    let tool_call = &request.tool_call;
    // 后台作业是**另一条通道**：它不产生 session_id，也不经前台输出的流式观察者，
    // 因此必须在 apply_patch 拦截与会话执行之前分流。
    if args.run_in_background {
        return execute_shell_background(tool_call, context, args);
    }
    let cwd = args.cwd;
    if allow_apply_patch
        && let Some(receipt) =
            execute_shell_apply_patch(request, context, &mut output_observer, &args.command, &cwd)
    {
        return receipt;
    }
    let command = args.command;
    let mut observer = output_observer.map(|observer| {
        let provider_call_id = tool_call.id.clone();
        let tool_call_id = request.tool_call_id;
        let part_id = request.part_id.clone();
        let tool_name = tool_call.dispatch_name().to_owned();
        let kind = request.kind;
        move |chunk: ShellCommandOutputChunk| {
            observer(ToolRuntimeEvent::Output(ToolOutputChunk {
                tool_call_id,
                part_id: part_id.clone(),
                provider_call_id: provider_call_id.clone(),
                tool_name: tool_name.clone(),
                kind,
                stream: match chunk.stream {
                    ShellCommandOutputStream::Stdout => ToolOutputStream::Stdout,
                    ShellCommandOutputStream::Stderr => ToolOutputStream::Stderr,
                },
                sequence: chunk.seq,
                text: String::from_utf8_lossy(&chunk.data).into_owned(),
            }));
        }
    });
    match ShellCommandToolRuntime.execute_with_output_observer(
        ShellCommandExecutionInput {
            owner_id: context.thread_session_id.clone(),
            command,
            cwd: cwd.clone(),
            timeout_ms: args.timeout_ms,
            yield_time_ms: args.yield_time_ms,
            tty: args.tty,
            run_in_background: args.run_in_background,
            max_output_tokens: args.max_output_tokens,
            remote_requested: false,
            tee_root: shell_tee_root(context),
            cancel_token: context.shell_cancel_token(),
            write_paths: evaluation.affected_paths,
        },
        observer.as_mut().map(|observer| observer as &mut _),
    ) {
        Ok(receipt) => match project_shell_receipt(tool_call, context, &receipt) {
            Ok(receipt) => receipt,
            Err(message) => blocked_tool_receipt(tool_call, message),
        },
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

/// 把命令转为后台作业，并把作业 ID 交给模型。
///
/// 回执不返回 `session_id`：作业注册在独立的作业注册表里，
/// 只能通过 `job_output` / `job_kill` 操作。
fn execute_shell_background(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
    args: AgentRunShellToolArgs,
) -> ToolReceipt {
    match ShellCommandToolRuntime.start_background(ShellCommandExecutionInput {
        owner_id: context.thread_session_id.clone(),
        command: args.command,
        cwd: args.cwd,
        timeout_ms: args.timeout_ms,
        yield_time_ms: args.yield_time_ms,
        tty: args.tty,
        run_in_background: args.run_in_background,
        max_output_tokens: args.max_output_tokens,
        remote_requested: false,
        tee_root: shell_tee_root(context),
        cancel_token: context.shell_cancel_token(),
        write_paths: Vec::new(),
    }) {
        Ok(job_id) => {
            tool_receipt_from_runtime(agent_run_job_started_tool_receipt(tool_call, job_id))
        }
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn execute_write_stdin(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    output_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    let tool_call = &request.tool_call;
    let input = match agent_run_write_stdin_tool_input_from_provider_args(
        &tool_call.canonical_input(),
        &context.thread_session_id,
    ) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let mut observer = output_observer.map(|observer| {
        let provider_call_id = tool_call.id.clone();
        let tool_call_id = request.tool_call_id;
        let part_id = request.part_id.clone();
        let tool_name = tool_call.dispatch_name().to_owned();
        let kind = request.kind;
        move |chunk: ShellCommandOutputChunk| {
            observer(ToolRuntimeEvent::Output(ToolOutputChunk {
                tool_call_id,
                part_id: part_id.clone(),
                provider_call_id: provider_call_id.clone(),
                tool_name: tool_name.clone(),
                kind,
                stream: match chunk.stream {
                    ShellCommandOutputStream::Stdout => ToolOutputStream::Stdout,
                    ShellCommandOutputStream::Stderr => ToolOutputStream::Stderr,
                },
                sequence: chunk.seq,
                text: String::from_utf8_lossy(&chunk.data).into_owned(),
            }));
        }
    });
    match ShellCommandToolRuntime.write_stdin_with_output_observer(
        input,
        observer.as_mut().map(|observer| observer as &mut _),
    ) {
        Ok(receipt) => match project_shell_receipt(tool_call, context, &receipt) {
            Ok(receipt) => receipt,
            Err(message) => blocked_tool_receipt(tool_call, message),
        },
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

pub(super) fn shell_tee_root(context: &ToolOrchestratorContext) -> Option<std::path::PathBuf> {
    context
        .config_path
        .as_deref()
        .and_then(|path| crate::runtime_paths::runtime_axon_log_path_static(path).ok())
}

fn project_shell_receipt(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
    receipt: &axon_agent_runtime::ShellCommandExecutionReceipt,
) -> Result<ToolReceipt, String> {
    let mut tool_receipt =
        tool_receipt_from_runtime(agent_run_shell_tool_receipt(tool_call, receipt));
    if receipt.full_output_path.is_some() {
        let output_ref = archive_shell_output(context, tool_call, receipt, &tool_receipt.summary)?
            .ok_or_else(|| "工具完整输出存在但未生成 OutputRef，已阻断回执".to_owned())?;
        let archive_root = context
            .config_path
            .as_deref()
            .ok_or_else(|| "工具完整输出存在但缺少配置路径，已阻断回执".to_owned())
            .and_then(|config_path| {
                crate::runtime_paths::runtime_axon_data_path_static(config_path)
                    .map_err(ToOwned::to_owned)
            })?;
        attach_output_archive(&mut tool_receipt, output_ref, &archive_root)?;
    }
    attach_post_run_lsp(&mut tool_receipt, &receipt.cwd, &receipt.changed_paths);
    Ok(tool_receipt)
}

/// 归档引用缓存：key = (tee 归档源路径, 会话输出代数)。
///
/// `write_stdin` 轮询时回执会被反复重建，若无缓存，每次都要全量读取 tee 归档并
/// 重算 SHA-256（R2 审计 N-B-02：归档单调增长时判重永不命中，O(N·S) 热路径）。
/// tee 归档**只追加**，代数（`output_generation`）未变即内容未变：直接复用上次
/// 归档引用，把每轮询成本降为 O(1)。同源新代插入时移除旧代条目，缓存规模以
/// 活跃会话数为上界。前提：`config_path`（即归档根）进程内恒定：归档根由启动
/// 配置决定，运行期不变。
fn archived_output_refs() -> &'static Mutex<HashMap<(String, u64), OutputRef>> {
    static ARCHIVED_OUTPUT_REFS: OnceLock<Mutex<HashMap<(String, u64), OutputRef>>> =
        OnceLock::new();
    ARCHIVED_OUTPUT_REFS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn archive_shell_output(
    context: &ToolOrchestratorContext,
    tool_call: &ProviderToolCall,
    receipt: &axon_agent_runtime::ShellCommandExecutionReceipt,
    preview: &str,
) -> Result<Option<OutputRef>, String> {
    let Some(source_path) = receipt.full_output_path.as_deref() else {
        return Ok(None);
    };
    let cache_key = (source_path.to_owned(), receipt.output_generation);
    if let Some(cached) = archived_output_refs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&cache_key)
    {
        return Ok(Some(cached.clone()));
    }
    let archive_root = context
        .config_path
        .as_deref()
        .ok_or_else(|| "工具完整输出存在但缺少配置路径，已阻断回执".to_owned())
        .and_then(|config_path| {
            crate::runtime_paths::runtime_axon_data_path_static(config_path)
                .map_err(ToOwned::to_owned)
        })?;
    let output_ref = ContentArchiveRuntime::default()
        .archive_file(
            &archive_root,
            std::path::Path::new(source_path),
            &axon_agent_runtime::tool_output_source_trace_from_dispatch_name(
                tool_call.dispatch_name(),
            ),
            preview,
        )
        .map(Some)
        .map_err(|error| format!("工具完整输出归档失败，已阻断回执：{error}"))?;
    let mut cache = archived_output_refs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.retain(|(source, _), _| source != source_path);
    if let Some(output_ref) = output_ref.clone() {
        cache.insert(cache_key, output_ref);
    }
    Ok(output_ref)
}

fn attach_output_archive(
    tool_receipt: &mut ToolReceipt,
    output_ref: OutputRef,
    archive_root: &std::path::Path,
) -> Result<(), String> {
    let output_ref_value = serde_json::to_value(&output_ref)
        .map_err(|error| format!("OutputRef metadata 序列化失败，已阻断回执：{error}"))?;
    let metadata = tool_receipt
        .metadata
        .get_or_insert_with(|| serde_json::json!({}));
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| "工具回执 metadata 必须是对象，已阻断回执".to_owned())?;
    object.insert(TOOL_METADATA_ARCHIVE_REF_KEY.to_owned(), output_ref_value);
    if let Some(archive_ref) = output_ref.archive_ref.as_ref() {
        object.insert(
            "archivePath".to_owned(),
            json!(
                archive_root
                    .join(&archive_ref.relative_path)
                    .display()
                    .to_string()
            ),
        );
    }
    tool_receipt.output_ref = Some(output_ref);
    Ok(())
}

fn attach_post_run_lsp(
    tool_receipt: &mut ToolReceipt,
    cwd: &str,
    changed_paths: &[std::path::PathBuf],
) {
    if changed_paths.is_empty() {
        return;
    }
    let lsp = serde_json::to_value(LspBackgroundRuntime::schedule(
        std::path::Path::new(cwd),
        changed_paths.to_vec(),
    ))
    .unwrap_or_else(|_| json!({"status": "unavailable", "files": []}));
    let touched_count = lsp
        .get("files")
        .and_then(serde_json::Value::as_array)
        .map_or(0, std::vec::Vec::len);
    let metadata = tool_receipt
        .metadata
        .get_or_insert_with(|| serde_json::json!({}));
    if let Some(object) = metadata.as_object_mut() {
        object.insert(
            "writeIntent".to_owned(),
            json!({
                "strategy": "static_shell_path_detection",
                "affectedPaths": changed_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>(),
            }),
        );
        object.insert("postRunLsp".to_owned(), lsp);
    }
    tool_receipt.filesystem_write_performed = true;
    let _ = write!(
        tool_receipt.summary,
        " post_run_lsp_scheduled={touched_count}"
    );
}

fn execute_shell_apply_patch(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    output_observer: &mut Option<&mut ToolRuntimeObserver<'_>>,
    command: &[String],
    cwd: &std::path::Path,
) -> Option<ToolReceipt> {
    let MaybeApplyPatch::Body(parsed) = maybe_parse_apply_patch(command) else {
        return None;
    };
    let mut effective_context = context.clone();
    effective_context.cwd = parsed.workdir.as_deref().map_or_else(
        || cwd.to_path_buf(),
        |workdir| cwd.join(std::path::Path::new(workdir)),
    );
    Some(execute_apply_patch_strategy(
        request,
        &effective_context,
        &parsed.patch,
        "shell apply_patch 结构化补丁写入".to_owned(),
        output_observer.take(),
    ))
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
