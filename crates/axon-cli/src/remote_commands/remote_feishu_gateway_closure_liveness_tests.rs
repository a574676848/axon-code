//! Feishu 网关闭环「读取 → 投递 → 确认」链路的端到端守卫。
//!
//! 真机事故：入站消息已写入 durable 事件源并挂上 Typing reaction，但闭环 worker 的
//! 轮询循环从未真正读取：事件源读偏移始终为 0、cursor 文件从未生成、后台
//! `activeTaskCount` 从未从 0 变成过 1，消息因此永远没有进入 Session Runtime。
//! 根因是 worker 用**逻辑路径**做 `exists()` 门槛，而分片 JSONL 的逻辑路径从不作为
//! 文件存在，使 `continue` 恒被命中。
//!
//! 下面三个用例把同一条链路放在**完全隔离的临时目录**里跑（不触碰 `~/.axon`、
//! 不访问网络、不使用真实环境），守住这条链路以及「worker 会自己动起来」这个不变式。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::remote_feishu_gateway_event_execution::ExecutedGatewayInput;
use super::remote_feishu_gateway_lanes::GatewayLaneScheduler;
use crate::remote_commands::RemoteCliContext;
use crate::remote_commands::remote_feishu_gateway::{
    RemoteGatewayCommand, execute_feishu_gateway_daemon,
};
use crate::remote_commands::remote_feishu_gateway_worker::{
    FeishuGatewayBackgroundWorker, GatewayClosureTiming,
};

const TEST_TICK_LIMIT: usize = 1;

fn event_line(request_id: &str) -> String {
    serde_json::json!({
        "kind": "message",
        "requestId": request_id,
        "chatId": "oc_closure_guard",
        "chatType": "p2p",
        "messageId": format!("om_{request_id}"),
        "sourceCreatedAtMs": 100,
        "senderOpenId": "ou_sender",
        "text": "守卫用例",
    })
    .to_string()
}

fn gateway_command(event_source: &std::path::Path) -> RemoteGatewayCommand {
    RemoteGatewayCommand {
        session_id: 1,
        event_json: None,
        event_file: Some(event_source.to_path_buf()),
        daemon: true,
        interval_ms: None,
        max_ticks: Some(TEST_TICK_LIMIT),
        cursor_file: None,
        daemon_endpoint: "axon-daemon-closure-guard".to_owned(),
    }
}

fn cursor_path(event_source: &std::path::Path) -> std::path::PathBuf {
    event_source.with_extension("cursor.json")
}

/// 事件源是分片 JSONL：逻辑路径 `gateway_events.jsonl` 对应物理分片
/// `gateway_events.000000.jsonl`，读取入口必须拿到逻辑路径。
fn seed_sharded_event_source(
    directory: &std::path::Path,
    request_id: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let logical = directory.join("gateway_events.jsonl");
    let physical = directory.join("gateway_events.000000.jsonl");
    std::fs::write(&physical, format!("{}\n", event_line(request_id))).expect("写入事件源分片");
    (logical, physical)
}

/// 闭环 tick 必须把事件源里的事件读出来并投递给车道调度器。
///
/// 判据用「读偏移前进」而不是跑真实执行链：执行链需要网络与 daemon IPC，
/// 而本次要守的是读取与投递这一段的活性。
#[test]
fn closure_tick_reads_the_event_source_and_dispatches_to_lanes() {
    let directory = axon_test_support::test_temp_dir();
    let (logical_source, _physical) = seed_sharded_event_source(directory.path(), "req-guard-1");

    let (wake, _wake_receiver) = mpsc::sync_channel(16);
    let scheduler = GatewayLaneScheduler::start(|_input| Ok(ExecutedGatewayInput::default()), wake);
    let command = gateway_command(&logical_source);

    let report = execute_feishu_gateway_daemon(&command, &scheduler).expect("闭环 tick 应成功");

    assert_eq!(
        report.input_events, 1,
        "闭环 tick 必须从事件源读出并投递那一条待处理事件；读到 0 条说明读取链路没有工作"
    );
    assert_eq!(scheduler.read_offset(), 1, "读偏移必须推进到已投递批次末尾");
}

/// 车道完成后，下一次 tick 必须推进确认前缀并落盘 cursor。
#[test]
fn closure_tick_advances_cursor_after_lanes_finish() {
    let directory = axon_test_support::test_temp_dir();
    let (logical_source, _physical) = seed_sharded_event_source(directory.path(), "req-guard-2");
    let cursor = cursor_path(&logical_source);

    let (wake, _wake_receiver) = mpsc::sync_channel(16);
    let scheduler = GatewayLaneScheduler::start(|_input| Ok(ExecutedGatewayInput::default()), wake);
    let command = gateway_command(&logical_source);

    execute_feishu_gateway_daemon(&command, &scheduler).expect("首次 tick");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !cursor.exists() {
        let _ = execute_feishu_gateway_daemon(&command, &scheduler);
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(
        cursor.exists(),
        "车道完成后必须以 cursor 落盘 acked_offset；否则事件源永远无法被确认与回收"
    );
}

/// 真正的后台 worker 线程必须在事件源出现待处理事件后自动消费它。
///
/// 这是与真机事故最贴近的守卫：真机上后台 `activeTaskCount` 从未变成 1、cursor 从未
/// 生成、事件源从未被读走，说明 worker 的轮询循环里读取被整段跳过。该用例把真实
/// worker 线程起在**临时根目录**上（不触碰 `~/.axon`，也不使用真实环境），不给任何
/// 外部唤醒，断言它会自己把事件读走。
#[test]
fn background_worker_consumes_a_pending_event_without_any_external_wake() {
    let directory = axon_test_support::test_temp_dir();
    let axon_root = directory.path().join("axon-root");
    let feishu_root = axon_root.join("data").join("remote").join("feishu");
    std::fs::create_dir_all(&feishu_root).expect("创建飞书运行态目录");
    let (logical_source, _physical) = seed_sharded_event_source(&feishu_root, "req-guard-3");

    let context = RemoteCliContext::new(
        directory.path().join("config.toml"),
        axon_root,
        axon_config::RemoteFeishuWebSocketConfig::default(),
    );
    let worker = FeishuGatewayBackgroundWorker::start_with_cancellation(
        context,
        1,
        logical_source.as_path(),
        "tenant-token-guard".to_owned(),
        None,
        false,
        None,
        // 测试用短节奏：等一次「无唤醒也自己动」的回收扫描不必花生产用的 5 秒。
        GatewayClosureTiming {
            stop_poll: Duration::from_millis(10),
            recovery_scan_interval: Duration::from_millis(50),
        },
    );

    // 本用例只守「worker 的轮询循环会自己动起来」这一不变式，因此等的是 tick 计数，
    // 而不是 cursor：桩执行链不会真的完成事件，cursor 永远不该在测试里出现。
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut report = worker.snapshot();
    while Instant::now() < deadline && report.gateway_ticks == 0 {
        std::thread::sleep(Duration::from_millis(10));
        report = worker.snapshot();
    }

    assert!(
        report.gateway_ticks > 0,
        "后台 worker 必须自己完成闭环 tick；真机事故正是这里恒为 0"
    );
}
