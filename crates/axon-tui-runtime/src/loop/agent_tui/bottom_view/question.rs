//! TUI 问答交互选择控件（UQ-202）。

use std::collections::HashSet;

use axon_domain::{UserAnswerItem, UserQuestionItem};

use super::BottomViewEvent;
use crate::app_shell::composer_dock::DockKind;
use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

/// 最多支持展示的选项数量上限（守卫 B10）。
pub const MAX_TUI_VISIBLE_OPTIONS: usize = 8;
/// 单题自定义答案的字符上限；超出部分不再写入，避免面板与回执无界增长。
pub const MAX_TUI_CUSTOM_ANSWER_CHARS: usize = 2000;

/// TUI 用户问答交互面板视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionView {
    lane_id: String,
    call_id: String,
    questions: Vec<UserQuestionItem>,
    current_index: usize,
    selected_options: Vec<HashSet<String>>,
    custom_inputs: Vec<String>,
    focused_option: usize,
    editing: bool,
    deadline_at_ms: i64,
}

impl QuestionView {
    /// 构造新的提问控件视图。
    ///
    /// `lane_id` 是问题所属的会话车道身份：作答必须回到提出问题的那条车道，
    /// 面板本身不重新推导，直接沿用事件携带的身份。
    pub(crate) fn new(
        lane_id: impl Into<String>,
        call_id: impl Into<String>,
        questions: Vec<UserQuestionItem>,
        deadline_at_ms: i64,
    ) -> Self {
        let count = questions.len();
        Self {
            lane_id: lane_id.into(),
            call_id: call_id.into(),
            questions,
            current_index: 0,
            selected_options: vec![HashSet::new(); count],
            custom_inputs: vec![String::new(); count],
            focused_option: 0,
            editing: false,
            deadline_at_ms,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn call_id(&self) -> &str {
        &self.call_id
    }

    /// 问题所属车道身份。
    #[allow(dead_code)]
    pub(crate) fn lane_id(&self) -> &str {
        &self.lane_id
    }

    pub(crate) const fn kind(&self) -> DockKind {
        DockKind::Question
    }

    pub(crate) fn current_question(&self) -> Option<&UserQuestionItem> {
        self.questions.get(self.current_index)
    }

    pub(crate) fn current_index(&self) -> usize {
        self.current_index
    }

    pub(crate) fn total_questions(&self) -> usize {
        self.questions.len()
    }

    pub(crate) fn focused_option(&self) -> usize {
        self.focused_option
    }

    /// 当前是否处于自定义答案编辑态。
    pub(crate) fn is_editing(&self) -> bool {
        self.editing
    }

    /// 当前题已键入的自定义答案。
    pub(crate) fn custom_input(&self) -> &str {
        self.custom_inputs
            .get(self.current_index)
            .map_or("", String::as_str)
    }

    pub(crate) fn is_option_selected(&self, label: &str) -> bool {
        self.selected_options
            .get(self.current_index)
            .is_some_and(|set| set.contains(label))
    }

    #[allow(dead_code)]
    pub(crate) fn deadline_at_ms(&self) -> i64 {
        self.deadline_at_ms
    }

    /// 处理键盘交互。
    ///
    /// 语义边界：`Esc` 只收起面板，既不取消提问也不作答；显式跳过是独立的
    /// `Tab` 操作，编码为 `{id, selected: []}`。
    pub(super) fn handle_key(&mut self, key: UiKeyEvent) -> BottomViewEvent {
        if self.questions.get(self.current_index).is_none() {
            return BottomViewEvent::Close;
        }
        if self.editing {
            return self.handle_editing_key(key);
        }
        match key.code {
            UiKeyCode::Up => {
                self.move_focus(-1);
                BottomViewEvent::None
            }
            UiKeyCode::Down => {
                self.move_focus(1);
                BottomViewEvent::None
            }
            UiKeyCode::Char(' ') if key.modifiers.is_plain_or_shift() => {
                self.toggle_focused_option();
                BottomViewEvent::None
            }
            UiKeyCode::Char(value) if key.modifiers.is_plain_or_shift() && !value.is_control() => {
                self.begin_custom_input(Some(value));
                BottomViewEvent::None
            }
            UiKeyCode::Backspace => {
                self.begin_custom_input(None);
                BottomViewEvent::None
            }
            UiKeyCode::Tab => self.skip_current_question(),
            UiKeyCode::Enter => self.finish_current_question(true),
            UiKeyCode::Esc => BottomViewEvent::Close,
            _ => BottomViewEvent::None,
        }
    }

    /// 编辑态按键：字符写入、Backspace 回退、上下回到选项、Enter 提交、Esc 退出编辑态。
    ///
    /// 上下键是「从自定义答案回到选项列表」的自然手势：退出编辑态、焦点交回选项，
    /// 已键入的内容原样保留。没有这一条，键入一个字符之后选项就再也选不回来了，
    /// 面板看起来像卡在自定义答案栏位上。
    fn handle_editing_key(&mut self, key: UiKeyEvent) -> BottomViewEvent {
        match key.code {
            UiKeyCode::Up => {
                self.leave_custom_input();
                self.move_focus(-1);
                BottomViewEvent::None
            }
            UiKeyCode::Down => {
                self.leave_custom_input();
                self.move_focus(1);
                BottomViewEvent::None
            }
            UiKeyCode::Char(value) if key.modifiers.is_plain_or_shift() && !value.is_control() => {
                self.push_custom_char(value);
                BottomViewEvent::None
            }
            // Space 在编辑态属于自定义答案的一部分，不再切换选项。
            UiKeyCode::Char(' ') if key.modifiers.is_plain_or_shift() => {
                self.push_custom_char(' ');
                BottomViewEvent::None
            }
            UiKeyCode::Backspace => {
                self.pop_custom_char();
                BottomViewEvent::None
            }
            UiKeyCode::Enter => self.finish_current_question(true),
            UiKeyCode::Esc => {
                // 退出编辑态但保留已输入的作答，面板不收起。
                self.leave_custom_input();
                BottomViewEvent::None
            }
            UiKeyCode::Tab => self.skip_current_question(),
            _ => BottomViewEvent::None,
        }
    }

    /// 在选项列表里移动焦点并环绕；没有选项时保持原位。
    fn move_focus(&mut self, delta: isize) {
        let len = self
            .current_question()
            .map_or(0, |question| question.options.len());
        if len == 0 {
            return;
        }
        let len = i64::try_from(len).unwrap_or(1);
        let focused = i64::try_from(self.focused_option).unwrap_or(0);
        let next = (focused + i64::try_from(delta).unwrap_or(0)).rem_euclid(len);
        self.focused_option = usize::try_from(next).unwrap_or(0);
    }

    /// 退出自定义答案编辑态；已键入内容保留。
    fn leave_custom_input(&mut self) {
        self.editing = false;
    }

    fn toggle_focused_option(&mut self) {
        let Some(current) = self.questions.get(self.current_index) else {
            return;
        };
        let Some(option) = current.options.get(self.focused_option) else {
            return;
        };
        let label = option.label.clone();
        let multi_select = current.multi_select;
        let Some(set) = self.selected_options.get_mut(self.current_index) else {
            return;
        };
        if multi_select {
            if !set.remove(&label) {
                set.insert(label);
            }
        } else {
            set.clear();
            set.insert(label);
        }
    }

    /// 进入或继续自定义答案编辑态；`seed` 为本次按键带来的首字符。
    fn begin_custom_input(&mut self, seed: Option<char>) {
        self.editing = true;
        if let Some(value) = seed {
            self.push_custom_char(value);
        }
    }

    fn push_custom_char(&mut self, value: char) {
        let Some(custom) = self.custom_inputs.get_mut(self.current_index) else {
            return;
        };
        if custom.chars().count() >= MAX_TUI_CUSTOM_ANSWER_CHARS {
            return;
        }
        custom.push(value);
    }

    fn pop_custom_char(&mut self) {
        if let Some(custom) = self.custom_inputs.get_mut(self.current_index) {
            custom.pop();
        }
    }

    /// 显式跳过当前题：清空该题的勾选与自定义答案，编码为 `{id, selected: []}`。
    fn skip_current_question(&mut self) -> BottomViewEvent {
        if let Some(set) = self.selected_options.get_mut(self.current_index) {
            set.clear();
        }
        if let Some(custom) = self.custom_inputs.get_mut(self.current_index) {
            custom.clear();
        }
        self.editing = false;
        self.finish_current_question(false)
    }

    /// 结束当前题：还有下一题就前移，否则交出全部作答。
    ///
    /// `default_select` 只在 Enter 路径为真：显式跳过必须原样编码空选择集，
    /// 不能被「单选题默认选中聚焦项」的兜底填回一个答案。
    fn finish_current_question(&mut self, default_select: bool) -> BottomViewEvent {
        self.editing = false;
        let Some(current) = self.questions.get(self.current_index) else {
            return BottomViewEvent::Close;
        };
        // 单选且既未勾选也未自定义时，默认选中当前聚焦项，避免交出空答案。
        if default_select
            && !current.multi_select
            && self.custom_input().trim().is_empty()
            && let Some(option) = current.options.get(self.focused_option)
        {
            let label = option.label.clone();
            if let Some(set) = self.selected_options.get_mut(self.current_index)
                && set.is_empty()
            {
                set.insert(label);
            }
        }

        if self.current_index + 1 < self.questions.len() {
            self.current_index += 1;
            self.focused_option = 0;
            return BottomViewEvent::None;
        }
        BottomViewEvent::QuestionAnswered {
            lane_id: self.lane_id.clone(),
            call_id: self.call_id.clone(),
            answers: self.build_answers(),
        }
    }

    /// 高度预算（守卫 B10）：面板高度 <= 8 项 * 2 行 + 2。
    pub(crate) fn height(&self) -> u16 {
        let opt_count = self
            .current_question()
            .map_or(0, |q| q.options.len().min(MAX_TUI_VISIBLE_OPTIONS));
        let rows = (opt_count * 2) + 2;
        u16::try_from(rows).unwrap_or(18)
    }

    fn build_answers(&self) -> Vec<UserAnswerItem> {
        self.questions
            .iter()
            .enumerate()
            .map(|(idx, q)| {
                let selected = self
                    .selected_options
                    .get(idx)
                    .map(|set| set.iter().cloned().collect())
                    .unwrap_or_default();
                let custom = self
                    .custom_inputs
                    .get(idx)
                    .filter(|s| !s.trim().is_empty())
                    .cloned();
                UserAnswerItem {
                    id: q.id.clone(),
                    selected,
                    custom,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent, UiKeyModifiers};
    use axon_domain::UserQuestionOption;

    fn sample_question_view() -> QuestionView {
        let q1 = UserQuestionItem {
            id: "q1".to_string(),
            question: "请选择部署模式".to_string(),
            header: Some("部署确认".to_string()),
            detail: None,
            options: vec![
                UserQuestionOption {
                    label: "Prod".to_string(),
                    description: None,
                },
                UserQuestionOption {
                    label: "Dev".to_string(),
                    description: None,
                },
            ],
            multi_select: false,
        };
        QuestionView::new("lane-1", "call-123", vec![q1], 60_000)
    }

    fn key(code: UiKeyCode) -> UiKeyEvent {
        UiKeyEvent {
            code,
            modifiers: UiKeyModifiers {
                control: false,
                alt: false,
                shift: false,
            },
        }
    }

    fn answered(event: BottomViewEvent) -> (String, String, Vec<UserAnswerItem>) {
        match event {
            BottomViewEvent::QuestionAnswered {
                lane_id,
                call_id,
                answers,
            } => (lane_id, call_id, answers),
            other => panic!("期望 QuestionAnswered，收到 {other:?}"),
        }
    }

    #[test]
    fn test_question_view_navigation_and_selection() {
        let mut view = sample_question_view();
        assert_eq!(view.focused_option(), 0);

        // Down
        view.handle_key(key(UiKeyCode::Down));
        assert_eq!(view.focused_option(), 1);

        // Down 环绕回 0
        view.handle_key(key(UiKeyCode::Down));
        assert_eq!(view.focused_option(), 0);

        // Up 环绕到 1
        view.handle_key(key(UiKeyCode::Up));
        assert_eq!(view.focused_option(), 1);

        // 空格选中 Dev
        view.handle_key(key(UiKeyCode::Char(' ')));
        assert!(view.is_option_selected("Dev"));
        assert!(!view.is_option_selected("Prod"));

        // Enter 提交
        let (lane_id, call_id, answers) = answered(view.handle_key(key(UiKeyCode::Enter)));
        assert_eq!(lane_id, "lane-1");
        assert_eq!(call_id, "call-123");
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].selected, vec!["Dev".to_string()]);
        assert_eq!(answers[0].custom, None);
    }

    #[test]
    fn test_question_view_escape_closes_without_answering() {
        let mut view = sample_question_view();
        view.handle_key(key(UiKeyCode::Char(' ')));
        let event = view.handle_key(key(UiKeyCode::Esc));
        assert_eq!(event, BottomViewEvent::Close);
        // 收起不等于取消：已勾选内容原样保留，重新展开仍在。
        assert!(view.is_option_selected("Prod"));
        assert!(!view.is_editing());
    }

    #[test]
    fn test_question_view_explicit_skip_encodes_empty_selection() {
        let mut view = sample_question_view();
        view.handle_key(key(UiKeyCode::Char(' ')));
        assert!(view.is_option_selected("Prod"));

        let (_, _, answers) = answered(view.handle_key(key(UiKeyCode::Tab)));

        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].id, "q1");
        assert!(answers[0].selected.is_empty(), "显式跳过必须编码为空选择集");
        assert_eq!(answers[0].custom, None);
    }

    #[test]
    fn test_question_view_custom_input_is_typed_and_submitted() {
        let mut view = sample_question_view();

        // 首个可打印字符进入编辑态
        view.handle_key(key(UiKeyCode::Char('A')));
        assert!(view.is_editing());
        for ch in "lpha".chars() {
            view.handle_key(key(UiKeyCode::Char(ch)));
        }
        assert_eq!(view.custom_input(), "Alpha");

        // 编辑态下空格属于自定义答案内容
        view.handle_key(key(UiKeyCode::Char(' ')));
        assert_eq!(view.custom_input(), "Alpha ");
        assert!(!view.is_option_selected("Prod"));

        // Backspace 回退
        view.handle_key(key(UiKeyCode::Backspace));
        assert_eq!(view.custom_input(), "Alpha");

        // Enter 提交该自定义答案
        let (_, _, answers) = answered(view.handle_key(key(UiKeyCode::Enter)));
        assert_eq!(answers[0].custom.as_deref(), Some("Alpha"));
        assert!(answers[0].selected.is_empty());
    }

    #[test]
    fn test_question_view_escape_leaves_editing_and_keeps_custom_answer() {
        let mut view = sample_question_view();
        view.handle_key(key(UiKeyCode::Char('X')));
        assert!(view.is_editing());

        assert_eq!(
            view.handle_key(key(UiKeyCode::Esc)),
            BottomViewEvent::None,
            "编辑态 Esc 只退出编辑态"
        );
        assert!(!view.is_editing());
        assert_eq!(view.custom_input(), "X");
    }

    #[test]
    fn test_question_view_arrows_leave_custom_input_and_return_to_options() {
        let mut view = sample_question_view();

        // 键入一个字符即进入编辑态：此前这里就是死胡同，上下键是空操作。
        view.handle_key(key(UiKeyCode::Char('X')));
        assert!(view.is_editing());
        assert_eq!(view.focused_option(), 0);

        // Down：退出编辑态、焦点交给下一项，已键入内容保留。
        assert_eq!(view.handle_key(key(UiKeyCode::Down)), BottomViewEvent::None);
        assert!(!view.is_editing(), "上下键必须把焦点交回选项列表");
        assert_eq!(view.focused_option(), 1);
        assert_eq!(
            view.custom_input(),
            "X",
            "退出编辑不得丢掉已键入的自定义答案"
        );

        // Up：同样退出编辑并回到上一项；此时焦点在 0，环绕到末项。
        view.handle_key(key(UiKeyCode::Char('Y')));
        assert!(view.is_editing());
        assert_eq!(view.custom_input(), "XY");
        view.handle_key(key(UiKeyCode::Up));
        assert!(!view.is_editing());
        assert_eq!(view.focused_option(), 0, "从 1 上移一格应回到 0");

        // 回到选项列表后，空格仍然切换选项而不是写进自定义答案。
        view.handle_key(key(UiKeyCode::Char(' ')));
        assert!(view.is_option_selected("Prod"));
        assert_eq!(
            view.custom_input(),
            "XY",
            "在选项列表里按空格不得改写自定义答案"
        );
    }

    #[test]
    fn test_question_view_height_bounded() {
        let view = sample_question_view();
        // 2 选项 -> 2*2 + 2 = 6
        assert_eq!(view.height(), 6);
        assert!(view.height() <= 18);
        assert_eq!(view.kind(), DockKind::Question);
        assert_eq!(view.call_id(), "call-123");
        assert_eq!(view.deadline_at_ms(), 60_000);
        assert_eq!(view.lane_id(), "lane-1");
    }
}
