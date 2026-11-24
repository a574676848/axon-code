//! 工作台路由的路径与查询解析：把 `subpath` / `query` 变成可执行的调用参数。
//!
//! 从 `workbench_http_adapter` 拆出：这一组是**纯字符串解析**，不读库、不碰运行态，
//! 单独成文后它的边界与百分号解码规则可以脱离路由一起读。

use super::*;

/// 从查询串取一个参数值；缺失时返回 None。
pub(super) fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    let query = query?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

/// 解析 `sessions/<sessionId>/interactions` 子路径，返回车道标识、条数上限、反向游标
/// 与翻页档标记。
///
/// 游标由 `before_ms` 与 `before_id` 两个参数共同给出，它们就是查询的排序键
/// `(received_at_ms, interaction_id)`；只给一半按未给处理，不做猜测。
/// `brief=1` 表示这一页只用于向前翻历史，会话级事实不重复下发。
pub(super) fn session_interactions_target(
    subpath: &str,
    query: Option<&str>,
) -> Option<(String, u32, Option<InteractionCursor>, bool)> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    if session_id.is_empty() || tail != "interactions" {
        return None;
    }
    let limit = query_param(query, "limit")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_SESSION_INTERACTION_LIMIT);
    let before = match (
        query_param(query, "before_ms").and_then(|value| value.parse::<u64>().ok()),
        query_param(query, "before_id"),
    ) {
        (Some(received_at_ms), Some(interaction_id)) if !interaction_id.is_empty() => {
            Some(InteractionCursor {
                received_at_ms,
                interaction_id,
            })
        }
        _ => None,
    };
    let brief = query_param(query, "brief").is_some_and(|value| value == "1");
    Some((session_id.to_owned(), limit, before, brief))
}

/// 解析 `sessions/<sessionId>/interrupt` 子路径，返回车道标识。
pub(super) fn session_interrupt_target(subpath: &str) -> Option<String> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    (tail == "interrupt" && !session_id.is_empty()).then(|| session_id.to_owned())
}

/// 解析 `sessions/<sessionId>/jobs/<jobId>/stop` 子路径，返回车道与作业标识。
pub(super) fn session_job_stop_target(subpath: &str) -> Option<(String, String)> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    let job_id = tail.strip_prefix("jobs/")?.strip_suffix("/stop")?;
    (!session_id.is_empty() && !job_id.is_empty())
        .then(|| (session_id.to_owned(), job_id.to_owned()))
}

/// 解析 `sessions/<sessionId>/subagents/<agent>/messages` 子路径。
///
/// 子代理标识在 URL 里是一个路径段（`/` 与 `:` 由调用方转义），这里解码回原样；
/// 解码失败按 404 处理，不把一段坏字节当成子代理名。
pub(super) fn session_subagent_message_target(subpath: &str) -> Option<(String, String)> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    let agent = tail.strip_prefix("subagents/")?.strip_suffix("/messages")?;
    if session_id.is_empty() || agent.is_empty() || agent.contains('/') {
        return None;
    }
    let agent = percent_decode(agent)?;
    (!agent.is_empty()).then(|| (session_id.to_owned(), agent))
}

/// 百分号解码；只用于本适配器自己的路径段，遇到坏转义返回 `None`。
pub(super) fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 解析 `sessions/<sessionId>/subagents` 子路径，返回车道标识。
pub(super) fn session_subagents_target(subpath: &str) -> Option<String> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    (tail == "subagents" && !session_id.is_empty()).then(|| session_id.to_owned())
}

/// 解析 `sessions/<sessionId>/jobs` 子路径，返回车道标识。
pub(super) fn session_jobs_target(subpath: &str) -> Option<String> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    (tail == "jobs" && !session_id.is_empty()).then(|| session_id.to_owned())
}

/// 解析 `sessions/<sessionId>/jobs/<jobId>/output` 子路径，返回车道与作业标识。
///
/// 三个 `jobs` 前缀的路由互不吞并：名册只认完整的 `jobs`，停止只认 `stop`，
/// 尾随只认 `output`。
pub(super) fn session_job_output_target(subpath: &str) -> Option<(String, String)> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    let jobs = tail.strip_prefix("jobs/")?;
    let (job_id, action) = jobs.split_once('/')?;
    (action == "output" && !session_id.is_empty() && !job_id.is_empty())
        .then(|| (session_id.to_owned(), job_id.to_owned()))
}

/// 解析 `sessions/<sessionId>/questions/<callId>/answer` 子路径。
pub(super) fn session_question_answer_target(subpath: &str) -> Option<(String, String)> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    let rest_q = tail.strip_prefix("questions/")?;
    let (call_id, action) = rest_q.split_once('/')?;
    (action == "answer" && !session_id.is_empty() && !call_id.is_empty())
        .then(|| (session_id.to_owned(), call_id.to_owned()))
}

/// 解析 `sessions/<sessionId>/questions` 子路径。
pub(super) fn session_questions_target(subpath: &str) -> Option<String> {
    let rest = subpath.strip_prefix("sessions/")?;
    let (session_id, tail) = rest.split_once('/')?;
    (tail == "questions" && !session_id.is_empty()).then(|| session_id.to_owned())
}

/// 解析 `interactions/<interactionId>/events` 子路径。
///
/// 历史帧走这条 GET 通道：`after` 是继续读取的序号（缺省从头），`limit` 是这一页
/// 的帧数上限，`tail` 为真时直接取该交互自身的尾窗。它与 WebSocket 订阅共用
/// 同一份 durable 语义，只是不再逐帧穿过事件通道。
pub(super) fn interaction_events_target(
    subpath: &str,
    query: Option<&str>,
) -> Option<(String, u64, u32, bool)> {
    let rest = subpath.strip_prefix("interactions/")?;
    let (interaction_id, action) = rest.rsplit_once('/')?;
    if interaction_id.is_empty() || interaction_id.contains('/') || action != "events" {
        return None;
    }
    // 交互标识不是 ULID 自有字符集（目标工具轮形如 `goal-tool:<id>:<id>`），客户端
    // 按 URL 规则把它编码成 `%3A` 送上来；不解码就永远查不到这条 Interaction。
    let interaction_id = percent_decode(interaction_id)?;
    if interaction_id.is_empty() {
        return None;
    }
    let after = query_param(query, "after")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let limit = query_param(query, "limit")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_INTERACTION_EVENTS_LIMIT);
    let tail = query_param(query, "tail")
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    Some((interaction_id, after, limit, tail))
}

/// 归属判据：owner 是车道本身，或车道当前运行会话的 `run_session_id`。
pub(crate) fn owner_matches_session(
    session_id: &str,
    owner: &str,
    run_session_id: Option<&str>,
) -> bool {
    owner == session_id || run_session_id == Some(owner)
}

/// 判断某个作业 owner 是否属于这条车道。
pub(crate) fn session_owns_owner(
    axon_root: &Path,
    session_id: &str,
    owner: &str,
) -> Result<bool, String> {
    if owner_matches_session(session_id, owner, None) {
        return Ok(true);
    }
    let store = runtime_store(axon_root);
    let session_id =
        DaemonSessionId::new(session_id).map_err(|error| format!("车道标识非法：{error:?}"))?;
    let record = match store.get_session(&session_id) {
        Ok(record) => record,
        Err(RuntimeStoreError::NotFound(_)) => return Ok(false),
        Err(error) => return Err(format!("读取车道运行会话失败：{error:?}")),
    };
    Ok(owner_matches_session(
        session_id.as_str(),
        owner,
        Some(record.current_run_id.as_str()),
    ))
}
