//! 飞书问答卡片的持久路由与消息映射（UQ-303, UQ-305, UQ-306）。
//!
//! 两张有界映射表：
//! - 路由表：`lane_id -> chat` 会话绑定。问答记录本身不带 chat（它属于 Agent 回合），
//!   而卡片必须发到用户所在会话；网关在处理入站消息时把复合车道与会话绑定落盘。
//! - 卡片表：`call_id -> 卡片消息 ID 列表`（`call_id` 即问答记录里的 `tool_call_id`，
//!   也是卡片按钮载荷里的 `call_id`；表内同时保留 `question_id`，因此它同时是
//!   `question_id -> message_id` 映射）。结算回写需要它在进程重启后依然可查。
//!
//! 两张表都是「有界 + 原子替换」的本地索引：容量超限时淘汰最旧条目，写入走
//! 临时文件加 rename，避免半截 JSON 让整表不可读。

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// 路由表文件名。
pub(crate) const REMOTE_FEISHU_QUESTION_ROUTE_FILE: &str = "remote_feishu_question_routes.json";
/// 卡片映射文件名。
pub(crate) const REMOTE_FEISHU_QUESTION_CARD_FILE: &str = "remote_feishu_question_cards.json";
/// 路由表条目上限。
pub(crate) const REMOTE_FEISHU_QUESTION_ROUTE_MAX_ENTRIES: usize = 256;
/// 卡片表条目上限。
pub(crate) const REMOTE_FEISHU_QUESTION_CARD_MAX_ENTRIES: usize = 512;

/// 复合车道到飞书会话的绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeishuQuestionRoute {
    /// 复合车道 ID（`session-lane-*`）。
    pub(crate) lane_id: String,
    /// 目标飞书会话。
    pub(crate) chat_id: String,
    /// 会话类型稳定值（p2p / group / unknown）。
    pub(crate) chat_type: String,
    /// 绑定时的 Workspace。
    pub(crate) workspace_id: String,
    /// 可选的回复目标；卡片优先挂在该消息的线程里。
    #[serde(default)]
    pub(crate) reply_to_message_id: Option<String>,
    /// 记录时间。
    pub(crate) recorded_at_ms: u64,
}

/// 一张问答卡片的投递与结算状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeishuQuestionCardRecord {
    /// 问答调用标识（工具调用 ID，也是按钮载荷里的 `call_id`）。
    pub(crate) call_id: String,
    /// 全局问答记录 ID。
    pub(crate) question_id: String,
    /// 所属复合车道。
    pub(crate) lane_id: String,
    /// 目标飞书会话。
    pub(crate) chat_id: String,
    /// 已创建的卡片消息 ID，按分片顺序排列。
    #[serde(default)]
    pub(crate) message_ids: Vec<String>,
    /// 结算结论是否已回写（PATCH 成功或已降级为文本通知）。
    pub(crate) settled: bool,
    /// 结算回写路径：`card`（PATCH 成功）或 `fallback`（降级为 outbox 文本通知）。
    #[serde(default)]
    pub(crate) settlement: Option<String>,
    /// 投递失败原因；有值时不再重复建卡。
    #[serde(default)]
    pub(crate) failure: Option<String>,
    /// 最近一次更新时间。
    pub(crate) updated_at_ms: u64,
}

/// 卡片记录是否已经结算完毕。
pub(crate) fn is_settled(record: &FeishuQuestionCardRecord) -> bool {
    record.settled
}

/// 解析路由表路径。
pub(crate) fn route_store_path(store_root: &Path) -> PathBuf {
    store_root.join(REMOTE_FEISHU_QUESTION_ROUTE_FILE)
}

/// 解析卡片表路径。
pub(crate) fn card_store_path(store_root: &Path) -> PathBuf {
    store_root.join(REMOTE_FEISHU_QUESTION_CARD_FILE)
}

/// 读取全部路由绑定（按记录时间倒序，最新优先）。
pub(crate) fn read_routes(store_root: &Path) -> Result<Vec<FeishuQuestionRoute>, String> {
    let mut routes: Vec<FeishuQuestionRoute> =
        read_bounded(route_store_path(store_root).as_path())?;
    routes.sort_by_key(|route| std::cmp::Reverse(route.recorded_at_ms));
    Ok(routes)
}

/// 解析指定车道的会话绑定。
pub(crate) fn lookup_route(
    store_root: &Path,
    lane_id: &str,
) -> Result<Option<FeishuQuestionRoute>, String> {
    Ok(read_routes(store_root)?
        .into_iter()
        .find(|route| route.lane_id == lane_id))
}

/// 写入或刷新一条会话绑定。
pub(crate) fn upsert_route(store_root: &Path, route: FeishuQuestionRoute) -> Result<(), String> {
    if route.lane_id.trim().is_empty() || route.chat_id.trim().is_empty() {
        return Err("飞书问答路由绑定缺少车道或会话标识".to_owned());
    }
    let path = route_store_path(store_root);
    let mut routes: Vec<FeishuQuestionRoute> = read_bounded(&path)?;
    routes.retain(|existing| existing.lane_id != route.lane_id);
    routes.push(route);
    routes.sort_by_key(|item| std::cmp::Reverse(item.recorded_at_ms));
    routes.truncate(REMOTE_FEISHU_QUESTION_ROUTE_MAX_ENTRIES);
    write_bounded(&path, &routes)
}

/// 读取全部卡片记录。
pub(crate) fn read_cards(store_root: &Path) -> Result<Vec<FeishuQuestionCardRecord>, String> {
    let mut records: Vec<FeishuQuestionCardRecord> =
        read_bounded(card_store_path(store_root).as_path())?;
    records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
    Ok(records)
}

/// 按 `call_id` 解析卡片记录。
pub(crate) fn lookup_card(
    store_root: &Path,
    call_id: &str,
) -> Result<Option<FeishuQuestionCardRecord>, String> {
    Ok(read_cards(store_root)?
        .into_iter()
        .find(|record| record.call_id == call_id))
}

/// 写入或刷新一条卡片记录。
pub(crate) fn upsert_card(
    store_root: &Path,
    record: FeishuQuestionCardRecord,
) -> Result<(), String> {
    if record.call_id.trim().is_empty() {
        return Err("飞书问答卡片记录缺少 call_id".to_owned());
    }
    let path = card_store_path(store_root);
    let mut records: Vec<FeishuQuestionCardRecord> = read_bounded(&path)?;
    records.retain(|existing| existing.call_id != record.call_id);
    records.push(record);
    records.sort_by_key(|item| std::cmp::Reverse(item.updated_at_ms));
    records.truncate(REMOTE_FEISHU_QUESTION_CARD_MAX_ENTRIES);
    write_bounded(&path, &records)
}

fn read_bounded<T>(path: &Path) -> Result<Vec<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("读取飞书问答索引失败: {error}"))?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<T>>(&content)
        .map_err(|error| format!("飞书问答索引 JSON 无效: {error}"))
}

fn write_bounded<T>(path: &Path, records: &[T]) -> Result<(), String>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建飞书问答索引目录失败: {error}"))?;
    }
    let content =
        serde_json::to_vec(records).map_err(|error| format!("序列化飞书问答索引失败: {error}"))?;
    let temporary_path = path.with_extension("tmp");
    let mut temporary = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary_path)
        .map_err(|error| format!("打开飞书问答索引临时文件失败: {error}"))?;
    temporary
        .write_all(&content)
        .and_then(|()| temporary.sync_all())
        .map_err(|error| format!("写入飞书问答索引临时文件失败: {error}"))?;
    drop(temporary);
    fs::rename(&temporary_path, path).map_err(|error| format!("替换飞书问答索引失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(lane_id: &str, chat_id: &str, recorded_at_ms: u64) -> FeishuQuestionRoute {
        FeishuQuestionRoute {
            lane_id: lane_id.to_owned(),
            chat_id: chat_id.to_owned(),
            chat_type: "p2p".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            reply_to_message_id: Some("om_1".to_owned()),
            recorded_at_ms,
        }
    }

    fn card(call_id: &str, updated_at_ms: u64) -> FeishuQuestionCardRecord {
        FeishuQuestionCardRecord {
            call_id: call_id.to_owned(),
            question_id: format!("uq-{call_id}"),
            lane_id: "session-lane-1".to_owned(),
            chat_id: "oc_1".to_owned(),
            message_ids: vec!["om_card_1".to_owned()],
            settled: false,
            settlement: None,
            failure: None,
            updated_at_ms,
        }
    }

    #[test]
    fn route_upsert_is_latest_wins_and_lookup_by_lane() {
        let root = axon_test_support::test_temp_root();

        upsert_route(&root, route("lane-1", "oc_old", 10)).expect("写入旧绑定");
        upsert_route(&root, route("lane-1", "oc_new", 20)).expect("刷新绑定");
        upsert_route(&root, route("lane-2", "oc_other", 15)).expect("写入另一绑定");

        let found = lookup_route(&root, "lane-1").expect("查询").expect("存在");
        assert_eq!(found.chat_id, "oc_new");
        assert_eq!(read_routes(&root).expect("读取全部").len(), 2);
        assert!(lookup_route(&root, "lane-missing").expect("查询").is_none());
    }

    #[test]
    fn card_upsert_survives_reopen_and_keeps_newest_state() {
        let root = axon_test_support::test_temp_root();

        upsert_card(&root, card("call-1", 10)).expect("写入卡片");
        let mut settled = card("call-1", 20);
        settled.settled = true;
        settled.settlement = Some("card".to_owned());
        upsert_card(&root, settled).expect("刷新卡片");

        let loaded = lookup_card(&root, "call-1").expect("查询").expect("存在");
        assert!(loaded.settled);
        assert_eq!(loaded.settlement.as_deref(), Some("card"));
        assert_eq!(read_cards(&root).expect("读取全部").len(), 1);
    }

    #[test]
    fn bounded_tables_evict_oldest_entries() {
        let root = axon_test_support::test_temp_root();

        for index in 0..(REMOTE_FEISHU_QUESTION_ROUTE_MAX_ENTRIES + 5) {
            upsert_route(&root, route(&format!("lane-{index}"), "oc_1", index as u64))
                .expect("写入绑定");
        }

        let routes = read_routes(&root).expect("读取绑定");
        assert_eq!(routes.len(), REMOTE_FEISHU_QUESTION_ROUTE_MAX_ENTRIES);
        assert!(
            lookup_route(&root, "lane-0").expect("查询").is_none(),
            "最旧条目必须被淘汰"
        );
    }

    #[test]
    fn rejects_route_without_lane_or_chat() {
        let root = axon_test_support::test_temp_root();
        let error = upsert_route(&root, route("", "oc_1", 1)).expect_err("空车道必须被拒");
        assert!(error.contains("车道"));
    }
}
