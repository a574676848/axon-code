//! 索引页的会话能力契约：三种会话类型都必须在同一页给出同形的 `session` 块。
//!
//! 断言的是契约本身（字段齐备、能力位取值、来源映射），不是某一次渲染的结果：
//! 前端删掉按渠道推断之后，唯一的依据就是这一块。

use std::path::PathBuf;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::{InteractionStore as _, RuntimeStore as _, SqliteRuntimeStore};

use super::session_interactions_payload;

struct Fixture {
    root: PathBuf,
    store: SqliteRuntimeStore,
    session_id: String,
    interaction_id: String,
}

impl Fixture {
    fn new(kind: &str, channel: &str, client: Option<&str>) -> Self {
        let root = axon_test_support::test_temp_root()
            .join(format!("session-capability-{}", uuid::Uuid::new_v4()));
        let data_dir = root.join(axon_config::AXON_RUNTIME_DATA_DIR);
        std::fs::create_dir_all(&data_dir).expect("创建运行态目录");
        let store = SqliteRuntimeStore::from_root(&data_dir);
        store.initialize().expect("初始化运行态库");
        let session_id = axon_session::new_session_id().as_str().to_owned();
        store
            .create_session(
                &DaemonSessionId::new(session_id.clone()).expect("合法会话标识"),
                ".",
                1,
            )
            .expect("登记会话行");
        // 会话类型与最近模型事实：生产链路由各自的登记入口写入，这里直接落列。
        store
            .with_read_connection(|connection| {
                connection.execute(
                    "UPDATE sessions SET kind=?2, title=?3, provider=?4, model=?5, model_name=?6 \
                     WHERE session_id=?1",
                    rusqlite::params![
                        session_id,
                        kind,
                        "目标执行：补完整交互索引",
                        "fixture-provider",
                        "fixture-model",
                        "fixture-alias",
                    ],
                )
            })
            .expect("写入会话事实");
        let interaction_id = format!("interaction-{}", uuid::Uuid::new_v4());
        let mut source =
            InteractionSource::new(channel, format!("msg-{}", uuid::Uuid::new_v4())).expect("来源");
        if let Some(client) = client {
            source = source.with_client(client).expect("客户端");
        }
        let record = InteractionRecord::new(
            DaemonSessionId::new(session_id.clone()).expect("合法会话标识"),
            InteractionId::new(interaction_id.clone()).expect("合法交互标识"),
            source,
            ScopeRef::workspace(format!("ws-{}", uuid::Uuid::new_v4())).expect("作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            ReplyRoute::new(channel, "terminal-fixture").expect("回复路由"),
            "把交互索引补成同形",
            7,
        )
        .expect("交互记录");
        store.append(&record).expect("交互入库");
        Self {
            root,
            store,
            session_id,
            interaction_id,
        }
    }

    fn payload(&self) -> serde_json::Value {
        session_interactions_payload(&self.root, &self.session_id, 10, None, false)
            .expect("索引载荷")
    }
}

#[test]
fn primary_session_capability_block_is_writable_and_answerable() {
    let fixture = Fixture::new("primary", "desktop", None);
    let payload = fixture.payload();
    let session = &payload["session"];
    assert_eq!(session["kind"], "primary");
    assert_eq!(session["origin"], "human");
    assert_eq!(session["writable"], true);
    assert_eq!(session["canAnswerQuestions"], true);
    assert_eq!(session["canInterrupt"], true, "主会话可被中断");
    assert_eq!(session["title"], "目标执行：补完整交互索引");
    assert_eq!(session["provider"], "fixture-provider");
    assert_eq!(session["model"], "fixture-model");
    assert_eq!(session["modelName"], "fixture-alias");
    assert_eq!(session["interactionCount"], 1);
    assert_eq!(session["latestInteractionId"], fixture.interaction_id);
}

#[test]
fn goal_session_capability_block_is_read_only() {
    let fixture = Fixture::new("goal", "agent", Some("goal"));
    let session = &fixture.payload()["session"];
    assert_eq!(session["kind"], "goal");
    assert_eq!(session["origin"], "goal");
    assert_eq!(session["writable"], false);
    assert_eq!(session["canAnswerQuestions"], false);
    assert_eq!(session["interactionCount"], 1);
}

#[test]
fn subagent_session_capability_block_is_read_only() {
    let fixture = Fixture::new("subagent", "agent", None);
    let session = &fixture.payload()["session"];
    assert_eq!(session["kind"], "subagent");
    assert_eq!(session["origin"], "subagent");
    assert_eq!(session["writable"], false);
    assert_eq!(
        session["canInterrupt"], false,
        "子会话没有独立中断入口，必须如实给 false"
    );
    assert_eq!(session["canAnswerQuestions"], false);
}

#[test]
fn capability_block_contract_keeps_every_field() {
    let fixture = Fixture::new("primary", "desktop", None);
    let session = fixture.payload()["session"].clone();
    let object = session.as_object().expect("session 必须是对象");
    let mut fields: Vec<&str> = object.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        vec![
            "canAnswerQuestions",
            "canInterrupt",
            "interactionCount",
            "kind",
            "latestInteractionId",
            "model",
            "modelName",
            "origin",
            "provider",
            "title",
            "writable",
        ]
    );
}

#[test]
fn missing_session_row_yields_null_block_and_keeps_index_readable() {
    let fixture = Fixture::new("primary", "desktop", None);
    let orphan = axon_session::new_session_id().as_str().to_owned();
    let payload = session_interactions_payload(&fixture.root, &orphan, 10, None, false)
        .expect("未登记会话的索引页仍必须可读");
    assert!(payload["session"].is_null());
    assert_eq!(payload["interactions"].as_array().map(Vec::len), Some(0));
    let _ = &fixture.store;
}
