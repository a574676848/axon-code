//! TUI reducer 契约。

/// reducer 产生的慢路径副作用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UiSideEffect {
    RuntimeSubmit,
    RuntimeSteer,
    RuntimeInterrupt { reason: &'static str },
    AnswerQuestion,
    OpenExternalEditor,
}

/// UI action 应用结果，主循环只按此结果决定是否调度帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiActionOutcome {
    pub(crate) needs_redraw: bool,
    pub(crate) side_effect: Option<UiSideEffect>,
}

impl UiActionOutcome {
    #[must_use]
    pub(crate) const fn ignored() -> Self {
        Self {
            needs_redraw: false,
            side_effect: None,
        }
    }

    #[must_use]
    pub(crate) const fn redraw() -> Self {
        Self {
            needs_redraw: true,
            side_effect: None,
        }
    }

    #[must_use]
    pub(crate) const fn side_effect(side_effect: UiSideEffect) -> Self {
        Self {
            needs_redraw: true,
            side_effect: Some(side_effect),
        }
    }
}

/// reducer 类型本身不持有状态，具体状态实现由宿主模块绑定。
#[derive(Debug, Default)]
pub(crate) struct UiReducer;

pub(crate) trait UiActionReducer<State> {
    fn reduce(state: &mut State, action: crate::ui_workflow::action::UiAction) -> UiActionOutcome;
}
