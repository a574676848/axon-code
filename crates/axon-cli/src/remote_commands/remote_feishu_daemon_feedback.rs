//! Daemon supervisor 的 Feishu processing feedback 执行。
//!
//! 与出站 egress 分离：本模块只把排队中的处理中反馈（Typing 清理、状态回执）
//! 推进到 Feishu API，不参与回复正文的 artifact 与 outbox 持久化。

use axon_remote_feishu::{
    RemoteFeishuFeedbackExecutorRuntime, RemoteFeishuFeedbackExecutorTickInput,
    RemoteFeishuFeedbackTaskRecord, RemoteFeishuFeedbackTaskStoreRuntime,
};

/// 由 Daemon supervisor 调用一次 Feishu processing feedback。
pub(crate) fn run_daemon_feedback_tick(
    config_path: &std::path::Path,
    limit: usize,
) -> Result<usize, String> {
    if limit == 0 {
        return Err("Feishu Daemon feedback 输入无效".to_owned());
    }
    crate::run_commands::load_run_env_file(&crate::setup_commands::env_path_for_config(
        config_path,
    ))?;
    let loaded = axon_config::ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取 Feishu Daemon feedback 配置失败：{error:?}"))?;
    let context = super::RemoteCliContext::new(
        config_path.to_path_buf(),
        crate::runtime_paths::runtime_axon_root_path(config_path)?,
        loaded.config.remote.feishu_websocket,
    )
    .feishu_runtime();
    if !context.feishu_websocket.enabled {
        return Ok(0);
    }
    let app_secret = context
        .feishu_app_secret()
        .ok_or_else(|| "当前凭证仓库缺少 Feishu app_secret".to_owned())?;
    let feedback_store_path =
        RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&context.root);
    let processing_store_path =
        axon_remote_feishu::RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(
            &context.root,
        );
    let report = RemoteFeishuFeedbackExecutorRuntime::tick(
        &RemoteFeishuFeedbackExecutorTickInput {
            feedback_store_path,
            processing_store_path: processing_store_path.clone(),
            session_id: axon_domain::SessionId(
                axon_remote_feishu::REMOTE_FEISHU_FEEDBACK_ALL_SESSIONS,
            ),
            now_ms: super::now_ms(),
            limit,
        },
        |task: &RemoteFeishuFeedbackTaskRecord, processing_path, accepted_at_ms| {
            execute_daemon_feedback_task(
                &context,
                &app_secret,
                task,
                processing_path,
                accepted_at_ms,
            )
        },
    )
    .map_err(|error| format!("执行 Feishu Daemon feedback 失败：{error:?}"))?;
    Ok(report.executed_items)
}

fn execute_daemon_feedback_task(
    context: &super::RemoteCliContext,
    app_secret: &str,
    task: &RemoteFeishuFeedbackTaskRecord,
    processing_store_path: &std::path::Path,
    accepted_at_ms: u64,
) -> Result<Vec<axon_remote_feishu::RemoteFeishuProcessingPlan>, axon_domain::AxonError> {
    let api_base_url = super::remote_feishu_delivery::feishu_api_base_url(context).or((!task
        .api_base_url
        .trim()
        .is_empty())
    .then_some(task.api_base_url.as_str()));
    let active_state =
        axon_remote_feishu::RemoteFeishuProcessingStateStoreRuntime::active_reaction(
            processing_store_path,
            &task.message_id,
        )?;
    if active_state.is_none()
        && task.outcome == axon_remote_feishu::RemoteFeishuProcessingOutcome::Success
    {
        return Ok(vec![
            axon_remote_feishu::RemoteFeishuProcessingRuntime::plan_complete(
                &task.message_id,
                task.outcome,
            )?,
        ]);
    }
    let completion =
        axon_remote_feishu::RemoteFeishuProcessingRuntime::execute_complete_with_secret(
            &task.app_id,
            app_secret,
            api_base_url,
            &task.message_id,
            task.outcome,
            active_state.as_ref(),
            accepted_at_ms,
        )?;
    for state in &completion {
        axon_remote_feishu::RemoteFeishuProcessingStateStoreRuntime::append_record(
            processing_store_path,
            state,
        )?;
    }
    Ok(vec![
        axon_remote_feishu::RemoteFeishuProcessingRuntime::plan_complete(
            &task.message_id,
            task.outcome,
        )?,
    ])
}
