//! 工作台子代理名册的 HTTP 载荷。
//!
//! 名册只回答一件事：这条车道下有哪些**用户可见的子代理会话**、它们此刻在不在跑。
//! 状态取自运行态轮次头，不取 JSONL 索引缓存（索引里的 `active` 在子会话收口后仍会停留）。

use std::collections::HashMap;
use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{
    InteractionQueryStore, RosterLastTurnStatus, RosterTurnStatus, RuntimeStore, RuntimeStoreError,
    SessionRuntimeRecord, SqliteRuntimeStore,
};

use super::runtime_store;
use crate::run_commands::run_thread_children::{child_session_rows, is_internal_agent_path};

/// 子代理会话在名册里的 slug：代理路径去掉开头斜杠、层级用下划线连接。
///
/// slug 只用于展示与前端对账，不参与任何查找：子会话身份是 `(root_session_id, agent_path)`。
fn slug_of_agent_path(agent_path: &str) -> String {
    agent_path.trim().trim_start_matches('/').replace('/', "_")
}

/// 一次车道子代理列举的上限。
const MAX_SESSION_SUBAGENTS: usize = 200;

/// 名册派生状态时回看的轮次头数量。
///
/// 名册只需要回答「最新一轮还在不在跑」，而 `interaction_turn_heads` 已按接收时间倒序，
/// 取一条就够：每个子会话一次点查，不会随历史长度放大。
const ROSTER_TURN_WINDOW: u32 = 1;

/// 运行侧终态到名册状态的归一。
///
/// 运行侧把失败写成 `errored`，而名册对外只给界面认得的取值（`failed` 才是失败）。
/// 其余取值原样透传，未知与缺失都读作未知，不在这里猜成已完成。
///
/// 它现在只服务 `threadTurnStatus` 这一列对账旁证：名册状态本身由运行态轮次头派生，
/// 这条从 JSONL 索引读到的末轮状态**不再**参与 `active`/`idle` 的判断。
fn normalize_last_turn_status(status: Option<&str>) -> Option<&str> {
    status.map(|value| if value == "errored" { "failed" } else { value })
}

/// 子会话的运行态：由它自己的轮次头派生。
///
/// 事实源是运行态库里的 `interaction_turn_heads`，不是 JSONL 索引缓存的末轮状态：
/// 索引里的 `active` 在子会话收口后仍然停留，据此判「一直在跑」永远退不出来。
/// 读不到运行态（没有会话行、或轮次头解码失败）时按「没有在途轮次」处理：名册只用来
/// 回答「还占不占着车道」，读不到就不阻止界面纠正，绝不因为索引写着 `active` 就保持
/// 运行中。
fn roster_turn_status(
    store: &SqliteRuntimeStore,
    runtime: Option<&SessionRuntimeRecord>,
) -> RosterTurnStatus {
    let Some(record) = runtime else {
        return RosterTurnStatus::default();
    };
    store
        .interaction_turn_heads(record.session_id.as_str(), ROSTER_TURN_WINDOW)
        .map_or_else(
            |_| RosterTurnStatus::default(),
            |heads| RosterTurnStatus::from_turn_heads(&heads),
        )
}

/// 车道的运行会话 ID；子代理会话挂在它下面。
fn session_run_session_id(axon_root: &Path, session_id: &str) -> Result<String, String> {
    let store = runtime_store(axon_root);
    let session_id =
        DaemonSessionId::new(session_id).map_err(|error| format!("车道标识非法：{error:?}"))?;
    match store.get_session(&session_id) {
        Ok(record) => Ok(record.current_run_id),
        Err(RuntimeStoreError::NotFound(_)) => Ok(String::new()),
        Err(error) => Err(format!("读取车道运行会话失败：{error:?}")),
    }
}

/// 某条车道下的子代理会话清单。
pub(super) fn session_subagents_payload(
    axon_root: &Path,
    session_id: &str,
) -> Result<serde_json::Value, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("车道标识不能为空".to_string());
    }
    let run_session_id = session_run_session_id(axon_root, session_id)?;
    if run_session_id.is_empty() {
        return Ok(serde_json::json!({
            "sessionId": session_id,
            "runSessionId": serde_json::Value::Null,
            "subagents": [],
        }));
    }
    let rows = child_session_rows(axon_root, &run_session_id, MAX_SESSION_SUBAGENTS)?;
    let store = runtime_store(axon_root);
    // 名册的 thread 标识是运行实例标识（`current_run_id`），会话行按稳定会话标识
    // （ULID）存放：两套标识不能直接对表，先按实例反查会话，再批量读回会话行。
    // 直接命中保留给「thread 标识就是会话标识」的旧记录。
    let mut session_of_thread: HashMap<String, String> = HashMap::new();
    let direct_ids: Vec<DaemonSessionId> = rows
        .iter()
        .filter_map(|row| DaemonSessionId::new(row.session_id.clone()).ok())
        .collect();
    let mut runtimes = store.sessions_by_ids(&direct_ids).unwrap_or_default();
    for row in &rows {
        if runtimes.contains_key(row.session_id.as_str()) {
            session_of_thread.insert(row.session_id.clone(), row.session_id.clone());
            continue;
        }
        // 实例标识查不到就是没有运行态会话：如实留空，不猜一个会话标识出来。
        if let Ok(Some(session_id)) = store.session_id_for_run(&row.session_id) {
            session_of_thread.insert(row.session_id.clone(), session_id.to_string());
        }
    }
    let resolved_ids: Vec<DaemonSessionId> = session_of_thread
        .values()
        .filter(|session_id| !runtimes.contains_key(session_id.as_str()))
        .filter_map(|session_id| DaemonSessionId::new(session_id.clone()).ok())
        .collect();
    if !resolved_ids.is_empty() {
        runtimes.extend(store.sessions_by_ids(&resolved_ids).unwrap_or_default());
    }
    let mut subagents = Vec::with_capacity(rows.len());
    for row in rows {
        // 会话标识是 ULID，路径只从登记表读：子代理的 slug 不是标识的一部分。
        let runtime = session_of_thread
            .get(&row.session_id)
            .and_then(|session_id| runtimes.get(session_id.as_str()));
        // 运行侧内务子代理（自迭代、反思、语义显著度）由轮后治理派生，不是用户要的
        // 委派：它们没有可进入的逐字稿，出现在名册里只会得到一条空视图。
        if runtime
            .and_then(|record| record.agent_path.as_deref())
            .is_some_and(is_internal_agent_path)
        {
            continue;
        }
        let slug = runtime
            .as_ref()
            .and_then(|record| record.agent_path.as_deref())
            .map(slug_of_agent_path)
            .unwrap_or_default();
        let has_runtime = runtime.is_some();
        // 状态列存的是会话自己的终态：在跑、完成、中断、失败。界面用它回答
        // 「这条子代理还在跑吗」，但它来自 JSONL 索引缓存，只作对账旁证：
        // 名册的在跑判定改由运行态轮次头派生，收口后的会话不会再被索引拖着显示在跑。
        let thread_turn_status = normalize_last_turn_status(row.status.as_deref());
        let turn_status = roster_turn_status(&store, runtime);
        // 匹配键只给可直接使用的那两个：gentPath 是路径本身，gentId 是它带前缀的
        // 稳定标识；没有运行态会话行时 sessionId 如实为 null 并给出原因，调用方不必猜。
        let agent_path = runtime
            .as_ref()
            .and_then(|record| record.agent_path.clone());
        let agent_id = agent_path.as_ref().map(|path| format!("subagent:{path}"));
        // 对外只给稳定会话标识：运行实例标识只在名册内部用来对齐 thread 行。
        let session_id = if has_runtime {
            session_of_thread.get(&row.session_id).cloned()
        } else {
            None
        };
        subagents.push(serde_json::json!({
            "sessionId": session_id.map_or(serde_json::Value::Null, serde_json::Value::String),
            "agentPath": agent_path,
            "agentId": agent_id,
            "unavailableReason": if has_runtime {
                serde_json::Value::Null
            } else {
                serde_json::Value::String("no-runtime-session".to_owned())
            },
            "slug": slug,
            "title": row.title,
            "model": row.model,
            "provider": row.provider,
            "updatedAtMs": row.updated_at_ms,
            "hasRuntime": has_runtime,
            "status": turn_status.status.as_str(),
            "lastTurnStatus": turn_status
                .last_turn_status
                .map(RosterLastTurnStatus::as_str),
            "threadTurnStatus": thread_turn_status,
            "lastTurnAtMs": row.last_turn_at_ms,
        }));
    }
    Ok(serde_json::json!({
        "sessionId": session_id,
        "runSessionId": run_session_id,
        "subagents": subagents,
    }))
}

#[cfg(test)]
#[path = "workbench_http_subagents_tests.rs"]
mod workbench_http_subagents_tests;

#[cfg(test)]
mod roster_turn_heads_tests {
    //! 名册状态必须来自运行态轮次头，而不是 JSONL 索引缓存的末轮状态。

    use axon_domain::{InteractionId, InteractionRecord};
    use axon_session::{
        InteractionStore as _, JsonlThreadEventStore, NewSessionRequest, SessionKind,
        SessionTrustScope, StartEmptyThreadCommand, StartTurnCommand, ThreadService,
        ThreadSettings, ThreadSource, Turn, TurnStatus, new_interaction_id, new_session_id,
    };

    use super::*;

    /// 建一条 thread 并留一轮 Active：这就是现场「永远停在 Active」的 JSONL 形状。
    fn active_thread(root: &Path, thread_id: &str, parent_thread_id: Option<&str>) {
        let service = ThreadService::new(JsonlThreadEventStore);
        service
            .start_empty_thread(
                root,
                StartEmptyThreadCommand {
                    thread_id: thread_id.to_owned(),
                    session_root_id: parent_thread_id.unwrap_or(thread_id).to_owned(),
                    parent_thread_id: parent_thread_id.map(ToOwned::to_owned),
                    forked_from_thread_id: None,
                    source: ThreadSource::CliRun,
                    ephemeral: false,
                    settings: ThreadSettings {
                        cwd: ".".to_owned(),
                        provider: "fixture".to_owned(),
                        model: "fixture-chat".to_owned(),
                        selected_skill_name: None,
                    },
                    created_at_ms: 10,
                },
            )
            .expect("建 thread");
        service
            .start_turn(
                root,
                StartTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms: 10,
                    turn: Turn {
                        turn_id: format!("{thread_id}-turn-1"),
                        sequence: 0,
                        status: TurnStatus::Active,
                        history_items: Vec::new(),
                        token_usage_json: None,
                        started_at_ms: 10,
                        completed_at_ms: None,
                        interrupted_at_ms: None,
                        failure: None,
                    },
                },
            )
            .expect("写 Active 轮次");
    }

    /// 给子会话落一条派发轮（子代理轮次的真实形状：来源是 `agent`，没有工作项）。
    fn append_dispatch_interaction(
        store: &SqliteRuntimeStore,
        session_id: &DaemonSessionId,
        sequence: u32,
    ) -> InteractionId {
        let interaction = InteractionRecord::new(
            session_id.clone(),
            new_interaction_id(),
            axon_domain::InteractionSource::new(
                axon_domain::AGENT_DISPATCH_SOURCE_CHANNEL,
                format!(
                    "{}{}:{}",
                    axon_domain::AGENT_DISPATCH_SOURCE_PREFIX,
                    session_id.as_str(),
                    sequence
                ),
            )
            .expect("来源"),
            axon_domain::ScopeRef::workspace("workspace-roster".to_owned()).expect("作用域"),
            axon_domain::AudienceContext::new(
                axon_domain::AudienceKind::Private,
                axon_domain::DisclosurePolicy::PrivateOnly,
            ),
            axon_domain::ReplyRoute::new("agent", session_id.as_str()).expect("路由"),
            "整理这份材料",
            10,
        )
        .expect("派发轮");
        let interaction_id = interaction.interaction_id.clone();
        store.append(&interaction).expect("落派发轮");
        interaction_id
    }

    fn subagent_session(
        store: &SqliteRuntimeStore,
        parent: &DaemonSessionId,
        agent_path: &str,
    ) -> SessionRuntimeRecord {
        store
            .create_session_with(&NewSessionRequest {
                session_id: new_session_id(),
                kind: SessionKind::Subagent,
                trust_scope: SessionTrustScope::Private,
                workspace_id: None,
                root_session_id: Some(parent.clone()),
                agent_path: Some(agent_path.to_owned()),
                goal_id: None,
                execution_id: None,
                cwd: ".".to_owned(),
                now_ms: 10,
            })
            .expect("建子代理会话")
    }

    #[test]
    fn subagent_roster_status_comes_from_turn_heads() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-subagent-roster-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let store = runtime_store(&root);
        let parent = new_session_id();
        let parent_record = store.create_session(&parent, ".", 10).expect("父会话");
        active_thread(&root, &parent_record.current_run_id, None);

        // 收口的子代理：JSONL 索引仍写着 Active，运行态的派发轮已经结算为 understood。
        let settled = subagent_session(&store, &parent, "/root/settled");
        active_thread(
            &root,
            &settled.current_run_id,
            Some(&parent_record.current_run_id),
        );
        let settled_interaction = append_dispatch_interaction(&store, &settled.session_id, 0);
        store
            .settle_dispatch(
                &settled_interaction,
                axon_session::DispatchTerminalKind::Understood,
                20,
            )
            .expect("结算派发轮");

        // 仍在跑的子代理：派发轮没有任何终态。
        let running = subagent_session(&store, &parent, "/root/running");
        active_thread(
            &root,
            &running.current_run_id,
            Some(&parent_record.current_run_id),
        );
        let _running_interaction = append_dispatch_interaction(&store, &running.session_id, 1);

        let payload = session_subagents_payload(&root, parent.as_str()).expect("名册载荷");
        let subagents = payload["subagents"].as_array().expect("子代理数组");
        assert_eq!(subagents.len(), 2, "两条子代理都要出现在名册里");

        let of = |session_id: &DaemonSessionId| {
            subagents
                .iter()
                .find(|row| row["sessionId"].as_str() == Some(session_id.as_str()))
                .unwrap_or_else(|| panic!("名册缺少子代理：{}", session_id.as_str()))
        };

        let settled_row = of(&settled.session_id);
        assert_eq!(
            settled_row["status"].as_str(),
            Some("idle"),
            "运行态已收口，名册不得跟着索引说在跑"
        );
        assert_eq!(settled_row["lastTurnStatus"].as_str(), Some("completed"));
        assert_eq!(
            settled_row["threadTurnStatus"].as_str(),
            Some("active"),
            "索引侧的末轮状态只作对账旁证，必须如实透出它仍写着 active"
        );

        let running_row = of(&running.session_id);
        assert_eq!(
            running_row["status"].as_str(),
            Some("active"),
            "派发轮没有终态就是在跑"
        );
        assert_eq!(running_row["lastTurnStatus"].as_str(), Some("active"));
    }
}
