//! 会话事件流订阅的契约测试。
//!
//! 与 `stream.rs` 分文件存放：订阅实现本身已接近单文件行数硬限，而测试与实现属于两件事，
//! 新增用例不再挤占实现的篇幅。

use super::*;

#[test]
fn stream_error_codes_map_to_actionable_chinese() {
    assert!(describe_stream_error("subscription_rejected").contains("工作区"));
    assert!(describe_stream_error("subscribe_missing_id").contains("版本"));
    assert!(describe_stream_error("subscribe_duplicate_id").contains("标识"));
    assert!(describe_stream_error("gateway_busy").contains("重试"));
    assert!(describe_stream_error("unknown_code").contains("unknown_code"));
}

#[test]
fn handshake_errors_classify_status_codes() {
    let unauthorized = tokio_tungstenite::tungstenite::Error::Http(Box::new(
        tokio_tungstenite::tungstenite::http::Response::builder()
            .status(401)
            .body(None)
            .expect("构造 401 响应"),
    ));
    assert!(describe_handshake_error(&unauthorized).contains("凭据"));

    let missing = tokio_tungstenite::tungstenite::Error::Http(Box::new(
        tokio_tungstenite::tungstenite::http::Response::builder()
            .status(404)
            .body(None)
            .expect("构造 404 响应"),
    ));
    assert!(describe_handshake_error(&missing).contains("版本"));
}

#[test]
fn subscribe_ids_are_derived_from_the_target_and_stay_distinct() {
    let interaction_target = SubscribeTarget::Interaction {
        workspace_id: "workspace-a".to_owned(),
        interaction_id: "interaction-1".to_owned(),
        resume_cursor: None,
    };
    let turns_target = SubscribeTarget::SessionTurns {
        workspace_id: "workspace-a".to_owned(),
        session_id: "parent--subagent--root".to_owned(),
    };
    assert_eq!(
        interaction_target.subscribe_id(),
        "interaction:interaction-1"
    );
    assert_eq!(
        turns_target.subscribe_id(),
        "session-turns:parent--subagent--root"
    );
    assert_ne!(
        interaction_target.subscribe_id(),
        turns_target.subscribe_id(),
        "正文流与轮次头必须落在不同的订阅标识上，否则一条连接上会互相顶掉"
    );
}

#[test]
fn turns_subscribe_frame_carries_the_id_and_no_interaction() {
    let frame = SubscribeTarget::SessionTurns {
        workspace_id: "workspace-a".to_owned(),
        session_id: "parent--subagent--root".to_owned(),
    }
    .into_frame("session-turns:parent--subagent--root".to_owned());
    let value = serde_json::to_value(frame).expect("订阅帧应可序列化");
    assert_eq!(value["type"], "subscribe");
    assert_eq!(value["subscribeId"], "session-turns:parent--subagent--root");
    assert!(
        value.get("sessionId").is_none(),
        "轮次头目标只由订阅标识承载，不重复会话字段"
    );
    assert!(
        value.get("interactionId").is_none(),
        "轮次头订阅不得带 Interaction 占位"
    );
    assert!(value.get("resumeCursor").is_none(), "轮次头没有历史可续订");
}

#[test]
fn published_events_reach_every_live_subscriber_and_drop_dead_ones() {
    let stream = ConversationStream::new();
    let first = stream.subscribe_events();
    let second = stream.subscribe_events();
    drop(second);
    stream.publish(StreamEvent::Subscribed {
        subscribe_id: "interaction:interaction-1".to_owned(),
        baseline: None,
        replay_truncated: false,
        resume_cursor: None,
    });
    assert_eq!(
        first.recv().expect("首个订阅者应收到事件"),
        StreamEvent::Subscribed {
            subscribe_id: "interaction:interaction-1".to_owned(),
            baseline: None,
            replay_truncated: false,
            resume_cursor: None,
        }
    );
    assert_eq!(
        stream.subscribers().lock().expect("订阅表").len(),
        1,
        "已关闭的订阅者必须被剔除，避免无界堆积"
    );
}

#[test]
fn subscription_receipt_forwards_truncation_and_resume_cursor() {
    // 网关回执里的截断事实与续读游标必须原样出现在前端事件上：宿主层吞掉任何一项，
    // 界面就只能把残缺回放当完整历史渲染，或从 0 重放一遍。
    let stream = ConversationStream::new();
    let receiver = stream.subscribe_events();
    let receipt = serde_json::json!({
        "type": "subscribed",
        "subscribeId": "interaction:i1",
        "replayTruncated": true,
        "resumeCursor": "{\"generation\":\"g\",\"sequence\":6230}",
    })
    .to_string();
    assert!(forward_frame(&stream, "interaction:i1", &receipt));
    match receiver.recv().expect("应收到订阅确认") {
        StreamEvent::Subscribed {
            replay_truncated,
            resume_cursor,
            ..
        } => {
            assert!(replay_truncated, "截断事实必须透出");
            assert_eq!(
                resume_cursor.as_deref(),
                Some(r#"{"generation":"g","sequence":6230}"#),
                "续读游标必须是服务端给的真实位置"
            );
        }
        other => panic!("应为订阅确认事件，实际 {other:?}"),
    }
}

#[test]
fn frames_are_routed_by_the_echoed_subscribe_id() {
    let stream = ConversationStream::new();
    let receiver = stream.subscribe_events();
    assert!(forward_frame(
        &stream,
        "interaction:interaction-1",
        r#"{"type":"event","subscribeId":"session:parent--subagent--root","event_type":"wire_event","payload":{},"cursor":"c1"}"#,
    ));
    match receiver.recv().expect("应收到帧") {
        StreamEvent::Frame { subscribe_id, .. } => assert_eq!(
            subscribe_id, "session:parent--subagent--root",
            "同一条连接上的帧必须按服务端回显的标识归位，而不是按发起连接的订阅"
        ),
        other => panic!("应为事件帧，实际 {other:?}"),
    }
}

#[test]
fn replay_marker_is_preserved_when_forwarding_frames() {
    let stream = ConversationStream::new();
    let receiver = stream.subscribe_events();
    for replay in [true, false] {
        let text = serde_json::json!({
            "type": "event", "subscribeId": "interaction:a",
            "event_type": "turn_completed", "payload": {}, "replay": replay
        })
        .to_string();
        assert!(forward_frame(&stream, "interaction:a", &text));
        let StreamEvent::Frame {
            replay: forwarded, ..
        } = receiver.recv().expect("应收到帧")
        else {
            panic!("应为事件帧");
        };
        assert_eq!(forwarded, replay, "转发不得丢失历史与实时的区别");
    }
}

#[test]
fn unknown_subscription_is_not_tracked_twice() {
    let stream = ConversationStream::new();
    let target = SubscribeTarget::Interaction {
        workspace_id: "workspace-a".to_owned(),
        interaction_id: "interaction-1".to_owned(),
        resume_cursor: None,
    };
    assert_eq!(
        stream.active_subscriptions(),
        0,
        "初始状态下不应有任何活跃订阅"
    );
    stream.unwatch(&target);
    stream.unsubscribe(&target.subscribe_id());
    assert_eq!(
        stream.active_subscriptions(),
        0,
        "取消一个不存在的订阅必须是幂等的，不得报错或留下残余条目"
    );
}

#[test]
fn watch_spawns_its_task_without_an_ambient_runtime() {
    // 普通测试线程没有环境运行时：`watch` 必须把任务落到订阅器自有的运行时上，
    // 否则一订阅就 panic（早先版本就是这样崩的）。任务收尾会自摘，故只断言上界。
    let stream = ConversationStream::new();
    let session = SubscribeTarget::Interaction {
        workspace_id: "workspace-a".to_owned(),
        interaction_id: "interaction-1".to_owned(),
        resume_cursor: None,
    };
    stream.watch(session.clone());
    assert!(stream.active_subscriptions() <= 1, "一次订阅最多登记一格");
    stream.unwatch(&session);
    assert_eq!(stream.active_subscriptions(), 0, "取消后必须清空");
}

#[test]
fn watch_is_idempotent_and_retire_only_releases_its_own_slot() {
    // 已在册时重复订阅必须幂等挡下；自摘只摘自己那一格，不得连带摘掉并存的别路。
    let stream = ConversationStream::new();
    let runtime = stream.runtime().expect("订阅器运行时");
    let registered = runtime.spawn(async {});
    let registered_id = registered.id();
    let mut tasks = stream.tasks().lock().expect("登记表");
    tasks.insert("interaction:a".to_owned(), registered);
    tasks.insert("session:b".to_owned(), runtime.spawn(async {}));
    drop(tasks);
    stream.watch(SubscribeTarget::Interaction {
        workspace_id: "workspace-a".to_owned(),
        interaction_id: "a".to_owned(),
        resume_cursor: None,
    });
    let kept = {
        let tasks = stream.tasks().lock().expect("登记表");
        tasks.get("interaction:a").map(|task| task.id())
    };
    assert_eq!(kept, Some(registered_id), "重复订阅不得换掉在册句柄");
    stream.retire("interaction:a");
    assert_eq!(stream.active_subscriptions(), 1, "自摘不得连带摘掉别路");
    stream.unsubscribe("session:b");
    assert_eq!(stream.active_subscriptions(), 0, "全部清掉后必须清空");
}
