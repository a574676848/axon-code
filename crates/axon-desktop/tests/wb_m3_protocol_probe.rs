//! 真实网关的多路订阅协议探针（WB-M3 协议隔离验证）。
//!
//! 默认不参与常规测试：它要求本机 daemon 正在运行且已是新协议版本。显式跑法：
//!
//! ```powershell
//! cargo test -p axon-desktop --test wb_m3_protocol_probe -- --ignored --nocapture
//! ```
//!
//! 探针复用桌面端自己的协议类型（`conversation::wire`）与地址解析（`gateway`），
//! 因此验证的就是真机窗口走的那条链路：同一条连接上并存两路订阅、标识回显、
//! 逐路取消、缺标识 / 重标识的可读错误码，以及「取消一路不影响另一路」。
//!
//! 两路都指向**真实存在**的目标（一条有历史 Interaction 的车道，加上它下面真实
//! 登记过的子代理会话）：只有订阅真的建立起来，「同标识重复」这类负例才会命中，
//! 事件帧与游标也才有可断言的内容。

use std::time::Duration;

use axon_desktop::conversation::history;
use axon_desktop::conversation::session_read;
use axon_desktop::conversation::wire::{ClientFrame, ServerFrame, WS_PATH};
use axon_desktop::gateway;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;

/// 单帧等待上限。
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);

type LiveSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 一条真实订阅目标。
#[derive(Debug, Clone)]
struct RealTarget {
    workspace_id: String,
    lane_id: String,
    interaction_id: String,
    /// 该车道下真实登记过的子代理会话标识；没有时为空。
    subagent_session_id: Option<String>,
}

/// 从真实库里挑一条有历史的车道，并尽量找一条它名下的子代理会话。
fn real_target() -> Option<RealTarget> {
    let payload = gateway::get_json_at("/api/v1/lanes").ok()?;
    let lanes = payload.get("lanes")?.as_array()?;
    for lane in lanes {
        let lane_id = lane.get("laneId")?.as_str()?.to_owned();
        let workspace_id = lane.get("workspaceId")?.as_str()?.to_owned();
        let Ok(interactions) = history::lane_interactions(&lane_id, Some(200), None) else {
            continue;
        };
        let Some(first) = interactions.first() else {
            continue;
        };
        let subagent_session_id = session_read::lane_subagents(&lane_id)
            .ok()
            .and_then(|roster| {
                roster
                    .subagents
                    .into_iter()
                    .find(|row| row.has_runtime)
                    .map(|row| row.session_id)
            });
        return Some(RealTarget {
            workspace_id,
            lane_id,
            interaction_id: first.interaction_id.clone(),
            subagent_session_id,
        });
    }
    None
}

/// 连接真实 daemon；不可达或缺凭据时返回 None，让探针以「环境未就绪」跳过。
async fn connect() -> Option<LiveSocket> {
    let base = gateway::websocket_base_url().ok()?;
    let token = gateway::api_token()?;
    let mut request = format!("{base}{WS_PATH}").into_client_request().ok()?;
    request.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).ok()?,
    );
    let connected = tokio::time::timeout(FRAME_TIMEOUT, tokio_tungstenite::connect_async(request))
        .await
        .ok()?
        .ok()?;
    Some(connected.0)
}

async fn send(socket: &mut LiveSocket, frame: &ClientFrame) {
    let text = serde_json::to_string(frame).expect("客户端帧应可序列化");
    socket
        .send(Message::Text(text.into()))
        .await
        .expect("发送客户端帧");
}

/// 读取下一帧服务端消息；连接关闭或超时返回 None。
async fn next_frame(socket: &mut LiveSocket) -> Option<ServerFrame> {
    loop {
        let incoming = tokio::time::timeout(FRAME_TIMEOUT, socket.next())
            .await
            .ok()?;
        match incoming? {
            Ok(Message::Text(text)) => return serde_json::from_str::<ServerFrame>(&text).ok(),
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => continue,
            Ok(Message::Binary(_) | Message::Close(_)) | Err(_) => return None,
        }
    }
}

/// 等指定订阅的确认帧；期间把属于别的订阅的帧收进 `collected`。
async fn wait_subscribed(
    socket: &mut LiveSocket,
    subscribe_id: &str,
    collected: &mut Vec<(String, Option<String>)>,
) -> Result<(), String> {
    loop {
        match next_frame(socket).await {
            Some(ServerFrame::Subscribed { subscribe_id: id }) => {
                return match id.as_deref() {
                    Some(value) if value == subscribe_id => Ok(()),
                    other => Err(format!("确认帧回显的标识不符：{other:?}")),
                };
            }
            Some(ServerFrame::Event {
                subscribe_id: id,
                cursor,
                ..
            }) => {
                collected.push((id.unwrap_or_default(), cursor));
            }
            Some(ServerFrame::Error { error, .. }) => return Err(error),
            Some(_) => continue,
            None => return Err("等待订阅确认时连接收口".to_string()),
        }
    }
}

/// 等一条错误帧，返回（错误码，回显的订阅标识）。
async fn wait_error(socket: &mut LiveSocket) -> Option<(String, Option<String>)> {
    loop {
        match next_frame(socket).await? {
            ServerFrame::Error {
                error,
                subscribe_id,
            } => return Some((error, subscribe_id)),
            _ => continue,
        }
    }
}

/// 等一条取消回执。
async fn wait_unsubscribed(socket: &mut LiveSocket) -> Option<String> {
    loop {
        match next_frame(socket).await? {
            ServerFrame::Unsubscribed { subscribe_id } => return subscribe_id,
            _ => continue,
        }
    }
}

/// 等 `pong`；返回 false 表示连接已不可用。
async fn wait_pong(socket: &mut LiveSocket) -> bool {
    loop {
        match next_frame(socket).await {
            Some(ServerFrame::Pong) => return true,
            Some(_) => continue,
            None => return false,
        }
    }
}

fn lane_frame(workspace_id: &str, subscribe_id: &str, interaction_id: &str) -> ClientFrame {
    ClientFrame::Subscribe {
        subscribe_id: subscribe_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        interaction_id: Some(interaction_id.to_owned()),
        session_id: None,
        resume_cursor: None,
    }
}

fn session_frame(workspace_id: &str, subscribe_id: &str, session_id: &str) -> ClientFrame {
    ClientFrame::Subscribe {
        subscribe_id: subscribe_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        interaction_id: None,
        session_id: Some(session_id.to_owned()),
        resume_cursor: None,
    }
}

/// 从**另一条**独立连接上探测某个订阅标识此刻是否仍被别的连接占着。
///
/// 这是「订阅句柄真的释放了吗」的可观测判据：标识在单连接内唯一，因此
/// - 本连接能订阅成功 → 原持有方已经释放；
/// - 收到 `subscribe_duplicate_id` → 原持有方仍然活着。
///
/// 探针用完立即取消，不留副作用。
async fn probe_subscription_holder(subscribe_id: &str, target: &RealTarget) -> HolderProbe {
    let Some(mut socket) = connect().await else {
        return HolderProbe::Unknown;
    };
    send(
        &mut socket,
        &lane_frame(&target.workspace_id, subscribe_id, &target.interaction_id),
    )
    .await;
    let mut collected = Vec::new();
    let outcome = match wait_subscribed(&mut socket, subscribe_id, &mut collected).await {
        Ok(()) => HolderProbe::Released,
        Err(code) if code == "subscribe_duplicate_id" => HolderProbe::Held,
        Err(_) => HolderProbe::Unknown,
    };
    if matches!(outcome, HolderProbe::Released) {
        send(
            &mut socket,
            &ClientFrame::Unsubscribe {
                subscribe_id: subscribe_id.to_owned(),
            },
        )
        .await;
        let _ = wait_unsubscribed(&mut socket).await;
    }
    let _ = socket.close(None).await;
    outcome
}

/// 订阅标识的持有状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HolderProbe {
    /// 标识已被释放，本连接可以重新占用。
    Released,
    /// 标识仍被原来的连接持有。
    Held,
    /// 环境问题导致无法判定。
    Unknown,
}

/// 一项观察结论。
struct Observation {
    name: &'static str,
    passed: bool,
}

impl Observation {
    fn new(name: &'static str, passed: bool) -> Self {
        Self { name, passed }
    }
}

/// 取出某一路订阅收到的 durable 游标（只保留非空游标，非 durable 帧不带游标）。
fn cursors_of(collected: &[(String, Option<String>)], subscribe_id: &str) -> Vec<String> {
    collected
        .iter()
        .filter(|(id, cursor)| id == subscribe_id && cursor.is_some())
        .filter_map(|(_, cursor)| cursor.clone())
        .collect()
}

/// 同一路订阅的游标必须单调不减：游标就是它自己的重放水位。
///
/// 用「不减」而不是「严格递增」：replay 与 live 交叠的位置允许同一个 durable
/// cursor 重复出现一次（订阅侧按 cursor 去重），严格递增会把这种正常形态判成失败。
fn is_monotonic(cursors: &[String]) -> bool {
    cursors
        .windows(2)
        .all(|pair| sequence_of(&pair[0]) <= sequence_of(&pair[1]))
}

/// 从 `{"generation":"...","sequence":N}` 里取出序号；解析失败按 0 处理。
fn sequence_of(cursor: &str) -> u64 {
    serde_json::from_str::<serde_json::Value>(cursor)
        .ok()
        .and_then(|value| value.get("sequence").and_then(serde_json::Value::as_u64))
        .unwrap_or(0)
}

async fn run_probe() -> Vec<Observation> {
    let mut observations = Vec::new();
    let Some(target) = real_target() else {
        eprintln!("[probe] 取不到真实车道与 Interaction，环境未就绪");
        return observations;
    };
    let Some(mut socket) = connect().await else {
        eprintln!("[probe] 网关不可达或缺凭据，环境未就绪");
        return observations;
    };

    let lane_subscribe_id = format!("interaction:{}", target.interaction_id);
    let session_subscribe_id = "session:wb-m3-probe-session".to_owned();
    let mut collected: Vec<(String, Option<String>)> = Vec::new();

    // 1. 同一条连接上连发两条订阅，两条都必须建立成功。
    send(
        &mut socket,
        &lane_frame(
            &target.workspace_id,
            &lane_subscribe_id,
            &target.interaction_id,
        ),
    )
    .await;
    let lane_ok = wait_subscribed(&mut socket, &lane_subscribe_id, &mut collected).await;
    observations.push(Observation::new(
        "同一条连接上的第一条订阅建立成功",
        lane_ok.is_ok(),
    ));
    if let Err(code) = &lane_ok {
        eprintln!("[probe] 第一路订阅失败：{code}");
    }

    if let Some(session_id) = target.subagent_session_id.clone() {
        send(
            &mut socket,
            &session_frame(&target.workspace_id, &session_subscribe_id, &session_id),
        )
        .await;
        let session_ok = wait_subscribed(&mut socket, &session_subscribe_id, &mut collected).await;
        observations.push(Observation::new(
            "同一条连接上的第二条订阅（会话级）建立成功",
            session_ok.is_ok(),
        ));
        if let Err(code) = &session_ok {
            eprintln!("[probe] 第二路订阅失败：{code}");
        }
    } else {
        eprintln!("[probe] 该车道下没有登记过的子代理会话，跳过第二路（会话级）订阅");
    }

    // 2. 重放事件必须带上所属订阅的标识，且每一路自己的游标单调推进。
    observations.push(Observation::new(
        "重放事件帧回显 subscribeId（事件按订阅归位）",
        collected
            .iter()
            .all(|(id, _)| id == &lane_subscribe_id || id == &session_subscribe_id),
    ));
    let lane_cursors = cursors_of(&collected, &lane_subscribe_id);
    let session_cursors = cursors_of(&collected, &session_subscribe_id);
    observations.push(Observation::new(
        "车道那一路的游标单调推进",
        is_monotonic(&lane_cursors),
    ));
    observations.push(Observation::new(
        "会话那一路的游标单调推进（两路互不干扰）",
        is_monotonic(&session_cursors),
    ));
    println!(
        "[probe] 目标车道 {}：车道订阅重放 {} 条事件，会话订阅重放 {} 条事件，合计 {} 条事件帧",
        target.lane_id,
        lane_cursors.len(),
        session_cursors.len(),
        collected.len()
    );

    // 3. 负例：空 subscribeId（协议上等价于缺字段）。
    send(
        &mut socket,
        &ClientFrame::Subscribe {
            subscribe_id: String::new(),
            workspace_id: target.workspace_id.clone(),
            interaction_id: Some(target.interaction_id.clone()),
            session_id: None,
            resume_cursor: None,
        },
    )
    .await;
    let blank = wait_error(&mut socket).await;
    observations.push(Observation::new(
        "空/缺 subscribeId 回可读的 subscribe_missing_id",
        blank
            .as_ref()
            .is_some_and(|(code, _)| code == "subscribe_missing_id"),
    ));

    // 4. 负例：已在册的标识重复订阅：只有真的订阅上了才会命中这一条。
    if lane_ok.is_ok() {
        send(
            &mut socket,
            &lane_frame(
                &target.workspace_id,
                &lane_subscribe_id,
                &target.interaction_id,
            ),
        )
        .await;
        let duplicate = wait_error(&mut socket).await;
        observations.push(Observation::new(
            "重复 subscribeId 回 subscribe_duplicate_id",
            duplicate
                .as_ref()
                .is_some_and(|(code, _)| code == "subscribe_duplicate_id"),
        ));
        observations.push(Observation::new(
            "订阅级错误帧回显 subscribeId",
            duplicate
                .as_ref()
                .and_then(|(_, id)| id.as_deref())
                .is_some_and(|id| id == lane_subscribe_id),
        ));

        // 重标识只拒绝那一次，不得影响已建立的那一路：再发一条事件查询仍然有回音。
        send(&mut socket, &ClientFrame::Ping {}).await;
        observations.push(Observation::new(
            "重标识被拒后连接与既有订阅仍然可用",
            wait_pong(&mut socket).await,
        ));

        // 5. 取消这一路：回执回显标识，且连接本身不受影响。
        send(
            &mut socket,
            &ClientFrame::Unsubscribe {
                subscribe_id: lane_subscribe_id.clone(),
            },
        )
        .await;
        let unsubscribed = wait_unsubscribed(&mut socket).await;
        observations.push(Observation::new(
            "取消订阅回执回显 subscribeId",
            unsubscribed.as_deref() == Some(lane_subscribe_id.as_str()),
        ));
        send(&mut socket, &ClientFrame::Ping {}).await;
        observations.push(Observation::new(
            "取消一路之后连接仍能完成 ping/pong（未被误关）",
            wait_pong(&mut socket).await,
        ));

        // 从另一条连接上确认：这一路的标识已经被真正释放（不是只从本地表里摘掉）。
        let holder = probe_subscription_holder(&lane_subscribe_id, &target).await;
        observations.push(Observation::new(
            "取消之后该订阅标识确实被释放（独立连接可重新占用）",
            holder == HolderProbe::Released,
        ));
        if holder == HolderProbe::Unknown {
            eprintln!("[probe] 独立连接探测未能判定标识持有状态");
        }

        // 取消之后同一标识可以重新订阅：证明条目真的被摘除，而不是残留占位。
        send(
            &mut socket,
            &lane_frame(
                &target.workspace_id,
                &lane_subscribe_id,
                &target.interaction_id,
            ),
        )
        .await;
        let mut again = Vec::new();
        observations.push(Observation::new(
            "取消后同一标识可以重新订阅（条目被真正摘除）",
            wait_subscribed(&mut socket, &lane_subscribe_id, &mut again)
                .await
                .is_ok(),
        ));
        send(
            &mut socket,
            &ClientFrame::Unsubscribe {
                subscribe_id: lane_subscribe_id.clone(),
            },
        )
        .await;
        let _ = wait_unsubscribed(&mut socket).await;
    }

    // 6. 两路都收口之后连接仍然存活。
    if target.subagent_session_id.is_some() {
        send(
            &mut socket,
            &ClientFrame::Unsubscribe {
                subscribe_id: session_subscribe_id.clone(),
            },
        )
        .await;
        let second = wait_unsubscribed(&mut socket).await;
        observations.push(Observation::new(
            "第二路取消同样精确到标识",
            second.as_deref() == Some(session_subscribe_id.as_str()),
        ));
    }
    send(&mut socket, &ClientFrame::Ping {}).await;
    observations.push(Observation::new(
        "两路都取消后连接仍存活（订阅释放不影响连接本身）",
        wait_pong(&mut socket).await,
    ));

    let _ = socket.close(None).await;
    observations
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "需要本机 daemon 运行新协议；用 --ignored 显式执行"]
async fn live_gateway_serves_multiple_subscriptions_per_connection() {
    let observations = run_probe().await;
    if observations.is_empty() {
        eprintln!("[probe] 跳过：环境未就绪（daemon 未运行、缺凭据或库里没有可订阅目标）");
        return;
    }
    let mut failed = Vec::new();
    for observation in &observations {
        let mark = if observation.passed {
            "PASSED"
        } else {
            "FAILED"
        };
        println!("  [{mark}] {}", observation.name);
        if !observation.passed {
            failed.push(observation.name);
        }
    }
    assert!(
        failed.is_empty(),
        "协议隔离验证未通过项：{}",
        failed.join(" / ")
    );
}
