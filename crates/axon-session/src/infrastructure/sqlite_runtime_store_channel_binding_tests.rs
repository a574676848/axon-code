//! `channel_bindings` 与入站路由的回归用例。

use crate::infrastructure::SqliteRuntimeStore;
use crate::ports::{
    ChannelBindingStore as _, InteractionQueryStore as _, InteractionStore as _, RuntimeStore as _,
};
use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};

fn store(name: &str) -> (std::path::PathBuf, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join(format!("{name}.sqlite3"));
    // 临时目录随作用域销毁，这里保留目录句柄避免提前删除。
    std::mem::forget(directory);
    let store = SqliteRuntimeStore::new(path.clone());
    store.initialize().expect("schema 应可初始化");
    (path, store)
}

fn session_id(value: &str) -> DaemonSessionId {
    DaemonSessionId::new(value).expect("合法会话 ID")
}

fn feishu_interaction(message_id: &str) -> InteractionRecord {
    InteractionRecord::new(
        InteractionId::new(format!("interaction-{message_id}")).expect("合法交互 ID"),
        InteractionSource::new("feishu", format!("chat-a:{message_id}")).expect("合法来源"),
        ScopeRef::workspace("workspace-a").expect("合法作用域"),
        AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace),
        ReplyRoute::new("feishu", "chat-a").expect("合法路由"),
        "检查构建",
        42,
    )
    .expect("合法交互")
}

#[test]
fn bind_then_resolve_round_trips_and_overwrites() {
    let (_path, store) = store("channel-binding-round-trip");
    store
        .bind_channel("feishu", "chat-a", &session_id("session-bound"), 100)
        .expect("绑定应成功");
    assert_eq!(
        store
            .resolve_channel_binding("feishu", "chat-a")
            .expect("解析应成功"),
        Some(session_id("session-bound"))
    );
    assert_eq!(
        store
            .resolve_channel_binding("feishu", "chat-b")
            .expect("解析应成功"),
        None,
        "未绑定的目的地必须回落到路由派生"
    );

    // 同一 (channel, destination) 再绑一次是改写，不是新增一行。
    store
        .bind_channel("feishu", "chat-a", &session_id("session-next"), 200)
        .expect("改写应成功");
    assert_eq!(
        store
            .resolve_channel_binding("feishu", "chat-a")
            .expect("解析应成功"),
        Some(session_id("session-next"))
    );
    let bindings = store
        .list_channel_bindings(&session_id("session-next"))
        .expect("列举应成功");
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].destination, "chat-a");
    assert_eq!(bindings[0].updated_at_ms, 200);
}

#[test]
fn unbind_removes_only_the_named_pair() {
    let (_path, store) = store("channel-binding-unbind");
    store
        .bind_channel("feishu", "chat-a", &session_id("session-1"), 100)
        .expect("绑定应成功");
    store
        .bind_channel("feishu", "chat-b", &session_id("session-1"), 100)
        .expect("绑定应成功");

    assert!(
        store
            .unbind_channel("feishu", "chat-a")
            .expect("解绑应成功")
    );
    assert_eq!(
        store
            .resolve_channel_binding("feishu", "chat-b")
            .expect("解析应成功"),
        Some(session_id("session-1")),
        "解绑一条不得影响另一条"
    );
    assert!(
        !store
            .unbind_channel("feishu", "chat-a")
            .expect("重复解绑应成功"),
        "已经不存在时返回未删除"
    );
}

#[test]
fn blank_channel_or_destination_is_rejected() {
    let (_path, store) = store("channel-binding-blank");
    let error = store
        .bind_channel("  ", "chat-a", &session_id("session-1"), 100)
        .expect_err("空渠道必须拒绝");
    assert!(error.to_string().contains("channel"));
}

#[test]
fn bound_channel_routes_inbound_message_into_the_target_session() {
    let (_path, store) = store("channel-binding-routing");
    store
        .bind_channel("feishu", "chat-a", &session_id("session-bound"), 100)
        .expect("绑定应成功");
    store
        .create_session(&session_id("session-bound"), "/workspace", 100)
        .expect("目标会话应可创建");

    store
        .append(&feishu_interaction("m-1"))
        .expect("入站事实应可写入");
    let record = store
        .get_interaction(&InteractionId::new("interaction-m-1").expect("合法交互 ID"))
        .expect("读取应成功")
        .expect("该交互必须存在");
    assert_eq!(
        record.runtime_session_id.as_deref(),
        Some("session-bound"),
        "绑定命中时消息必须进入指定会话"
    );

    // 解绑后同一条路由回落到按路由派生的会话身份。
    store
        .unbind_channel("feishu", "chat-a")
        .expect("解绑应成功");
    store
        .append(&feishu_interaction("m-2"))
        .expect("入站事实应可写入");
    let derived = store
        .get_interaction(&InteractionId::new("interaction-m-2").expect("合法交互 ID"))
        .expect("读取应成功")
        .expect("该交互必须存在");
    let derived_session = derived.runtime_session_id.expect("派生会话必须回读");
    assert_ne!(derived_session, "session-bound", "解绑后不得继续投进旧绑定");
}
