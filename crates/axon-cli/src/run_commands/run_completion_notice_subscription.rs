//! 临时 Job 完成通知的运行面接入（合同 §5.1.5）。
//!
//! 一次性 run 与后台作业同进程：作业终态一产生就经通知总线回到当前 run，
//! 由本模块把它转成 `CompletionNotice` 运行时注入，落在既有 steer 通道上：
//! 下一个 provider 请求因此必然看到它，无需模型轮询。

use std::sync::Arc;

use axon_agent_runtime::{
    AgentRuntimeNoteKind, CompletionNoticeSubscription, ShellSessionReclaimSubscription,
    subscribe_completion_notices, subscribe_shell_session_reclaims,
};

use super::run_steer::try_send_deferred_runtime_note;

/// 把当前 run 的 owner 接入完成通知总线；返回的句柄 drop 即退订。
///
/// `session_key` 同时是作业 owner 与 steer 通道的注册键（两者同源为
/// `request.session_id`），因此通知的归属与注入目标天然一致。
pub(crate) fn subscribe_run_completion_notices(session_key: &str) -> CompletionNoticeSubscription {
    let owner = session_key.to_owned();
    let inject_key = session_key.to_owned();
    subscribe_completion_notices(
        &owner,
        Arc::new(move |batch| {
            let content = batch.render();
            if let Err(error) = try_send_deferred_runtime_note(
                &inject_key,
                AgentRuntimeNoteKind::CompletionNotice,
                content,
            ) {
                // 注入通道不可用不是数据丢失：作业终态仍在注册表里，
                // 模型可继续用 job_output 拉取，因此这里只记录不阻断。
                axon_logger::warning(&format!(
                    "完成通知注入失败，模型可改用 job_output 拉取: session={inject_key}, error={error:?}"
                ));
            }
        }),
    )
}

/// 把当前 run 接入**前台会话空闲回收**通知；返回的句柄 drop 即退订。
///
/// 与完成通知同源接入（同一个 owner、同一条 steer 通道、同一个 note 载体），
/// 但用独立 tag：会话回收通知的后续动作是「别再轮询这条 `session_id`」，
/// 与作业完成通知的「用 `job_output` 读结果」是两件事。
pub(crate) fn subscribe_run_shell_session_reclaims(
    session_key: &str,
) -> ShellSessionReclaimSubscription {
    let owner = session_key.to_owned();
    let inject_key = session_key.to_owned();
    subscribe_shell_session_reclaims(
        &owner,
        Arc::new(move |notice| {
            let content = notice.render();
            if let Err(error) = try_send_deferred_runtime_note(
                &inject_key,
                AgentRuntimeNoteKind::ShellSessionReclaimed,
                content,
            ) {
                // 注入通道不可用不是数据丢失：会话终态仍是注册表事实，
                // 模型下次 write_stdin 会读到 cancelled 终态，因此这里只记录不阻断。
                axon_logger::warning(&format!(
                    "会话回收通知注入失败，模型可在 write_stdin 回执中读到终态: session={inject_key}, error={error:?}"
                ));
            }
        }),
    )
}
