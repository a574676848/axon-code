use super::*;
use axon_domain::InteractionId;
use axon_domain::InteractionStatus;
use axon_session::{InteractionQueryStore, InteractionStore, SqliteRuntimeStore};

const NOW_MS: u64 = 10_000;
const LEASE_MS: u64 = 1_000;

/// 测试夹具：会话标识由调用方显式给出，这里只把可读串包成合法标识。
fn test_session_id(workspace_id: &str) -> DaemonSessionId {
    DaemonSessionId::new(format!("session-{workspace_id}")).expect("测试会话标识")
}

fn event(workspace_id: &str) -> ExternalEventEnvelope {
    ExternalEventEnvelope {
        client: None,
        source_channel: "github".to_owned(),
        event_id: "push-123".to_owned(),
        session_id: Some(test_session_id(workspace_id)),
        workspace_id: workspace_id.to_owned(),
        observed_at_ms: NOW_MS,
        expires_at_ms: NOW_MS + 60_000,
        content: "仓库发生新的提交".to_owned(),
        title: None,
        correlation_key: None,
        reply_channel: "github".to_owned(),
        reply_destination: "issue-42".to_owned(),
        attachments: Vec::new(),
        model: None,
        reasoning_effort: None,
    }
}

/// 模拟"存在活跃主回合"的 sink：append_turn_input 永远报告命中。
/// 用它证明外部事件不会被合并进活动轮，而是各自独立成轮。
struct AlwaysAppendSink;

impl ActiveTurnSteerSink for AlwaysAppendSink {
    fn try_steer(&self, _interaction: &InteractionRecord) -> Result<bool, String> {
        Ok(true)
    }

    fn append_turn_input(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<Option<InteractionId>, String> {
        Ok(Some(interaction.interaction_id.clone()))
    }
}

#[test]
fn external_events_stay_separate_turns_even_with_an_active_turn() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let adapter = ExternalEventIngress::new(
        store.clone(),
        AlwaysAppendSink,
        "github-webhook",
        "workspace-a",
        NOW_MS,
        LEASE_MS,
    )
    .expect("适配器");

    let first = adapter.submit(event("workspace-a")).expect("第一条");
    let mut second = event("workspace-a");
    second.event_id = "push-456".to_owned();
    let second = adapter.submit(second).expect("第二条");
    let duplicate = adapter.submit(event("workspace-a")).expect("重复第一条");

    assert!(first.inserted && second.inserted, "不同事件必须各自成轮");
    assert_ne!(
        first.interaction_id, second.interaction_id,
        "不同外部事件不得合并进同一轮"
    );
    assert!(
        !duplicate.inserted,
        "同一 message_id 必须被 durable 幂等挡住"
    );
    assert_eq!(
        duplicate.interaction_id, first.interaction_id,
        "重复投递必须指回首次落盘那一条"
    );
    assert!(
        store
            .get_interaction(&first.interaction_id)
            .expect("读取第一条")
            .is_some()
            && store
                .get_interaction(&second.interaction_id)
                .expect("读取第二条")
                .is_some(),
        "两条不同外部事件必须各自落一行 Interaction"
    );
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

    assert!(first.inserted, "首次提交必须新写入一行");
    assert!(!duplicate.inserted, "重放不得新写入第二行");
    // 重放拿到的必须是**同一条**交互：回执标识与落盘标识一致，客户端才不会各投一路。
    assert_eq!(
        first.interaction_id, duplicate.interaction_id,
        "重放回执必须指回首次落盘的那一条"
    );
    let leases = store
        .claim_processing(10, NOW_MS, LEASE_MS)
        .expect("读取 Interaction");
    assert_eq!(leases.len(), 1, "同一条外部事件只允许一行 Interaction");
    assert_eq!(leases[0].interaction.interaction_id, first.interaction_id);
    assert_eq!(leases[0].interaction.status, InteractionStatus::Processing);
    // 标识是服务端生成的 ULID：不带来源前缀，也不由事件内容派生。
    assert!(
        axon_session::is_ulid(leases[0].interaction.interaction_id.as_str()),
        "外部事件交互标识必须是 ULID：{}",
        leases[0].interaction.interaction_id.as_str()
    );
}

#[test]
fn the_same_event_id_on_another_channel_is_a_different_interaction() {
    // 幂等键是 (source_channel, source_message_id)：渠道不同就是另一条事件。
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let adapter = ingress(store.clone());

    let github = adapter.submit(event("workspace-a")).expect("github 事件");
    let mut jira = event("workspace-a");
    jira.source_channel = "jira".to_owned();
    jira.reply_channel = "jira".to_owned();
    let jira = adapter.submit(jira).expect("jira 事件");

    assert!(github.inserted && jira.inserted);
    assert_ne!(github.interaction_id, jira.interaction_id);
    assert_eq!(
        store
            .claim_processing(10, NOW_MS, LEASE_MS)
            .expect("读取 Interaction")
            .len(),
        2
    );
}

#[test]
fn webhook_event_without_explicit_client_is_attributed_to_the_channel() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let adapter = ingress(store.clone());
    let mut webhook = event("workspace-a");
    webhook.source_channel = "webhook".to_owned();
    webhook.reply_channel = "webhook".to_owned();

    adapter.submit(webhook).expect("提交 webhook 事件");

    let leases = store
        .claim_processing(10, NOW_MS, LEASE_MS)
        .expect("读取 Interaction");
    assert_eq!(leases[0].interaction.source.client(), Some("webhook"));
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
            client: None,
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

#[test]
fn explicit_turn_settings_are_carried_by_the_interaction_and_absent_values_stay_empty() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let adapter = ingress(store.clone());

    let mut explicit = event("workspace-a");
    explicit.event_id = "push-model".to_owned();
    explicit.model = Some("  balanced  ".to_owned());
    explicit.reasoning_effort = Some("  high  ".to_owned());
    let receipt = adapter.submit(explicit).expect("提交带显式设置的事件");
    let stored = store
        .get_interaction(&receipt.interaction_id)
        .expect("读取 Interaction")
        .expect("Interaction 必须已持久化");
    assert_eq!(stored.model.as_deref(), Some("balanced"));
    assert_eq!(stored.reasoning_effort.as_deref(), Some("high"));

    // 没有显式设置的事件必须保持 None：模型与强度由会话历史与配置决定。
    let plain = adapter.submit(event("workspace-a")).expect("提交普通事件");
    let plain_stored = store
        .get_interaction(&plain.interaction_id)
        .expect("读取 Interaction")
        .expect("Interaction 必须已持久化");
    assert_eq!(plain_stored.model, None);
    assert_eq!(plain_stored.reasoning_effort, None);
}

#[test]
fn blank_turn_settings_are_rejected_before_durable_append() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let adapter = ingress(store.clone());
    let mut blank_model = event("workspace-a");
    blank_model.model = Some("   ".to_owned());

    assert!(matches!(
        adapter.submit(blank_model),
        Err(ExternalEventIngressError::InvalidInput(message)) if message.contains("model")
    ));

    let mut blank_effort = event("workspace-a");
    blank_effort.event_id = "push-blank-effort".to_owned();
    blank_effort.reasoning_effort = Some("   ".to_owned());

    assert!(matches!(
        adapter.submit(blank_effort),
        Err(ExternalEventIngressError::InvalidInput(message))
            if message.contains("reasoning_effort")
    ));
}

/// 附件上限是入口契约的一部分：上限内必须受理，超出必须按分类拒绝并说清数量。
///
/// 桌面端一次贴 12 张截图就是真实用法，上限与「超限原因可见」缺一不可。
#[test]
fn attachment_count_is_bounded_at_the_published_limit() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let adapter = ingress(store);
    let files = write_attachments(directory.path(), MAX_EXTERNAL_EVENT_ATTACHMENTS);

    let mut within_limit = event("workspace-a");
    within_limit.attachments = files.clone();
    adapter
        .submit(within_limit)
        .expect("上限之内的附件必须受理");

    let mut over_limit = event("workspace-a");
    over_limit.event_id = "push-too-many".to_owned();
    let mut extra = files.clone();
    extra.push(files[0].clone());
    over_limit.attachments = extra;

    let error = adapter.submit(over_limit).expect_err("超出上限必须拒绝");
    assert_eq!(error.code(), "attachments_too_many");
    assert!(
        matches!(
            &error,
            ExternalEventIngressError::AttachmentsTooMany { count, limit }
                if *count == MAX_EXTERNAL_EVENT_ATTACHMENTS + 1
                    && *limit == MAX_EXTERNAL_EVENT_ATTACHMENTS
        ),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains(&MAX_EXTERNAL_EVENT_ATTACHMENTS.to_string()),
        "原因必须写明上限：{error}"
    );
}

#[test]
fn missing_attachment_is_rejected_with_its_own_code() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let adapter = ingress(store);
    let mut missing = event("workspace-a");
    missing.attachments = vec![
        directory
            .path()
            .join("not-here.png")
            .to_string_lossy()
            .to_string(),
    ];

    let error = adapter.submit(missing).expect_err("路径不存在必须拒绝");
    assert_eq!(error.code(), "attachment_unreadable");
    assert!(
        matches!(error, ExternalEventIngressError::AttachmentUnreadable(_)),
        "附件不可读必须与数量超限分开归类"
    );
}

/// 在临时目录里放 `count` 个附件文件，返回它们的绝对路径。
fn write_attachments(root: &std::path::Path, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let path = root.join(format!("shot-{index}.png"));
            std::fs::write(&path, b"png").expect("写附件");
            path.to_string_lossy().to_string()
        })
        .collect()
}
