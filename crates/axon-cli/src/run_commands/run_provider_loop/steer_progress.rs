//! 运行中输入在安全边界消费时的进度投影。

use axon_agent_runtime::{AgentTurnSteerInput, AgentTurnSteerSource};

use super::RunProgressSink;

pub(super) struct ProgressSteerSource<'source, 'progress> {
    source: &'source dyn AgentTurnSteerSource,
    progress: Option<&'source RunProgressSink<'progress>>,
}

impl<'source, 'progress> ProgressSteerSource<'source, 'progress> {
    pub(super) fn new(
        source: &'source dyn AgentTurnSteerSource,
        progress: Option<&'source RunProgressSink<'progress>>,
    ) -> Self {
        Self { source, progress }
    }

    fn project_consumed_messages(&self, inputs: &[AgentTurnSteerInput]) {
        let Some(progress) = self.progress else {
            return;
        };
        for message in inputs
            .iter()
            .map(|input| &input.message)
            .filter(|message| message.role == "user" && !message.cache_excluded)
        {
            progress.steered_user_message(message.content.clone());
        }
    }
}

impl AgentTurnSteerSource for ProgressSteerSource<'_, '_> {
    fn drain(&self) -> Vec<AgentTurnSteerInput> {
        let inputs = self.source.drain();
        self.project_consumed_messages(&inputs);
        inputs
    }

    fn close_and_drain(&self) -> Vec<AgentTurnSteerInput> {
        let inputs = self.source.close_and_drain();
        self.project_consumed_messages(&inputs);
        inputs
    }

    fn reopen(&self) {
        self.source.reopen();
    }

    fn pending_background_jobs(&self) -> usize {
        self.source.pending_background_jobs()
    }
}

/// 给 steer 通道补上 pre-final 守卫判据（合同 §5.1.6）。
///
/// 注入通道本身不认识作业注册表，而守卫判据要求「当前 run 的 owner 范围内是否存在
/// 未终态作业」。本包装层把 owner 与作业注册表接上，其余行为原样透传；
/// 「至多延迟一轮」的边界由 turn runtime 的独立标记控制，不在本层重复实现。
pub(super) struct BackgroundJobGuardSteerSource<S> {
    inner: S,
    owner_id: String,
}

impl<S> BackgroundJobGuardSteerSource<S> {
    pub(super) fn new(inner: S, owner_id: String) -> Self {
        Self { inner, owner_id }
    }
}

impl<S: AgentTurnSteerSource> AgentTurnSteerSource for BackgroundJobGuardSteerSource<S> {
    fn drain(&self) -> Vec<AgentTurnSteerInput> {
        self.inner.drain()
    }

    fn close_and_drain(&self) -> Vec<AgentTurnSteerInput> {
        self.inner.close_and_drain()
    }

    fn reopen(&self) {
        self.inner.reopen();
    }

    fn pending_background_jobs(&self) -> usize {
        axon_agent_runtime::JobRuntime::global()
            .pending_job_ids(&self.owner_id)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use axon_protocol::wire::WireEventKind;

    use super::*;
    use crate::run_commands::run_steer::RunSteerBridge;

    #[test]
    fn consumed_steer_emits_user_message_at_safe_boundary() {
        let bridge = RunSteerBridge::default();
        let inbox = bridge.open();
        bridge.try_send("补充约束".to_owned()).unwrap();
        let mut events = Vec::new();

        {
            let mut emit = |event| events.push(event);
            let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
            let source = ProgressSteerSource::new(&inbox, Some(&sink));

            let inputs = source.drain();

            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].message.content, "补充约束");
        }
        assert!(matches!(
            &events[0].event.kind,
            WireEventKind::UserMessage(message) if message.content == "补充约束"
        ));
    }

    #[test]
    fn internal_runtime_note_is_not_projected_as_user_message() {
        use axon_agent_runtime::{AgentModelMessage, AgentRuntimeNoteKind};

        let bridge = RunSteerBridge::default();
        let inbox = bridge.open();
        inbox
            .try_send_message(AgentModelMessage::runtime_note(
                AgentRuntimeNoteKind::DeferredDiagnostics,
                "late diagnostics",
            ))
            .unwrap();
        let mut events = Vec::new();

        {
            let mut emit = |event| events.push(event);
            let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
            let source = ProgressSteerSource::new(&inbox, Some(&sink));

            let inputs = source.drain();

            assert_eq!(inputs.len(), 1);
            assert!(inputs[0].message.cache_excluded);
        }
        assert!(events.is_empty());
    }
}
