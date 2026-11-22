//! 历史帧信封的契约：读不到归属时必须说清「为什么」，不能只给空正文。
//!
//! 两种事实在同一条路由上无法从帧数分辨：
//!
//! - 这一轮确实没有过程（没有可读帧，也没有别的归属来源）；
//! - 过程在事件库里，但归属无法唯一确定（派发 join 多义）。
//!
//! 后者此前在信封里只剩 `events: []`，界面只能把它渲染成「这一轮没跑过」。这里钉住
//! 透出的原因与可读提示。

use std::path::PathBuf;

use axon_domain::{
    AGENT_DISPATCH_SOURCE_CHANNEL, AGENT_DISPATCH_SOURCE_PREFIX, AudienceContext, AudienceKind,
    DaemonSessionId, DisclosurePolicy, InteractionId, InteractionRecord, InteractionSource,
    ReplyRoute, ScopeRef,
};
use axon_session::{InteractionStore as _, RuntimeStore as _, SqliteRuntimeStore};

use super::interaction_events::interaction_events_payload;

/// 一次信封取数的隔离现场。
struct Fixture {
    _directory: axon_test_support::TestTempDir,
    /// 运行态根目录：数据落在其 `data` 子目录。
    root: PathBuf,
    store: SqliteRuntimeStore,
    session_id: String,
}

impl Fixture {
    fn new() -> Self {
        let directory = axon_test_support::test_temp_dir();
        let root = directory.path().to_path_buf();
        let store = SqliteRuntimeStore::from_root(root.join(axon_config::AXON_RUNTIME_DATA_DIR));
        store.initialize().expect("初始化运行态库");
        let session_id = axon_session::new_session_id().as_str().to_owned();
        store
            .create_session(
                &DaemonSessionId::new(session_id.clone()).expect("合法会话标识"),
                "/workspace",
                10,
            )
            .expect("登记会话");
        Self {
            _directory: directory,
            root,
            store,
            session_id,
        }
    }

    /// 登记一条轮次并返回它的标识。
    fn interaction(&self, channel: &str, source_message_id: &str, received_at_ms: u64) -> String {
        let interaction_id = format!("interaction-{}", received_at_ms);
        let record = InteractionRecord::new(
            DaemonSessionId::new(self.session_id.clone()).expect("合法会话标识"),
            InteractionId::new(interaction_id.clone()).expect("合法交互标识"),
            InteractionSource::new(channel, source_message_id).expect("来源"),
            ScopeRef::workspace("workspace-lane-h").expect("作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            ReplyRoute::new(channel, "terminal-fixture").expect("回复路由"),
            "把无归属原因透出到信封",
            received_at_ms,
        )
        .expect("交互记录");
        self.store.append(&record).expect("交互入库");
        interaction_id
    }
}

/// 取一次历史帧页并解析信封正文。
fn envelope(fixture: &Fixture, interaction_id: &str) -> serde_json::Value {
    let response = interaction_events_payload(&fixture.root, interaction_id, 0, 50, false);
    assert_eq!(response.status, 200, "历史帧页应正常返回");
    serde_json::from_slice(&response.body).expect("信封正文应为 JSON")
}

#[test]
fn interaction_history_page_exposes_unattributed_reason() {
    let fixture = Fixture::new();
    // 同一条子会话上有两条派发轮次：归属多义，读侧不得猜测。
    let ambiguous = fixture.interaction(
        AGENT_DISPATCH_SOURCE_CHANNEL,
        &format!("{AGENT_DISPATCH_SOURCE_PREFIX}dispatch-a"),
        30,
    );
    fixture.interaction(
        AGENT_DISPATCH_SOURCE_CHANNEL,
        &format!("{AGENT_DISPATCH_SOURCE_PREFIX}dispatch-b"),
        31,
    );
    let body = envelope(&fixture, &ambiguous);

    assert!(
        body["events"].as_array().is_some_and(Vec::is_empty),
        "多义归属读不出帧，正文只能是空数组：{body}"
    );
    let reason = body["unattributedReason"]
        .as_str()
        .unwrap_or_else(|| panic!("必须透出无归属原因：{body}"));
    assert!(reason.contains("没有归属"), "{reason}");
    assert!(reason.contains("派发轮次"), "{reason}");
    let notice = body["unattributedNotice"]
        .as_str()
        .unwrap_or_else(|| panic!("必须给出可读中文提示：{body}"));
    assert!(notice.starts_with(reason), "提示必须原样带上原因：{notice}");
    assert!(
        notice.contains("不等于这一轮没有过程"),
        "必须说明空正文的含义：{notice}"
    );
    // 协议既有字段不受影响。
    assert_eq!(body["hasMore"], false);
    assert_eq!(body["skippedArchivedFrames"], 0);
    assert_eq!(body["limit"], 50);

    // 对照：普通轮次没有可读帧时，如实表示「没有归属问题」，不得伪造原因。
    let ordinary = fixture.interaction("tui", "message-ordinary", 32);
    let ordinary_body = envelope(&fixture, &ordinary);
    assert_eq!(ordinary_body["unattributedReason"], serde_json::Value::Null);
    assert_eq!(ordinary_body["unattributedNotice"], serde_json::Value::Null);
}
