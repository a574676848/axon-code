//! 会话菜单：`/api/v1/sessions` 的读取与置顶归档写入。
//!
//! 会话清单是左侧菜单的**状态与顺序事实源**：网关按「置顶优先、再按最近活动倒序」
//! 返回，并直接给出 `pinned` / `archived`，客户端不再重算一遍。置顶与归档写回同一张
//! 表，因此桌面、TUI 与 CLI 读到的永远是同一份事实，不会出现「桌面归档了、另一个
//! 入口还在列表里」这种各存一份状态的分叉。
//!
//! 这里只做协议整形：地址、凭据与 TLS 信任来源全部由 [`crate::gateway`] 提供，
//! 桌面端不直连运行态数据库。

use serde::{Deserialize, Serialize};

use crate::gateway;

/// 会话路由前缀（完整路径，`/api/v1` 已含）。
const SESSIONS_PATH: &str = "/api/v1/sessions";

/// 一条会话菜单条目，字段与网关载荷逐项一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMenuEntry {
    /// 会话标识；与车道标识是同一个串。
    pub session_id: String,
    /// 展示标题；网关已按首条入站消息回落，缺省时为空串。
    #[serde(default)]
    pub title: String,
    /// 运行态状态字面值（`created` / `running` / `completed` 等）。
    #[serde(default)]
    pub status: String,
    /// 是否置顶。
    #[serde(default)]
    pub pinned: bool,
    /// 是否归档。
    #[serde(default)]
    pub archived: bool,
    /// 归属 Workspace；未绑定时为空。
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// 最近活动时间（毫秒）。
    #[serde(default)]
    pub last_activity_at_ms: u64,
    /// 已收口回合数。
    #[serde(default)]
    pub turn_count: u64,
    /// 累计墙钟耗时（毫秒）。
    #[serde(default)]
    pub total_active_ms: u64,
    /// 首字延迟中位数（毫秒）；没有样本时为空。
    #[serde(default)]
    pub ttft_p50_ms: Option<u64>,
    /// 首字延迟最大值（毫秒）。
    #[serde(default)]
    pub ttft_max_ms: u64,
    /// 仍在运行的后台作业数。
    #[serde(default)]
    pub open_job_count: u64,
}

/// 组装会话清单地址。
///
/// 过滤条件缺省即整项省略，让网关用自己的默认值：只有真的收窄范围时才写查询串，
/// 两边不各存一份默认口径。
fn list_path(workspace: Option<&str>, include_archived: bool) -> String {
    let mut path = String::from(SESSIONS_PATH);
    let mut separator = '?';
    if let Some(workspace) = workspace.map(str::trim).filter(|value| !value.is_empty()) {
        path.push_str(&format!("{separator}workspace={}", encode(workspace)));
        separator = '&';
    }
    if include_archived {
        path.push_str(&format!("{separator}include_archived=1"));
    }
    path
}

/// 读取会话清单。
///
/// `include_archived` 为真时归档会话一并返回：列表要按归档筛选展示，自己再发一次
/// 「已归档」请求就会把同一份事实读两遍，两次之间还可能被另一端改写。
pub fn list(
    workspace: Option<&str>,
    include_archived: bool,
) -> Result<Vec<SessionMenuEntry>, String> {
    let payload = gateway::get_json_at(&list_path(workspace, include_archived))?;
    // 网关载荷是 `{ sessions: [...], totalRecords, rawPayloadPersisted }`；
    // 兼容裸数组形态，避免把「有会话」读成空列表。
    let items = payload
        .get("sessions")
        .cloned()
        .unwrap_or_else(|| payload.clone());
    serde_json::from_value(items).map_err(|error| format!("解析会话清单失败：{error}"))
}

/// 写入置顶与归档状态。
///
/// 两个字段都可缺省，但至少要有一个：空报文会被网关以 400 拒绝，与其发出去再翻译，
/// 不如在本地就说清楚。
pub fn patch(session_id: &str, pinned: Option<bool>, archived: Option<bool>) -> Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_string());
    }
    if pinned.is_none() && archived.is_none() {
        return Err("置顶或归档至少要给出一项".to_string());
    }
    let mut body = serde_json::Map::new();
    if let Some(pinned) = pinned {
        body.insert("pinned".to_string(), serde_json::Value::Bool(pinned));
    }
    if let Some(archived) = archived {
        body.insert("archived".to_string(), serde_json::Value::Bool(archived));
    }
    let path = format!("{SESSIONS_PATH}/{}", encode(session_id));
    let (status, response) = gateway::patch_raw_at(&path, &serde_json::Value::Object(body))?;
    match status {
        200..=299 => Ok(()),
        // 404 是「这条会话还不在运行态登记里」：新建但尚未产生回合的会话就是这种状态。
        // 它与路由缺失（后台版本过旧）修复动作不同，必须分开说。
        404 => {
            Err("会话尚未在运行态登记，置顶与归档要等这条会话产生首条消息后才能保存".to_string())
        }
        400 => Err("网关拒绝了这次置顶归档请求：请求内容不完整".to_string()),
        _ => Err(gateway::gateway_status_message(status, &response)),
    }
}

/// 百分号编码：只保留 `unreserved` 字符，其余按字节转义。
///
/// 查询串与路径段共用一个实现：会话标识与工作区标识都由服务端生成，但客户端不能
/// 假定它们永远不含保留字符，拼串之前先编码一次没有代价。
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{SessionMenuEntry, encode, list_path};

    #[test]
    fn list_path_omits_filters_that_were_not_asked_for() {
        // 缺省即省略：不给网关塞一份本地默认值。
        assert_eq!(list_path(None, false), "/api/v1/sessions");
        assert_eq!(list_path(Some("  "), false), "/api/v1/sessions");
        assert_eq!(list_path(None, true), "/api/v1/sessions?include_archived=1");
        // 工作区过滤在前、归档开关在后，两者共存只用一个分隔符。
        assert_eq!(
            list_path(Some("workspace-1"), true),
            "/api/v1/sessions?workspace=workspace-1&include_archived=1"
        );
    }

    #[test]
    fn identifiers_are_percent_encoded_before_they_reach_the_path() {
        // 保留字符必须转义，否则一条带斜杠的标识会被网关读成两条不同的路由。
        assert_eq!(encode("session-session-a_b.c~d"), "session-session-a_b.c~d");
        assert_eq!(encode("a/b c"), "a%2Fb%20c");
        assert_eq!(
            list_path(Some("workspace 1&2"), false),
            "/api/v1/sessions?workspace=workspace%201%262"
        );
    }

    #[test]
    fn entries_parse_the_gateway_payload_without_local_reinterpretation() {
        // 网关字段是 camelCase，缺失的读数按零/空处理，不猜。
        let payload = serde_json::json!([{
            "sessionId": "session-session-a",
            "title": "修复会话历史分页",
            "status": "completed",
            "pinned": true,
            "archived": false,
            "workspaceId": "workspace-1",
            "lastActivityAtMs": 1_700_000_000_000_u64,
            "turnCount": 3,
            "totalActiveMs": 42_000,
            "ttftP50Ms": 240,
            "ttftMaxMs": 900,
            "openJobCount": 0
        }]);
        let entries: Vec<SessionMenuEntry> = serde_json::from_value(payload).expect("载荷可解析");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "session-session-a");
        assert!(entry.pinned);
        assert!(!entry.archived);
        assert_eq!(entry.turn_count, 3);
        assert_eq!(entry.ttft_p50_ms, Some(240));

        // 没有观测的读数不进界面：缺字段解析成空值而不是报错。
        let sparse: Vec<SessionMenuEntry> =
            serde_json::from_value(serde_json::json!([{ "sessionId": "session-session-b" }]))
                .expect("缺省字段可解析");
        assert_eq!(sparse[0].title, "");
        assert_eq!(sparse[0].workspace_id, None);
        assert_eq!(sparse[0].ttft_p50_ms, None);
    }
}
