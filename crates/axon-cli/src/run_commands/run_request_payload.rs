use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU8, Ordering},
    thread,
};

use axon_agent_runtime::{
    AgentModelAttachmentProjection, AgentModelMessage, AgentModelPayload, AgentModelStablePrefix,
    AttachmentBlobStore, AttachmentIngressRuntime, LedgerContextProjection, ToolRegistry,
    default_attachment_archive_root,
};
use axon_config::AxonConfig;
use axon_domain::{DisclosurePolicy, GoalStatus, ScopeRef};
use axon_session::{
    AXON_RUNTIME_DATA_DIR, DecisionStore, GoalStore, ResultStore, RuntimeStore, SqliteRuntimeStore,
    WorkspaceResolver, WorkspaceSnapshot,
};
use axon_session_runtime::{TurnPerformanceRecorder, TurnPerformanceStage};

use super::run_memory_recall::{RunMemoryRecallEvent, memory_scope, prepare_memory_recall_for_run};
use super::run_model_payload::{
    CliRunModelPayloadInput, CliRunSkillSelection, build_cli_run_model_payload,
    prepare_cli_run_skill_context,
};
use super::{RunCommand, RunStartupSnapshot};
use crate::runtime_user_soul::load_user_soul_message;

const LOCAL_UPLOAD_SOURCE_PREFIX: &str = "local-upload://";
const LEDGER_ACTIVE_GOAL_LIMIT: u32 = 32;
const LEDGER_UNREAD_RESULT_LIMIT: u32 = 16;
const WORKSPACE_PREWARM_TASK: &str = "Workspace snapshot 预热";
type WorkspaceSnapshotResult = Result<Option<WorkspaceSnapshot>, String>;

#[path = "run_request_payload_workspace_prewarm.rs"]
mod workspace_prewarm;
pub(super) use workspace_prewarm::RunWorkspaceSnapshotPrewarm;

pub(super) struct BuildRunModelPayloadInput<'a> {
    pub(super) config_path: &'a Path,
    pub(super) config: &'a AxonConfig,
    pub(super) cwd: &'a Path,
    pub(super) command: &'a RunCommand,
    pub(super) startup: &'a RunStartupSnapshot,
    /// 本次请求使用的稳定前缀：主 turn 传主 turn 前缀，子代理传子代理前缀。
    pub(super) stable_prefix: &'a AgentModelStablePrefix,
    pub(super) prompt_for_model: &'a str,
    pub(super) session_id: &'a str,
    pub(super) model_history_json: Option<&'a str>,
    pub(super) attached_files: &'a [PathBuf],
    pub(super) supports_vision: bool,
    pub(super) context_window: u64,
    pub(super) include_user_soul: bool,
    pub(super) include_personal_memory: bool,
    pub(super) memory_disclosure_policy: DisclosurePolicy,
    pub(super) performance: Option<&'a dyn TurnPerformanceRecorder>,
    pub(super) workspace_snapshot_prewarm: Option<RunWorkspaceSnapshotPrewarm>,
}

pub(super) struct BuildRunModelPayloadOutput {
    pub(super) model_payload: AgentModelPayload,
    pub(super) skill_selection: CliRunSkillSelection,
    pub(super) memory_recall: Option<RunMemoryRecallEvent>,
    pub(super) memory_disclosure_policy: DisclosurePolicy,
}

pub(super) fn build_run_model_payload(
    input: &BuildRunModelPayloadInput<'_>,
    registry: &ToolRegistry,
) -> Result<BuildRunModelPayloadOutput, String> {
    let skill_soul_ready = AtomicU8::new(0);
    let (workspace_snapshot, memory_recall, skill_context, soul_context_message) =
        thread::scope(|scope| {
            let skill = scope.spawn(|| {
                let result = prepare_cli_run_skill_context(
                    input.startup,
                    input.prompt_for_model,
                    input.context_window,
                );
                record_pair_ready(input.performance, &skill_soul_ready);
                result
            });
            let soul = scope.spawn(|| {
                let result = if input.include_user_soul {
                    load_user_soul_message()
                } else {
                    Ok(None)
                };
                record_pair_ready(input.performance, &skill_soul_ready);
                result
            });
            // 异步预热 Provider 连接池与 TLS 运行环境，使网络建连与本地 Memory 召回并行
            let _prewarm_handle = scope.spawn(|| {
                axon_rustls_provider::ensure_rustls_crypto_provider();
            });
            let workspace_snapshot = match input.workspace_snapshot_prewarm.as_ref() {
                Some(prewarm) => prewarm.resolve()?,
                None => resolve_run_workspace_snapshot_with_telemetry(
                    input.config_path,
                    input.cwd,
                    input.performance,
                )?,
            };
            let memory_recall = prepare_memory_recall_for_run(
                input.config_path,
                input.config,
                input.prompt_for_model,
                input.command,
                memory_scope(workspace_snapshot.as_ref()),
                input.performance,
            );
            record_stage(input.performance, TurnPerformanceStage::MemoryReady);
            Ok::<_, String>((
                workspace_snapshot,
                memory_recall,
                join_dynamic(skill, "Skill route")?,
                join_dynamic(soul, "Soul load")??,
            ))
        })?;
    let ledger_context =
        rehydrate_ledger_context_with_snapshot(input.config_path, workspace_snapshot.as_ref())?;
    record_stage(input.performance, TurnPerformanceStage::LedgerReady);
    let (model_payload, skill_selection) = build_cli_run_model_payload(
        CliRunModelPayloadInput {
            memory_hits: &memory_recall.hits,
            soul_context_message: soul_context_message.as_ref(),
            include_personal_memory: input.include_personal_memory,
            memory_disclosure_policy: input.memory_disclosure_policy,
            cwd: &input.cwd.to_string_lossy(),
            workspace_snapshot: workspace_snapshot.as_ref(),
            startup: input.startup,
            stable_prefix: input.stable_prefix,
            context_window: input.context_window,
            max_subagent_threads: input.config.agent.max_subagent_threads,
            max_subagent_active_turns: input.config.agent.max_subagent_active_turns,
            model_history_json: input.model_history_json,
            ledger_context: ledger_context.as_ref(),
        },
        skill_context,
        registry,
    )?;
    let model_payload = attach_vision_files_to_payload(
        model_payload,
        input.attached_files,
        input.session_id,
        input.supports_vision,
    )?;
    record_stage(input.performance, TurnPerformanceStage::PayloadReady);
    Ok(BuildRunModelPayloadOutput {
        model_payload,
        skill_selection,
        memory_recall: memory_recall.event,
        memory_disclosure_policy: input.memory_disclosure_policy,
    })
}

fn record_stage(performance: Option<&dyn TurnPerformanceRecorder>, stage: TurnPerformanceStage) {
    if let Some(performance) = performance {
        performance.record(stage);
    }
}

fn record_pair_ready(performance: Option<&dyn TurnPerformanceRecorder>, ready: &AtomicU8) {
    if performance.is_some() && ready.fetch_add(1, Ordering::Relaxed) == 1 {
        record_stage(performance, TurnPerformanceStage::SkillSoulReady);
    }
}

fn record_workspace_git_probes(
    performance: Option<&dyn TurnPerformanceRecorder>,
    snapshot: Option<&WorkspaceSnapshot>,
) {
    let (Some(performance), Some(snapshot)) = (performance, snapshot) else {
        return;
    };
    performance.record_workspace_git_probes(
        u64::try_from(snapshot.git.remote_probe_count).unwrap_or(u64::MAX),
        u64::try_from(snapshot.git.status_probe_count).unwrap_or(u64::MAX),
    );
}

fn resolve_run_workspace_snapshot_with_telemetry(
    config_path: &Path,
    cwd: &Path,
    performance: Option<&dyn TurnPerformanceRecorder>,
) -> WorkspaceSnapshotResult {
    let snapshot = resolve_run_workspace_snapshot(config_path, cwd)?;
    record_workspace_git_probes(performance, snapshot.as_ref());
    record_stage(performance, TurnPerformanceStage::WorkspaceReady);
    Ok(snapshot)
}

fn resolve_run_workspace_snapshot(
    config_path: &Path,
    cwd: &Path,
) -> Result<Option<WorkspaceSnapshot>, String> {
    let Some(workspace_root) = crate::runtime_tools::resolve_workspace_cwd(config_path, Some(cwd))?
    else {
        return Ok(None);
    };
    WorkspaceResolver
        .resolve_snapshot(&workspace_root, axon_time::unix_now_ms())
        .map(Some)
        .map_err(|error| format!("Workspace 解析失败：{error}"))
}

fn join_dynamic<T>(handle: thread::ScopedJoinHandle<'_, T>, goal: &str) -> Result<T, String> {
    handle
        .join()
        .map_err(|_| format!("{goal} 动态准备任务异常退出"))
}

fn attach_vision_files_to_payload(
    payload: AgentModelPayload,
    files: &[PathBuf],
    session_id: &str,
    supports_vision: bool,
) -> Result<AgentModelPayload, String> {
    let archive_root =
        default_attachment_archive_root().ok_or_else(|| "无法解析附件归档根目录".to_owned())?;
    attach_vision_files_to_payload_with_archive_root(
        payload,
        files,
        session_id,
        supports_vision,
        &archive_root,
    )
}

fn attach_vision_files_to_payload_with_archive_root(
    mut payload: AgentModelPayload,
    files: &[PathBuf],
    session_id: &str,
    supports_vision: bool,
    archive_root: &Path,
) -> Result<AgentModelPayload, String> {
    let mut ingress = AttachmentIngressRuntime::default();
    let mut store = AttachmentBlobStore::new(archive_root);
    let mut images = Vec::new();
    let mut local_files = Vec::new();
    for (index, path) in files.iter().enumerate() {
        let mime_hint = image_mime_type(path);
        if mime_hint.is_some() && !supports_vision {
            continue;
        }
        let bytes = std::fs::read(path).map_err(|error| format!("读取附件失败：{error}"))?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("附件文件名无效：{}", path.display()))?
            .to_owned();
        let record = ingress
            .ingest_local_upload_with_blob_store(
                axon_agent_runtime::AttachmentLocalUploadRequest {
                    file_name: file_name.clone(),
                    mime_hint: mime_hint.map(ToOwned::to_owned),
                    bytes,
                    source_trace: local_upload_source_trace(session_id, index),
                    sensitivity: axon_domain::AttachmentSensitivity::Normal,
                },
                &mut store,
                0,
            )
            .map_err(|error| error.to_string())?;
        match AttachmentIngressRuntime::project_for_model(&record.attachment)
            .map_err(|error| error.to_string())?
        {
            AgentModelAttachmentProjection::Image(image) if supports_vision => images.push(image),
            AgentModelAttachmentProjection::Image(_) => {}
            AgentModelAttachmentProjection::File(mut file) => {
                file.name = file_name;
                // 原始绝对路径随引用一起交给模型：预览被截断时它才是读取原文件的正路。
                file.path = Some(path.to_string_lossy().to_string());
                local_files.push(file);
            }
        }
    }
    if images.is_empty() && local_files.is_empty() {
        return Ok(payload);
    }
    if let Some(message) = payload
        .messages
        .iter_mut()
        .rev()
        .find(|message| message.role == "user")
    {
        *message = message
            .clone()
            .with_local_images(images)
            .with_local_files(local_files);
    } else {
        payload.messages.push(
            AgentModelMessage::text("user", "")
                .with_local_images(images)
                .with_local_files(local_files),
        );
    }
    Ok(payload)
}

fn local_upload_source_trace(session_id: &str, index: usize) -> String {
    format!("{LOCAL_UPLOAD_SOURCE_PREFIX}{session_id}/{index}")
}

fn image_mime_type(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn rehydrate_ledger_context(
    config_path: &Path,
    cwd: &Path,
) -> Result<Option<LedgerContextProjection>, String> {
    let snapshot = resolve_run_workspace_snapshot(config_path, cwd)?;
    rehydrate_ledger_context_with_snapshot(config_path, snapshot.as_ref())
}

fn rehydrate_ledger_context_with_snapshot(
    config_path: &Path,
    snapshot: Option<&WorkspaceSnapshot>,
) -> Result<Option<LedgerContextProjection>, String> {
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    let axon_root = super::resolved_axon_root(config_path).map_err(ToOwned::to_owned)?;
    let runtime_root = axon_root.join(AXON_RUNTIME_DATA_DIR);
    let now_ms = axon_time::unix_now_ms();
    let scope = ScopeRef::workspace(snapshot.workspace.workspace_id.clone())
        .map_err(|error| format!("Workspace 作用域无效：{error}"))?;
    let store = SqliteRuntimeStore::from_root(runtime_root);
    store
        .initialize()
        .map_err(|error| format!("初始化 Ledger 事实源失败：{error}"))?;
    let active_goals = store
        .list_active_goals_for_scope(&scope, LEDGER_ACTIVE_GOAL_LIMIT)
        .map_err(|error| format!("读取 active Goal 失败：{error}"))?
        .into_iter()
        .filter(|goal| goal.status == GoalStatus::Active)
        .collect();
    let waiting_decision = store
        .get_latest_decision(&scope, now_ms)
        .map_err(|error| format!("读取最新 Decision 失败：{error}"))?;
    let unread_results = store
        .list_unread_results(&scope, LEDGER_UNREAD_RESULT_LIMIT)
        .map_err(|error| format!("读取未读 Result 失败：{error}"))?;
    Ok(Some(LedgerContextProjection {
        scope,
        now_ms,
        active_goals,
        waiting_decision,
        unread_results,
    }))
}

#[cfg(test)]
#[path = "run_request_payload_contract_tests.rs"]
mod contract_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use axon_session::{ExecutionStore, RuntimeStore};
    use axon_session_runtime::AtomicTurnPerformanceRecorder;
    use std::sync::Arc;

    #[test]
    fn workspace_snapshot_prewarm_preserves_snapshot_and_probe_contract() {
        let root = axon_test_support::test_temp_dir();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&home).expect("创建 Home 根目录");
        std::fs::create_dir_all(&workspace).expect("创建 Workspace");
        let config_path = home.join("config.toml");
        let status = axon_process::non_interactive_command("git")
            .args(["init", "-q"])
            .current_dir(&workspace)
            .status()
            .expect("Git 可执行");
        assert!(status.success(), "初始化 Git Workspace 失败");
        let performance = Arc::new(AtomicTurnPerformanceRecorder::new());

        let prewarm = RunWorkspaceSnapshotPrewarm::spawn(
            config_path,
            workspace.clone(),
            Some(performance.clone()),
        );
        let snapshot = prewarm
            .resolve()
            .expect("预热任务成功")
            .expect("应解析 Workspace");

        assert_eq!(snapshot.workspace_root, workspace.canonicalize().unwrap());
        assert_eq!(snapshot.git.remote_probe_count, 1);
        assert_eq!(snapshot.git.status_probe_count, 1);
        let telemetry = performance.snapshot();
        assert_eq!(telemetry.workspace_git_remote_probe_count, 1);
        assert_eq!(telemetry.workspace_git_status_probe_count, 1);
        assert!(
            telemetry
                .stages
                .iter()
                .any(|timing| timing.stage == TurnPerformanceStage::WorkspaceReady)
        );
    }

    #[test]
    fn vision_supported_payload_attaches_local_images() {
        let image_path = axon_test_support::test_temp_root().join(format!(
            "axon-tui-vision-payload-{}.png",
            std::process::id()
        ));
        std::fs::write(&image_path, b"\x89PNG\r\n\x1a\n").unwrap();
        let payload = AgentModelPayload {
            messages: vec![AgentModelMessage::text("user", "分析图片")],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        };

        let archive_root = axon_test_support::test_temp_root()
            .join(format!("axon-tui-vision-archive-{}", std::process::id()));
        let payload = attach_vision_files_to_payload_with_archive_root(
            payload,
            std::slice::from_ref(&image_path),
            "vision-session",
            true,
            &archive_root,
        )
        .unwrap();

        assert_eq!(payload.messages[0].local_images.len(), 1);
        assert_eq!(payload.messages[0].local_images[0].mime_type, "image/png");
        assert!(payload.messages[0].local_images[0].path.is_empty());
        assert!(payload.messages[0].local_images[0].archive_ref.is_some());
        assert_eq!(
            payload.messages[0].local_images[0].source_ref.as_deref(),
            Some("local-upload://vision-session/0")
        );
        let _ = std::fs::remove_file(image_path);
        let _ = std::fs::remove_dir_all(archive_root);
    }

    #[test]
    fn text_only_model_keeps_file_attachment_with_real_name() {
        let root = axon_test_support::test_temp_dir();
        let file_path = root.path().join("report.txt");
        std::fs::write(&file_path, "生产文件正文").unwrap();
        let archive_root = root.path().join("archive");
        let payload = AgentModelPayload {
            messages: vec![AgentModelMessage::text("user", "")],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        };

        let payload = attach_vision_files_to_payload_with_archive_root(
            payload,
            std::slice::from_ref(&file_path),
            "file-session",
            false,
            &archive_root,
        )
        .unwrap();

        let file = &payload.messages[0].local_files[0];
        assert_eq!(file.name, "report.txt");
        assert_eq!(file.preview, "生产文件正文");
        assert_eq!(
            file.source_ref.as_deref(),
            Some("local-upload://file-session/0")
        );
        assert!(file.archive_ref.is_some());
    }

    #[test]
    fn ledger_rehydrate_reads_scoped_facts_after_restart() {
        let root = axon_test_support::test_temp_dir();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&home).expect("创建 Home 根目录");
        std::fs::create_dir_all(workspace.join(".git")).expect("创建 Git 标记");
        let config_path = home.join("config.toml");
        let store = SqliteRuntimeStore::from_root(home.join(AXON_RUNTIME_DATA_DIR));
        let scope = ScopeRef::workspace(
            WorkspaceResolver
                .resolve(&workspace, 10)
                .expect("解析 Workspace")
                .workspace_id,
        )
        .expect("构造作用域");
        let mut goal = axon_domain::GoalRecord::new(
            axon_domain::GoalId::new("rehydrate-goal").expect("Goal ID"),
            scope.clone(),
            "真实任务",
            axon_domain::GoalPlan {
                objective: "从 SQLite 恢复".to_owned(),
                steps: vec!["读取事实源".to_owned()],
            },
            vec!["不使用摘要推断".to_owned()],
            10,
        )
        .expect("Goal");
        store.create_goal(&goal).expect("创建 Goal");
        goal.transition(GoalStatus::Active, 11).expect("激活 Goal");
        store.update_goal(&goal, 0).expect("更新 Goal");

        let decision = axon_domain::DecisionRecord {
            decision_id: axon_domain::DecisionId::new("rehydrate-decision").expect("Decision ID"),
            scope: scope.clone(),
            actor: "user".to_owned(),
            effect: axon_domain::DecisionEffect::Allow,
            revision: 1,
            expires_at_ms: None,
            created_at_ms: 11,
        };
        store.append_decision(&decision, 0).expect("写入 Decision");

        let execution = axon_domain::ExecutionRecord::new(
            axon_domain::ExecutionId::new("rehydrate-execution").expect("Execution ID"),
            goal.goal_id.clone(),
            12,
        );
        store.enqueue_execution(&execution).expect("创建 Execution");
        let result = axon_domain::ResultRecord::success(
            axon_domain::ResultId::new("rehydrate-result").expect("Result ID"),
            goal.goal_id.clone(),
            execution.execution_id.clone(),
            "真实结果",
            13,
        );
        store.initialize().expect("初始化数据库");
        let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
        connection
            .execute(
                "INSERT INTO jarvis_results(result_id,goal_id,execution_id,succeeded,summary,receipt_refs_json,artifact_refs_json,error_kind,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,?8)",
                rusqlite::params![
                    result.result_id.as_str(),
                    result.goal_id.as_str(),
                    result.execution_id.as_str(),
                    result.succeeded,
                    result.summary,
                    serde_json::to_string(&result.receipt_refs).expect("回执 JSON"),
                    serde_json::to_string(&result.artifact_refs).expect("产物 JSON"),
                    i64::try_from(result.created_at_ms).expect("时间戳"),
                ],
            )
            .expect("写入 Result");

        let projection = rehydrate_ledger_context(&config_path, &workspace)
            .expect("重新装配 Ledger")
            .expect("应有 Ledger 投影");
        assert_eq!(projection.active_goals, vec![goal]);
        assert_eq!(projection.waiting_decision, Some(decision));
        assert_eq!(projection.unread_results, vec![result]);
    }

    #[test]
    fn ledger_rehydrate_skips_home_without_workspace() {
        let root = axon_test_support::test_temp_dir();
        let home = root.path().join("home");
        let cwd = home.join("data");
        std::fs::create_dir_all(&cwd).expect("创建 Home 子目录");
        let config_path = home.join("config.toml");

        let projection =
            rehydrate_ledger_context(&config_path, &cwd).expect("Home 目录不应阻断 Ledger 读取");

        assert_eq!(projection, None);
    }

    #[test]
    fn ledger_rehydrate_acknowledges_unread_results() {
        let root = axon_test_support::test_temp_dir();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&home).expect("创建 Home 根目录");
        std::fs::create_dir_all(workspace.join(".git")).expect("创建 Git 标记");
        let config_path = home.join("config.toml");
        let store = SqliteRuntimeStore::from_root(home.join(AXON_RUNTIME_DATA_DIR));
        let scope = ScopeRef::workspace(
            WorkspaceResolver
                .resolve(&workspace, 10)
                .expect("解析 Workspace")
                .workspace_id,
        )
        .expect("构造作用域");
        let mut goal = axon_domain::GoalRecord::new(
            axon_domain::GoalId::new("rehydrate-goal").expect("Goal ID"),
            scope.clone(),
            "真实任务",
            axon_domain::GoalPlan {
                objective: "从 SQLite 恢复".to_owned(),
                steps: vec!["读取事实源".to_owned()],
            },
            vec!["不使用摘要推断".to_owned()],
            10,
        )
        .expect("Goal");
        store.create_goal(&goal).expect("创建 Goal");
        goal.transition(GoalStatus::Active, 11).expect("激活 Goal");
        store.update_goal(&goal, 0).expect("更新 Goal");

        let decision = axon_domain::DecisionRecord {
            decision_id: axon_domain::DecisionId::new("rehydrate-decision").expect("Decision ID"),
            scope: scope.clone(),
            actor: "user".to_owned(),
            effect: axon_domain::DecisionEffect::Allow,
            revision: 1,
            expires_at_ms: None,
            created_at_ms: 11,
        };
        store.append_decision(&decision, 0).expect("写入 Decision");

        let execution = axon_domain::ExecutionRecord::new(
            axon_domain::ExecutionId::new("rehydrate-execution").expect("Execution ID"),
            goal.goal_id.clone(),
            12,
        );
        store.enqueue_execution(&execution).expect("创建 Execution");
        let result = axon_domain::ResultRecord::success(
            axon_domain::ResultId::new("rehydrate-result").expect("Result ID"),
            goal.goal_id.clone(),
            execution.execution_id.clone(),
            "真实结果",
            13,
        );
        store.initialize().expect("初始化数据库");
        let connection = rusqlite::Connection::open(store.path()).expect("打开数据库");
        connection
            .execute(
                "INSERT INTO jarvis_results(result_id,goal_id,execution_id,succeeded,summary,receipt_refs_json,artifact_refs_json,error_kind,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,?8)",
                rusqlite::params![
                    result.result_id.as_str(),
                    result.goal_id.as_str(),
                    result.execution_id.as_str(),
                    result.succeeded,
                    result.summary,
                    serde_json::to_string(&result.receipt_refs).expect("回执 JSON"),
                    serde_json::to_string(&result.artifact_refs).expect("产物 JSON"),
                    i64::try_from(result.created_at_ms).expect("时间戳"),
                ],
            )
            .expect("写入 Result");

        let projection = rehydrate_ledger_context(&config_path, &workspace)
            .expect("rehydrate 必须成功")
            .expect("projection 必须存在");
        assert_eq!(projection.unread_results, vec![result]);
    }
}
