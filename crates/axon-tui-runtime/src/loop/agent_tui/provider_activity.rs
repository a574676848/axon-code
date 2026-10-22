use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use axon_protocol::wire::{WireProviderStatus, WireProviderStatusPhase, WireProviderTransport};

const TRANSIENT_STATUS_LINGER: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderActivityTone {
    Active,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderActivityView {
    pub(crate) label: String,
    pub(crate) prefix: &'static str,
    pub(crate) tone: ProviderActivityTone,
    pub(crate) animated: bool,
    pub(crate) show_turn_elapsed: bool,
}

#[derive(Debug, Clone)]
struct ProviderActivityEntry {
    view: ProviderActivityView,
    phase: WireProviderStatusPhase,
    visible_for: Option<Duration>,
    visible_until: Option<Instant>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProviderActivityState {
    active: Option<ProviderActivityEntry>,
    queued: VecDeque<ProviderActivityEntry>,
    response_started: bool,
}

impl ProviderActivityState {
    /// 提交后立即投影连接阶段：连接中先于生成态可见。
    pub(crate) fn begin_connecting(&mut self, provider: &str, model: &str, now: Instant) {
        let mut entry = ProviderActivityEntry::connecting(provider, model);
        entry.activate(now);
        self.active = Some(entry);
        self.queued.clear();
        self.response_started = false;
    }

    pub(crate) fn push(&mut self, status: WireProviderStatus, now: Instant) {
        if self.response_started
            && matches!(
                status.phase,
                WireProviderStatusPhase::Connecting | WireProviderStatusPhase::Connected
            )
            && !self.recovery_status_pending()
        {
            return;
        }
        if status.phase == WireProviderStatusPhase::Connecting
            && self
                .active
                .as_ref()
                .is_some_and(|entry| entry.phase == WireProviderStatusPhase::Connecting)
        {
            let mut entry = ProviderActivityEntry::from_status(status);
            entry.activate(now);
            self.active = Some(entry);
            return;
        }
        let generating = status.phase == WireProviderStatusPhase::Generating;
        let mut entry = ProviderActivityEntry::from_status(status);
        if generating {
            self.response_started = true;
            entry.activate(now);
            self.active = Some(entry);
            self.queued.clear();
            return;
        }
        let Some(active) = self.active.as_mut() else {
            entry.activate(now);
            self.active = Some(entry);
            return;
        };
        active.linger_from(now);
        if let Some(previous) = self.queued.back_mut()
            && previous.visible_for.is_none()
        {
            previous.visible_for = Some(TRANSIENT_STATUS_LINGER);
        }
        self.queued.push_back(entry);
    }

    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        let expired = self
            .active
            .as_ref()
            .and_then(|entry| entry.visible_until)
            .is_some_and(|deadline| now >= deadline);
        if !expired {
            return false;
        }
        self.active = self.queued.pop_front().map(|mut entry| {
            entry.activate(now);
            entry
        });
        true
    }

    pub(crate) fn current(&self) -> Option<&ProviderActivityView> {
        self.active.as_ref().map(|entry| &entry.view)
    }

    pub(crate) fn clear(&mut self) {
        self.active = None;
        self.queued.clear();
        self.response_started = false;
    }

    fn recovery_status_pending(&self) -> bool {
        self.active_is_recovery_status()
            || self.queued.iter().any(|entry| {
                matches!(
                    entry.phase,
                    WireProviderStatusPhase::Failed
                        | WireProviderStatusPhase::Failover
                        | WireProviderStatusPhase::Retrying
                )
            })
    }

    fn active_is_recovery_status(&self) -> bool {
        self.active.as_ref().is_some_and(|entry| {
            matches!(
                entry.phase,
                WireProviderStatusPhase::Failed
                    | WireProviderStatusPhase::Failover
                    | WireProviderStatusPhase::Retrying
            )
        })
    }
}

impl ProviderActivityEntry {
    fn from_status(status: WireProviderStatus) -> Self {
        let phase = status.phase;
        let transient = matches!(
            phase,
            WireProviderStatusPhase::Connected
                | WireProviderStatusPhase::Failed
                | WireProviderStatusPhase::Failover
        );
        Self {
            view: provider_activity_view(status),
            phase,
            visible_for: transient.then_some(TRANSIENT_STATUS_LINGER),
            visible_until: None,
        }
    }

    fn connecting(provider: &str, model: &str) -> Self {
        let route = provider_model_route(provider, model);
        Self {
            // 真实传输方式只由 wire Connecting 状态给出，本地占位不臆造传输标签。
            view: active_view(format!("{route} 正在连接")),
            phase: WireProviderStatusPhase::Connecting,
            visible_for: None,
            visible_until: None,
        }
    }

    fn activate(&mut self, now: Instant) {
        self.visible_until = self.visible_for.map(|duration| now + duration);
    }

    fn linger_from(&mut self, now: Instant) {
        if self.visible_until.is_none() {
            self.visible_for = Some(TRANSIENT_STATUS_LINGER);
            self.visible_until = Some(now + TRANSIENT_STATUS_LINGER);
        }
    }
}

fn provider_activity_view(status: WireProviderStatus) -> ProviderActivityView {
    let route = provider_model_route(&status.provider, &status.model);
    match status.phase {
        WireProviderStatusPhase::Connecting => active_view(format!(
            "{route} 正在连接（{}）",
            transport_label(status.transport)
        )),
        WireProviderStatusPhase::Connected => success_view(format!("{route} 连接成功")),
        WireProviderStatusPhase::Generating => generating_view(route),
        WireProviderStatusPhase::Retrying => active_view(format!(
            "{route} 正在重试（{} · 第 {} 次）",
            transport_label(status.transport),
            status.attempt_index
        )),
        WireProviderStatusPhase::Failed => error_view(format!(
            "{route} 请求失败{}{}",
            status
                .http_status
                .map(|code| format!("（HTTP {code}）"))
                .unwrap_or_default(),
            provider_error_detail(status.error_message.as_deref())
        )),
        WireProviderStatusPhase::Failover => active_view(format!(
            "{} → {} 正在故障转移",
            status.provider,
            status
                .next_provider
                .unwrap_or_else(|| "下一供应商".to_owned())
        )),
    }
}

fn active_view(label: String) -> ProviderActivityView {
    ProviderActivityView {
        label,
        prefix: "◌ ",
        tone: ProviderActivityTone::Active,
        animated: true,
        show_turn_elapsed: true,
    }
}

fn error_view(label: String) -> ProviderActivityView {
    ProviderActivityView {
        label,
        prefix: "! ",
        tone: ProviderActivityTone::Error,
        animated: false,
        show_turn_elapsed: true,
    }
}

fn success_view(label: String) -> ProviderActivityView {
    ProviderActivityView {
        label,
        prefix: "✓ ",
        tone: ProviderActivityTone::Success,
        animated: false,
        show_turn_elapsed: true,
    }
}

fn generating_view(route: String) -> ProviderActivityView {
    ProviderActivityView {
        label: format!("{route} 生成回复中"),
        prefix: "◌ ",
        tone: ProviderActivityTone::Active,
        animated: true,
        show_turn_elapsed: true,
    }
}

fn provider_error_detail(error_message: Option<&str>) -> String {
    let message = error_message
        .map(str::split_whitespace)
        .map(Iterator::collect::<Vec<_>>)
        .map(|parts| parts.join(" "))
        .unwrap_or_default();
    if message.is_empty() {
        String::new()
    } else {
        format!("：{message}")
    }
}

fn provider_model_route(provider: &str, model: &str) -> String {
    match (provider.trim(), model.trim()) {
        ("", "") => "供应商".to_owned(),
        ("", model) => model.to_owned(),
        (provider, "") => provider.to_owned(),
        (provider, model) => format!("{provider}/{model}"),
    }
}

fn transport_label(transport: WireProviderTransport) -> &'static str {
    match transport {
        WireProviderTransport::Http => "HTTP",
        WireProviderTransport::WebSocket => "WS",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_only_appears_while_connecting_or_retrying() {
        let connecting = status(
            WireProviderStatusPhase::Connecting,
            WireProviderTransport::WebSocket,
        );
        let connected = status(
            WireProviderStatusPhase::Connected,
            WireProviderTransport::WebSocket,
        );
        let generating = status(
            WireProviderStatusPhase::Generating,
            WireProviderTransport::WebSocket,
        );

        assert!(
            provider_activity_view(connecting)
                .label
                .ends_with("正在连接（WS）")
        );
        assert_eq!(provider_activity_view(connected).label, "p/m 连接成功");
        assert_eq!(provider_activity_view(generating).label, "p/m 生成回复中");
    }

    #[test]
    fn failure_includes_http_status_and_single_line_provider_error() {
        let mut failed = status(WireProviderStatusPhase::Failed, WireProviderTransport::Http);
        failed.error_message = Some("upstream\n  overloaded".to_owned());

        assert_eq!(
            provider_activity_view(failed).label,
            "p/m 请求失败（HTTP 502）：upstream overloaded"
        );
    }

    #[test]
    fn begin_connecting_projects_route_before_wire_status() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.begin_connecting("p", "m", now);

        let current = state.current().expect("提交后必须立即展示连接活动");
        assert_eq!(current.label, "p/m 正在连接");
        assert_eq!(current.tone, ProviderActivityTone::Active);
        assert!(current.show_turn_elapsed);
        assert!(state.queued.is_empty());
    }

    #[test]
    fn wire_connecting_refreshes_local_placeholder_without_queueing() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.begin_connecting("p", "m", now);

        state.push(
            status(
                WireProviderStatusPhase::Connecting,
                WireProviderTransport::WebSocket,
            ),
            now + Duration::from_secs(1),
        );

        let current = state.current().expect("真实连接态应更新本地占位");
        assert_eq!(current.label, "p/m 正在连接（WS）");
        assert!(state.queued.is_empty());
    }

    #[test]
    fn connected_updates_activity_after_connecting_is_projected() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.begin_connecting("p", "m", now);
        state.push(
            status(
                WireProviderStatusPhase::Connected,
                WireProviderTransport::Http,
            ),
            now,
        );

        assert_eq!(
            state.current().expect("连接中应保持可见").label,
            "p/m 正在连接"
        );
        assert!(state.advance(now + TRANSIENT_STATUS_LINGER));
        let connected = state.current().expect("连接成功后应更新活动状态");
        assert_eq!(connected.label, "p/m 连接成功");
        assert_eq!(connected.tone, ProviderActivityTone::Success);
        assert!(!connected.animated);
    }

    #[test]
    fn generating_replaces_connection_status_without_transient_latency_text() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.begin_connecting("p", "m", now);
        state.push(
            status(
                WireProviderStatusPhase::Generating,
                WireProviderTransport::Http,
            ),
            now + Duration::from_secs(9),
        );

        let current = state.current().expect("Generating 应立即成为活动状态");
        assert_eq!(current.label, "p/m 生成回复中");
        assert!(current.show_turn_elapsed);
        assert!(state.queued.is_empty());
    }

    #[test]
    fn connection_status_does_not_replace_generating_after_response_started() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.push(
            status(
                WireProviderStatusPhase::Generating,
                WireProviderTransport::Http,
            ),
            now,
        );

        state.push(
            status(
                WireProviderStatusPhase::Connecting,
                WireProviderTransport::WebSocket,
            ),
            now + Duration::from_secs(1),
        );
        state.push(
            status(
                WireProviderStatusPhase::Connected,
                WireProviderTransport::WebSocket,
            ),
            now + Duration::from_secs(1),
        );

        let current = state.current().expect("生成态应继续保持为活动状态");
        assert_eq!(current.label, "p/m 生成回复中");
        assert!(state.queued.is_empty());
    }

    #[test]
    fn recovery_connection_status_remains_visible_after_generation() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.push(
            status(
                WireProviderStatusPhase::Generating,
                WireProviderTransport::Http,
            ),
            now,
        );
        state.push(
            status(WireProviderStatusPhase::Failed, WireProviderTransport::Http),
            now,
        );
        state.push(
            status(
                WireProviderStatusPhase::Failover,
                WireProviderTransport::Http,
            ),
            now,
        );
        state.push(
            status(
                WireProviderStatusPhase::Connecting,
                WireProviderTransport::WebSocket,
            ),
            now,
        );

        assert!(state.advance(now + TRANSIENT_STATUS_LINGER));
        assert!(
            state
                .current()
                .is_some_and(|view| view.label.contains("请求失败"))
        );
        assert!(state.advance(now + TRANSIENT_STATUS_LINGER * 2));
        assert!(
            state
                .current()
                .is_some_and(|view| view.label.contains("故障转移"))
        );
        assert!(state.advance(now + TRANSIENT_STATUS_LINGER * 3));
        assert!(
            state
                .current()
                .is_some_and(|view| view.label.ends_with("正在连接（WS）"))
        );
    }

    #[test]
    fn transient_failure_and_failover_are_shown_before_next_connection() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.push(
            status(WireProviderStatusPhase::Failed, WireProviderTransport::Http),
            now,
        );
        let mut failover = status(
            WireProviderStatusPhase::Failover,
            WireProviderTransport::Http,
        );
        failover.next_provider = Some("p-next".to_owned());
        state.push(failover, now);
        state.push(
            status(
                WireProviderStatusPhase::Connecting,
                WireProviderTransport::WebSocket,
            ),
            now,
        );

        assert!(
            state
                .current()
                .is_some_and(|view| view.label.contains("请求失败"))
        );
        assert!(state.advance(now + TRANSIENT_STATUS_LINGER));
        assert!(
            state
                .current()
                .is_some_and(|view| view.label.contains("故障转移"))
        );
        assert!(state.advance(now + TRANSIENT_STATUS_LINGER * 2));
        assert!(
            state
                .current()
                .is_some_and(|view| view.label.ends_with("正在连接（WS）"))
        );
    }

    #[test]
    fn generating_label_is_stable_across_activity_ticks() {
        let now = Instant::now();
        let mut state = ProviderActivityState::default();
        state.push(
            status(
                WireProviderStatusPhase::Connecting,
                WireProviderTransport::Http,
            ),
            now,
        );
        state.push(
            status(
                WireProviderStatusPhase::Connected,
                WireProviderTransport::Http,
            ),
            now,
        );
        state.push(
            status(
                WireProviderStatusPhase::Generating,
                WireProviderTransport::Http,
            ),
            now,
        );

        assert_eq!(
            state.current().expect("生成态应存在").label,
            "p/m 生成回复中"
        );
        assert!(!state.advance(now + Duration::from_secs(4)));
        assert_eq!(
            state.current().expect("生成态应存在").label,
            "p/m 生成回复中"
        );
    }

    fn status(
        phase: WireProviderStatusPhase,
        transport: WireProviderTransport,
    ) -> WireProviderStatus {
        WireProviderStatus {
            provider: "p".to_owned(),
            model: "m".to_owned(),
            phase,
            transport,
            attempt_index: 2,
            http_status: Some(502),
            error_message: None,
            next_provider: None,
            next_model: None,
        }
    }
}
