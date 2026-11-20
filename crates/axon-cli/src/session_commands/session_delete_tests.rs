//! `axon sessions delete` 测试：验收「会话族整体删除、失败即回滚」这一条语义。

use std::path::{Path, PathBuf};

use axon_domain::DaemonSessionId;
use axon_session::{
    JsonlThreadEventStore, ResumePointerStore as _, RuntimeStore as _, SqliteRuntimeStore,
    StartEmptyThreadCommand, ThreadService, ThreadSettings, ThreadSource,
};
use rusqlite::params;

use super::{SessionDeleteCommand, TargetKind, run_session_delete};
use crate::OutputFormat;
use crate::sessions_http_adapter::read_session_rows;

/// 一次删除用例的隔离现场。
struct Fixture {
    _directory: axon_test_support::TestTempDir,
    /// 配置路径：运行根取其所在目录，数据落在其 `data` 子目录。
    config_path: PathBuf,
    /// 运行态根目录（thread 事件面在这里）。
    root: PathBuf,
    /// 数据根目录（运行态库、事件目录与归档目录在这里）。
    data_root: PathBuf,
    store: SqliteRuntimeStore,
    session_id: DaemonSessionId,
    /// 会话当前的运行窗口：thread 分片按它命名。
    run_id: String,
}

fn fixture(session_id: &str) -> Fixture {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    let root = directory.path().to_path_buf();
    let data_root = root.join("data");
    let store = SqliteRuntimeStore::from_root(&data_root);
    store.initialize().expect("初始化运行态库");
    let session_id = DaemonSessionId::new(session_id.to_owned()).expect("合法会话 ID");
    let record = store
        .create_session(&session_id, "/workspace", 1)
        .expect("登记会话");
    Fixture {
        _directory: directory,
        config_path,
        root,
        data_root,
        store,
        session_id,
        run_id: record.current_run_id,
    }
}

/// 写一条 thread 分片，让「删分片」这一步有真实对象可删。
fn seed_thread(fixture: &Fixture) {
    ThreadService::new(JsonlThreadEventStore)
        .start_empty_thread(
            &fixture.root,
            StartEmptyThreadCommand {
                thread_id: fixture.run_id.clone(),
                session_root_id: fixture.run_id.clone(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: "/workspace".to_owned(),
                    provider: "provider".to_owned(),
                    model: "model".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 1,
            },
        )
        .expect("写入 thread 分片");
}

/// 写入一整族运行态事实：入站、工作项、指标、投影、调度意图与恢复指针。
fn seed_session_family(fixture: &Fixture, content: &str, work_item_status: &str) {
    let session_id = fixture.session_id.as_str();
    let interaction_id = format!("inbox-{session_id}");
    fixture
        .store
        .with_read_connection(|connection| {
            connection.execute(
                "INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,
                     audience_json,reply_route_json,content,session_id,received_at_ms,updated_at_ms)
                 VALUES(?1,'tui',?2,'{}','{}','{}',?3,?4,10,10)",
                params![interaction_id, format!("msg-{session_id}"), content, session_id],
            )?;
            connection.execute(
                "INSERT INTO work_items(execution_id,goal_id,session_id,attempt,priority,inbox_id,
                     status,created_at_ms,updated_at_ms)
                 VALUES(?1,NULL,?2,0,0,?3,?4,10,10)",
                params![
                    format!("exec-{session_id}"),
                    session_id,
                    interaction_id,
                    work_item_status
                ],
            )?;
            connection.execute(
                "INSERT INTO work_item_history(work_id,session_id,attempt,priority,terminal_kind,
                     status,created_at_ms,updated_at_ms,finished_at_ms)
                 VALUES(?1,?2,0,0,'understood','completed',10,10,10)",
                params![format!("hist-{session_id}"), session_id],
            )?;
            connection.execute(
                "INSERT INTO session_metrics(session_id,turn_count,updated_at_ms)
                 VALUES(?1,1,10)",
                params![session_id],
            )?;
            connection.execute(
                "INSERT INTO turn_activity(session_id,interaction_id,received_at_ms,updated_at_ms)
                 VALUES(?1,?2,10,10)",
                params![session_id, interaction_id],
            )?;
            connection.execute(
                "INSERT INTO thread_turn_facts(thread_id,assistant_part_id,recorded_at_ms,updated_at_ms)
                 VALUES(?1,'part-1',10,10)",
                params![fixture.run_id.as_str()],
            )?;
            connection.execute(
                "INSERT INTO schedules(schedule_id,session_id,kind,expression,payload_json,
                     delivery_mode,status,failure_count)
                 VALUES(?1,?2,'interval','60s','{}','push','active',0)",
                params![format!("schedule-{session_id}"), session_id],
            )?;
            Ok(())
        })
        .expect("写入会话族事实");
    fixture
        .store
        .write_resume_pointer(&fixture.session_id, 10)
        .expect("写入恢复指针");
    fixture
        .store
        .with_outbox_read_connection(|connection| {
            connection.execute(
                "INSERT INTO user_questions(question_id,idempotency_key,session_id,interaction_id,
                     turn_id,tool_call_id,questions_json,state,deadline_at_ms,created_at_ms,expires_at_ms)
                 VALUES('question-1','idem-1',?1,?2,'turn-1','call-1','[]','pending',10,10,10)",
                params![session_id, interaction_id],
            )
        })
        .expect("写入待答提问");
}

/// 造出事件目录、事件归档目录与会话上下文目录，验证它们跟着会话一起回收。
fn seed_session_dirs(fixture: &Fixture) -> (PathBuf, PathBuf, PathBuf) {
    let segment = axon_eventlog::EventLog::encode_segment(fixture.session_id.as_str());
    let events = fixture.data_root.join("events").join(&segment);
    let archive = fixture
        .data_root
        .join("archive")
        .join("session-events")
        .join(&segment);
    let context = fixture
        .root
        .join("sessions")
        .join(fixture.session_id.as_str());
    for directory in [&events, &archive, &context] {
        std::fs::create_dir_all(directory).expect("建会话目录");
        std::fs::write(directory.join("payload.bin"), b"x").expect("写会话文件");
    }
    (events, archive, context)
}

fn delete(
    format: OutputFormat,
    fixture: &Fixture,
    session_id: &str,
    kind: TargetKind,
) -> crate::CliRunResult {
    run_session_delete(
        format,
        &fixture.config_path,
        &SessionDeleteCommand {
            session_id: session_id.to_owned(),
        },
        kind,
    )
}

fn report_of(result: &crate::CliRunResult) -> serde_json::Value {
    serde_json::from_str(&result.stdout).expect("JSON 回执")
}

#[test]
fn deleting_a_session_removes_the_whole_family_and_the_thread_shard() {
    let fixture = fixture("session-delete-target");
    seed_thread(&fixture);
    seed_session_family(&fixture, "要删掉的会话", "completed");
    let (events, archive, context) = seed_session_dirs(&fixture);

    let result = delete(
        OutputFormat::Json,
        &fixture,
        fixture.session_id.as_str(),
        TargetKind::Session,
    );

    assert_eq!(result.exit_code, 0, "删除应成功：{}", result.stderr);
    let report = report_of(&result);
    assert_eq!(report["status"], "session_deleted");
    assert_eq!(report["sessionRowDeleted"], true);
    assert_eq!(report["deletedRows"]["sessions"], 1);
    assert_eq!(report["deletedRows"]["inbox"], 1);
    assert_eq!(report["deletedRows"]["workItems"], 1);
    assert_eq!(report["deletedRows"]["workItemHistory"], 1);
    assert_eq!(report["deletedRows"]["sessionMetrics"], 1);
    assert_eq!(report["deletedRows"]["turnActivity"], 1);
    assert_eq!(report["deletedRows"]["threadTurnFacts"], 1);
    assert_eq!(report["deletedRows"]["schedules"], 1);
    assert_eq!(report["deletedRows"]["resumePointer"], 1);
    assert_eq!(report["deletedRows"]["userQuestions"], 1);

    // 控制面会话已不存在：`axon sessions list`、TUI 与桌面端都不会再返回它。
    assert!(
        fixture.store.get_session(&fixture.session_id).is_err(),
        "会话行必须被删掉"
    );
    let rows = read_session_rows(&fixture.store).expect("读取会话清单");
    assert!(
        rows.iter()
            .all(|row| row.session_id != fixture.session_id.as_str()),
        "删除后会话清单不得再返回该会话"
    );
    assert!(
        fixture
            .store
            .read_resume_pointer()
            .expect("读取恢复指针")
            .is_none(),
        "指向已删会话的恢复指针必须一并清掉"
    );

    // 磁盘产物同样回收：thread 分片、事件目录、归档目录与会话上下文目录。
    assert_eq!(report["threadShardsRemoved"], 1);
    assert!(
        JsonlThreadEventStore::thread_shard_paths(&fixture.root, &fixture.run_id)
            .expect("读取分片")
            .is_empty(),
        "thread 分片必须删除"
    );
    assert!(!events.exists(), "事件目录必须回收");
    assert!(!archive.exists(), "事件归档目录必须回收");
    assert!(!context.exists(), "会话上下文目录必须回收");
}

#[test]
fn a_running_turn_blocks_the_delete_and_leaves_everything_in_place() {
    let fixture = fixture("session-delete-running");
    seed_thread(&fixture);
    seed_session_family(&fixture, "正在跑的会话", "running");

    let result = delete(
        OutputFormat::Text,
        &fixture,
        fixture.session_id.as_str(),
        TargetKind::Session,
    );

    assert_ne!(result.exit_code, 0, "在途回合不得被删掉");
    assert!(result.stderr.contains("回合"), "{}", result.stderr);
    assert!(
        fixture.store.get_session(&fixture.session_id).is_ok(),
        "被拒绝时必须整体回滚，会话行原样保留"
    );
    assert!(
        !JsonlThreadEventStore::thread_shard_paths(&fixture.root, &fixture.run_id)
            .expect("读取分片")
            .is_empty(),
        "被拒绝时 thread 分片不得被删"
    );
    let rows = read_session_rows(&fixture.store).expect("读取会话清单");
    assert_eq!(rows.len(), 1, "会话仍在清单里");
}

#[test]
fn the_thread_entry_deletes_the_session_that_owns_the_run_window() {
    // Thread 级入口只认运行窗口；那个窗口正是某条会话的当前运行时，只删分片会留下
    // 指向空分片的 `current_run_id`，因此整族一起删。
    let fixture = fixture("session-delete-thread");
    seed_thread(&fixture);
    seed_session_family(&fixture, "按 thread 删", "completed");

    let result = delete(
        OutputFormat::Json,
        &fixture,
        &fixture.run_id,
        TargetKind::Thread,
    );

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(report_of(&result)["sessionRowDeleted"], true);
    assert!(
        fixture.store.get_session(&fixture.session_id).is_err(),
        "所属会话必须一并删除"
    );
}

#[test]
fn unknown_session_fails_without_touching_anything() {
    let fixture = fixture("session-delete-known");

    let result = delete(
        OutputFormat::Text,
        &fixture,
        "session-does-not-exist",
        TargetKind::Session,
    );

    assert_ne!(result.exit_code, 0);
    assert!(result.stderr.contains("未找到会话"), "{}", result.stderr);
}

#[test]
fn text_output_reports_what_was_deleted() {
    let fixture = fixture("session-delete-report");
    seed_thread(&fixture);
    seed_session_family(&fixture, "报告口径", "completed");
    seed_session_dirs(&fixture);

    let result = delete(
        OutputFormat::Text,
        &fixture,
        fixture.session_id.as_str(),
        TargetKind::Session,
    );

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    for expected in [
        "已删除会话",
        "sessions: 1",
        "inbox: 1",
        "session_metrics: 1",
        "thread_shards_removed: 1",
        "event_log_removed: true",
    ] {
        assert!(
            result.stdout.contains(expected),
            "{expected} 缺失：{}",
            result.stdout
        );
    }
    assert!(
        result.stderr.is_empty(),
        "无残留时不该有告警：{}",
        result.stderr
    );
}

/// 会话数据根只是 `<root>/data`：用例里显式断言一次，避免路径解析漂移后测试变成空跑。
#[test]
fn fixture_keeps_the_runtime_roots_apart() {
    let fixture = fixture("session-delete-roots");
    assert_eq!(fixture.data_root, Path::new(&fixture.root).join("data"));
    assert_eq!(
        fixture.store.path().to_path_buf(),
        fixture.data_root.join("control.sqlite3")
    );
    assert_eq!(
        fixture.store.events_root().to_path_buf(),
        fixture.data_root.join("events")
    );
}
