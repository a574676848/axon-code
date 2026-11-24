//! 车道查询服务。
//!
//! 把端口返回的原始汇总整理成用户可见的车道：解析 Workspace 与回复路由、
//! 派生中文可读标题、按最近活动排序。业务口径集中在这里，表现层只做展示。

use axon_domain::{AudienceContext, ReplyRoute, ScopeRef};
use serde::{Deserialize, Serialize};

use crate::ports::{InteractionStoreError, SessionQueryStore, SessionSummaryRow};

/// 车道标题的最大字符数。
const SESSION_TITLE_MAX_CHARS: usize = 32;

/// 车道标题兜底文案。
const SESSION_TITLE_FALLBACK: &str = "未命名车道";

/// 用户可见的一条车道。
///
/// 实现 `Serialize` 是为了让网关等 adapter 直接把它作为协议载荷，
/// 避免每个 adapter 各自再抄一份同名字段而导致口径漂移。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// 车道稳定标识。
    pub session_id: String,
    /// 所属 Workspace 标识；未绑定时为空。
    pub workspace_id: String,
    /// 会话表面渠道。
    pub channel: String,
    /// 会话表面目标。
    pub destination: String,
    /// 受众边界标签。
    pub audience: String,
    /// 派生出的车道标题。
    pub title: String,
    /// 该车道下的 Interaction 总数。
    pub interaction_count: u64,
    /// 该车道当前是否有一条 Interaction 正在处理中。
    pub running: bool,
    /// 最早活动时间（毫秒）。
    pub created_at_ms: u64,
    /// 最近活动时间（毫秒）。
    pub updated_at_ms: u64,
}

/// 车道查询服务。
pub struct SessionQueryService<S> {
    store: S,
}

impl<S: SessionQueryStore> SessionQueryService<S> {
    /// 以给定存储构造服务。
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// 列出全部车道。
    pub fn list_sessions(&self) -> Result<Vec<SessionSnapshot>, InteractionStoreError> {
        let rows = self.store.list_session_summaries()?;
        Ok(rows.iter().map(project_session).collect())
    }
}

/// 把一行原始汇总投影成用户可见的车道。
fn project_session(row: &SessionSummaryRow) -> SessionSnapshot {
    let workspace_id = serde_json::from_str::<ScopeRef>(&row.scope_json)
        .ok()
        .and_then(|scope| scope.workspace_id().map(|value| value.as_str().to_string()))
        .unwrap_or_default();

    let route = serde_json::from_str::<ReplyRoute>(&row.reply_route_json).ok();
    let audience = serde_json::from_str::<AudienceContext>(&row.audience_json).ok();

    SessionSnapshot {
        session_id: row.session_id.clone(),
        workspace_id,
        channel: route.as_ref().map_or_else(
            || row.latest_channel.clone(),
            |value| value.channel().to_string(),
        ),
        destination: route
            .as_ref()
            .map_or_else(String::new, |value| value.destination().to_string()),
        audience: audience.as_ref().map_or_else(String::new, audience_label),
        title: preferred_session_title(&row.stored_title, &row.latest_content),
        interaction_count: row.interaction_count,
        running: row.processing_count > 0,
        created_at_ms: row.first_received_ms,
        updated_at_ms: row.last_updated_ms,
    }
}

/// 受众边界的展示标签。
fn audience_label(audience: &AudienceContext) -> String {
    match audience.kind() {
        axon_domain::AudienceKind::Private => "私密",
        axon_domain::AudienceKind::Group => "群组",
    }
    .to_string()
}

/// 车道标题：优先用控制面已生成的标题，缺失时才从消息正文派生。
///
/// 生成端（轮后任务）与读取端共用 `sessions.title` 这一列，三端因此看到同一个标题；
/// 正文派生只作为「标题还没生成」时的占位，不再与生成结果抢同一份事实。
fn preferred_session_title(stored: &str, latest_content: &str) -> String {
    let stored_title = clip_title_text(stored);
    if !stored_title.is_empty() {
        return stored_title;
    }
    session_title(latest_content)
}

/// 压掉多余空白并截断到上限；空文本返回空串。
fn clip_title_text(source: &str) -> String {
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized
        .chars()
        .take(SESSION_TITLE_MAX_CHARS)
        .collect::<String>()
}

/// 从最近一条 Interaction 正文派生车道标题。
///
/// 取第一条非空行，按中文句读切出第一个短句，并限制长度；
/// 与运行时会话命名保持同一套可读性口径。
fn session_title(content: &str) -> String {
    let source = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(SESSION_TITLE_FALLBACK);
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let content = normalized
        .trim_start_matches(['#', '>', '-', '*', '+'])
        .trim();
    let clause = content
        .split(['。', '！', '？', '!', '?'])
        .next()
        .unwrap_or(content)
        .trim();
    let title = clause
        .chars()
        .take(SESSION_TITLE_MAX_CHARS)
        .collect::<String>();
    if title.is_empty() {
        SESSION_TITLE_FALLBACK.to_string()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionQueryService, preferred_session_title, session_title};
    use crate::ports::{InteractionStoreError, SessionQueryStore, SessionSummaryRow};
    use axon_domain::{
        AudienceContext, AudienceKind, DisclosurePolicy, ReplyRoute, ScopeRef, WorkspaceId,
    };

    /// 固定测试数据的车道存储。
    struct FixedStore(Vec<SessionSummaryRow>);

    impl SessionQueryStore for FixedStore {
        fn list_session_summaries(&self) -> Result<Vec<SessionSummaryRow>, InteractionStoreError> {
            Ok(self.0.clone())
        }
    }

    fn row(content: &str) -> SessionSummaryRow {
        // 上下文列全部由领域类型自身序列化生成，避免手写 JSON 与领域契约漂移。
        let scope = ScopeRef::Workspace(WorkspaceId::new("workspace-a").unwrap());
        let audience = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
        let route = ReplyRoute::new("tui", "terminal-1").unwrap();
        SessionSummaryRow {
            session_id: "session-session-test".to_string(),
            interaction_count: 3,
            first_received_ms: 100,
            last_updated_ms: 900,
            processing_count: 0,
            latest_content: content.to_string(),
            stored_title: String::new(),
            latest_channel: "tui".to_string(),
            scope_json: serde_json::to_string(&scope).unwrap(),
            audience_json: serde_json::to_string(&audience).unwrap(),
            reply_route_json: serde_json::to_string(&route).unwrap(),
        }
    }

    #[test]
    fn title_takes_first_clause_and_bounds_length() {
        assert_eq!(
            session_title("  ## 修复 Provider 延迟。后续说明"),
            "修复 Provider 延迟"
        );
        assert_eq!(session_title("\n\n"), "未命名车道");
        let long = session_title(&"超长标题".repeat(20));
        assert_eq!(long.chars().count(), 32);
    }

    #[test]
    fn generated_title_wins_over_content_derivation() {
        // 生成标题存在时，读取端不得再拿最近一条消息覆盖它。
        assert_eq!(
            preferred_session_title("排查 Provider 超时", "只回复两个字：收到"),
            "排查 Provider 超时"
        );
        // 标题尚未生成（或只有空白）时才回落到正文派生。
        assert_eq!(
            preferred_session_title("   ", "只回复两个字：收到"),
            "只回复两个字：收到"
        );
    }

    #[test]
    fn service_projects_session_identity_and_counts() {
        let service = SessionQueryService::new(FixedStore(vec![row("设计工作台侧栏交互")]));
        let sessions = service.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        let session = &sessions[0];
        assert_eq!(session.title, "设计工作台侧栏交互");
        assert_eq!(session.interaction_count, 3);
        assert!(!session.running, "没有在途 Interaction 时不得标记为运行中");
        assert_eq!(session.workspace_id, "workspace-a");
        assert_eq!(session.channel, "tui");
        assert_eq!(session.destination, "terminal-1");
        assert_eq!(session.audience, "私密");
    }
}
