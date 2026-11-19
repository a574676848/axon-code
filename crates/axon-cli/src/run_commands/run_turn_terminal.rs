//! 一轮对话的终态落账：完成态与失败态的唯一记账入口。
//!
//! 这里只产出事实，不产出展示。文本摘要、JSON 事件与退出码都由调用端按自己的表面渲染，
//! 记账与渲染分开，不同 turn 入口才能共用同一套终态事实而各自保留输出格式。
//!
//! 轮后治理、人格结算与记忆反馈都属于「运行后」：它们只在这里发生一次，
//! 与展示无关，也不允许任一入口再写第二份。

use std::path::Path;

use axon_agent_runtime::AGENT_RUN_ERROR_TYPE_INTERRUPTED;
use axon_config::AxonConfig;

use super::{
    RunRequest,
    run_execution::{RunExecutionError, RunResponse},
    run_memory_autonomy::apply_memory_autonomy_feedback,
    run_memory_usefulness::{
        RunMemoryUsefulnessEvent, memory_usefulness_for_completed, memory_usefulness_for_failed,
    },
    run_post_run_governance::{
        PostRunGovernanceJob, PostRunGovernanceReceipt, start_post_run_governance_job,
    },
    run_session_store::RunSessionRecord,
    run_turn_spec::TurnCompletion,
};

/// 一轮已完成的对话：只携带记账事实，不含任何输出格式。
pub(crate) struct CompletedTurn {
    pub(crate) response: RunResponse,
    pub(crate) session_title: Option<String>,
    pub(crate) memory_usefulness: Option<RunMemoryUsefulnessEvent>,
    pub(crate) memory_autonomy: Option<axon_agent_runtime::MemoryAutonomyFeedbackReport>,
    pub(crate) post_run_governance: Option<PostRunGovernanceReceipt>,
    /// 上一轮的记忆反思提示；本轮治理还没跑，拿不到自己那条。
    pub(crate) memory_reflection_notice: Option<String>,
}

/// 一轮失败的对话：错误本身与它落下的终态记录。
///
/// 失败正文、退出码与结构化事件由调用端渲染；这里只保证同一轮只记一次账。
pub(crate) struct FailedTurn {
    pub(crate) error: Box<RunExecutionError>,
    pub(crate) failure: axon_domain::RuntimeFailure,
    pub(crate) memory_usefulness: Option<RunMemoryUsefulnessEvent>,
    pub(crate) memory_autonomy: Option<axon_agent_runtime::MemoryAutonomyFeedbackReport>,
}

/// 收口一轮已完成的对话。
///
/// 运行后唯一的差异就在这里：回传调用方表面的一轮要自己读上一轮的记忆反思提示；
/// 交给执行子会话的一轮不读，也没有自己的记忆命中反馈。
pub(crate) fn complete_turn(
    spec: &super::run_turn_spec::TurnSpec,
    request: &RunRequest,
    response: RunResponse,
) -> CompletedTurn {
    let config_path = request.config_path.as_path();
    let memory_usefulness = match spec.completion {
        TurnCompletion::Surface => memory_usefulness_for_completed(request, &response),
        TurnCompletion::ChildSession => None,
    };
    let record = RunSessionRecord::from_completed(
        request,
        &response,
        memory_usefulness.as_ref(),
        &request.startup.workspace_instructions,
    );
    // 终态记账只走一处：指标、逐字稿投影与标题生成都在里面按 profile 决定。
    let profile = spec.profile;
    let session_title =
        super::run_turn_accounting::account_completed_turn(config_path, request, &record, profile);

    // 反思是轮后异步治理的产物：本轮治理尚未启动，能确定看到的只有上一次的结果。
    // 这条提示随 run 输出交给界面，用户在下一轮开头看到上一轮的记忆动作。
    let memory_reflection_notice = match spec.completion {
        TurnCompletion::Surface => super::run_thread_tail::previous_reflection_notice_at_tail(
            config_path,
            &record.thread_id,
            &record.assistant_part_id,
        ),
        TurnCompletion::ChildSession => None,
    };
    let governance_job = PostRunGovernanceJob {
        config_path: config_path.to_path_buf(),
        request: request.clone(),
        session_record: record,
        memory_usefulness: memory_usefulness.clone(),
    };
    let post_run_governance = profile
        .accounting
        .governance
        .then(|| start_post_run_governance_job(&governance_job));
    settle_dispatch_terminal(request, axon_session::DispatchTerminalKind::Understood);
    CompletedTurn {
        response,
        session_title: Some(session_title),
        memory_usefulness,
        memory_autonomy: None,
        post_run_governance,
        memory_reflection_notice,
    }
}

/// 收口一轮失败的对话。
///
/// 失败轮不入治理队列，因此人格结算在这里直接完成，保证失败转移在生产路径可达。
/// 用户中断与真正的失败分开记账：前者按「本轮被中断」，后者才累加失败连击。
///
/// 去向决定这一段落进哪里：回到调用方表面的一轮落轮次失败记录并做人格结算；
/// 交给执行子会话的一轮只把失败写进它自己的终态事件，不替主 turn 做人格决定。
pub(crate) fn fail_turn(
    config_path: &Path,
    config: &AxonConfig,
    spec: &super::run_turn_spec::TurnSpec,
    request: &RunRequest,
    error: Box<RunExecutionError>,
    partial_response: String,
) -> FailedTurn {
    let timestamp = super::run_execution::now_ms();
    let failure = super::run_session_store::runtime_failure_from_execution_error(
        &request.session_id,
        &error,
        &partial_response,
        timestamp,
    );
    // 终态先落：中断与失败都必须在派发轮的入站行上留下结论，界面才不会永远停在「已收到」。
    let interrupted = error.error_type == AGENT_RUN_ERROR_TYPE_INTERRUPTED
        || request
            .cancel_token
            .as_ref()
            .is_some_and(axon_agent_runtime::RunCancelToken::is_cancelled);
    settle_dispatch_terminal(
        request,
        if interrupted {
            axon_session::DispatchTerminalKind::Interrupted
        } else {
            axon_session::DispatchTerminalKind::Rejected
        },
    );
    let (memory_usefulness, memory_autonomy) = match spec.completion {
        TurnCompletion::ChildSession => {
            // 中断不是失败：只有真正的失败才带结构化失败事实。
            let interrupted = error.error_type == AGENT_RUN_ERROR_TYPE_INTERRUPTED;
            super::run_subagent_executor::persist_child_turn_terminal(
                request,
                if interrupted {
                    "interrupted"
                } else {
                    "errored"
                },
                &error.message,
                (!interrupted).then_some(&failure),
            );
            (None, None)
        }
        TurnCompletion::Surface => {
            let final_status = if request
                .cancel_token
                .as_ref()
                .is_some_and(axon_agent_runtime::RunCancelToken::is_cancelled)
            {
                axon_persona::TurnFinalStatus::Interrupted
            } else {
                axon_persona::TurnFinalStatus::Failed
            };
            let part_id = format!("failed:{timestamp}");
            super::run_persona_settlement::settle_persona_turn(
                &super::run_persona_settlement::PersonaSettlementInput {
                    config_path,
                    session_id: &request.session_id,
                    assistant_part_id: &part_id,
                    disclosure: request.goal_disclosure_policy,
                    final_status,
                    user_affect: None,
                    user_fatigue: None,
                    persona_drift_bp: None,
                    drift_reason: None,
                    scene_audience: None,
                    // 失败路径没有轨迹也没有评估：不给信号，结算退回旧行为。
                    signals: None,
                },
            );
            if let Err(persistence_error) =
                persist_failed_run(config_path, request, &error, partial_response, &failure)
            {
                axon_logger::warning(&format!(
                    "failed run persistence skipped: session_id={}, error={persistence_error}",
                    request.session_id
                ));
            }
            let memory_usefulness = memory_usefulness_for_failed(request, &error);
            let memory_autonomy = apply_memory_autonomy_feedback(
                config_path,
                config,
                request,
                memory_usefulness.as_ref(),
                false,
            );
            (memory_usefulness, memory_autonomy)
        }
    };
    FailedTurn {
        error,
        failure,
        memory_usefulness,
        memory_autonomy,
    }
}

fn persist_failed_run(
    config_path: &Path,
    request: &RunRequest,
    error: &RunExecutionError,
    partial_response: String,
    failure: &axon_domain::RuntimeFailure,
) -> Result<(), String> {
    let started = RunSessionRecord::started(request);
    let persisted = super::run_thread_store::turn_record_by_sequence(
        config_path,
        &super::RunIdentities::new(request.session_id.clone(), request.run_session_id.clone()),
        None,
    )?
    .map(|(record, _)| record);
    let mut record = select_failed_run_record(started, persisted);
    // 身份只认本轮请求：反查回来的记录只是同一段 Thread 的投影，标识以请求为准。
    record.session_id.clone_from(&request.session_id);
    record.thread_id.clone_from(&request.run_session_id);
    record
        .parent_session_id
        .clone_from(&request.parent_session_id);
    record
        .parent_thread_id
        .clone_from(&request.parent_thread_id);
    record.mark_failed(failure.clone(), partial_response, error);
    super::run_thread_store::mirror_failed_session_to_thread(config_path, &record, failure)
}

pub(super) fn select_failed_run_record(
    started: RunSessionRecord,
    persisted: Option<RunSessionRecord>,
) -> RunSessionRecord {
    persisted
        .filter(|record| record.assistant_part_id == started.assistant_part_id)
        .unwrap_or(started)
}

/// 派发轮的终态：只落在执行侧派发的入站行上。
///
/// 正常入站轮的终态由 daemon 按 lease 结算，`settle_dispatch` 在库里就只认
/// `source_channel='agent'`，因此这里对任何一轮调用都不会污染主会话的状态机。
fn settle_dispatch_terminal(request: &RunRequest, terminal: axon_session::DispatchTerminalKind) {
    if request.interaction_id.is_empty() {
        return;
    }
    let Ok(interaction_id) = axon_domain::InteractionId::new(request.interaction_id.clone()) else {
        return;
    };
    // 用进程内缓存的 store：终态写入不必为了两条 UPDATE 重建连接与重跑建库检查。
    let Ok(store) = super::run_turn_metrics::shared_store(&request.config_path) else {
        return;
    };
    use axon_session::InteractionStore as _;
    if let Err(error) =
        store.settle_dispatch(&interaction_id, terminal, super::run_execution::now_ms())
    {
        axon_logger::warning(&format!(
            "派发轮终态未落库: session_id={}, terminal={}, error={error}",
            request.session_id,
            terminal.as_str()
        ));
    }
}
