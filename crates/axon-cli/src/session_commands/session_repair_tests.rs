//! `axon debug session repair` 测试：序号重排与归属回填的存量修复语义。
//!
//! 夹具直接按事件日志层的公开编码器拼出「逆序日志」：写侧的 `append_once` 现在会核对
//! 文件尾并拒绝逆序追加，坏日志只能来自它落地之前的旧版本，因此这里按真实布局手写帧。

use std::path::{Path, PathBuf};

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};
use axon_eventlog::{EventLog, EventLogReader, Header, RuntimeEventKind, RuntimeEventRecord};
use axon_session::{
    InteractionStore as _, NewSessionRequest, RuntimeStore as _, SessionKind, SqliteRuntimeStore,
    SubagentSessionRequest,
};

use super::{SessionRepairCommand, run_session_repair};
use crate::OutputFormat;

/// 现场基线的会话标识：6231 条事件、28 处逆序。
const FIELD_SESSION: &str = "01M7YFWWXS9Y28RGCP7BWNN4A2";

/// 一次修复用例的隔离现场。
struct Fixture {
    _directory: axon_test_support::TestTempDir,
    /// 配置路径：运行根取其所在目录，数据落在其 `data` 子目录。
    config_path: PathBuf,
    data_root: PathBuf,
    store: SqliteRuntimeStore,
}

fn fixture() -> Fixture {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    let data_root = directory.path().join("data");
    let store = SqliteRuntimeStore::from_root(&data_root);
    store.initialize().expect("初始化运行态库");
    Fixture {
        _directory: directory,
        config_path,
        data_root,
        store,
    }
}

/// 一条普通逐轮事件。
fn wire_event(sequence: u64, emitted_at_ms: u64) -> RuntimeEventRecord {
    RuntimeEventRecord::new(
        sequence,
        emitted_at_ms,
        RuntimeEventKind::Other("wire_event".to_owned()),
        serde_json::json!({ "text": format!("帧 {sequence}") }),
    )
}

/// 一条单轮指标事件（会话级事件类型，用于断言清单带事件类型）。
fn turn_metrics_event(sequence: u64, emitted_at_ms: u64) -> RuntimeEventRecord {
    RuntimeEventRecord::new(
        sequence,
        emitted_at_ms,
        RuntimeEventKind::TurnMetrics,
        serde_json::json!({ "turnId": format!("turn-{sequence}") }),
    )
}

/// 按给定的批次顺序手写出日志文件：批次之间可以逆序，批次之内保持连续。
fn write_log(log: &EventLog, batches: &[Vec<RuntimeEventRecord>]) {
    let header = Header::new(log.session_id(), 1_000, Some("/workspace".to_owned()));
    let mut bytes = axon_eventlog::encode_frame(&header.encode_line().expect("header 编码"))
        .expect("header 帧");
    for batch in batches {
        let mut plain = Vec::new();
        for record in batch {
            plain.extend_from_slice(&record.encode_line().expect("记录编码"));
            plain.push(b'\n');
        }
        bytes.extend(axon_eventlog::encode_frame(&plain).expect("数据帧"));
    }
    let path = log.path();
    std::fs::create_dir_all(path.parent().expect("事件目录")).expect("建事件目录");
    std::fs::write(&path, bytes).expect("写事件日志");
}

/// 登记一条根会话，供子代理会话继承工作区。
fn root_session(store: &SqliteRuntimeStore, session_id: &str) -> DaemonSessionId {
    let id = DaemonSessionId::new(session_id.to_owned()).expect("合法会话标识");
    store
        .create_session_with(&NewSessionRequest {
            workspace_id: Some("workspace-lane-h".to_owned()),
            ..NewSessionRequest::primary(id.clone(), "/workspace", 10)
        })
        .expect("登记根会话");
    id
}

/// 登记一条子代理会话。
fn subagent_session(store: &SqliteRuntimeStore, root: &DaemonSessionId, path: &str) -> String {
    let record = store
        .resolve_subagent_session(&SubagentSessionRequest {
            root_session_id: root.clone(),
            agent_path: path.to_owned(),
            cwd: "/workspace".to_owned(),
            now_ms: 20,
        })
        .expect("登记子代理会话");
    assert_eq!(record.kind, SessionKind::Subagent);
    record.session_id.as_str().to_owned()
}

/// 在给定会话里登记一条派发轮次。
fn dispatch_interaction(
    store: &SqliteRuntimeStore,
    session_id: &str,
    interaction_id: &str,
    received_at_ms: u64,
) {
    let source = InteractionSource::new(
        axon_domain::AGENT_DISPATCH_SOURCE_CHANNEL,
        format!(
            "{}{interaction_id}",
            axon_domain::AGENT_DISPATCH_SOURCE_PREFIX
        ),
    )
    .expect("派发来源");
    let record = InteractionRecord::new(
        DaemonSessionId::new(session_id.to_owned()).expect("合法会话标识"),
        InteractionId::new(interaction_id.to_owned()).expect("合法交互标识"),
        source,
        ScopeRef::workspace("workspace-lane-h").expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new(axon_domain::AGENT_DISPATCH_SOURCE_CHANNEL, "subagent").expect("回复路由"),
        "派发子代理",
        received_at_ms,
    )
    .expect("交互记录");
    store.append(&record).expect("交互入库");
}

/// 跑一次修复命令。
fn repair(
    fixture: &Fixture,
    session: Option<&str>,
    reorder: bool,
    attribute: bool,
    apply: bool,
) -> (crate::CliRunResult, serde_json::Value) {
    let result = run_session_repair(
        OutputFormat::Json,
        &fixture.config_path,
        &SessionRepairCommand {
            session: session.map(str::to_owned),
            reorder,
            attribute,
            dry_run: false,
            apply,
        },
    );
    let body = serde_json::from_str(&result.stdout).unwrap_or_else(|_| {
        panic!(
            "修复回执应为 JSON：stdout={} stderr={}",
            result.stdout, result.stderr
        )
    });
    (result, body)
}

/// 读回日志里的序号序列（按文件顺序）。
fn sequences_of(path: &Path) -> Vec<u64> {
    axon_eventlog::read_tail_records(path, usize::MAX)
        .expect("读回事件日志")
        .into_iter()
        .map(|record| record.sequence)
        .collect()
}

#[test]
fn reorder_dry_run_lists_out_of_order_frames() {
    let fixture = fixture();
    let session = DaemonSessionId::new(FIELD_SESSION.to_owned()).expect("合法会话标识");
    let log = fixture.store.event_log(&session);
    // 现场形状：单条 59 的下一帧落在 57，尾部的 55 又落在 60 之后。
    write_log(
        &log,
        &[
            vec![wire_event(56, 1_056)],
            vec![wire_event(58, 1_058), wire_event(59, 1_059)],
            vec![wire_event(57, 1_057)],
            vec![turn_metrics_event(60, 1_060)],
            vec![wire_event(55, 1_055)],
        ],
    );
    let before = std::fs::read(log.path()).expect("读原日志");

    let (result, body) = repair(&fixture, Some(FIELD_SESSION), true, false, false);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(body["status"], "repair_dry_run");
    assert_eq!(body["mode"], "reorder");
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["scannedSessions"], 1);
    assert_eq!(body["dirtySessions"], 1);
    assert_eq!(body["inversionCount"], 2);
    let session_report = &body["sessions"][0];
    assert_eq!(session_report["sessionId"], FIELD_SESSION);
    assert_eq!(session_report["eventCount"], 6);
    assert_eq!(session_report["repaired"], false);
    assert_eq!(session_report["backupPath"], serde_json::Value::Null);
    let inversions = session_report["inversions"]
        .as_array()
        .expect("逆序清单应为数组");
    assert_eq!(inversions.len(), 2, "两处逆序都必须列出来");
    assert_eq!(inversions[0]["position"], 3);
    assert_eq!(inversions[0]["previousSequence"], 59);
    assert_eq!(inversions[0]["currentSequence"], 57);
    assert_eq!(inversions[0]["previousEventType"], "wire_event");
    assert_eq!(inversions[0]["currentEventType"], "wire_event");
    assert_eq!(inversions[0]["previousEmittedAtMs"], 1_059);
    assert_eq!(inversions[0]["currentEmittedAtMs"], 1_057);
    assert_eq!(inversions[1]["position"], 5);
    assert_eq!(inversions[1]["previousSequence"], 60);
    assert_eq!(inversions[1]["currentSequence"], 55);
    assert_eq!(inversions[1]["previousEventType"], "turn_metrics");
    assert_eq!(inversions[1]["currentEventType"], "wire_event");
    assert_eq!(inversions[1]["duplicate"], false);

    // dry-run 不写任何文件：原字节与备份都保持原样。
    assert_eq!(
        std::fs::read(log.path()).expect("读原日志"),
        before,
        "dry-run 不得改动事件日志"
    );
    assert_eq!(
        backup_paths(&fixture.data_root, FIELD_SESSION).len(),
        0,
        "dry-run 不得产生备份文件"
    );
}

#[test]
fn reorder_repair_is_idempotent() {
    let fixture = fixture();
    let session = DaemonSessionId::new(FIELD_SESSION.to_owned()).expect("合法会话标识");
    let log = fixture.store.event_log(&session);
    write_log(
        &log,
        &[
            vec![wire_event(56, 1_056)],
            vec![wire_event(58, 1_058), wire_event(59, 1_059)],
            vec![wire_event(57, 1_057)],
            vec![wire_event(60, 1_060)],
        ],
    );

    let (first_result, first) = repair(&fixture, Some(FIELD_SESSION), true, false, true);
    assert_eq!(first_result.exit_code, 0, "{}", first_result.stderr);
    assert_eq!(first["status"], "repair_applied");
    let first_session = &first["sessions"][0];
    assert_eq!(first_session["repaired"], true);
    assert_eq!(
        first_session["inversions"].as_array().map(Vec::len),
        Some(1)
    );
    let backup = first_session["backupPath"]
        .as_str()
        .expect("执行态必须给出备份路径")
        .to_owned();
    assert!(
        Path::new(&backup).is_file(),
        "重写之前必须先备份原日志：{backup}"
    );
    assert_eq!(
        backup_paths(&fixture.data_root, FIELD_SESSION).len(),
        1,
        "一次修复只产生一个备份"
    );

    // 重排之后序号严格递增，标准读路径重新可读（逆序日志此前会被判为损坏）。
    assert_eq!(sequences_of(&log.path()), vec![56, 57, 58, 59, 60]);
    let reader = EventLogReader::open(&log.path()).expect("重排后日志必须可正常读取");
    assert_eq!(
        reader
            .records()
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        vec![56, 57, 58, 59, 60]
    );
    let after_first = std::fs::read(log.path()).expect("读重排后的日志");

    let (second_result, second) = repair(&fixture, Some(FIELD_SESSION), true, false, true);
    assert_eq!(second_result.exit_code, 0, "{}", second_result.stderr);
    assert_eq!(second["inversionCount"], 0, "第二次不得再发现逆序");
    let second_session = &second["sessions"][0];
    assert_eq!(
        second_session["inversions"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(second_session["repaired"], false);
    assert_eq!(second_session["backupPath"], serde_json::Value::Null);
    assert_eq!(
        std::fs::read(log.path()).expect("读第二次之后的日志"),
        after_first,
        "第二次执行不得产生任何字节变化"
    );
    assert_eq!(
        backup_paths(&fixture.data_root, FIELD_SESSION).len(),
        1,
        "第二次执行不得再产生备份"
    );
}

#[test]
fn attribute_dry_run_classifies_backfillable_rows() {
    let fixture = fixture();
    let root = root_session(&fixture.store, "01M7Z4BF1C8CR9N3Q9H3N9CCV7");
    let backfillable = subagent_session(&fixture.store, &root, "root/delegate-a");
    let ambiguous = subagent_session(&fixture.store, &root, "root/delegate-b");
    dispatch_interaction(
        &fixture.store,
        &backfillable,
        "01M7Z4BFYY8QX8DKF83S6DHBRE",
        30,
    );
    dispatch_interaction(&fixture.store, &ambiguous, "01M7Z4BFYY8QX8DKF83S6DHBRF", 31);
    dispatch_interaction(&fixture.store, &ambiguous, "01M7Z4BFYY8QX8DKF83S6DHBRG", 32);

    let backfillable_log = fixture
        .store
        .event_log(&DaemonSessionId::new(backfillable.clone()).expect("合法会话标识"));
    write_log(
        &backfillable_log,
        &[
            vec![wire_event(0, 1_000), wire_event(1, 1_001)],
            vec![wire_event(2, 1_002)],
        ],
    );
    let ambiguous_log = fixture
        .store
        .event_log(&DaemonSessionId::new(ambiguous.clone()).expect("合法会话标识"));
    write_log(
        &ambiguous_log,
        &[vec![wire_event(0, 1_000)], vec![wire_event(1, 1_001)]],
    );

    let (result, body) = repair(&fixture, None, false, true, false);

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(body["status"], "repair_dry_run");
    assert_eq!(body["mode"], "attribute");
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["scannedSessions"], 2);
    assert_eq!(body["backfillableFrames"], 3);
    assert_eq!(body["undeterminedFrames"], 2);

    let sessions = body["sessions"].as_array().expect("逐会话结论应为数组");
    let unique = sessions
        .iter()
        .find(|item| item["sessionId"] == backfillable.as_str())
        .expect("可唯一确定的会话必须在报告里");
    assert_eq!(unique["verdict"], "backfillable");
    assert_eq!(unique["unattributedFrames"], 3);
    assert_eq!(unique["eventCount"], 3);
    assert_eq!(unique["interactionId"], "01M7Z4BFYY8QX8DKF83S6DHBRE");
    assert_eq!(unique["repaired"], false);

    let undetermined = sessions
        .iter()
        .find(|item| item["sessionId"] == ambiguous.as_str())
        .expect("多义会话必须在报告里");
    assert_eq!(undetermined["verdict"], "undetermined");
    assert_eq!(undetermined["unattributedFrames"], 2);
    assert_eq!(undetermined["interactionId"], serde_json::Value::Null);
    let reason = undetermined["reason"].as_str().expect("必须给出原因");
    assert!(reason.contains("2 条派发轮次"), "{reason}");
    assert_eq!(
        backup_paths(&fixture.data_root, &backfillable).len(),
        0,
        "dry-run 不得产生备份文件"
    );
}

#[test]
fn attribute_repair_writes_interaction_columns() {
    let fixture = fixture();
    let root = root_session(&fixture.store, "01M7Z4BF1C8CR9N3Q9H3N9CCV7");
    let session_id = subagent_session(&fixture.store, &root, "root/delegate-a");
    dispatch_interaction(
        &fixture.store,
        &session_id,
        "01M7Z4BFYY8QX8DKF83S6DHBRE",
        30,
    );
    let session = DaemonSessionId::new(session_id.clone()).expect("合法会话标识");
    let log = fixture.store.event_log(&session);
    // 三帧没有归属，一帧本来就带着别的归属：回填只补空的那批。
    write_log(
        &log,
        &[
            vec![wire_event(1, 1_001), wire_event(2, 1_002)],
            vec![
                wire_event(3, 1_003),
                wire_event(4, 1_004).with_interaction("other-turn"),
            ],
        ],
    );

    let (first_result, first) = repair(&fixture, Some(&session_id), false, true, true);
    assert_eq!(first_result.exit_code, 0, "{}", first_result.stderr);
    assert_eq!(first["status"], "repair_applied");
    assert_eq!(first["backfillableFrames"], 3);
    let session_report = &first["sessions"][0];
    assert_eq!(session_report["repaired"], true);
    assert!(
        session_report["backupPath"]
            .as_str()
            .is_some_and(|path| Path::new(path).is_file()),
        "回填之前必须先备份原日志"
    );

    let reader = EventLogReader::open(&log.path()).expect("回填后日志必须可正常读取");
    let attributed = reader
        .records()
        .iter()
        .map(|record| record.interaction_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        attributed,
        vec![
            Some("01M7Z4BFYY8QX8DKF83S6DHBRE".to_owned()),
            Some("01M7Z4BFYY8QX8DKF83S6DHBRE".to_owned()),
            Some("01M7Z4BFYY8QX8DKF83S6DHBRE".to_owned()),
            Some("other-turn".to_owned()),
        ],
        "空归属帧必须补上派发轮次，已有归属不得被改写"
    );
    // 归属列写入后，按键读取能直接命中这批帧。
    let keyed = axon_eventlog::read_interaction_after(
        &log.path(),
        "01M7Z4BFYY8QX8DKF83S6DHBRE",
        0,
        usize::MAX,
    )
    .expect("按归属读取");
    assert_eq!(keyed.len(), 3, "按归属列必须读到回填的三帧");

    let before_second = std::fs::read(log.path()).expect("读回填后的日志");
    let (second_result, second) = repair(&fixture, Some(&session_id), false, true, true);
    assert_eq!(second_result.exit_code, 0, "{}", second_result.stderr);
    let second_report = &second["sessions"][0];
    assert_eq!(second_report["unattributedFrames"], 0);
    assert_eq!(second_report["verdict"], "clean");
    assert_eq!(second_report["repaired"], false);
    assert_eq!(
        std::fs::read(log.path()).expect("读第二次之后的日志"),
        before_second,
        "第二次执行不得产生任何字节变化"
    );
    assert_eq!(
        backup_paths(&fixture.data_root, &session_id).len(),
        1,
        "第二次执行不得再产生备份"
    );
}

/// 某个会话目录下的备份文件清单。
fn backup_paths(data_root: &Path, session_id: &str) -> Vec<PathBuf> {
    let directory = data_root
        .join("events")
        .join(EventLog::encode_segment(session_id));
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".bak-"))
        })
        .collect()
}
