use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
    thread,
};

use axon_domain::SessionId;
use axon_remote_feishu::{
    RemoteFeishuDeliveryRuntime, RemoteFeishuExecutorTickInput,
    RemoteFeishuFeedbackTaskStoreRuntime,
};
use axon_remote_runtime::{RemoteNotificationOutboxRuntime, RemoteTransportKind};

use super::{
    RemoteCliContext, now_ms,
    remote_feishu_delivery::{
        feishu_api_base_url, project_feishu_dispatch_plan, resolve_agent_reply_artifact,
    },
};

const REMOTE_FEISHU_DELIVERY_WORKER_COUNT: usize = 4;

/// 由 Daemon supervisor 调用一次 Feishu delivery；待投递 Session 只经状态索引限量发现。
pub(crate) fn run_daemon_delivery_tick(
    config_path: &Path,
    attempt_owner: &str,
    limit: usize,
) -> Result<usize, String> {
    if limit == 0 || attempt_owner.trim().is_empty() {
        return Err("Feishu Daemon delivery 输入无效".to_owned());
    }
    crate::run_commands::load_run_env_file(&crate::setup_commands::env_path_for_config(
        config_path,
    ))?;
    let loaded = axon_config::ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取 Feishu Daemon 配置失败：{error:?}"))?;
    let context = RemoteCliContext::new(
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
    let session_ids = RemoteNotificationOutboxRuntime::pending_session_ids_for_transport(
        &context.outbox_store_path,
        RemoteTransportKind::Feishu,
        now_ms(),
        limit,
    )
    .map_err(|error| format!("读取 Feishu Daemon outbox 失败：{error:?}"))?;
    let worker_limit = limit.min(axon_agent_runtime::REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS);
    let budget = Arc::new(Mutex::new(worker_limit));
    let queue = Arc::new(Mutex::new(VecDeque::from(session_ids)));
    let worker_count = queue
        .lock()
        .map(|items| items.len().min(REMOTE_FEISHU_DELIVERY_WORKER_COUNT))
        .unwrap_or(0);
    let executed = thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let context = context.clone();
            let app_secret = app_secret.clone();
            let attempt_owner = attempt_owner.to_owned();
            let budget = Arc::clone(&budget);
            workers.push(scope.spawn(move || {
                let mut local_executed = 0usize;
                loop {
                    let session_id = queue.lock().ok().and_then(|mut items| items.pop_front());
                    let Some(session_id) = session_id else {
                        break;
                    };
                    let session_limit = budget.lock().ok().and_then(|mut remaining| {
                        let amount = (*remaining).min(
                            axon_agent_runtime::REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS,
                        );
                        (*remaining).checked_sub(amount).map(|next| {
                            *remaining = next;
                            amount
                        })
                    });
                    let Some(session_limit) = session_limit.filter(|limit| *limit > 0) else {
                        break;
                    };
                    match execute_worker_tick(
                        &context,
                        session_id,
                        session_limit,
                        &app_secret,
                        &attempt_owner,
                    ) {
                        Ok(report) => local_executed = local_executed.saturating_add(report.executed_items),
                        Err(error) => axon_logger::warning(&format!(
                            "Feishu Daemon delivery 执行失败，将由 outbox 重试: session={} error={error}",
                            session_id.0
                        )),
                    }
                }
                local_executed
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap_or_default())
            .sum::<usize>()
    });
    Ok(executed)
}

fn execute_worker_tick(
    context: &RemoteCliContext,
    session_id: SessionId,
    limit: usize,
    app_secret: &str,
    attempt_owner: &str,
) -> Result<axon_remote_feishu::RemoteFeishuExecutorTickReport, String> {
    axon_remote_feishu::RemoteFeishuExecutorRuntime::tick(
        &execute_worker_input(context, session_id, limit, attempt_owner),
        |record| build_execute_worker_dispatch_plan(context, record),
        |dispatch_plan, record, sent_at_ms| {
            execute_worker_dispatch(context, app_secret, dispatch_plan, record, sent_at_ms)
        },
        |dispatch_plan, record, execution, accepted_at_ms| {
            let artifact = resolve_agent_reply_artifact(context, record)
                .map_err(axon_domain::AxonError::runtime_failure)?;
            Ok(RemoteFeishuFeedbackTaskStoreRuntime::task_from_delivery(
                dispatch_plan,
                execution,
                artifact
                    .as_ref()
                    .and_then(|reply| reply.processing_message_id.as_deref()),
                feishu_api_base_url(context),
                accepted_at_ms,
            ))
        },
    )
    .map_err(|error| format!("{error:?}"))
}

fn execute_worker_input(
    context: &RemoteCliContext,
    session_id: SessionId,
    limit: usize,
    attempt_owner: &str,
) -> RemoteFeishuExecutorTickInput {
    let now_ms = now_ms();
    RemoteFeishuExecutorTickInput {
        outbox_store_path: context.outbox_store_path.clone(),
        processing_store_path:
            axon_remote_feishu::RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(
                &context.root,
            ),
        scheduler_store_path:
            axon_remote_feishu::RemoteFeishuSchedulerStoreRuntime::resolve_store_path(&context.root),
        feedback_store_path: RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(
            &context.root,
        ),
        session_id,
        now_ms,
        accepted_at_ms: now_ms,
        limit,
        attempt_owner: attempt_owner.to_owned(),
    }
}

fn build_execute_worker_dispatch_plan(
    context: &RemoteCliContext,
    record: &axon_remote_runtime::RemoteNotificationOutboxRecord,
) -> axon_domain::AxonResult<axon_remote_feishu::RemoteFeishuWebSocketDispatchPlan> {
    project_feishu_dispatch_plan(context, record).map_err(axon_domain::AxonError::runtime_failure)
}

fn execute_worker_dispatch(
    context: &RemoteCliContext,
    app_secret: &str,
    dispatch_plan: &axon_remote_feishu::RemoteFeishuWebSocketDispatchPlan,
    record: &axon_remote_runtime::RemoteNotificationOutboxRecord,
    sent_at_ms: u64,
) -> axon_domain::AxonResult<axon_remote_feishu::RemoteFeishuDeliveryExecutionReport> {
    let agent_reply = resolve_agent_reply_artifact(context, record)
        .map_err(axon_domain::AxonError::runtime_failure)?;
    match agent_reply.as_ref() {
        Some(reply) => RemoteFeishuDeliveryRuntime::execute_agent_reply_delivery_with_secret(
            dispatch_plan,
            app_secret,
            feishu_api_base_url(context),
            record,
            reply,
            sent_at_ms,
        ),
        None => RemoteFeishuDeliveryRuntime::execute_with_secret(
            dispatch_plan,
            app_secret,
            feishu_api_base_url(context),
            record,
            sent_at_ms,
        ),
    }
}
