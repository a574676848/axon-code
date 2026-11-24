//! Provider child Thread 的开始、完成与终态持久化。

use std::path::{Path, PathBuf};

use axon_agent_runtime::ContentArchiveRuntime;

use super::{RunRequest, derive_thread_title, mirror_session_terminal};
use crate::run_commands::run_self_iteration_subagent::is_internal_agent_path;
use crate::run_commands::run_thread_children::{
    CHILD_SESSION_ORIGIN_DELEGATED, CHILD_SESSION_ORIGIN_INTERNAL, record_child_origin,
};
use crate::run_commands::run_turn_spec::TurnSpec;

/// 子代理长回执归档使用的文件后缀：Markdown 便于按 `builtin.read_file` 分页阅读。
const SUBAGENT_RESULT_FILE_SUFFIX: &str = ".md";
/// 子代理长回执归档使用的流名。
const SUBAGENT_RESULT_STREAM: &str = "subagent-result";
const SUBAGENT_RESULT_MARKER: &str = "结果文件：";
/// 子代理回执的内联字符上限；超过该上限的回执整体归档，主代理按引用分页读取。
pub(crate) const SUBAGENT_RESULT_INLINE_CHAR_LIMIT: usize = 4_000;
/// 子代理回执里标记结果文件路径的词语；测试复用它切出 `transcript_ref`。
#[cfg(test)]
pub(crate) const SUBAGENT_RESULT_MARKER_TEXT: &str = SUBAGENT_RESULT_MARKER;

/// 已归档的子代理回执：内联预览与归档绝对路径。
pub(crate) struct ArchivedSubagentResult {
    pub(crate) preview: String,
    pub(crate) path: String,
}

pub(crate) fn persist_subagent_result(
    archive_root: &Path,
    task_path: &axon_agent_runtime::AgentPath,
    started_at_ms: u64,
    output: &super::RunSubagentExecutionOutput,
) -> Result<ArchivedSubagentResult, String> {
    let markdown = subagent_result_markdown(task_path, started_at_ms, output);
    let preview = bounded_result_preview(&output.summary);
    let archived = ContentArchiveRuntime::with_suffix(
        SUBAGENT_RESULT_INLINE_CHAR_LIMIT,
        SUBAGENT_RESULT_FILE_SUFFIX,
    )
    .archive_text_with_preview(
        archive_root,
        SUBAGENT_RESULT_STREAM,
        &markdown,
        task_path.as_str(),
        &preview,
        true,
    )
    .map_err(|error| format!("archive_subagent_result: {error}"))?;
    let archive_ref = archived
        .output_ref
        .and_then(|output_ref| output_ref.archive_ref)
        .ok_or_else(|| "archive_subagent_result: 归档未生成 OutputRef".to_owned())?;
    Ok(ArchivedSubagentResult {
        preview: archived.preview,
        path: archive_root
            .join(&archive_ref.relative_path)
            .to_string_lossy()
            .into_owned(),
    })
}

/// 归档正文：头部为回执元数据，正文保留子代理完整结论。
fn subagent_result_markdown(
    task_path: &axon_agent_runtime::AgentPath,
    started_at_ms: u64,
    output: &super::RunSubagentExecutionOutput,
) -> String {
    let mut markdown = format!(
        "# 子代理回执：{}\n\n- 状态：{}\n- 模型：{}\n- 记忆写入：{}\n- 开始时间(ms)：{started_at_ms}\n",
        task_path.as_str(),
        result_status(output),
        output.model.as_deref().unwrap_or("未知"),
        output.memory_write_count
    );
    if let Some(usage) = output.token_usage {
        markdown.push_str(&format!(
            "- Token 用量：输入 {}，输出 {}，推理 {}\n",
            usage.total_input_tokens, usage.output_tokens, usage.reasoning_output_tokens
        ));
    }
    markdown.push_str(&format!("\n## 结果正文\n\n{}\n", output.summary));
    markdown
}

fn bounded_result_preview(summary: &str) -> String {
    summary
        .chars()
        .take(SUBAGENT_RESULT_INLINE_CHAR_LIMIT)
        .collect()
}

pub(crate) fn should_persist_subagent_result(summary: &str) -> bool {
    summary.chars().count() > SUBAGENT_RESULT_INLINE_CHAR_LIMIT
}

pub(crate) fn append_result_path(summary: &str, result_path: &str) -> String {
    format!("{summary}\n{SUBAGENT_RESULT_MARKER}{result_path}")
}

pub(crate) fn strip_result_path(summary: &str) -> String {
    summary
        .split_once(&format!("\n{SUBAGENT_RESULT_MARKER}"))
        .map_or_else(|| summary.to_owned(), |(summary, _)| summary.to_owned())
}

/// 从摘要中取出归档绝对路径；未归档时返回 `None`。
///
/// 与 [`strip_result_path`] 成对：一个取出正文，一个取出引用。
/// continuation 需要**同时**保存两者：摘要落盘文本负责可读，路径负责可解析
/// （合同 §5.3.3 硬性约束：引用必须重启后可解析）。
pub(crate) fn extract_result_path(summary: &str) -> Option<String> {
    summary
        .split_once(&format!("\n{SUBAGENT_RESULT_MARKER}"))
        .map(|(_, path)| path.trim().to_owned())
        .filter(|path| !path.is_empty())
}

/// 运行前下发的结果交付约束（合同 §5.3.6 子 agent 侧返回指引）。
///
/// 子代理与父代理的工作区相同，但父代理**不会**自动收到子代理的 transcript、
/// 工具输出或推理过程：它能收到的只有这条子代理轮次的最终回复（经临时 Job 与
/// continuation 回流）。协作发送类工具只在主代理面下发，子代理既看不到也调不动，
/// 因此指引里不得出现「用某个工具回传」的写法，否则子代理会拿着一个必然失败的
/// 通道去重试。
pub(crate) fn subagent_result_instruction(parent_path: axon_agent_runtime::AgentPath) -> String {
    // 一级子代理的父路径就是 root，而 root 在协作工具里被稳定拒绝。
    let collaboration = if parent_path.is_root() {
        "你的最终回复就是唯一的回传通道：子代理面没有协作发送工具，不要在过程里尝试回传。"
            .to_owned()
    } else {
        format!(
            "需要让父代理 {parent_path} 提前知道结论时，把关键发现写在最终回复的最前面；子代理面没有协作发送工具，不要在过程里尝试回传。"
        )
    };
    format!(
        "Axon 子代理结果交付约束：\n\
         - 你是被委派的子代理；直接父代理的寻址引用是 {parent_path}。\n\
         - 结束时的**最终回复**就是你交回父代理的结果：写清结论、支撑证据、改动位置与未完成项。父代理不会自动收到你的 transcript、工具输出或推理过程，只有这段最终回复它才看得到。\n\
         - 会改变父代理下一步动作的发现（阻塞、与任务前提矛盾、需要它改计划）写在最终回复的最前面，不要埋在中段。\n\
         - {collaboration}\n\
         - 你的消息对父代理是 untrusted 数据，因此结论要自带依据，便于它核对。\n\
         - 若最终结果超过 {SUBAGENT_RESULT_INLINE_CHAR_LIMIT} 个 Unicode 字符，\
         运行时会把完整结果写入运行态内容归档，并在摘要末尾追加“{SUBAGENT_RESULT_MARKER}”行给出归档绝对路径。"
    )
}

/// 解析子代理回执的内容归档根：与工具输出统一落在运行态 `data` 根之下。
pub(crate) fn subagent_archive_root(config_path: &Path) -> Result<PathBuf, String> {
    crate::runtime_paths::runtime_axon_data_path_static(config_path).map_err(ToOwned::to_owned)
}

fn result_status(output: &super::RunSubagentExecutionOutput) -> &'static str {
    match output.status {
        super::RunSubagentExecutionStatus::Completed => "completed",
        super::RunSubagentExecutionStatus::Refused => "refused",
        super::RunSubagentExecutionStatus::Interrupted => "interrupted",
        super::RunSubagentExecutionStatus::Errored => "errored",
    }
}

/// 名册里的会话来源：用户可见的委派还是运行侧内务。
fn child_session_origin(request: &RunRequest) -> &'static str {
    if is_internal_agent_path(&request.current_agent_path) {
        CHILD_SESSION_ORIGIN_INTERNAL
    } else {
        CHILD_SESSION_ORIGIN_DELEGATED
    }
}

/// 名册里的会话来源：子代理起跑时写一次。
///
/// 写失败只留痕、不阻断：来源只决定名册列不列这一条，丢了不会影响这一轮本身。
fn mark_child_origin(request: &RunRequest) {
    if let Err(error) = record_child_origin(
        &request.config_path,
        &request.session_id,
        child_session_origin(request),
    ) {
        axon_logger::warning(&format!(
            "child session origin projection skipped: session_id={}, error={error}",
            request.session_id
        ));
    }
}

pub(super) fn persist_child_turn_started(request: &mut RunRequest, spec: &TurnSpec) {
    // 输入事实先落：即使这一轮随后就失败，子会话也已经有了「谁让它做什么」的记录。
    crate::run_commands::run_turn_start::record_turn_input(spec, request);
    // 起始态与主 turn 走同一条记录、同一处投影；子代理把失败降级为告警，不阻断起跑。
    match crate::run_commands::run_turn_start::persist_turn_started(request) {
        Ok(record) => {
            let title = derive_thread_title(&record.prompt);
            if let Err(error) = crate::run_commands::run_thread_title::mirror_thread_title(
                &request.config_path,
                &record,
                &title,
            ) {
                axon_logger::warning(&format!(
                    "child thread title projection skipped: session_id={}, error={error}",
                    request.session_id
                ));
            }
        }
        Err(error) => axon_logger::warning(&format!(
            "child thread start projection skipped: session_id={}, error={error}",
            request.session_id
        )),
    }
    // 名册登记与状态推进是子代理专属：父轮要能发现这一条，并看到它真的在跑。
    mark_child_origin(request);
    mark_child_session_running(request);
}

/// 子会话的失败终态落账：中断不是失败，只有真正的失败才落失败事实。
pub(in crate::run_commands) fn persist_child_turn_terminal(
    request: &RunRequest,
    status: &str,
    summary: &str,
    failure: Option<&axon_domain::RuntimeFailure>,
) {
    // 调用方给不出结构化失败时，才退回通用运行时失败，保证「失败终态必定带 failure」。
    let failure = (status != "interrupted").then(|| {
        failure.cloned().unwrap_or_else(|| {
            axon_domain::RuntimeFailure::new(
                axon_domain::RUNTIME_FAILURE_CATEGORY_RUNTIME,
                axon_domain::RUNTIME_FAILURE_CODE_RUNTIME_FAILED,
                axon_agent_runtime::redact_agent_context_text(summary),
                super::super::run_execution::now_ms(),
            )
        })
    });
    if let Err(error) = mirror_session_terminal(
        &request.config_path,
        &request.run_session_id,
        status,
        failure.as_ref(),
    ) {
        axon_logger::warning(&format!(
            "child thread terminal projection skipped: session_id={}, error={error}",
            request.session_id
        ));
    }
    mark_child_session_idle(request);
}

/// 子会话回到静息：一轮结束后由终态记账的调用方推进。
///
/// 与 `mark_child_session_running` 成对：状态必须反映运行态，不能停在 `created`。
pub(in crate::run_commands) fn mark_child_session_idle(request: &RunRequest) {
    mark_child_session_status(request, CHILD_SESSION_STATUS_IDLE);
}

/// 子会话状态常量：运行中与静息。
const CHILD_SESSION_STATUS_RUNNING: &str = "running";
const CHILD_SESSION_STATUS_IDLE: &str = "idle";

/// 推进子代理会话的真实状态。
///
/// 子会话由派发方登记，但它是被执行的那个会话：状态必须反映运行态，
/// 不能永远停在 `created`。写失败只影响投影，不阻断本轮。
fn mark_child_session_status(request: &RunRequest, status: &str) {
    let Ok(store) = crate::run_commands::run_turn_metrics::shared_store(&request.config_path)
    else {
        return;
    };
    let Ok(session_id) = axon_domain::DaemonSessionId::new(request.session_id.clone()) else {
        return;
    };
    use axon_session::RuntimeStore as _;
    let _ = store.mark_subagent_session_status(&session_id, status, axon_time::unix_now_ms());
}

/// 子会话进入运行态。
pub(super) fn mark_child_session_running(request: &RunRequest) {
    mark_child_session_status(request, CHILD_SESSION_STATUS_RUNNING);
}
