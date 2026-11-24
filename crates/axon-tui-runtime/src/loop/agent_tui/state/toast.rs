//! Agent TUI 临时 toast 状态。

use std::time::{Duration, Instant};

pub(crate) const TOAST_VISIBLE_DURATION: Duration = Duration::from_millis(2200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToastKind {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ToastMessage {
    pub(crate) text: String,
    pub(crate) kind: ToastKind,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ToastState {
    message: Option<ToastMessage>,
    shown_at: Option<Instant>,
}

impl ToastState {
    pub(crate) fn show(&mut self, text: impl Into<String>, kind: ToastKind, now: Instant) {
        self.message = Some(ToastMessage {
            text: text.into(),
            kind,
        });
        self.shown_at = Some(now);
    }

    pub(crate) fn current(&self, now: Instant) -> Option<&ToastMessage> {
        let shown_at = self.shown_at?;
        if now.duration_since(shown_at) <= TOAST_VISIBLE_DURATION {
            return self.message.as_ref();
        }
        None
    }

    pub(crate) fn expire_if_due(&mut self, now: Instant) -> bool {
        let Some(shown_at) = self.shown_at else {
            return false;
        };
        if now.duration_since(shown_at) <= TOAST_VISIBLE_DURATION {
            return false;
        }
        self.message = None;
        self.shown_at = None;
        true
    }

    pub(crate) fn signature(&self, now: Instant) -> Option<(&str, ToastKind)> {
        self.current(now)
            .map(|message| (message.text.as_str(), message.kind))
    }
}

#[cfg(test)]
mod tests {
    use super::{TOAST_VISIBLE_DURATION, ToastKind, ToastState};
    use std::time::Duration;

    #[test]
    fn toast_expires_once_after_visible_duration() {
        let mut state = ToastState::default();
        let shown_at = std::time::Instant::now();
        state.show("已复制 3 个字符", ToastKind::Success, shown_at);

        assert!(!state.expire_if_due(shown_at + TOAST_VISIBLE_DURATION));
        assert!(state.expire_if_due(shown_at + TOAST_VISIBLE_DURATION + Duration::from_millis(1)));
        assert!(!state.expire_if_due(shown_at + TOAST_VISIBLE_DURATION + Duration::from_millis(2)));
        assert!(
            state
                .current(shown_at + TOAST_VISIBLE_DURATION + Duration::from_millis(2))
                .is_none()
        );
    }

    #[test]
    fn showing_new_toast_resets_expiry_window() {
        let mut state = ToastState::default();
        let shown_at = std::time::Instant::now();
        state.show("第一次复制", ToastKind::Success, shown_at);
        state.show(
            "第二次复制",
            ToastKind::Success,
            shown_at + TOAST_VISIBLE_DURATION,
        );

        assert!(!state.expire_if_due(shown_at + TOAST_VISIBLE_DURATION + Duration::from_millis(1)));
        assert_eq!(
            state
                .current(shown_at + TOAST_VISIBLE_DURATION + Duration::from_millis(1))
                .map(|message| message.text.as_str()),
            Some("第二次复制")
        );
    }
}
