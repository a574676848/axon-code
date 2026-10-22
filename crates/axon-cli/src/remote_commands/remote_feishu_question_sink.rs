//! 守护进程侧飞书问题卡 sink：后台预热凭据，提问事件当场建卡。
//!
//! 建立卡片客户端要取一次 tenant access token（一次网络往返），因此既不能放在守护
//! 进程启动路径上同步做（拖慢启动），也不能放在提问所在的 turn 上做（拖慢提问）。
//! 这里在后台线程预热，事件到达时直接取用；预热未完成或凭据不可用时静默跳过，
//! 投递由网关后台 tick 兜底（`remote_feishu_question_card_tick`），且投递本身幂等。

use std::sync::{Arc, Mutex};

use axon_domain::{UserAnswerItem, UserQuestionItem};

use crate::user_question::UserQuestionEventSink;

use super::RemoteCliContext;
use super::remote_feishu_question_action::FeishuQuestionCardSink;
use super::remote_feishu_question_delivery::{
    FeishuHttpQuestionCardWriter, FeishuQuestionCardWriter,
};

/// 预热的飞书问题卡 sink。
pub(crate) struct WarmFeishuQuestionCardSink {
    /// 飞书运行态上下文：卡片表与网关路由表必须落在同一个根目录下。
    context: RemoteCliContext,
    /// 后台预热结果；`None` 表示尚未就绪或通道不可用。
    writer: Arc<Mutex<Option<Arc<dyn FeishuQuestionCardWriter>>>>,
}

impl WarmFeishuQuestionCardSink {
    /// 启动后台预热并返回 sink。
    pub(crate) fn spawn(context: RemoteCliContext) -> Arc<Self> {
        let context = context.feishu_runtime();
        let writer: Arc<Mutex<Option<Arc<dyn FeishuQuestionCardWriter>>>> =
            Arc::new(Mutex::new(None));
        let warm_slot = Arc::clone(&writer);
        let warm_context = context.clone();
        // 预热失败不影响任何主链：提问照常进入等待，卡片由后台 tick 补投。
        std::thread::spawn(move || {
            let connected = FeishuHttpQuestionCardWriter::connect(&warm_context);
            if connected.is_none() {
                axon_logger::warning("飞书问答卡片通道不可用，提问投递交由网关后台 tick 兜底");
            }
            if let Ok(mut slot) = warm_slot.lock() {
                *slot = connected;
            }
        });
        Arc::new(Self { context, writer })
    }

    /// 未预热形态：仅用于测试无 writer 时的降级路径。
    #[cfg(test)]
    pub(crate) fn without_writer(context: RemoteCliContext) -> Self {
        Self {
            context,
            writer: Arc::new(Mutex::new(None)),
        }
    }
}

impl UserQuestionEventSink for WarmFeishuQuestionCardSink {
    fn emit_question_event(
        &self,
        session_id: &str,
        part_id: &str,
        call_id: &str,
        status: &str,
        questions: &[UserQuestionItem],
        answers: &[UserAnswerItem],
        handoff: Option<&str>,
        deadline_at_ms: i64,
    ) {
        let writer = self
            .writer
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone));
        // 未就绪即跳过：网关后台 tick 会按共享存储里的待答记录补投，重复投递幂等。
        let Some(writer) = writer else {
            return;
        };
        FeishuQuestionCardSink::new(&self.context, Some(writer)).emit_question_event(
            session_id,
            part_id,
            call_id,
            status,
            questions,
            answers,
            handoff,
            deadline_at_ms,
        );
    }
}

/// 守护进程装配用的飞书问题卡 sink；飞书未启用时返回 `None`。
///
/// 返回 `Some` 只代表通道已开始预热，不代表凭据一定可用：凭据缺失时 sink 静默跳过，
/// 由后台 tick 兜底，因此调用方无需感知凭据状态。
pub(crate) fn warm_feishu_question_card_sink(
    config_path: std::path::PathBuf,
    axon_root: std::path::PathBuf,
    feishu_websocket: axon_config::RemoteFeishuWebSocketConfig,
) -> Option<Arc<dyn UserQuestionEventSink>> {
    if !feishu_websocket.enabled {
        return None;
    }
    let context = RemoteCliContext::new(config_path, axon_root, feishu_websocket);
    Some(WarmFeishuQuestionCardSink::spawn(context) as Arc<dyn UserQuestionEventSink>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_config::RemoteFeishuWebSocketConfig;

    fn feishu_context(enabled: bool) -> RemoteCliContext {
        let config = RemoteFeishuWebSocketConfig {
            enabled,
            ..RemoteFeishuWebSocketConfig::default()
        };
        RemoteCliContext::new(
            std::path::PathBuf::from("config.toml"),
            axon_test_support::test_temp_root(),
            config,
        )
    }

    #[test]
    fn disabled_feishu_does_not_register_the_sink() {
        let sink = warm_feishu_question_card_sink(
            std::path::PathBuf::from("config.toml"),
            axon_test_support::test_temp_root(),
            RemoteFeishuWebSocketConfig::default(),
        );
        assert!(sink.is_none(), "飞书未启用时不应注册卡片 sink");
    }

    #[test]
    fn enabled_feishu_registers_a_sink() {
        let config = RemoteFeishuWebSocketConfig {
            enabled: true,
            ..RemoteFeishuWebSocketConfig::default()
        };
        let sink = warm_feishu_question_card_sink(
            std::path::PathBuf::from("config.toml"),
            axon_test_support::test_temp_root(),
            config,
        );
        assert!(sink.is_some(), "飞书启用后应注册卡片 sink 并开始预热");
    }

    #[test]
    fn emit_before_warmup_is_a_silent_no_op() {
        // 未预热完成时投递必须无副作用：既不 panic，也不写任何卡片记录。
        let context = feishu_context(true).feishu_runtime();
        let store_root = context.root.clone();
        let sink = WarmFeishuQuestionCardSink::without_writer(feishu_context(true));
        sink.emit_question_event(
            "lane-1",
            "part:question:call-1",
            "call-1",
            "pending",
            &[],
            &[],
            Some("等我"),
            1_000,
        );
        assert!(
            super::super::remote_feishu_question_store::read_cards(&store_root)
                .map(|cards| cards.is_empty())
                .unwrap_or(true),
            "预热未完成时不应写入卡片记录"
        );
    }
}
