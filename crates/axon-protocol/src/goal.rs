//! 主 turn Goal 工具的 IPC 命令合同。
//!
//! 工具执行进程只负责解析模型输入与绑定运行时上下文；查询与写入一律由
//! Daemon 在权威事实源上完成，避免出现第二套 Goal 状态。

use axon_domain::{
    DisclosurePolicy, GoalCancelCommand, GoalCreateCommand, GoalDetailQuery, GoalListQuery,
    GoalUpdateCommand,
};
use serde::{Deserialize, Serialize};

/// 主 turn Goal 工具命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "tool", content = "command")]
pub enum GoalToolCommand {
    /// 创建持久 Goal，并按需要创建首个 queued Execution。
    Create(GoalCreateCommand),
    /// 按 scope、状态与游标列出可见 Goal 摘要。
    List(GoalListQuery),
    /// 聚合指定 Goal 的 Activity 视图。
    Get(GoalDetailQuery),
    /// 按 expected revision 更新允许字段。
    Update(GoalUpdateCommand),
    /// 取消 Goal 并传播取消事实。
    Cancel(GoalCancelCommand),
    /// 启动或继续已有 Goal。
    Start(GoalStartCommand),
}

/// 主 turn 启动已有 Goal 的输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalStartCommand {
    /// 要启动的 Goal。
    pub goal_id: String,
    /// 最近读取的 Goal 版本。
    pub expected_revision: u64,
}

/// Goal 的业务来源，用于完成通知向主 turn 说明触发来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GoalSource {
    /// 主 turn 一次性创建的 Goal。
    OneOff,
    /// Axon cron 到期创建的 Goal。
    AxonCron {
        /// 触发该 Goal 的 cron job 标识。
        job_id: String,
        /// 投递渠道，例如 feishu。
        channel: String,
        /// 渠道内投递目标，例如 Feishu chat_id。
        destination: String,
    },
}

/// 主 turn Goal 工具请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalToolRequest {
    /// 调用来源 actor，用于审批与审计记录。
    pub actor: String,
    /// 运行时绑定的 Workspace ID；None 表示仅全局作用域可用。
    pub workspace_id: Option<String>,
    /// 工具调用幂等键，用于派生稳定的 Goal / Execution 标识。
    pub idempotency_key: String,
    /// Goal 来源；缺省时按一次性 Goal 处理。
    #[serde(default)]
    pub source: Option<GoalSource>,
    /// 运行时受众披露边界；缺省时兼容旧客户端并按私聊处理。
    #[serde(default)]
    pub disclosure_policy: Option<DisclosurePolicy>,
    /// 创建 Interaction 的运行时路由与受众，不属于模型工具输入。
    #[serde(default)]
    pub transport_context: Option<serde_json::Value>,
    /// 具体工具命令。
    pub command: GoalToolCommand,
}

/// 启动已有 Goal 的 Daemon 应用命令。
///
/// 运行身份和回复路由由宿主绑定，模型仅提供 GoalStartCommand。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalStartRequest {
    /// Goal 标识。
    pub goal_id: String,
    /// 发起启动的 actor。
    pub actor: String,
    /// 运行时绑定的 Workspace ID。
    pub workspace_id: Option<String>,
    /// 预期 Goal revision。
    pub expected_revision: u64,
    /// 启动动作幂等键。
    pub idempotency_key: String,
    /// Goal 来源。
    #[serde(default)]
    pub source: Option<GoalSource>,
    /// 当前受众披露边界。
    #[serde(default)]
    pub disclosure_policy: Option<DisclosurePolicy>,
    /// 首次启动 Proposed Goal 时绑定的运行时路由。
    #[serde(default)]
    pub transport_context: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::{GoalSource, GoalToolCommand, GoalToolRequest};
    use axon_domain::{DisclosurePolicy, GoalCreateCommand, GoalCreationScope};

    fn create_request(source: Option<GoalSource>) -> GoalToolRequest {
        GoalToolRequest {
            actor: "test".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            idempotency_key: "goal-1".to_owned(),
            source,
            disclosure_policy: Some(DisclosurePolicy::PrivateOnly),
            transport_context: None,
            command: GoalToolCommand::Create(GoalCreateCommand {
                scope: GoalCreationScope::CurrentWorkspace,
                title: "测试 Goal".to_owned(),
                objective: "验证来源".to_owned(),
                steps: vec!["执行".to_owned()],
                acceptance: vec!["完成".to_owned()],
                create_execution: true,
            }),
        }
    }

    #[test]
    fn axon_cron_source_round_trips_through_json() {
        let request = create_request(Some(GoalSource::AxonCron {
            job_id: "job-42".to_owned(),
            channel: "feishu".to_owned(),
            destination: "oc_test".to_owned(),
        }));
        let encoded = serde_json::to_string(&request).expect("Goal 请求应可序列化");
        let decoded: GoalToolRequest =
            serde_json::from_str(&encoded).expect("Goal 请求应可反序列化");
        assert_eq!(decoded.source, request.source);
    }

    #[test]
    fn missing_source_defaults_to_legacy_one_off_semantics() {
        let mut value = serde_json::to_value(create_request(None)).expect("Goal 请求应可转 JSON");
        value
            .as_object_mut()
            .expect("Goal 请求应为对象")
            .remove("source");
        let decoded: GoalToolRequest =
            serde_json::from_value(value).expect("缺省来源的旧请求应可解析");
        assert_eq!(decoded.source, None);
        assert_eq!(
            decoded.disclosure_policy,
            Some(DisclosurePolicy::PrivateOnly)
        );
    }

    #[test]
    fn missing_disclosure_policy_uses_private_sentinel() {
        let mut value = serde_json::to_value(create_request(Some(GoalSource::OneOff)))
            .expect("Goal 请求应可转 JSON");
        value
            .as_object_mut()
            .expect("Goal 请求应为对象")
            .remove("disclosurePolicy");
        let decoded: GoalToolRequest =
            serde_json::from_value(value).expect("缺省边界的旧请求应可解析");
        assert_eq!(decoded.disclosure_policy, None);
    }
}
