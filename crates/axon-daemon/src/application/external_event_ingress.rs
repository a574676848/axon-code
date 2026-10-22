//! 外部 Watch/Event 适配器的统一 durable ingress。

use super::interaction_ingress::{
    ActiveTurnSteerSink, InteractionIngressReceipt, PrimaryTurnIngress,
};
use axon_crypto::sha256_hex;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{InteractionStore, InteractionStoreError};
use thiserror::Error;

/// 默认允许的外部事件观测时钟偏差。
pub const DEFAULT_EXTERNAL_EVENT_MAX_CLOCK_SKEW_MS: u64 = 5 * 60 * 1_000;
/// 外部事件标识的最大字符数。
pub const MAX_EXTERNAL_EVENT_ID_LENGTH: usize = 256;
/// 单条外部事件允许携带的附件数量上限。
pub const MAX_EXTERNAL_EVENT_ATTACHMENTS: usize = 8;
/// 外部事件简报标题的最大字符数。
pub const MAX_EXTERNAL_EVENT_TITLE_LENGTH: usize = 512;
/// 外部事件业务关联键的最大字符数。
pub const MAX_EXTERNAL_EVENT_CORRELATION_KEY_LENGTH: usize = 256;
const EXTERNAL_EVENT_SOURCE_PREFIX: &str = "external:";
const EXTERNAL_EVENT_INTERACTION_PREFIX: &str = "external-interaction-";

/// 外部 Watch/Event 适配器输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEventEnvelope {
    /// 外部来源渠道，例如 webhook provider 名称。
    pub source_channel: String,
    /// 外部系统提供的稳定事件标识。
    pub event_id: String,
    /// 事件所属 Workspace。
    pub workspace_id: String,
    /// 适配器观测事件的时间。
    pub observed_at_ms: u64,
    /// 事件失效时间。
    pub expires_at_ms: u64,
    /// 交给主 turn 的有界文本内容。
    pub content: String,
    /// 事件简报标题；`None` 表示正文保持原始内容。
    pub title: Option<String>,
    /// 业务关联键；`Some` 时该事件参与 Gateway 滑动窗口防抖。
    pub correlation_key: Option<String>,
    /// 回复渠道，必须与来源渠道一致。
    pub reply_channel: String,
    /// 回复目标。
    pub reply_destination: String,
    /// 渠道入站阶段落地的本地附件绝对路径。
    ///
    /// 图片成为 vision 输入，文件成为可读附件；空表示纯文本事件。
    pub attachments: Vec<String>,
}

/// 外部事件 ingress 错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExternalEventIngressError {
    /// 输入字段为空或格式无效。
    #[error("外部事件字段无效：{0}")]
    InvalidInput(String),
    /// 事件作用域与适配器绑定的 Workspace 不一致。
    #[error("外部事件 Workspace scope 不匹配")]
    WorkspaceMismatch,
    /// 观测时间超出允许的时钟偏差。
    #[error("外部事件观测时间超出允许时钟偏差")]
    ClockSkew,
    /// 事件已经过期。
    #[error("外部事件已过期")]
    Expired,
    /// Interaction durable store 失败。
    #[error(transparent)]
    Store(#[from] InteractionStoreError),
}

/// 外部事件统一 durable ingress。
#[derive(Debug, Clone)]
pub struct ExternalEventIngress<S, K> {
    adapter_id: String,
    workspace_id: Option<String>,
    max_clock_skew_ms: u64,
    now_ms: u64,
    ingress: PrimaryTurnIngress<S, K>,
}

impl<S, K> ExternalEventIngress<S, K>
where
    S: InteractionStore,
    K: ActiveTurnSteerSink,
{
    /// 创建绑定单一 Workspace 的外部事件适配器。
    pub fn new(
        store: S,
        steer_sink: K,
        adapter_id: impl Into<String>,
        workspace_id: impl Into<String>,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Self, ExternalEventIngressError> {
        Self::new_with_clock_skew(
            store,
            steer_sink,
            adapter_id,
            workspace_id,
            now_ms,
            lease_duration_ms,
            DEFAULT_EXTERNAL_EVENT_MAX_CLOCK_SKEW_MS,
        )
    }

    /// 创建可配置时钟偏差的外部事件适配器，便于不同 provider 的时钟策略。
    pub fn new_with_clock_skew(
        store: S,
        steer_sink: K,
        adapter_id: impl Into<String>,
        workspace_id: impl Into<String>,
        now_ms: u64,
        lease_duration_ms: u64,
        max_clock_skew_ms: u64,
    ) -> Result<Self, ExternalEventIngressError> {
        let workspace_id = non_empty(workspace_id.into(), "workspace_id")?;
        Self::build(
            store,
            steer_sink,
            adapter_id,
            Some(workspace_id),
            now_ms,
            lease_duration_ms,
            max_clock_skew_ms,
        )
    }

    /// 创建不绑定 Workspace 的外部事件适配器。
    ///
    /// 适用于运行在 Home policy 上的 watcher：每个事件的 Workspace 由策略
    /// 根据当前工作地点解析后写入 envelope，适配器本身不缓存项目 Workspace。
    pub fn new_unbound(
        store: S,
        steer_sink: K,
        adapter_id: impl Into<String>,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Self, ExternalEventIngressError> {
        Self::new_unbound_with_clock_skew(
            store,
            steer_sink,
            adapter_id,
            now_ms,
            lease_duration_ms,
            DEFAULT_EXTERNAL_EVENT_MAX_CLOCK_SKEW_MS,
        )
    }

    /// 创建可配置时钟偏差且不绑定 Workspace 的适配器。
    pub fn new_unbound_with_clock_skew(
        store: S,
        steer_sink: K,
        adapter_id: impl Into<String>,
        now_ms: u64,
        lease_duration_ms: u64,
        max_clock_skew_ms: u64,
    ) -> Result<Self, ExternalEventIngressError> {
        Self::build(
            store,
            steer_sink,
            adapter_id,
            None,
            now_ms,
            lease_duration_ms,
            max_clock_skew_ms,
        )
    }

    fn build(
        store: S,
        steer_sink: K,
        adapter_id: impl Into<String>,
        workspace_id: Option<String>,
        now_ms: u64,
        lease_duration_ms: u64,
        max_clock_skew_ms: u64,
    ) -> Result<Self, ExternalEventIngressError> {
        let adapter_id = non_empty(adapter_id.into(), "adapter_id")?;
        if now_ms == 0 {
            return Err(ExternalEventIngressError::InvalidInput(
                "now_ms 必须大于 0".to_owned(),
            ));
        }
        if lease_duration_ms == 0 {
            return Err(ExternalEventIngressError::InvalidInput(
                "lease_duration_ms 必须大于 0".to_owned(),
            ));
        }
        Ok(Self {
            adapter_id,
            workspace_id,
            max_clock_skew_ms,
            now_ms,
            ingress: PrimaryTurnIngress::new(store, steer_sink, now_ms, lease_duration_ms),
        })
    }

    /// 校验并提交一个外部事件。
    pub fn submit(
        &self,
        event: ExternalEventEnvelope,
    ) -> Result<InteractionIngressReceipt, ExternalEventIngressError> {
        self.validate(&event)?;
        let interaction = self.interaction(&event)?;
        self.ingress.route_input(&interaction).map_err(Into::into)
    }

    fn validate(&self, event: &ExternalEventEnvelope) -> Result<(), ExternalEventIngressError> {
        let event_workspace_id = non_empty(event.workspace_id.clone(), "workspace_id")?;
        if self
            .workspace_id
            .as_deref()
            .is_some_and(|workspace_id| workspace_id != event_workspace_id)
        {
            return Err(ExternalEventIngressError::WorkspaceMismatch);
        }
        non_empty(event.source_channel.clone(), "source_channel")?;
        non_empty(event.event_id.clone(), "event_id")?;
        non_empty(event.content.clone(), "content")?;
        bounded_field(&event.event_id, "event_id", MAX_EXTERNAL_EVENT_ID_LENGTH)?;
        if let Some(title) = event.title.as_deref() {
            bounded_field(title, "title", MAX_EXTERNAL_EVENT_TITLE_LENGTH)?;
        }
        if let Some(correlation_key) = event.correlation_key.as_deref() {
            bounded_field(
                correlation_key,
                "correlation_key",
                MAX_EXTERNAL_EVENT_CORRELATION_KEY_LENGTH,
            )?;
        }
        validate_attachments(&event.attachments)?;
        non_empty(event.reply_channel.clone(), "reply_channel")?;
        non_empty(event.reply_destination.clone(), "reply_destination")?;
        if event.reply_channel != event.source_channel {
            return Err(ExternalEventIngressError::InvalidInput(
                "reply_channel 必须与 source_channel 一致".to_owned(),
            ));
        }
        if event.expires_at_ms <= self.now_ms || event.expires_at_ms <= event.observed_at_ms {
            return Err(ExternalEventIngressError::Expired);
        }
        let distance = event.observed_at_ms.abs_diff(self.now_ms);
        if distance > self.max_clock_skew_ms {
            return Err(ExternalEventIngressError::ClockSkew);
        }
        Ok(())
    }

    fn interaction(
        &self,
        event: &ExternalEventEnvelope,
    ) -> Result<InteractionRecord, ExternalEventIngressError> {
        let workspace_id = event.workspace_id.trim();
        let scope = ScopeRef::workspace(workspace_id.to_owned())
            .map_err(|error| ExternalEventIngressError::InvalidInput(error.to_string()))?;
        let source_key = format!(
            "{EXTERNAL_EVENT_SOURCE_PREFIX}{}:{}:{}",
            self.adapter_id, workspace_id, event.event_id
        );
        let digest = sha256_hex(source_key.as_bytes());
        let source = InteractionSource::new(
            event.source_channel.clone(),
            format!("{EXTERNAL_EVENT_SOURCE_PREFIX}{digest}"),
        )
        .map_err(|error| ExternalEventIngressError::InvalidInput(error.to_string()))?;
        let interaction_id =
            InteractionId::new(format!("{EXTERNAL_EVENT_INTERACTION_PREFIX}{digest}"))
                .map_err(|error| ExternalEventIngressError::InvalidInput(error.to_string()))?;
        let route = ReplyRoute::new(event.reply_channel.clone(), event.reply_destination.clone())
            .map_err(|error| ExternalEventIngressError::InvalidInput(error.to_string()))?;
        let mut record = InteractionRecord::new(
            interaction_id,
            source,
            scope,
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            route,
            event.content.clone(),
            event.observed_at_ms,
        )
        .map_err(|error| ExternalEventIngressError::InvalidInput(error.to_string()))?;
        // 附件只描述输入素材，原样带到 Provider turn 的 transportContext.files。
        record.attachments = event.attachments.clone();
        Ok(record)
    }
}

fn non_empty(value: String, field: &str) -> Result<String, ExternalEventIngressError> {
    if value.trim().is_empty() {
        return Err(ExternalEventIngressError::InvalidInput(format!(
            "{field} 不能为空"
        )));
    }
    Ok(value)
}

fn bounded_field(
    value: &str,
    field: &str,
    max_chars: usize,
) -> Result<(), ExternalEventIngressError> {
    if value.chars().count() > max_chars {
        return Err(ExternalEventIngressError::InvalidInput(format!(
            "{field} 不能超过 {max_chars} 个字符"
        )));
    }
    Ok(())
}

/// 校验附件清单：数量有界，路径必须指向真实存在的文件。
fn validate_attachments(attachments: &[String]) -> Result<(), ExternalEventIngressError> {
    if attachments.len() > MAX_EXTERNAL_EVENT_ATTACHMENTS {
        return Err(ExternalEventIngressError::InvalidInput(format!(
            "单条事件最多携带 {MAX_EXTERNAL_EVENT_ATTACHMENTS} 个附件"
        )));
    }
    for path in attachments {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(ExternalEventIngressError::InvalidInput(
                "附件路径不能为空".to_owned(),
            ));
        }
        if !std::path::Path::new(trimmed).is_file() {
            return Err(ExternalEventIngressError::InvalidInput(format!(
                "附件不存在或不是文件：{trimmed}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::InteractionStatus;
    use axon_session::{InteractionQueryStore, InteractionStore, SqliteRuntimeStore};

    const NOW_MS: u64 = 10_000;
    const LEASE_MS: u64 = 1_000;

    fn event(workspace_id: &str) -> ExternalEventEnvelope {
        ExternalEventEnvelope {
            source_channel: "github".to_owned(),
            event_id: "push-123".to_owned(),
            workspace_id: workspace_id.to_owned(),
            observed_at_ms: NOW_MS,
            expires_at_ms: NOW_MS + 60_000,
            content: "仓库发生新的提交".to_owned(),
            title: None,
            correlation_key: None,
            reply_channel: "github".to_owned(),
            reply_destination: "issue-42".to_owned(),
            attachments: Vec::new(),
        }
    }

    fn ingress(
        store: SqliteRuntimeStore,
    ) -> ExternalEventIngress<
        SqliteRuntimeStore,
        super::super::interaction_ingress::NoActiveTurnSteerSink,
    > {
        ExternalEventIngress::new(
            store,
            super::super::interaction_ingress::NoActiveTurnSteerSink,
            "github-webhook",
            "workspace-a",
            NOW_MS,
            LEASE_MS,
        )
        .expect("适配器")
    }

    #[test]
    fn external_event_is_durable_and_deduplicated_by_event_id() {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let adapter = ingress(store.clone());

        let first = adapter.submit(event("workspace-a")).expect("首次提交");
        let duplicate = adapter.submit(event("workspace-a")).expect("重复提交");

        assert!(first.inserted);
        assert!(!duplicate.inserted);
        let leases = store
            .claim_processing(10, NOW_MS, LEASE_MS)
            .expect("读取 Interaction");
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].interaction.status, InteractionStatus::Processing);
        assert!(
            leases[0]
                .interaction
                .interaction_id
                .as_str()
                .starts_with(EXTERNAL_EVENT_INTERACTION_PREFIX)
        );
    }

    #[test]
    fn expired_event_is_rejected_before_durable_append() {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let adapter = ingress(store.clone());
        let mut expired = event("workspace-a");
        expired.expires_at_ms = NOW_MS;
        assert_eq!(
            adapter.submit(expired),
            Err(ExternalEventIngressError::Expired)
        );
        assert!(
            store
                .claim_processing(10, NOW_MS, LEASE_MS)
                .expect("读取 Interaction")
                .is_empty()
        );
    }

    #[test]
    fn workspace_scope_mismatch_is_fail_closed() {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let adapter = ingress(store.clone());
        assert_eq!(
            adapter.submit(event("workspace-b")),
            Err(ExternalEventIngressError::WorkspaceMismatch)
        );
        assert!(
            store
                .claim_processing(10, NOW_MS, LEASE_MS)
                .expect("读取 Interaction")
                .is_empty()
        );
    }

    #[test]
    fn stale_event_is_rejected_when_clock_skew_exceeds_policy() {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let adapter = ExternalEventIngress::new_with_clock_skew(
            store,
            super::super::interaction_ingress::NoActiveTurnSteerSink,
            "github-webhook",
            "workspace-a",
            NOW_MS,
            LEASE_MS,
            100,
        )
        .expect("适配器");
        let mut stale = event("workspace-a");
        stale.observed_at_ms = NOW_MS - 101;
        assert_eq!(
            adapter.submit(stale),
            Err(ExternalEventIngressError::ClockSkew)
        );
    }

    #[test]
    fn unbound_ingress_resolves_workspace_per_event() {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let ingress = ExternalEventIngress::new_unbound(
            store.clone(),
            super::super::interaction_ingress::NoActiveTurnSteerSink,
            "home-policy",
            NOW_MS,
            LEASE_MS,
        )
        .expect("创建不绑定 Workspace 的 ingress");
        ingress
            .submit(event("workspace-a"))
            .expect("提交第一个事件");
        ingress
            .submit(ExternalEventEnvelope {
                event_id: "push-456".to_owned(),
                workspace_id: "workspace-b".to_owned(),
                ..event("workspace-a")
            })
            .expect("提交第二个 Workspace 事件");
        assert!(
            store
                .latest_external_interaction("workspace-a")
                .expect("读取 Workspace A 事件")
                .is_some()
        );
        assert!(
            store
                .latest_external_interaction("workspace-b")
                .expect("读取 Workspace B 事件")
                .is_some()
        );
    }
}
