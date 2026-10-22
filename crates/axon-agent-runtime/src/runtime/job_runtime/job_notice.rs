//! 作业完成通知的组装与投递。
//!
//! 从 `registry` 拆出：通知只依赖「作业事实」与运行代次，不碰注册表的锁，
//! 因此它是注册表之外的一层纯映射；放在一起会让注册表同时承担生命周期与载荷两件事。

use super::*;

/// 完成通知所需的作业事实；在持锁期间组装、在释放锁之后投递。
pub(super) struct JobNoticeFacts {
    pub(super) owner_id: String,
    pub(super) producer: JobProducerKind,
    pub(super) label: String,
    pub(super) terminal: JobTerminalState,
    pub(super) summary: Option<String>,
    pub(super) artifact_ref: PathBuf,
}

pub(super) fn build_job_notice(id: JobId, facts: JobNoticeFacts) -> Option<CompletionNotice> {
    let kind = facts.terminal.status.terminal_kind()?;
    let epoch = runtime_epoch();
    Some(CompletionNotice::new(
        facts.producer,
        format!("{}:{epoch}:{id}:{}", facts.producer.as_str(), kind.as_str()),
        format!("{epoch}:{id}"),
        facts.owner_id,
        kind,
        facts
            .terminal
            .reason
            .as_deref()
            .unwrap_or_else(|| kind.as_str()),
        facts.summary.as_deref().unwrap_or_default(),
        artifact_ref(&facts.artifact_ref),
        collaboration_ref(facts.producer, &facts.label),
    ))
}

/// 投递一条临时 Job 完成通知。
pub(super) fn publish_job_notice(id: JobId, facts: JobNoticeFacts) {
    if let Some(notice) = build_job_notice(id, facts) {
        publish_completion_notice(notice);
    }
}

/// 归档引用：空路径表示该生产者没有输出归档。
pub(super) fn artifact_ref(path: &Path) -> Option<String> {
    (!path.as_os_str().is_empty()).then(|| path.to_string_lossy().into_owned())
}

/// 协作引用：子代理作业给出其 AgentPath（后续可继续 `send_message`）；
/// Shell 作业没有协作面，返回 `None`。这把「生命周期走 job_id、协作走 AgentPath」
/// 的寻址规则固化进通知载荷。
pub(super) fn collaboration_ref(producer: JobProducerKind, label: &str) -> Option<String> {
    match producer {
        JobProducerKind::Shell => None,
        JobProducerKind::Subagent => Some(label.to_owned()),
    }
}
