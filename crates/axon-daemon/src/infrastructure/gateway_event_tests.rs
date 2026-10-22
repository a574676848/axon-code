//! Gateway 事件增强契约测试：正文归档、滑窗防抖与 SOP 引用。

use super::event_options::EVENT_SOP_BRIEF_MANDATE;
use super::*;
use crate::application::{EventStreamBridge, NoActiveTurnSteerSink};
use crate::infrastructure::event_scripts::{EVENT_PROVIDER_SOP_FILE_NAME, EventProviderRegistry};
use axon_agent_runtime::CONTENT_ARCHIVE_DIR_NAME;
use axon_domain::InteractionRecord;
use axon_session::{InteractionQueryStore, SqliteRuntimeStore};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const ACTION_LEASE_DURATION_MS: u64 = 120_000;
const BASE_NOW_MS: u64 = 1_760_000_000_000;
const EVENT_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Clone)]
struct TestClock {
    now_ms: Arc<AtomicU64>,
}

impl TestClock {
    fn new(now_ms: u64) -> Self {
        Self {
            now_ms: Arc::new(AtomicU64::new(now_ms)),
        }
    }

    fn advance(&self, delta_ms: u64) {
        self.now_ms.fetch_add(delta_ms, Ordering::AcqRel);
    }
}

impl RuntimeClock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Acquire)
    }
}

struct Fixture {
    data_root: PathBuf,
    providers: Arc<EventProviderRegistry>,
    clock: TestClock,
    wake_count: Arc<AtomicU64>,
    // 字段顺序即析构顺序：临时目录必须最后析构，先释放事件库与数据根下的 SQLite 句柄。
    _temp: axon_test_support::TestTempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = axon_test_support::test_temp_dir();
        let data_root = temp.path().join("data");
        fs::create_dir_all(&data_root).expect("创建运行数据目录");
        let providers = Arc::new(EventProviderRegistry::new(temp.path().join("events")));
        Self {
            data_root,
            providers,
            clock: TestClock::new(BASE_NOW_MS),
            wake_count: Arc::new(AtomicU64::new(0)),
            _temp: temp,
        }
    }

    fn events_root(&self) -> PathBuf {
        self.providers.root().to_path_buf()
    }

    fn bridge(&self) -> DaemonGatewayBridge<SqliteRuntimeStore, TestClock> {
        DaemonGatewayBridge::new_with_events(
            GatewayHostDependencies {
                store: self.store(),
                active_turn_steer_sink: Arc::new(NoActiveTurnSteerSink),
                clock: self.clock.clone(),
                action_lease_duration_ms: ACTION_LEASE_DURATION_MS,
                stream_bridge: Arc::new(EventStreamBridge::new()),
                host_generation: None,
            },
            GatewayEventOptions::new(Arc::clone(&self.providers), self.data_root.clone())
                .with_waker(self.waker()),
        )
    }

    fn waker(&self) -> Arc<dyn Fn() + Send + Sync> {
        let wake_count = Arc::clone(&self.wake_count);
        Arc::new(move || {
            wake_count.fetch_add(1, Ordering::AcqRel);
        })
    }

    /// 主 turn 唤醒次数：只统计真正落 durable 的新事件。
    fn wake_count(&self) -> u64 {
        self.wake_count.load(Ordering::Acquire)
    }

    fn store(&self) -> SqliteRuntimeStore {
        SqliteRuntimeStore::from_root(&self.data_root)
    }

    fn interaction(&self, receipt: &SubmitInteractionReceipt) -> InteractionRecord {
        self.store()
            .get_interaction(&receipt.interaction_id)
            .expect("读取 Interaction")
            .expect("Interaction 必须已持久化")
    }

    fn event(&self, event_id: &str, correlation_key: Option<&str>) -> ExternalEventEnvelope {
        ExternalEventEnvelope {
            source_channel: "jira".to_owned(),
            event_id: event_id.to_owned(),
            workspace_id: String::new(),
            observed_at_ms: self.clock.now_ms(),
            expires_at_ms: self.clock.now_ms() + EVENT_TTL_MS,
            content: "短正文".to_owned(),
            title: None,
            correlation_key: correlation_key.map(ToOwned::to_owned),
            reply_channel: "jira".to_owned(),
            reply_destination: "jira:AXON-1".to_owned(),
            attachments: Vec::new(),
        }
    }
}

fn archive_directory(fixture: &Fixture) -> PathBuf {
    fixture.data_root.join(CONTENT_ARCHIVE_DIR_NAME)
}

#[test]
fn oversized_content_is_archived_and_interaction_only_keeps_the_reference() {
    let fixture = Fixture::new();
    let bridge = fixture.bridge();
    let body = "长".repeat(EVENT_CONTENT_INLINE_LIMIT_CHARS + 1);
    let mut event = fixture.event("jira:AXON-1", None);
    event.content.clone_from(&body);
    let receipt = bridge
        .submit_external_event(event)
        .expect("超长正文事件必须被接收");
    let interaction = fixture.interaction(&receipt);
    assert!(interaction.content.contains("正文归档："));
    assert!(interaction.content.chars().count() < EVENT_CONTENT_INLINE_LIMIT_CHARS);
    let archived_path = interaction
        .content
        .lines()
        .find_map(|line| line.strip_prefix("正文归档："))
        .and_then(|value| value.split('（').next())
        .map(PathBuf::from)
        .expect("简报必须给出归档路径");
    assert!(archived_path.starts_with(archive_directory(&fixture)));
    assert_eq!(
        fs::read_to_string(&archived_path).expect("读取归档正文"),
        body
    );
    assert_eq!(
        fs::read_dir(archive_directory(&fixture))
            .expect("归档目录存在")
            .count(),
        1
    );
}

#[test]
fn burst_with_same_correlation_key_merges_into_one_interaction() {
    let fixture = Fixture::new();
    let bridge = fixture.bridge();
    let first = bridge
        .submit_external_event(fixture.event("jira:AXON-1#1", Some("jira:AXON-1:assigned")))
        .expect("首条事件");
    fixture.clock.advance(500);
    let second = bridge
        .submit_external_event(fixture.event("jira:AXON-1#2", Some("jira:AXON-1:assigned")))
        .expect("窗口内重复事件");
    assert!(second.deduplicated);
    assert_eq!(second.interaction_id, first.interaction_id);
    assert_eq!(
        fs::read_dir(archive_directory(&fixture))
            .map(|entries| entries.count())
            .unwrap_or_default(),
        0
    );
    fixture.clock.advance(4_000);
    let third = bridge
        .submit_external_event(fixture.event("jira:AXON-1#3", Some("jira:AXON-1:assigned")))
        .expect("窗口过期后的事件");
    assert!(!third.deduplicated);
    assert_ne!(third.interaction_id, first.interaction_id);
    let distinct = bridge
        .submit_external_event(fixture.event("jira:AXON-1#4", Some("jira:AXON-1:commented")))
        .expect("不同关联键的事件");
    assert_ne!(distinct.interaction_id, third.interaction_id);
}

#[test]
fn events_without_correlation_key_are_never_merged() {
    let fixture = Fixture::new();
    let bridge = fixture.bridge();
    let first = bridge
        .submit_external_event(fixture.event("jira:AXON-1#1", None))
        .expect("首条事件");
    let second = bridge
        .submit_external_event(fixture.event("jira:AXON-1#2", None))
        .expect("第二条事件");
    assert!(!second.deduplicated);
    assert_ne!(second.interaction_id, first.interaction_id);
}

#[test]
fn registered_provider_brief_carries_title_and_sop_mandate() {
    let fixture = Fixture::new();
    let provider_directory = fixture.events_root().join("jira");
    fs::create_dir_all(&provider_directory).expect("创建 provider 目录");
    fs::write(
        provider_directory.join(EVENT_PROVIDER_SOP_FILE_NAME),
        "# Jira SOP",
    )
    .expect("写入 SOP 文档");
    let bridge = fixture.bridge();
    let mut event = fixture.event("jira:AXON-1", Some("jira:AXON-1:assigned"));
    event.title = Some("缺陷单已指派".to_owned());
    let receipt = bridge
        .submit_external_event(event)
        .expect("提交 provider 事件");
    let interaction = fixture.interaction(&receipt);
    assert!(interaction.content.contains("标题：缺陷单已指派"));
    assert!(interaction.content.contains("正文：短正文"));
    assert!(interaction.content.contains(&format!(
        "流程文档：{}{EVENT_SOP_BRIEF_MANDATE}",
        provider_directory.join(EVENT_PROVIDER_SOP_FILE_NAME).display()
    )));
    assert!(interaction.content.contains("事件标识：jira:AXON-1"));
}

#[test]
fn short_event_without_title_keeps_content_verbatim() {
    let fixture = Fixture::new();
    let bridge = fixture.bridge();
    let receipt = bridge
        .submit_external_event(fixture.event("jira:AXON-1", None))
        .expect("提交事件");
    assert_eq!(fixture.interaction(&receipt).content, "短正文");
}

#[test]
fn durable_insert_wakes_primary_turn_exactly_once() {
    let fixture = Fixture::new();
    let bridge = fixture.bridge();
    bridge
        .submit_external_event(fixture.event("jira:AXON-1#1", Some("jira:AXON-1:assigned")))
        .expect("首条事件");
    assert_eq!(
        fixture.wake_count(),
        1,
        "新事件落 durable 后必须唤醒主 turn"
    );

    // 防抖吸收与重复事件不产生新的 durable 记录，不得重复唤醒。
    fixture.clock.advance(500);
    bridge
        .submit_external_event(fixture.event("jira:AXON-1#2", Some("jira:AXON-1:assigned")))
        .expect("窗口内重复事件");
    assert_eq!(fixture.wake_count(), 1, "被吸收的事件不得重复唤醒");

    // 窗口过期后的新事件仍是新 durable 记录，必须再次唤醒。
    fixture.clock.advance(4_000);
    bridge
        .submit_external_event(fixture.event("jira:AXON-1#3", Some("jira:AXON-1:assigned")))
        .expect("窗口过期后的事件");
    assert_eq!(fixture.wake_count(), 2, "窗口过期后的新事件必须再次唤醒");
}

#[test]
fn gateway_stream_receives_runtime_observation_without_cursor() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session_id = axon_domain::DaemonSessionId::new("session-gw-obs").expect("Session ID");
    let interaction_id = InteractionId::new("interaction-gw-obs").expect("Interaction ID");
    let stream_bridge = Arc::new(EventStreamBridge::new());
    let subscription = stream_bridge
        .subscribe_with_capacity(session_id.clone(), interaction_id.clone(), None, 4)
        .expect("订阅实时流");
    let mut stream = DaemonGatewayEventStream {
        store,
        stream_bridge,
        session_id: session_id.clone(),
        interaction_id: Some(interaction_id.clone()),
        replay_page_size: 2,
        host_generation: None,
        subscription,
        pending: VecDeque::new(),
        replay_cursor: None,
    };

    let frame = axon_domain::RuntimeObservationFrame::new(
        1_000,
        session_id.as_str(),
        interaction_id.as_str(),
    )
    .with_lsp(axon_domain::LspMetricsSnapshot {
        active_clients: 2,
        pending_requests: 0,
        diagnostics_count: 0,
        last_latency_ms: 0,
    })
    .with_gateway(axon_domain::GatewayMetrics {
        active_turn_count: 4,
    });
    stream
        .stream_bridge
        .publish_observation(&session_id, &frame);

    let event = stream
        .recv_timeout(Duration::from_millis(50))
        .expect("接收不应报错")
        .expect("应收到观测事件");
    assert_eq!(event.event_type, "runtime_observation");
    assert!(event.cursor.is_none(), "非持久观测帧不得携带游标");
    let parsed: serde_json::Value = serde_json::from_str(&event.payload_json).expect("有效 JSON");
    assert_eq!(parsed["lsp"]["activeClients"], 2);
    assert_eq!(parsed["gateway"]["activeTurnCount"], 4);
}
