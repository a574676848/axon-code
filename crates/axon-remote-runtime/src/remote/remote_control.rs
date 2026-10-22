//! 远控消息收件箱规划。
//!
//! 该模块只处理远控入口消息的分类、去重和安全动作规划，不连接网络，
//! 也不直接修改终端或 Agent 会话。

use axon_domain::{AxonError, AxonResult, RemoteAction, RemoteCapabilityPolicy, SessionId};

/// 远控控制命令前缀。
pub const REMOTE_CONTROL_COMMAND_PREFIX: &str = "/";
/// 远控 delivery id 最大长度。
pub const REMOTE_CONTROL_DELIVERY_ID_MAX_LEN: usize = 128;
/// 远控普通消息预览最大字符数。
pub const REMOTE_CONTROL_MESSAGE_PREVIEW_CHARS: usize = 240;
/// 远控重复投递丢弃原因。
pub const REMOTE_CONTROL_REASON_DUPLICATE_DROPPED: &str = "remote_control:duplicate_dropped";
/// 忙碌会话控制命令旁路原因。
pub const REMOTE_CONTROL_REASON_CONTROL_BYPASS: &str = "remote_control:control_bypass";
/// 空闲会话普通消息本机投递原因。
pub const REMOTE_CONTROL_REASON_IDLE_DISPATCH: &str = "remote_control:idle_dispatch";
/// 忙碌会话普通消息排队原因。
pub const REMOTE_CONTROL_REASON_BUSY_QUEUED: &str = "remote_control:busy_queued";
/// 空消息拒绝原因。
pub const REMOTE_CONTROL_REASON_EMPTY_MESSAGE: &str = "remote_control:empty_message";
/// 远控消息输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlMessageInput {
    /// 远控投递 ID，用于幂等去重。
    pub delivery_id: String,
    /// 目标会话。
    pub session_id: SessionId,
    /// 远控原始文本。
    pub raw_text: String,
    /// 目标会话是否正在运行 Agent turn。
    pub session_busy: bool,
    /// 最近已处理 delivery id。
    pub seen_delivery_ids: Vec<String>,
}

/// 远控消息处理计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlMessagePlan {
    /// 目标会话。
    pub session_id: SessionId,
    /// 投递 ID。
    pub delivery_id: String,
    /// 投递回执。
    pub receipt: RemoteControlDeliveryReceipt,
    /// 处理动作。
    pub action: RemoteControlDispatchAction,
    /// 解析出的控制命令。
    pub command: Option<RemoteControlCommand>,
    /// 控制命令的首个参数。
    pub command_argument: Option<String>,
    /// 有界预览。
    pub bounded_preview: String,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控投递动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlDispatchAction {
    /// 直接交给控制命令处理器。
    InlineControl,
    /// 提交普通消息给空闲会话的本机 runtime。
    DispatchUserMessage,
    /// 放入普通消息队列，等待当前 turn 完成。
    QueueUserMessage,
    /// 丢弃重复投递。
    DropDuplicate,
    /// 拒绝输入。
    Reject,
}

/// 远控投递状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlDeliveryStatus {
    /// 已交给本机 runtime inline 处理。
    AcceptedInline,
    /// 已进入普通消息队列。
    Queued,
    /// 重复投递已幂等丢弃。
    DuplicateDropped,
    /// 输入被拒绝。
    Rejected,
}

/// 远控去重状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlDedupeStatus {
    /// 新投递。
    New,
    /// 重复投递。
    Duplicate,
}

/// 远控投递回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlDeliveryReceipt {
    /// 目标会话。
    pub session_id: SessionId,
    /// 投递 ID。
    pub delivery_id: String,
    /// 投递状态。
    pub status: RemoteControlDeliveryStatus,
    /// 去重状态。
    pub dedupe_status: RemoteControlDedupeStatus,
    /// 是否进入队列。
    pub queued: bool,
    /// 是否要求本机 runtime 执行。
    pub local_execution_required: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控控制命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlCommand {
    /// 查看状态。
    Status,
    /// 查看队列。
    Queue,
    /// 停止当前运行。
    Stop,
}

/// 远控收件箱 runtime。
#[derive(Debug, Clone)]
pub struct RemoteControlInboxRuntime {
    policy: RemoteCapabilityPolicy,
}

impl RemoteControlInboxRuntime {
    /// 创建远控收件箱 runtime。
    #[must_use]
    pub const fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self { policy }
    }

    /// 规划远控消息投递。
    pub fn plan(&self, input: &RemoteControlMessageInput) -> AxonResult<RemoteControlMessagePlan> {
        validate_delivery_id(&input.delivery_id)?;
        let trimmed = input.raw_text.trim();
        if trimmed.is_empty() {
            return Ok(plan(
                input,
                RemoteControlDispatchAction::Reject,
                None,
                None,
                "",
                vec![REMOTE_CONTROL_REASON_EMPTY_MESSAGE.to_owned()],
            ));
        }
        if input
            .seen_delivery_ids
            .iter()
            .any(|seen| seen == &input.delivery_id)
        {
            return Ok(plan(
                input,
                RemoteControlDispatchAction::DropDuplicate,
                None,
                None,
                trimmed,
                vec![REMOTE_CONTROL_REASON_DUPLICATE_DROPPED.to_owned()],
            ));
        }

        if let Some((command, argument)) = parse_control_command(trimmed) {
            self.ensure_command_allowed(command)?;
            return Ok(plan(
                input,
                RemoteControlDispatchAction::InlineControl,
                Some(command),
                argument,
                trimmed,
                vec![REMOTE_CONTROL_REASON_CONTROL_BYPASS.to_owned()],
            ));
        }

        let action = if input.session_busy {
            RemoteControlDispatchAction::QueueUserMessage
        } else {
            RemoteControlDispatchAction::DispatchUserMessage
        };
        let reason = if input.session_busy {
            REMOTE_CONTROL_REASON_BUSY_QUEUED
        } else {
            REMOTE_CONTROL_REASON_IDLE_DISPATCH
        };
        Ok(plan(
            input,
            action,
            None,
            None,
            trimmed,
            vec![reason.to_owned()],
        ))
    }

    fn ensure_command_allowed(&self, command: RemoteControlCommand) -> AxonResult<()> {
        match command {
            RemoteControlCommand::Stop => self.policy.ensure_allowed(RemoteAction::ControlThread),
            RemoteControlCommand::Status | RemoteControlCommand::Queue => {
                self.policy.ensure_allowed(RemoteAction::ReadBoundedSummary)
            }
        }
    }
}

fn validate_delivery_id(delivery_id: &str) -> AxonResult<()> {
    if delivery_id.trim().is_empty()
        || delivery_id.chars().count() > REMOTE_CONTROL_DELIVERY_ID_MAX_LEN
    {
        return Err(AxonError::runtime_failure(
            "远控 delivery id 不能为空或过长",
        ));
    }
    Ok(())
}

fn parse_control_command(value: &str) -> Option<(RemoteControlCommand, Option<String>)> {
    if matches!(value.trim(), "停止" | "终止" | "终止执行") {
        return Some((RemoteControlCommand::Stop, None));
    }
    let mut parts = value.split_whitespace();
    let command = parts.next()?.strip_prefix(REMOTE_CONTROL_COMMAND_PREFIX)?;
    let command = match command.to_ascii_lowercase().as_str() {
        "status" => RemoteControlCommand::Status,
        "queue" => RemoteControlCommand::Queue,
        "stop" | "cancel" | "interrupt" => RemoteControlCommand::Stop,
        _ => return None,
    };
    Some((command, parts.next().map(ToOwned::to_owned)))
}

fn plan(
    input: &RemoteControlMessageInput,
    action: RemoteControlDispatchAction,
    command: Option<RemoteControlCommand>,
    command_argument: Option<String>,
    preview: &str,
    reasons: Vec<String>,
) -> RemoteControlMessagePlan {
    let reasons_for_receipt = reasons.clone();
    RemoteControlMessagePlan {
        session_id: input.session_id,
        delivery_id: input.delivery_id.clone(),
        receipt: receipt(input, action, reasons_for_receipt),
        action,
        command,
        command_argument,
        bounded_preview: bounded_preview(preview),
        reasons,
    }
}

fn receipt(
    input: &RemoteControlMessageInput,
    action: RemoteControlDispatchAction,
    reasons: Vec<String>,
) -> RemoteControlDeliveryReceipt {
    let status = match action {
        RemoteControlDispatchAction::InlineControl
        | RemoteControlDispatchAction::DispatchUserMessage => {
            RemoteControlDeliveryStatus::AcceptedInline
        }
        RemoteControlDispatchAction::QueueUserMessage => RemoteControlDeliveryStatus::Queued,
        RemoteControlDispatchAction::DropDuplicate => RemoteControlDeliveryStatus::DuplicateDropped,
        RemoteControlDispatchAction::Reject => RemoteControlDeliveryStatus::Rejected,
    };
    RemoteControlDeliveryReceipt {
        session_id: input.session_id,
        delivery_id: input.delivery_id.clone(),
        status,
        dedupe_status: if action == RemoteControlDispatchAction::DropDuplicate {
            RemoteControlDedupeStatus::Duplicate
        } else {
            RemoteControlDedupeStatus::New
        },
        queued: action == RemoteControlDispatchAction::QueueUserMessage,
        local_execution_required: matches!(
            action,
            RemoteControlDispatchAction::InlineControl
                | RemoteControlDispatchAction::DispatchUserMessage
                | RemoteControlDispatchAction::QueueUserMessage
        ),
        reasons,
    }
}

fn bounded_preview(value: &str) -> String {
    let mut preview: String = value
        .chars()
        .take(REMOTE_CONTROL_MESSAGE_PREVIEW_CHARS)
        .collect();
    if value.chars().count() > REMOTE_CONTROL_MESSAGE_PREVIEW_CHARS {
        preview.push_str("...");
    }
    preview
}
