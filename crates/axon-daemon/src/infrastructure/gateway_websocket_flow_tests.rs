//! 网关 WebSocket 的连接级行为测试：阻塞提交不拖垮其他连接、多订阅游标独立、断开释放与非法订阅拒绝。
use super::*;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc as std_mpsc;
use std::sync::{Condvar, Mutex as StdMutex};
use tokio_tungstenite::tungstenite::Message;

#[test]
fn blocked_durable_submit_does_not_stall_other_connections() {
    let (entered, entered_receiver) = std_mpsc::channel();
    let release = Arc::new((StdMutex::new(false), Condvar::new()));
    let context = test_context(
        Arc::new(BlockingIngress {
            entered,
            release: Arc::clone(&release),
        }),
        Arc::new(RejectingEgress),
    );
    let (address, stop, server) = start_server(context);
    let client = client_runtime();

    // 连接 1 提交：服务端 durable submit 进入阻塞且不会自行返回。
    client.block_on(async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(submit_frame().into()))
            .await
            .expect("发送 submit 帧");
    });
    entered_receiver
        .recv_timeout(RECOVERY_TIMEOUT)
        .expect("durable submit 应被调用");

    // submit 仍阻塞时，另一条连接必须仍可握手并拿到 pong。
    let responsive = client
        .block_on(round_trip(address, ping_frame()))
        .is_some_and(|text| text.contains("pong"));

    // 先释放并收口，再断言：失败路径也不留悬挂线程。
    {
        let (lock, condvar) = &*release;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condvar.notify_all();
    }
    stop.store(true, Ordering::Release);
    drop(client);
    let _ = server.join();

    assert!(
        responsive,
        "durable submit 阻塞期间其他连接必须仍可服务：runtime 线程不得执行同步 durable 调用"
    );
}

#[test]
fn connection_teardown_does_not_block_the_gateway_runtime() {
    let context = test_context(Arc::new(RejectingIngress), Arc::new(StoryEgress));
    let (address, stop, server) = start_server(context);
    let client = client_runtime();

    // 订阅后由客户端主动关闭：服务端走 Close 收口分支，并等待事件投影线程退出。
    let closed = client.block_on(async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(subscribe_frame("interaction-1").into()))
            .await
            .expect("发送订阅帧");
        let subscribed = read_until_subscribed(&mut socket, "interaction:interaction-1").await;
        socket.close(None).await.expect("发送关闭帧");
        while let Ok(Some(Ok(_))) = tokio::time::timeout(TEARDOWN_TIMEOUT, socket.next()).await {}
        subscribed
    });
    assert!(closed, "订阅应成功建立");
    assert!(
        client
            .block_on(round_trip(address, ping_frame()))
            .is_some_and(|text| text.contains("pong")),
        "连接收口不得阻塞 runtime：新连接必须仍能完成 ping/pong"
    );

    stop.store(true, Ordering::Release);
    drop(client);
    let _ = server.join();
}

#[test]
fn one_connection_serves_two_subscriptions_with_independent_cursors() {
    let context = test_context(Arc::new(RejectingIngress), Arc::new(TaggedEgress));
    let (address, stop, server) = start_server(context);
    let client = client_runtime();

    let observed = client.block_on(async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(subscribe_frame("interaction-1").into()))
            .await
            .expect("发送第一路订阅帧");
        socket
            .send(Message::Text(
                session_subscribe_frame("session:sub-1", "parent--subagent--root").into(),
            ))
            .await
            .expect("发送第二路订阅帧");

        let first = read_until_subscribed(&mut socket, "interaction:interaction-1").await;
        let second = read_until_subscribed(&mut socket, "session:sub-1").await;
        // 两路事件交错到达：必须按标识分流，而不是按到达顺序切分。
        let (lane_cursors, session_cursors) =
            collect_two_stream_cursors(&mut socket, "interaction:interaction-1", "session:sub-1")
                .await;
        (first, second, lane_cursors, session_cursors)
    });

    stop.store(true, Ordering::Release);
    drop(client);
    let _ = server.join();

    let (first, second, lane_cursors, session_cursors) = observed;
    assert!(first && second, "一条连接上的两路订阅都必须收到确认帧");
    assert_eq!(
        lane_cursors,
        vec!["{\"sequence\":0}".to_owned(), "{\"sequence\":1}".to_owned()],
        "车道那一路的游标必须自己单调推进"
    );
    assert_eq!(
        session_cursors,
        vec!["{\"sequence\":0}".to_owned(), "{\"sequence\":1}".to_owned()],
        "会话那一路有独立的游标，不受另一路影响"
    );
}

#[test]
fn unsubscribe_stops_only_the_named_subscription() {
    let live = Arc::new(AtomicUsize::new(0));
    let context = test_context(
        Arc::new(RejectingIngress),
        Arc::new(CountingEgress {
            live: Arc::clone(&live),
        }),
    );
    let (address, stop, server) = start_server(context);
    let client = client_runtime();

    let outcome = client.block_on(async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(subscribe_frame("interaction-1").into()))
            .await
            .expect("发送第一路订阅帧");
        assert!(read_until_subscribed(&mut socket, "interaction:interaction-1").await);
        socket
            .send(Message::Text(subscribe_frame("interaction-2").into()))
            .await
            .expect("发送第二路订阅帧");
        assert!(read_until_subscribed(&mut socket, "interaction:interaction-2").await);
        // 确认帧在服务端建立订阅之后才发出，因此此刻读到的就是「两路并存」。
        let both = live.load(Ordering::Acquire);

        socket
            .send(Message::Text(
                unsubscribe_frame("interaction:interaction-1").into(),
            ))
            .await
            .expect("发送取消订阅帧");
        let released = wait_for_release(&live, 1).await;

        // 剩下那一路必须仍在服务：ping 依然得到 pong。
        socket
            .send(Message::Text(ping_frame().into()))
            .await
            .expect("发送保活帧");
        let alive = loop {
            match next_frame(&mut socket).await {
                Some(text) if text.contains("pong") => break true,
                Some(_) => {}
                None => break false,
            }
        };
        (both, released, alive)
    });

    stop.store(true, Ordering::Release);
    drop(client);
    let _ = server.join();

    let (both, released, alive) = outcome;
    assert_eq!(both, 2, "同一连接上必须真的并存两路订阅");
    assert!(released, "取消订阅必须真正释放该路的订阅句柄");
    assert!(alive, "取消一路不得影响同一连接上的另一路");
}

#[test]
fn disconnect_releases_every_subscription_of_the_connection() {
    let live = Arc::new(AtomicUsize::new(0));
    let context = test_context(
        Arc::new(RejectingIngress),
        Arc::new(CountingEgress {
            live: Arc::clone(&live),
        }),
    );
    let (address, stop, server) = start_server(context);
    let client = client_runtime();

    let established = client.block_on(async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(subscribe_frame("interaction-1").into()))
            .await
            .expect("发送第一路订阅帧");
        assert!(read_until_subscribed(&mut socket, "interaction:interaction-1").await);
        socket
            .send(Message::Text(
                session_subscribe_frame("session:sub-1", "parent--subagent--root").into(),
            ))
            .await
            .expect("发送第二路订阅帧");
        assert!(read_until_subscribed(&mut socket, "session:sub-1").await);
        // 台账在断开之前采样：断开之后才读会把「已经释放」误当成「从未建立」。
        let established = live.load(Ordering::Acquire);
        socket.close(None).await.expect("发送关闭帧");
        established
    });
    assert_eq!(established, 2, "两条订阅都应在断开前处于存活状态");

    let released = client.block_on(wait_for_release(&live, 0));
    stop.store(true, Ordering::Release);
    drop(client);
    let _ = server.join();

    assert!(
        released,
        "连接断开必须按连接整体释放全部订阅，不得泄漏投影线程与订阅句柄"
    );
}

#[test]
fn subscribe_without_id_is_rejected_with_readable_code() {
    let context = test_context(Arc::new(RejectingIngress), Arc::new(StoryEgress));
    let (address, stop, server) = start_server(context);
    let client = client_runtime();

    let response = client.block_on(async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(
                serde_json::json!({
                    "type": "subscribe",
                    "workspaceId": "workspace-a",
                    "interactionId": "interaction-1",
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("发送缺标识的订阅帧");
        next_frame(&mut socket).await
    });

    stop.store(true, Ordering::Release);
    drop(client);
    let _ = server.join();

    let frame = parse_frame(response.as_deref().unwrap_or_default()).expect("错误帧应为 JSON");
    assert_eq!(
        frame.get("error").and_then(serde_json::Value::as_str),
        Some("subscribe_missing_id"),
        "缺 subscribeId 必须给出可读错误码，而不是 serde 的裸报错文案"
    );
}

#[test]
fn duplicate_subscribe_id_is_rejected_without_dropping_the_connection() {
    let context = test_context(Arc::new(RejectingIngress), Arc::new(StoryEgress));
    let (address, stop, server) = start_server(context);
    let client = client_runtime();

    let outcome = client.block_on(async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(subscribe_frame("interaction-1").into()))
            .await
            .expect("发送第一路订阅帧");
        assert!(read_until_subscribed(&mut socket, "interaction:interaction-1").await);

        socket
            .send(Message::Text(subscribe_frame("interaction-1").into()))
            .await
            .expect("重复发送同一标识的订阅帧");
        let duplicate = loop {
            let Some(text) = next_frame(&mut socket).await else {
                break None;
            };
            let Some(frame) = parse_frame(&text) else {
                continue;
            };
            if frame.get("type").and_then(serde_json::Value::as_str) == Some("error") {
                break Some(
                    frame
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
        };

        // 连接必须仍然可用：原先那一路继续服务。
        socket
            .send(Message::Text(ping_frame().into()))
            .await
            .expect("发送保活帧");
        let alive = loop {
            let Some(text) = next_frame(&mut socket).await else {
                break false;
            };
            if text.contains("pong") {
                break true;
            }
        };
        (duplicate, alive)
    });

    stop.store(true, Ordering::Release);
    drop(client);
    let _ = server.join();

    let (duplicate, alive) = outcome;
    assert_eq!(
        duplicate.as_deref(),
        Some("subscribe_duplicate_id"),
        "同一连接内标识重复必须被拒绝并给出可读错误码"
    );
    assert!(
        alive,
        "重标识只拒绝那一次订阅，不得关掉连接或影响已建立的那一路"
    );
}

/// 轮询等待存活订阅数回落到期望值；用于观测句柄释放的真实时机。
async fn wait_for_release(live: &AtomicUsize, expected: usize) -> bool {
    let deadline = tokio::time::Instant::now() + RECOVERY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if live.load(Ordering::Acquire) == expected {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    live.load(Ordering::Acquire) == expected
}
