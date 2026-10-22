use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolOrchestratorContext, ToolReceipt, ToolRuntimeObserver,
    failed_tool_input_receipt, failed_tool_receipt, tool_receipt_from_runtime,
};
use axon_agent_runtime::{
    AgentRunEditToolReceiptInput, HASHLINE_NOOP_CHANGE_MARKER, HashlineApplyFile,
    HashlineApplyRequest, HashlineFileApplyReport, HashlineFileOperation, WorkspaceFileSnapshot,
    WorkspaceFileSnapshotLookup, agent_run_edit_input_is_apply_patch,
    agent_run_edit_tool_args_from_provider_input, agent_run_edit_tool_receipt, apply_hashline_edit,
    file_change_with_hashline_snapshot, hashline_headers,
};
use axon_patch_model::{PatchFileChange, PatchFileOperation, PatchPlan, inherit_text_file_format};
const EDIT_STRATEGY_HASHLINE: &str = "hashline";
const EDIT_STRATEGY_APPLY_PATCH: &str = "apply_patch";
const PATCH_OPERATION_ADD: &str = "add";
const PATCH_OPERATION_DELETE: &str = "delete";

struct PreparedHashlineFiles {
    files: Vec<HashlineApplyFile>,
    resolved_paths: HashMap<String, ResolvedHashlinePath>,
}
struct ResolvedHashlinePath {
    path: String,
    recovered: bool,
}

struct ResolvedHashlineFile {
    path: String,
    canonical_path: PathBuf,
    current_content: String,
    snapshot: WorkspaceFileSnapshot,
    recovered: bool,
}

/// 执行统一文件编辑；Hashline 用于已读取文件，PathDiff 用于文件级变更。
pub(crate) fn execute_edit(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    mut runtime_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    let tool_call = &request.tool_call;
    let input = match agent_run_edit_tool_args_from_provider_input(&tool_call.input) {
        Ok(input) => input,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    if agent_run_edit_input_is_apply_patch(&input.input) {
        return execute_apply_patch_strategy(
            request,
            context,
            &input.input,
            input.summary,
            runtime_observer,
        );
    }

    let headers = match hashline_headers(&input.input) {
        Ok(headers) => headers,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    let PreparedHashlineFiles {
        files,
        resolved_paths,
    } = match prepare_hashline_files(context, &headers) {
        Ok(files) => files,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    let mut report = match apply_hashline_edit(HashlineApplyRequest {
        input: input.input,
        files,
    }) {
        Ok(report) => report,
        Err(message) => return failed_tool_receipt(tool_call, message),
    };
    if let Err(message) = apply_resolved_hashline_paths(&mut report.files, resolved_paths) {
        return failed_tool_receipt(tool_call, message);
    }
    let file_changes = report
        .files
        .iter()
        .map(hashline_file_change)
        .collect::<Vec<_>>();
    let warnings = report
        .files
        .iter()
        .flat_map(|file| {
            file.warnings
                .iter()
                .map(|warning| format!("{}: {warning}", file.path))
        })
        .collect::<Vec<_>>();
    let (operations_applied, lsp) =
        match apply_hashline_report_effects(&report.files, request, context, &mut runtime_observer)
        {
            Ok(effects) => effects,
            Err(message) => return failed_tool_receipt(tool_call, message),
        };
    tool_receipt_from_runtime(agent_run_edit_tool_receipt(AgentRunEditToolReceiptInput {
        tool_call: tool_call.clone(),
        strategy: EDIT_STRATEGY_HASHLINE.to_owned(),
        paths: edit_paths(&report.files),
        summary: input.summary,
        files: serde_json::Value::Array(file_changes),
        operations_applied,
        warnings,
        lsp,
    }))
}

fn hashline_file_change(file: &HashlineFileApplyReport) -> serde_json::Value {
    let target_path = file.result_path();
    let next_content = match file.operation {
        HashlineFileOperation::Delete => "",
        HashlineFileOperation::Update | HashlineFileOperation::Move { .. } => &file.next_content,
    };
    let changes = crate::tool_orchestrator_fs::apply_diff_file_changes(
        target_path,
        Some(&file.previous_content),
        next_content,
    );
    let mut change = changes
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| serde_json::json!({ "path": target_path }));
    if let Some(object) = change.as_object_mut() {
        let operation = match file.operation {
            HashlineFileOperation::Update => "update",
            HashlineFileOperation::Delete => "delete",
            HashlineFileOperation::Move { .. } => "move",
        };
        object.insert("operation".to_owned(), serde_json::json!(operation));
        object.insert("sourcePath".to_owned(), serde_json::json!(file.path));
        object.insert(
            "blockResolutions".to_owned(),
            serde_json::json!(
                file.block_resolutions
                    .iter()
                    .map(|resolution| {
                        serde_json::json!({
                            "anchorLine": resolution.anchor_line,
                            "start": resolution.start,
                            "end": resolution.end,
                            "operation": resolution.operation.as_str(),
                        })
                    })
                    .collect::<Vec<_>>()
            ),
        );
    }
    if matches!(file.operation, HashlineFileOperation::Delete) {
        change
    } else {
        file_change_with_hashline_snapshot(change, target_path, &file.next_content)
    }
}

fn edit_paths(reports: &[HashlineFileApplyReport]) -> Vec<String> {
    reports
        .iter()
        .flat_map(|report| [report.path.to_owned(), report.result_path().to_owned()])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn prepare_hashline_files(
    context: &ToolOrchestratorContext,
    headers: &[axon_agent_runtime::HashlineHeader],
) -> Result<PreparedHashlineFiles, String> {
    let mut files = Vec::with_capacity(headers.len());
    let mut resolved_paths = HashMap::with_capacity(headers.len());
    let mut canonical_sources = BTreeMap::<PathBuf, String>::new();
    for header in headers {
        let resolved = resolve_hashline_file(context, header)?;
        if let Some(previous) =
            canonical_sources.insert(resolved.canonical_path.clone(), header.path.clone())
        {
            return Err(format!(
                "edit 的多个 Hashline section 指向同一文件：{previous} 与 {}；请合并到一个 header",
                header.path
            ));
        }
        resolved_paths.insert(
            header.path.clone(),
            ResolvedHashlinePath {
                path: resolved.path,
                recovered: resolved.recovered,
            },
        );
        files.push(HashlineApplyFile {
            path: header.path.clone(),
            current_content: resolved.current_content,
            snapshot: resolved.snapshot,
        });
    }
    Ok(PreparedHashlineFiles {
        files,
        resolved_paths,
    })
}

fn resolve_hashline_file(
    context: &ToolOrchestratorContext,
    header: &axon_agent_runtime::HashlineHeader,
) -> Result<ResolvedHashlineFile, String> {
    let authored = crate::tool_orchestrator_fs::resolve_tool_path(&header.path)
        .map_err(|message| message.replace("apply_diff", "edit"))?;
    let current_content =
        crate::tool_orchestrator_fs::current_file_content(&context.cwd, &authored)
            .map_err(|message| message.replace("apply_diff", "edit"))?;
    let Some(current_content) = current_content else {
        return recover_hashline_file(context, header, &authored);
    };
    let canonical_path = context
        .cwd
        .join(&authored)
        .canonicalize()
        .map_err(|error| format!("edit 路径不可访问：{error}"))?;
    let snapshot = match context.workspace_files.lookup(&canonical_path, &header.tag) {
        WorkspaceFileSnapshotLookup::Found(snapshot) => snapshot,
        WorkspaceFileSnapshotLookup::Missing => {
            return Err(format!(
                "[builtin.edit 执行失败: HASHLINE_SNAPSHOT_MISSING]\n\
                 目标文件: {}\n\
                 【原因定位】：未找到文件对应的会话快照 [{}]。Hashline 模式依赖由 builtin.read_file 生成的真实内容哈希。\n\
                 【建议动作】：\n\
                 1. 请先调用 builtin.read_file(path: \"{}\") 读取文件获取最新快照；\n\
                 2. 或改用 PathDiff 范式（*** Begin Patch ... *** End Patch）进行无快照编辑。",
                header.path, header.tag, header.path
            ));
        }
        WorkspaceFileSnapshotLookup::Ambiguous => {
            return Err(format!(
                "edit 快照标签 {} 发生碰撞；请重新读取 {}",
                header.tag, header.path
            ));
        }
    };
    Ok(ResolvedHashlineFile {
        path: tool_path_from_canonical(&context.cwd, &canonical_path)?,
        canonical_path,
        current_content,
        snapshot,
        recovered: false,
    })
}

fn recover_hashline_file(
    context: &ToolOrchestratorContext,
    header: &axon_agent_runtime::HashlineHeader,
    authored: &Path,
) -> Result<ResolvedHashlineFile, String> {
    let file_name = authored
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("edit 路径缺少有效文件名：{}", header.path))?;
    let canonical_root = context
        .cwd
        .canonicalize()
        .map_err(|error| format!("edit 工作区路径不可访问：{error}"))?;
    let mut candidates = BTreeMap::<(PathBuf, String), ResolvedHashlineFile>::new();
    for snapshot in context
        .workspace_files
        .find_by_tag_and_file_name(&header.tag, file_name)
    {
        let Ok(canonical_path) = snapshot.path().canonicalize() else {
            continue;
        };
        if canonical_path.strip_prefix(&canonical_root).is_err() {
            continue;
        }
        let current_content = match std::fs::read_to_string(&canonical_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("edit 读取恢复路径失败：{error}")),
        };
        let key = (canonical_path.clone(), snapshot.sha256().to_owned());
        candidates.entry(key).or_insert(ResolvedHashlineFile {
            path: tool_path_from_canonical(&context.cwd, &canonical_path)?,
            canonical_path,
            current_content,
            snapshot,
            recovered: true,
        });
    }
    // 只保留磁盘内容仍与自身快照逐字节一致的候选：内容已被外部改动的路径
    // 不再是「模型看到的那个版本」，据以施加编辑会写错内容。
    let mut unmodified = candidates
        .into_values()
        .filter(|candidate| candidate.snapshot.content() == candidate.current_content);
    let Some(first) = unmodified.next() else {
        return Err(format!(
            "edit 找不到 {}，且快照 {} 没有唯一的工作区内同名现存路径；请重新读取文件",
            header.path, header.tag
        ));
    };
    // 多个存活候选时自动采用的前提是它们内容逐字节相同：施加同一编辑得到同一结果，
    // 选谁语义等价。`candidates` 是有序 `BTreeMap`，迭代序即路径字典序升序，
    // 因此 `first` 天然就是路径字典序最小者，无需额外排序。
    if unmodified.all(|candidate| candidate.snapshot.content() == first.snapshot.content()) {
        return Ok(first);
    }
    // 标签只有 6 位十六进制空间，不同内容理论上可共享前缀；此时等价前提不成立，
    // 必须回落到保守拒绝，不得凭空挑一个。
    Err(format!(
        "edit 快照 {} 对应的多个同名现存路径内容不同，无法确定目标；请使用真实路径重新读取后重试",
        header.tag
    ))
}

fn tool_path_from_canonical(cwd: &Path, canonical_path: &Path) -> Result<String, String> {
    let canonical_root = cwd
        .canonicalize()
        .map_err(|error| format!("edit 工作区路径不可访问：{error}"))?;
    let display_path = canonical_path
        .strip_prefix(&canonical_root)
        .unwrap_or(canonical_path);
    Ok(display_path.to_string_lossy().replace('\\', "/"))
}

fn apply_resolved_hashline_paths(
    reports: &mut [HashlineFileApplyReport],
    mut resolved_paths: HashMap<String, ResolvedHashlinePath>,
) -> Result<(), String> {
    for report in reports {
        let authored_path = report.path.clone();
        let resolved = resolved_paths
            .remove(&authored_path)
            .ok_or_else(|| format!("edit 缺少 {authored_path} 的解析路径"))?;
        if resolved.recovered && resolved.path != authored_path {
            report.warnings.push(format!(
                "已根据快照标签将 {authored_path} 恢复为 {}",
                resolved.path
            ));
        }
        report.path = resolved.path;
    }
    if resolved_paths.is_empty() {
        Ok(())
    } else {
        Err("edit 收到未使用的 Hashline 路径解析结果".to_owned())
    }
}

fn apply_hashline_report_effects(
    reports: &[HashlineFileApplyReport],
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    runtime_observer: &mut Option<&mut ToolRuntimeObserver<'_>>,
) -> Result<(usize, serde_json::Value), String> {
    let changed = reports
        .iter()
        .filter(|report| report.changed)
        .collect::<Vec<_>>();
    if changed.is_empty() {
        let path = reports
            .first()
            .map_or("目标文件", |report| report.path.as_str());
        return Err(format!(
            "{HASHLINE_NOOP_CHANGE_MARKER}\n\
             目标文件: {path}\n\
             【原因定位】：edit 对 {path} 未产生内容变化，提交的编辑应用后与原文件完全相同。\n\
             【建议动作】：请检查 '+' 新增行是否与 '-' 删除行内容一致，或该改动已在此前被应用，无需重复提交相同修改。"
        ));
    }
    let total = changed.len();
    for (index, report) in changed.iter().enumerate() {
        crate::tool_orchestrator_fs_progress::emit_file_progress(
            runtime_observer,
            request,
            "applying",
            index,
            total,
            Some(&report.path),
            Some("准备提交编辑事务"),
        );
    }
    let applied = materialize_hashline_writes(context, &changed)?;
    let effects = changed
        .iter()
        .map(|report| match &report.operation {
            HashlineFileOperation::Update => {
                crate::tool_orchestrator_fs_write_effects::WorkspaceWriteEffect::changed(
                    &report.path,
                    report.seen_lines.iter().copied().collect(),
                )
            }
            HashlineFileOperation::Delete => {
                crate::tool_orchestrator_fs_write_effects::WorkspaceWriteEffect::deleted(
                    &report.path,
                )
            }
            HashlineFileOperation::Move { destination } => {
                crate::tool_orchestrator_fs_write_effects::WorkspaceWriteEffect::moved(
                    &report.path,
                    destination,
                    report.seen_lines.iter().copied().collect(),
                )
            }
        })
        .collect::<Vec<_>>();
    let lsp =
        crate::tool_orchestrator_fs_write_effects::reconcile_workspace_write(context, &effects);
    for (index, report) in changed.iter().enumerate() {
        crate::tool_orchestrator_fs_progress::emit_file_progress(
            runtime_observer,
            request,
            "applying",
            index.saturating_add(1),
            total,
            Some(report.result_path()),
            Some("编辑事务已提交"),
        );
    }
    Ok((applied, lsp))
}

fn materialize_hashline_writes(
    context: &ToolOrchestratorContext,
    reports: &[&HashlineFileApplyReport],
) -> Result<usize, String> {
    verify_hashline_preconditions(context, reports)?;
    let mut plan = PatchPlan::default();
    let mut snapshot = HashMap::with_capacity(reports.len());
    for report in reports {
        let content = report.previous_content.clone();
        let change = match &report.operation {
            HashlineFileOperation::Update => PatchFileChange::Update {
                move_path: None,
                new_content: inherit_text_file_format(&content, &report.next_content),
            },
            HashlineFileOperation::Delete => PatchFileChange::Delete {
                previous_content: Some(content.clone()),
            },
            HashlineFileOperation::Move { destination } => PatchFileChange::Update {
                move_path: Some(destination.clone()),
                new_content: inherit_text_file_format(&content, &report.next_content),
            },
        };
        snapshot.insert(report.path.clone(), content);
        plan.operations.push(PatchFileOperation {
            path: report.path.clone(),
            change,
        });
    }
    axon_apply::transactionally_materialize(&context.cwd, &plan, &snapshot)
        .map_err(|error| format!("edit 事务提交失败：{error}"))?;
    Ok(plan.operations.len())
}

fn verify_hashline_preconditions(
    context: &ToolOrchestratorContext,
    reports: &[&HashlineFileApplyReport],
) -> Result<(), String> {
    for report in reports {
        let relative = crate::tool_orchestrator_fs::resolve_tool_path(&report.path)?;
        let current = crate::tool_orchestrator_fs::current_file_content(&context.cwd, &relative)?
            .ok_or_else(|| format!("edit 写入前文件已消失：{}", report.path))?;
        if current != report.previous_content {
            return Err(format!(
                "[builtin.edit 执行失败: CONCURRENT_MODIFICATION]\n\
                 目标文件: {}\n\
                 【事务状态】：得益于原子事务预检，本次修改已全部安全回滚，磁盘未产生脏写入。\n\
                 【原因定位】：写入前检测到目标文件已被外部修改，内容快照已过期。\n\
                 【建议动作】：请调用 builtin.read_file(path: \"{}\") 获取最新内容与行哈希后重试。",
                report.path, report.path
            ));
        }
        if let HashlineFileOperation::Move { destination } = &report.operation {
            let destination = crate::tool_orchestrator_fs::resolve_tool_path(destination)?;
            if destination == relative {
                return Err(format!("edit 的 MV 目标与源路径相同：{}", report.path));
            }
            if context.cwd.join(&destination).exists() {
                return Err(format!(
                    "edit 的 MV 目标已存在：{}；本批次未执行",
                    destination.display()
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn execute_apply_patch_strategy(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    patch: &str,
    summary: String,
    mut runtime_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    let tool_call = &request.tool_call;
    let report = match axon_apply::ApplyPatchExecutor.apply(&context.cwd, patch) {
        Ok(report) => report,
        Err(error) => return failed_tool_receipt(tool_call, error.to_string()),
    };
    let total = report.files.len().max(1);
    for (index, file) in report.files.iter().enumerate() {
        crate::tool_orchestrator_fs_progress::emit_file_progress(
            &mut runtime_observer,
            request,
            "applying",
            index.saturating_add(1),
            total,
            Some(&file.path),
            Some("补丁文件已处理"),
        );
    }
    let file_changes = report
        .files
        .iter()
        .map(|file| pathdiff_report_file_change(&context.cwd, file))
        .collect::<Vec<_>>();
    let effects = report
        .files
        .iter()
        .map(pathdiff_write_effect)
        .collect::<Vec<_>>();
    let lsp =
        crate::tool_orchestrator_fs_write_effects::reconcile_workspace_write(context, &effects);
    let paths = report
        .files
        .iter()
        .flat_map(|file| file.source_path.iter().chain(std::iter::once(&file.path)))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let operations_applied = report
        .added
        .saturating_add(report.updated)
        .saturating_add(report.deleted);
    tool_receipt_from_runtime(agent_run_edit_tool_receipt(AgentRunEditToolReceiptInput {
        tool_call: tool_call.clone(),
        strategy: EDIT_STRATEGY_APPLY_PATCH.to_owned(),
        paths,
        summary,
        files: serde_json::Value::Array(file_changes),
        operations_applied,
        warnings: Vec::new(),
        lsp,
    }))
}

fn pathdiff_report_file_change(
    root: &Path,
    file: &axon_apply::ApplyPatchFileReport,
) -> serde_json::Value {
    let report = serde_json::json!({
        "path": file.path,
        "sourcePath": file.source_path,
        "operation": file.operation,
        "diff": file.diff,
        "seenLines": file.seen_lines,
    });
    if file.operation == PATCH_OPERATION_DELETE {
        return report;
    }
    match axon_apply::read_workspace_file(root, &file.path) {
        Ok(Some(content)) => file_change_with_hashline_snapshot(report, &file.path, &content),
        Ok(None) | Err(_) => report,
    }
}

fn pathdiff_write_effect(
    file: &axon_apply::ApplyPatchFileReport,
) -> crate::tool_orchestrator_fs_write_effects::WorkspaceWriteEffect {
    use crate::tool_orchestrator_fs_write_effects::WorkspaceWriteEffect;

    if let Some(source_path) = file.source_path.as_deref() {
        return WorkspaceWriteEffect::moved(source_path, &file.path, file.seen_lines.clone());
    }
    match file.operation.as_str() {
        PATCH_OPERATION_ADD => WorkspaceWriteEffect::created(&file.path, file.seen_lines.clone()),
        PATCH_OPERATION_DELETE => WorkspaceWriteEffect::deleted(&file.path),
        _ => WorkspaceWriteEffect::changed(&file.path, file.seen_lines.clone()),
    }
}
