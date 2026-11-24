//! 工作台协议视图：把领域记录摊平成小驼峰 JSON 载荷。
//!
//! 从 `workbench_http_adapter` 拆出：这些类型是**对外协议**，字段名一旦发布就不能随领域类型改名，
//! 因此它们值得单独成文，免得协议与路由逻辑混在一起被顺手重构掉。

use super::*;

/// 工作区协议载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    /// Workspace 标识。
    pub workspace_id: String,
    /// 显示名称。
    pub display_name: String,
    /// 工作区根目录。
    pub workspace_root: String,
    /// Git remote。
    pub git_remote: Option<String>,
    /// 默认分支。
    pub default_branch: Option<String>,
    /// 是否固定保留。
    pub pinned: bool,
    /// 最近访问时间（毫秒）。
    pub last_seen_at_ms: u64,
}

impl From<&WorkspaceRecord> for WorkspaceView {
    fn from(record: &WorkspaceRecord) -> Self {
        Self {
            workspace_id: record.workspace_id.clone(),
            display_name: record.display_name.clone(),
            workspace_root: record.workspace_root.clone(),
            git_remote: record.git_remote.clone(),
            default_branch: record.default_branch.clone(),
            pinned: record.pinned,
            last_seen_at_ms: record.last_seen_at_ms,
        }
    }
}

/// 车道内一条 Interaction 的协议载荷。
///
/// 索引页承载「不读事件流就能画出一轮」的全部事实：稳定标识、状态与接收时间、入站正文
/// 与附件、最终交付正文、用过的工具名与逐轮读数。用户气泡与结论因此不必回放整轮帧，
/// 只有展开某一轮的过程时才去读事件流。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionView {
    /// Interaction 标识。
    pub interaction_id: String,
    /// 处理状态。
    pub status: String,
    /// 来源渠道。
    pub channel: String,
    /// 发起这一轮的客户端；老记录为空串。
    pub client: String,
    /// 接收时间（毫秒）。
    pub received_at_ms: u64,
    /// 入站正文。会话事件流不含用户消息事件，回放时用户气泡只能由这里还原。
    pub content: String,
    /// 入站时落地的本地附件绝对路径；回放历史时据此还原图片与文件。
    pub attachments: Vec<String>,
    /// 本轮显式指定的模型；入口没有选择时为空。
    ///
    /// 折叠成一行时它是最有信息量的那一项：历史索引不读事件流，模型是唯一能
    /// 「不花一次扫描就说清这轮怎么跑的」的聚合事实。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 本轮显式指定的推理强度；入口没有选择时为空。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// 本轮最终交付正文。
    ///
    /// 历史轮折叠后正文与用户消息一样要直接可读，不能只留一行摘要等人点开。正文由网关
    /// 从收口事件的载荷里取，客户端因此不必为了看见结论而回放整条记录的帧。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub answer_text: String,
    /// 本轮用过的工具名，按出现顺序、已去重。
    ///
    /// 索引轮的过程抬头据此立起与实时同一句动作摘要（「执行了命令并已读取文件」），
    /// 读者不必先展开这一轮才看得见它做过什么。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_names: Vec<String>,
    /// 已收口轮次的墙钟用时（毫秒）；读不到时间事实时为空。
    ///
    /// 只给没有 `turn_metrics` 事件的老记录兜底：有指标事件的轮次由 `metrics` 给出完整
    /// 读数，两列同时填等于同一份事实发两遍。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// 这一轮的读数（用时、token、首字、速率）；一个事实都读不到时为空。
    ///
    /// 由该轮自己的 `turn_metrics` 事件定向取出，与 `session_metrics` 同源：索引页因此
    /// 不必为了显示一行指标去回放整轮正文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TurnMetricsView>,
}

/// 一轮的读数载荷：字段与 `turn_metrics` 事件一一对应，未测得的一律缺省。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnMetricsView {
    /// 本轮墙钟用时（毫秒）。
    pub active_ms: u64,
    /// 首字延迟；未测得时缺省。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    /// 生成速率（0.1 tok/s 的整数刻度）；未测得时缺省。
    ///
    /// 用整数传：浮点会让整份协议失去 `Eq`，而显示口径本来就是一位小数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_speed_tps_x10: Option<u64>,
    /// 工具调用次数。
    pub tool_calls: u32,
    /// 未缓存输入。
    pub input_tokens: u64,
    /// 缓存读取。
    pub cache_read_tokens: u64,
    /// 缓存写入。
    pub cache_write_tokens: u64,
    /// 输出。
    pub output_tokens: u64,
    /// 推理输出。
    pub reasoning_tokens: u64,
}
