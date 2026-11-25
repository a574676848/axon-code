//! 工作台子代理名册的 HTTP 载荷。
//!
//! 名册只回答一件事：这条车道下有哪些**用户可见的子代理会话**、它们此刻在不在跑。
//! 状态取自运行态轮次头，不取 JSONL 索引缓存（索引里的 `active` 在子会话收口后仍会停留）。
//!
//! 名册的候选集合来自 `sessions` 的 `(kind, root_session_id)` 索引：一次查询拿到本车道
//! 已登记的子代理会话，成本与全局 thread 数无关。thread 侧的装饰性事实（标题、模型与
//! 末轮状态）按这些会话各自的运行实例标识定向读取，不再把整份 thread 派生索引读出来
//! 逐行过滤。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{
    InteractionQueryStore, JsonlThreadEventStore, RosterLastTurnStatus, RosterTurnStatus,
    RuntimeStore, RuntimeStoreError, SqliteRuntimeStore, TurnStatus,
};

use super::runtime_store;
use crate::run_commands::run_thread_children::is_internal_agent_path;

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

/// 末轮状态回到运行侧口径的字符串。
fn turn_status_text(status: TurnStatus) -> &'static str {
    match status {
        TurnStatus::Active => "active",
        TurnStatus::Completed => "completed",
        TurnStatus::Interrupted => "interrupted",
        TurnStatus::Failed => "failed",
    }
}

/// 一条子会话的 thread 侧事实：标题、模型与末轮状态。
struct ThreadFacts {
    title: Option<String>,
    provider: String,
    model: String,
    /// thread 事件里的末轮状态；只作对账旁证。
    last_turn_status: Option<String>,
    updated_at_ms: u64,
    last_turn_at_ms: Option<u64>,
}

/// 一次性读出全部 thread 侧事实，按 thread 标识索引。
///
/// 只读一次派生索引（按会话量线性、与子代理数无关），在内存里按 id 过滤；不要逐子会话
/// 打开各自的 thread 分片，那是「子代理数 × 各自历史长度」的整文件读加全量投影，
/// 在长会话上比名册本身贵两个数量级。
fn thread_facts_by_thread(axon_root: &Path) -> std::collections::HashMap<String, ThreadFacts> {
    let snapshot = match JsonlThreadEventStore.read_thread_index(axon_root) {
        Ok(Some(snapshot)) => snapshot,
        _ => return std::collections::HashMap::new(),
    };
    snapshot
        .entries
        .into_iter()
        .map(|entry| {
            let facts = ThreadFacts {
                title: entry.name,
                provider: entry.provider,
                model: entry.model,
                last_turn_status: entry
                    .last_turn_status
                    .map(turn_status_text)
                    .map(str::to_owned),
                updated_at_ms: entry.updated_at_ms,
                last_turn_at_ms: entry.last_turn_at_ms,
            };
            (entry.thread_id, facts)
        })
        .collect()
}

/// 车道下一条已登记的子代理会话。
///
/// 会话行同时给出稳定会话标识与运行实例标识：前者是名册对外的身份，后者只用于回显。
struct LaneChildSession {
    session_id: DaemonSessionId,
    agent_path: String,
    /// 运行实例标识：thread 侧事实按它从派生索引里取。
    run_id: String,
    /// 控制面登记过的标题；子代理会话一般不写这一列。
    stored_title: Option<String>,
    stored_provider: String,
    stored_model: String,
    last_activity_at_ms: u64,
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
    session_id: &DaemonSessionId,
) -> RosterTurnStatus {
    store
        .interaction_turn_heads(session_id.as_str(), ROSTER_TURN_WINDOW)
        .map_or_else(
            |_| RosterTurnStatus::default(),
            |heads| RosterTurnStatus::from_turn_heads(&heads),
        )
}

/// 本车道已登记的子代理会话；按最近活动倒序，条数有界。
///
/// 走 `sessions_by_kind_root`：`(kind, root_session_id)` 两列一次定位，不读任何 thread
/// 索引，也不随全局会话数增长。
fn lane_child_sessions(
    store: &SqliteRuntimeStore,
    lane_session_id: &str,
    limit: usize,
) -> Result<Vec<LaneChildSession>, String> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = store
        .with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT session_id, agent_path, current_run_id, title, provider, model,
                        last_activity_at_ms
                   FROM sessions
                  WHERE kind = 'subagent' AND root_session_id = ?1
                  ORDER BY last_activity_at_ms DESC, session_id ASC
                  LIMIT ?2",
            )?;
            let rows = statement.query_map(rusqlite::params![lane_session_id, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    row.get::<_, i64>(6)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| format!("读取车道子代理会话失败：{error:?}"))?;
    let mut children = Vec::with_capacity(rows.len());
    for (session_id, agent_path, run_id, stored_title, provider, model, last_activity_at_ms) in rows
    {
        // 代理路径是子代理身份的一半：没有登记路径的行进不了逐字稿视图，不列。
        let Some(agent_path) = agent_path else {
            continue;
        };
        let Ok(session_id) = DaemonSessionId::new(session_id) else {
            continue;
        };
        children.push(LaneChildSession {
            session_id,
            agent_path,
            run_id,
            stored_title,
            stored_provider: provider,
            stored_model: model,
            last_activity_at_ms: u64::try_from(last_activity_at_ms).unwrap_or(0),
        });
    }
    Ok(children)
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
    let store = runtime_store(axon_root);
    let lane = DaemonSessionId::new(session_id.to_owned())
        .map_err(|error| format!("车道标识非法：{error:?}"))?;
    /*
     * 车道自己的运行实例标识随载荷下发；读不到只影响这个回显字段，不阻断名册。
     *
     * 名册的候选集合由会话行按 `(kind, root_session_id)` 取，与运行实例标识无关：
     * 重建或修复过的会话可能出现运行实例为空、子会话行仍在的情况，此时名册照旧可用。
     */
    let run_session_id = match store.get_session(&lane) {
        Ok(record) => record.current_run_id,
        Err(RuntimeStoreError::NotFound(_)) => String::new(),
        Err(error) => return Err(format!("读取车道运行会话失败：{error:?}")),
    };
    // 运行侧内务子代理（自迭代、反思、语义显著度）由轮后治理派生，不是用户要的
    // 委派：它们没有可进入的逐字稿，出现在名册里只会得到一条空视图。
    let children = lane_child_sessions(&store, session_id, MAX_SESSION_SUBAGENTS)?
        .into_iter()
        .filter(|child| !is_internal_agent_path(&child.agent_path))
        .collect::<Vec<_>>();
    /*
     * thread 侧事实一次读齐：只读一份派生索引，在内存里按 thread 标识过滤。
     *
     * 不逐子会话读 thread 分片：那是一次整文件读加全量投影，子代理一多、历史一长，
     * 这个轮询端点就会退化成 O(子代理数 × 各自历史长度)，而名册只需要几个标量。
     */
    let thread_facts = thread_facts_by_thread(axon_root);
    let mut roster = children
        .into_iter()
        .map(|child| {
            let turn_status = roster_turn_status(&store, &child.session_id);
            (child, turn_status)
        })
        .collect::<Vec<_>>();
    roster.sort_by(|left, right| {
        let key = |entry: &(LaneChildSession, RosterTurnStatus)| {
            thread_facts
                .get(&entry.0.run_id)
                .map_or(entry.0.last_activity_at_ms, |facts| facts.updated_at_ms)
        };
        key(right)
            .cmp(&key(left))
            .then_with(|| left.0.session_id.as_str().cmp(right.0.session_id.as_str()))
    });
    roster.truncate(MAX_SESSION_SUBAGENTS);
    let mut subagents = Vec::with_capacity(roster.len());
    for (child, turn_status) in roster {
        let slug = slug_of_agent_path(&child.agent_path);
        // thread 侧事实优先，索引里没有这条 thread 时才回落到控制面登记值。
        let facts = thread_facts.get(&child.run_id);
        let title = facts
            .and_then(|facts| facts.title.clone())
            .or_else(|| child.stored_title.clone());
        let provider = facts.map_or_else(
            || child.stored_provider.clone(),
            |facts| facts.provider.clone(),
        );
        let model = facts.map_or_else(|| child.stored_model.clone(), |facts| facts.model.clone());
        let updated_at_ms = facts.map_or(child.last_activity_at_ms, |facts| facts.updated_at_ms);
        let thread_turn_status = facts.and_then(|facts| facts.last_turn_status.clone());
        let last_turn_at_ms = facts.and_then(|facts| facts.last_turn_at_ms);
        subagents.push(serde_json::json!({
            "sessionId": child.session_id.as_str(),
            "agentPath": child.agent_path,
            "agentId": format!("subagent:{}", child.agent_path),
            // 行来自已登记的会话，运行态一定存在：原因码留空，字段保留给旧客户端。
            "unavailableReason": serde_json::Value::Null,
            "slug": slug,
            "title": title,
            "model": model,
            "provider": provider,
            "updatedAtMs": updated_at_ms,
            "hasRuntime": true,
            "status": turn_status.status.as_str(),
            "lastTurnStatus": turn_status
                .last_turn_status
                .map(RosterLastTurnStatus::as_str),
            // thread 事件里的末轮状态只作对账旁证，可能与运行态轮次头不同（索引会滞后）。
            "threadTurnStatus": thread_turn_status,
            "lastTurnAtMs": last_turn_at_ms,
        }));
    }
    Ok(serde_json::json!({
        "sessionId": session_id,
        "runSessionId": if run_session_id.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(run_session_id)
        },
        "subagents": subagents,
    }))
}

#[cfg(test)]
#[path = "workbench_http_subagents_lane_tests.rs"]
mod lane_tests;

#[cfg(test)]
mod roster_turn_heads_tests {
    //! 名册状态必须来自运行态轮次头，而不是 JSONL 索引缓存的末轮状态。

    use axon_domain::{InteractionId, InteractionRecord};
    use axon_session::{
        InteractionStore as _, JsonlThreadEventStore, NewSessionRequest, SessionKind,
        SessionRuntimeRecord, SessionTrustScope, StartEmptyThreadCommand, StartTurnCommand,
        ThreadService, ThreadSettings, ThreadSource, Turn, TurnStatus, new_interaction_id,
        new_session_id,
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
            "thread 侧的末轮状态只作对账旁证，必须如实透出它仍写着 active"
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
