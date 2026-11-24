//! Thread 事件正文外置的单元测试：写入、还原、校验、回收。

use super::*;
use crate::domain::{ThreadEvent, ThreadSettings, ThreadSource};

/// 每条用例独占一个临时根，避免正文目录互相污染。
fn temp_root(label: &str) -> std::path::PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-thread-body-{label}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

/// 一段明显超过内联阈值的正文：用来证明「超阈值就会外置」。
fn oversized_event() -> ThreadEvent {
    ThreadEvent::ThreadNamed {
        name: Some("注".repeat(THREAD_EVENT_INLINE_BODY_LIMIT_BYTES)),
    }
}

fn inline_event() -> ThreadEvent {
    ThreadEvent::ThreadNamed {
        name: Some("小正文".to_owned()),
    }
}

#[test]
fn oversized_body_roundtrips_through_the_external_file() {
    let root = temp_root("roundtrip");
    let thread_id = "thread-body-roundtrip";
    let event = oversized_event();
    let encoded = serde_json::to_vec(&event).expect("事件应可序列化");
    assert!(exceeds_inline_limit(encoded.len()), "夹具必须越过内联阈值");

    let reference = externalize(&root, thread_id, 7, &event).expect("正文应可外置");

    assert_eq!(
        reference.bytes,
        u64::try_from(encoded.len()).unwrap_or(u64::MAX)
    );
    assert_eq!(reference.digest.len(), 64, "摘要应为 SHA-256 十六进制");
    assert!(
        reference.path.ends_with("/00000000000000000007.json.zst"),
        "引用按序号命名：{}",
        reference.path
    );
    let body = body_root(&root).join(&reference.path);
    assert!(body.is_file(), "正文文件必须落盘：{}", body.display());

    let restored = resolve(&root, thread_id, 7, &reference).expect("正文应可还原");
    assert_eq!(restored, event, "还原结果必须与写入事件一致");
}

#[test]
fn inline_sized_events_are_not_forced_through_the_binary_path() {
    let root = temp_root("inline");
    let event = inline_event();
    let encoded = serde_json::to_vec(&event).expect("事件应可序列化");
    assert!(
        !exceeds_inline_limit(encoded.len()),
        "小事件不该被判定为需要外置"
    );
    // 小事件仍然可以走外置通道（调用方按需），但不会被阈值强制。
    let reference = externalize(&root, "thread-inline", 1, &event).expect("外置通道本身可用");
    let restored = resolve(&root, "thread-inline", 1, &reference).expect("正文应可还原");
    assert_eq!(restored, event);
}

#[test]
fn digest_mismatch_is_reported_instead_of_returning_wrong_history() {
    let root = temp_root("digest");
    let thread_id = "thread-body-digest";
    let mut reference = externalize(&root, thread_id, 2, &oversized_event()).expect("正文应可外置");
    reference.digest = "0".repeat(64);

    let error = resolve(&root, thread_id, 2, &reference).expect_err("摘要不符必须报错");
    assert!(error.contains("摘要不符"), "错误要说清是校验失败：{error}");
}

#[test]
fn missing_body_reports_the_missing_file() {
    let root = temp_root("missing");
    let thread_id = "thread-body-missing";
    let reference = externalize(&root, thread_id, 3, &oversized_event()).expect("正文应可外置");
    let body = body_root(&root).join(&reference.path);
    std::fs::remove_file(&body).expect("删除正文");

    let error = resolve(&root, thread_id, 3, &reference).expect_err("正文缺失必须报错");
    assert!(error.contains("正文缺失"), "错误要指出缺失：{error}");
}

#[test]
fn reference_outside_the_thread_body_directory_is_rejected() {
    let root = temp_root("traversal");
    let thread_id = "thread-body-traversal";
    let mut reference = externalize(&root, thread_id, 4, &oversized_event()).expect("正文应可外置");
    reference.path = "../../secrets.json.zst".to_owned();

    let error = resolve(&root, thread_id, 4, &reference).expect_err("路径穿越必须被拒绝");
    assert!(error.contains("引用不合法"), "错误要指出引用非法：{error}");
}

#[test]
fn pruning_keeps_only_the_checkpoint_body() {
    let root = temp_root("prune");
    let thread_id = "thread-body-prune";
    let first = externalize(&root, thread_id, 1, &oversized_event()).expect("正文应可外置");
    let second = externalize(&root, thread_id, 2, &oversized_event()).expect("正文应可外置");
    let directory = body_root(&root);

    prune_bodies_except(&root, thread_id, 2).expect("清理应成功");

    assert!(
        !directory.join(&first.path).exists(),
        "被替换事件的正文必须删除"
    );
    assert!(
        directory.join(&second.path).exists(),
        "checkpoint 自己的正文必须保留"
    );
}

#[test]
fn removing_a_thread_clears_its_body_directory() {
    let root = temp_root("remove");
    let thread_id = "thread-body-remove";
    let reference = externalize(&root, thread_id, 5, &oversized_event()).expect("正文应可外置");

    remove_bodies(&root, thread_id).expect("删除应成功");

    assert!(
        !body_root(&root).join(&reference.path).exists(),
        "thread 删除后正文不得残留"
    );
    // 幂等：目录已不存在时删除同样成功。
    remove_bodies(&root, thread_id).expect("重复删除应幂等");
}

#[test]
fn externalized_events_keep_a_lossless_wire_shape() {
    // 引用必须是纯数据：只描述正文位置与校验信息，不携带正文本身。
    let reference = ThreadEventBodyRef {
        bytes: 42,
        digest: "a".repeat(64),
        path: "thread/00000000000000000001.json.zst".to_owned(),
    };
    let json = reference.to_json();
    assert_eq!(
        json.get("type").and_then(serde_json::Value::as_str),
        Some(THREAD_EVENT_BODY_ARCHIVED_TYPE)
    );
    assert_eq!(ThreadEventBodyRef::from_json(&json), Some(reference));
    assert_eq!(
        ThreadEventBodyRef::from_json(&serde_json::json!({"type": "threadNamed"})),
        None
    );
}

#[test]
fn thread_settings_fixture_stays_serializable() {
    // 事件构造依赖 settings 形状；这条用例把夹具钉住，避免字段漂移悄悄改变正文体量。
    let settings = ThreadSettings {
        cwd: ".".to_owned(),
        provider: "local".to_owned(),
        model: "model".to_owned(),
        selected_skill_name: None,
    };
    let event = ThreadEvent::ThreadStarted {
        session_root_id: "thread-body-settings".to_owned(),
        parent_thread_id: None,
        forked_from_thread_id: None,
        source: ThreadSource::CliRun,
        ephemeral: false,
        settings,
    };
    let encoded = serde_json::to_vec(&event).expect("事件应可序列化");
    assert!(!exceeds_inline_limit(encoded.len()));
}
