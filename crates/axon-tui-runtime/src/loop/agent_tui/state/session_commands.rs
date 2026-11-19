//! 会话切换、`/new` 与 `/reset` 的客户端语义。
//!
//! 三个入口都属于「会话归属」变更，但边界不同：
//! - `/sessions` 选中：绑定目标会话，清空视图，按冷启动同一条历史回放入口载入历史。
//! - `/new`：断开当前会话绑定，下一轮由宿主分配新的 session id，可见转录与计数清零。
//! - `/reset`：保留 session id 与会话历史，只重置本地运行实例。
//!
//! 边界说明：`control.sqlite3` 的 `sessions.run_generation` 由 Daemon 会话控制面维护，
//! TUI 进程不直连该数据库、也不存在可调用的服务端重置接口。因此 `/reset` 按客户端
//! 等价语义实现：会话与会话历史不变，只把本地运行实例（轮台账、实时读数、活动 cell、
//! 生成指标）复位，使下一轮在同一个会话上开一个新运行实例。

use crate::r#loop::agent_tui::{
    AgentTuiRunOutput, AgentTuiSessionSummary, AgentTuiTokenUsage, TUI_HINT_IDLE,
    session_generation::SessionGenerationLedger,
    state::{AgentRuntimeStatus, AgentTuiState},
};

/// 会话历史无法回放时的提示标题。
const SESSION_HISTORY_UNAVAILABLE_TITLE: &str = "会话历史未载入";
/// 会话历史无法回放时的下一步建议。
const SESSION_HISTORY_UNAVAILABLE_HINT: &str =
    "可先用 /sessions 选择其他会话，或直接在该会话里继续对话。";
/// `/sessions` 切换后的视图提示。
const SESSION_SWITCH_VIEW_NOTE: &str = "已切换到目标会话，正在加载历史...";
/// `/new` 之后的视图提示。
const NEW_SESSION_VIEW_NOTE: &str = "已开启新会话，下一轮使用新的 session id。";

impl AgentTuiState {
    /// `/sessions` 选中会话：绑定目标会话并清空当前视图。
    ///
    /// 这里只登记「待载入历史」的请求；真正的历史回放由主循环取得
    /// [`Self::take_pending_session_switch`] 后交给宿主读取，再走与冷启动同一条
    /// [`Self::apply_session_history`] 路径写进转录，不新增渲染分支。
    pub(super) fn begin_session_switch(&mut self, session_id: &str) {
        self.bind_session_identity(session_id);
        self.begin_resume_replay();
        self.push_replay_note(SESSION_SWITCH_VIEW_NOTE);
        self.bridge.pending_session_switch = Some(session_id.to_owned());
    }

    /// 把会话身份绑到目标会话，并清掉上一条会话的读数。
    ///
    /// 顶栏的「累计 / 上一轮 / 摘要 / 运行模型」都是**会话级**读数：身份换了而读数没换，
    /// 界面就会把别人的数当成本会话的数。同一会话重复绑定（每轮都会带一次身份行）不重置，
    /// 否则累计读数会被逐轮清零。
    pub(in crate::r#loop::agent_tui) fn bind_session_identity(&mut self, session_id: &str) {
        if self.header.overrides.session.as_deref() == Some(session_id) {
            return;
        }
        self.header.overrides.session = Some(session_id.to_owned());
        self.header.overrides.continue_last = false;
        self.header.session_usage = AgentTuiTokenUsage::default();
        self.header.last_turn_usage = AgentTuiTokenUsage::default();
        self.header.session_summary = AgentTuiSessionSummary::default();
        self.header.session_run_model = None;
        self.bridge.session_generation = SessionGenerationLedger::default();
    }

    /// 取出待载入历史的会话 ID。
    pub(in crate::r#loop::agent_tui) fn take_pending_session_switch(&mut self) -> Option<String> {
        self.bridge.pending_session_switch.take()
    }

    /// 应用宿主读到的会话历史。
    ///
    /// 复用冷启动的 `restore_historical_turn`，因此会话切换与冷启动共用同一条
    /// 历史回放路径，不产生第二套渲染分支。
    pub(in crate::r#loop::agent_tui) fn apply_session_history(
        &mut self,
        output: AgentTuiRunOutput,
    ) {
        if output.is_empty() {
            self.fail_session_history("该会话没有可回放的历史");
            return;
        }
        self.restore_historical_turn(output);
        self.settle_session_history_replay();
    }

    /// 宿主无法按会话读取历史：把回放占位收成失败态并给出下一步。
    pub(in crate::r#loop::agent_tui) fn fail_session_history(
        &mut self,
        message: impl Into<String>,
    ) {
        self.fail_resume_replay();
        self.push_local_error(
            SESSION_HISTORY_UNAVAILABLE_TITLE,
            message,
            Some(SESSION_HISTORY_UNAVAILABLE_HINT.to_owned()),
        );
    }

    /// `/new`：断开当前会话绑定，下一轮使用新的 session id。
    ///
    /// 与 `/clear` 的差别是会话归属与计数：`/clear` 只清视图并保留顶栏读数，
    /// `/new` 同时清掉会话读数与会话绑定，顶栏此后显示 `new`。
    pub(super) fn start_new_session(&mut self) {
        self.header.overrides.session = None;
        self.header.overrides.continue_last = false;
        self.header.session_usage = AgentTuiTokenUsage::default();
        self.header.last_turn_usage = AgentTuiTokenUsage::default();
        self.header.session_summary = AgentTuiSessionSummary::default();
        self.header.session_run_model = None;
        self.bridge.session_generation = SessionGenerationLedger::default();
        self.clear_transient_runtime_state();
        self.clear_with_note(NEW_SESSION_VIEW_NOTE);
        self.clear_draft();
        self.header.hint = TUI_HINT_IDLE;
    }

    /// `/reset`：保留 session id 与会话历史，只重置本地运行实例。
    ///
    /// 服务端的 `run_generation` 由 Daemon 会话控制面自增，TUI 不可触达；
    /// 客户端等价做法是把本地运行实例整体复位，下一轮在同一会话上重新开始。
    /// 可见转录与会话级读数都不动，保证「会话与会话历史不变」。
    pub(super) fn reset_session_run(&mut self) {
        self.header.overrides.continue_last = false;
        self.clear_transient_runtime_state();
        self.bridge.reset_turn_ledger();
        self.bridge.session_generation = SessionGenerationLedger::default();
        self.bridge.subagent_activity.clear_and_suppress();
        self.finish_restored_runtime();
        self.bridge.runtime_status = AgentRuntimeStatus::Ready;
        self.header.hint = TUI_HINT_IDLE;
    }

    /// 把一句提示写进回放占位之后的第一个可见位置。
    fn push_replay_note(&mut self, note: &str) {
        self.cells
            .transcript
            .push(crate::surface::cell::RenderCellRecord::system(note));
    }
}

#[cfg(test)]
mod tests {
    use crate::r#loop::agent_tui::run_output::{
        AgentTuiMessagePart, AgentTuiRunLine, AgentTuiRunOutput, AgentTuiRunSessionInfo,
    };
    use crate::r#loop::agent_tui::state::runtime_flow_test_support::test_state;
    use crate::surface::replay_snapshot::ReplaySnapshotPhase;

    fn state_with_session(session_id: &str) -> crate::r#loop::agent_tui::state::AgentTuiState {
        let mut state = test_state();
        state.header.overrides.session = Some(session_id.to_owned());
        state.header.session_summary.record_session(
            session_id.to_owned(),
            None,
            Some("历史会话".to_owned()),
        );
        state.header.session_usage = crate::AgentTuiTokenUsage::new(11, 0, 0, 5, 0);
        state.header.last_turn_usage = state.header.session_usage;
        state
    }

    #[test]
    fn session_switch_binds_target_session_and_requests_history() {
        let mut state = test_state();

        state.begin_session_switch("session-9");

        assert_eq!(state.header.overrides.session.as_deref(), Some("session-9"));
        assert!(!state.header.overrides.continue_last);
        assert_eq!(
            state.header.replay_snapshot.map(|replay| replay.phase),
            Some(ReplaySnapshotPhase::Loading)
        );
        assert_eq!(
            state.take_pending_session_switch().as_deref(),
            Some("session-9")
        );
        assert_eq!(state.take_pending_session_switch(), None);
    }

    #[test]
    fn session_switch_replays_history_through_cold_start_entry() {
        let mut state = test_state();
        state.begin_session_switch("session-9");
        let _ = state.take_pending_session_switch();

        let mut output = AgentTuiRunOutput::empty();
        output.push(AgentTuiRunLine::Session(AgentTuiRunSessionInfo::new(
            "session-9",
            None,
        )));
        output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::user(
            "part:user:history",
            "上一轮提问",
        )));
        output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
            "part:assistant:history",
            "上一轮回答",
        )));
        state.apply_session_history(output);

        assert_eq!(
            state.header.session_summary.session_id.as_deref(),
            Some("session-9")
        );
        assert_eq!(
            state.header.replay_snapshot, None,
            "回放收口后不应残留加载相位"
        );
        let transcript = state
            .cells
            .transcript
            .iter()
            .map(|cell| cell.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(transcript.contains("上一轮提问"), "历史用户轮必须回放");
        assert!(transcript.contains("上一轮回答"), "历史助手轮必须回放");
        assert!(
            !transcript.contains(crate::surface::replay_snapshot::RESUME_LOADING_TITLE),
            "回放完成后不能留下加载占位"
        );
    }

    #[test]
    fn empty_history_marks_replay_failed_instead_of_silent_success() {
        let mut state = test_state();
        state.begin_session_switch("session-9");

        state.apply_session_history(AgentTuiRunOutput::empty());

        assert_eq!(
            state.header.replay_snapshot.map(|replay| replay.phase),
            Some(ReplaySnapshotPhase::Error)
        );
        let transcript = state
            .cells
            .transcript
            .iter()
            .map(|cell| cell.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(transcript.contains(super::SESSION_HISTORY_UNAVAILABLE_TITLE));
    }

    #[test]
    fn new_session_drops_session_identity_and_session_metrics() {
        let mut state = state_with_session("session-1");

        state.start_new_session();

        assert_eq!(state.header.overrides.session, None);
        assert_eq!(state.header.session_summary.session_id, None);
        assert_eq!(state.header.session_summary.turns, 0);
        assert_eq!(state.session_usage().total_tokens(), 0);
        assert_eq!(state.header.session_run_model, None);
    }

    #[test]
    fn reset_session_keeps_session_identity_and_history_metrics() {
        let mut state = state_with_session("session-1");
        state.header.session_summary.record_turn(
            false,
            crate::AgentTuiTokenUsage::new(11, 0, 0, 5, 0),
            std::time::Duration::from_secs(3),
        );

        state.reset_session_run();

        assert_eq!(state.header.overrides.session.as_deref(), Some("session-1"));
        assert_eq!(
            state.header.session_summary.session_id.as_deref(),
            Some("session-1")
        );
        assert_eq!(state.header.session_summary.turns, 1);
        assert_eq!(state.session_usage().total_tokens(), 16);
        assert!(!state.is_working());
    }
}

#[cfg(test)]
mod session_binding_tests {
    use crate::r#loop::agent_tui::{AgentTuiTokenUsage, state::runtime_flow_test_support::test_state};

    fn usage_with(input_tokens: u64) -> AgentTuiTokenUsage {
        AgentTuiTokenUsage {
            input_tokens,
            ..AgentTuiTokenUsage::default()
        }
    }

    #[test]
    fn switching_session_clears_previous_session_readings() {
        let mut state = test_state();
        state.bind_session_identity("session-a");
        state.header.session_usage = usage_with(42);
        state.header.last_turn_usage = usage_with(7);

        state.begin_session_switch("session-b");

        assert_eq!(
            state.header.session_usage.input_tokens, 0,
            "累计读数必须跟着会话身份走"
        );
        assert_eq!(
            state.header.last_turn_usage.input_tokens, 0,
            "上一轮读数不得跨会话残留"
        );
        assert_eq!(state.header.overrides.session.as_deref(), Some("session-b"));
    }

    #[test]
    fn rebinding_the_same_session_keeps_cumulative_readings() {
        let mut state = test_state();
        state.bind_session_identity("session-a");
        state.header.session_usage = usage_with(42);

        // 每轮输出都会带一次身份行：同一会话重复绑定不得清空累计。
        state.bind_session_identity("session-a");

        assert_eq!(state.header.session_usage.input_tokens, 42);
    }
}
