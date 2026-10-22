//! Agent TUI 底部交互视图状态。

use crate::app_shell::composer_dock::{DockCloseResult, DockLifecycle, DockState};
use crate::r#loop::agent_tui::{
    bottom_view::{
        BottomView, BottomViewAction, BottomViewEvent, SelectionView, SkillsToggleSummary,
        SkillsToggleView,
    },
    state::{AgentTuiState, AgentTuiSubmit},
};
use crate::ui_workflow::action::UiKeyEvent;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn bottom_view_open(&self) -> bool {
        self.composer.bottom_views.is_open()
    }

    pub(in crate::r#loop::agent_tui) fn bottom_view_height(&self) -> u16 {
        self.composer
            .bottom_views
            .top_lifecycle()
            .map_or(0, DockLifecycle::visible_height)
    }

    pub(in crate::r#loop::agent_tui) fn bottom_view(&self) -> Option<&BottomView> {
        self.composer.bottom_views.top_view()
    }

    pub(in crate::r#loop::agent_tui) fn bottom_view_lifecycle(&self) -> Option<DockLifecycle> {
        self.composer.bottom_views.top_lifecycle()
    }

    pub(in crate::r#loop::agent_tui) fn open_skills_menu(&mut self) {
        self.open_bottom_view(BottomView::Selection(SelectionView::skills_menu()));
    }

    pub(in crate::r#loop::agent_tui) fn open_skills_list_from_view(&mut self) {
        self.composer.bottom_views.clear();
        self.replace_draft("$".to_owned());
    }

    pub(in crate::r#loop::agent_tui) fn open_skills_toggle_view(&mut self) {
        let skills = self.header.catalog.skills.clone();
        let disabled_skills = self.header.catalog.disabled_skills.clone();
        self.open_bottom_view(BottomView::SkillsToggle(SkillsToggleView::new(
            &skills,
            &disabled_skills,
        )));
    }

    pub(in crate::r#loop::agent_tui) fn handle_bottom_view_key(
        &mut self,
        key: UiKeyEvent,
    ) -> Option<AgentTuiSubmit> {
        if self
            .composer
            .bottom_views
            .top_lifecycle()
            .is_some_and(|lifecycle| lifecycle.state() == DockState::Closing)
        {
            return None;
        }
        let view = self.composer.bottom_views.top_mut()?;
        match view.view_mut().handle_key(key) {
            BottomViewEvent::None => None,
            BottomViewEvent::Close => {
                self.close_top_bottom_view();
                None
            }
            BottomViewEvent::Action(action) => {
                self.close_top_bottom_view();
                self.apply_bottom_view_action(action)
            }
            BottomViewEvent::SkillsClosed(summary) => {
                self.close_top_bottom_view();
                self.push_skills_toggle_summary(summary);
                None
            }
            BottomViewEvent::QuestionAnswered {
                lane_id,
                call_id,
                answers,
            } => {
                // 作答不新起一轮、也不自造可见正文：它必须走统一答案入口，
                // 由服务端用唯一编码器落地消息并回发 settled 事件。
                // 面板保持打开直到发送结果回落，失败时用户可原样重试。
                Some(self.question_answer_submission(
                    crate::r#loop::agent_tui::AgentTuiQuestionAnswer {
                        lane_id,
                        call_id,
                        answers,
                    },
                ))
            }
        }
    }

    /// 作答成功后收起提问面板；失败时面板保持打开以便重试。
    pub(in crate::r#loop::agent_tui) fn close_question_view(&mut self) -> DockCloseResult {
        if matches!(
            self.composer.bottom_views.top_view(),
            Some(BottomView::Question(_))
        ) {
            return self.close_top_bottom_view();
        }
        DockCloseResult::NoDock
    }

    pub(in crate::r#loop::agent_tui) fn open_bottom_view(&mut self, view: BottomView) {
        self.close_composer_popup();
        self.composer.command_suggestions.clear();
        self.composer.bottom_views.push(view);
    }

    fn close_top_bottom_view(&mut self) -> DockCloseResult {
        self.composer.bottom_views.request_close_top()
    }

    fn apply_bottom_view_action(&mut self, action: BottomViewAction) -> Option<AgentTuiSubmit> {
        match action {
            BottomViewAction::SkillsList => {
                self.open_skills_list_from_view();
                None
            }
            BottomViewAction::SkillsToggle => {
                self.open_skills_toggle_view();
                None
            }
        }
    }

    fn push_skills_toggle_summary(&mut self, summary: SkillsToggleSummary) {
        self.header.catalog.disabled_skills = summary.disabled_skills;
        if !summary.changes.is_empty() {
            self.record_skill_settings_change(summary.changes.clone());
        }
        self.push_history_item(
            "Skills",
            "启用状态",
            "管理",
            Some(format!(
                "启用 {}，禁用 {}，本次启用 {}，本次禁用 {}",
                summary.enabled_count,
                summary.disabled_count,
                summary.enabled_delta,
                summary.disabled_delta
            )),
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

    use super::*;
    use crate::r#loop::agent_tui::{
        AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiLaunchRequest, AgentTuiRunOverrides,
    };

    #[test]
    fn bottom_view_opens_as_eased_dock_lifecycle() {
        let mut state = test_state();
        let now = std::time::Instant::now();

        state.open_bottom_view(BottomView::Selection(SelectionView::skills_menu()));

        let lifecycle = state
            .bottom_view_lifecycle()
            .expect("bottom view should have lifecycle");
        assert_eq!(lifecycle.state(), DockState::Opening);
        assert_eq!(state.bottom_view_height(), 0);
        assert!(state.tick(now + std::time::Duration::from_millis(200)));
        assert_eq!(
            state.bottom_view_lifecycle().map(DockLifecycle::state),
            Some(DockState::Open)
        );
        assert!(state.bottom_view_height() > 0);
    }

    #[test]
    fn bottom_view_esc_close_finishes_on_tick() {
        let mut state = test_state();
        let now = std::time::Instant::now();
        state.open_bottom_view(BottomView::Selection(SelectionView::skills_menu()));
        assert!(state.tick(now + std::time::Duration::from_millis(200)));

        assert!(
            state
                .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Esc))
                .is_none()
        );
        assert_eq!(
            state.bottom_view_lifecycle().map(DockLifecycle::state),
            Some(DockState::Closing)
        );
        assert!(!state.bottom_view_open());
        assert!(state.bottom_view_lifecycle().is_some());
        assert!(state.tick(now + std::time::Duration::from_millis(400)));
        assert!(!state.bottom_view_open());
        assert!(
            state
                .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Esc))
                .is_none()
        );
    }

    fn test_state() -> AgentTuiState {
        AgentTuiState::new_for_test(&AgentTuiLaunchRequest {
            cwd: axon_test_support::test_temp_root(),
            prompt: None,
            workspace_instruction_paths: Vec::new(),
            config_status: AgentTuiConfigStatus::new("fixture", "model", "已认证"),
            overrides: AgentTuiRunOverrides::default(),
            catalog: AgentTuiCatalog::default(),
            runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
        })
    }
}
