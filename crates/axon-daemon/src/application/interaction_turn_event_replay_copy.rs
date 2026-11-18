//! 可回放过程读数的暂存与边界落库。
//!
//! `usageUpdate` 帧按 token 逐帧到达，数值逐帧变化。实时通道照常逐帧投递，持久面只留
//! 每条 provider 流最后一条：暂存是一次内存替换，写入发生在流收口、语义事件前与轮次边界。
//!
//! 与帧分类分开：这里只管「什么时候写」，哪些帧属于这一类由
//! `interaction_turn_event_frame_class` 决定。

use axon_session::RuntimeStore;
use axon_session_runtime::{RuntimeClock, RuntimeEvent};

use super::super::interaction_turn_generation_watch::usage_payload;
use super::RuntimeTurnEventSink;

/// 等待落库的过程读数（当前只有用量快照）。
///
/// 一条 provider 流会按 token 推出成百上千帧用量，数值逐帧变化；全都落库会把事件面
/// 撑成「每 token 一行」。这里只留最后一条，流收口时写一次即可，读数完全等价。
pub(super) struct PendingReplayCopy {
    pub(super) event_type: String,
    pub(super) payload_json: String,
}

impl<'a, S, P, C> RuntimeTurnEventSink<'a, S, P, C> {
    /// 暂存一条可回放的过程读数：同一条流只保留最后一条。
    ///
    /// 载荷由调用方复用实时投递那一份（已带 `interactionId`），这里只做一次字符串替换：
    /// 解析 JSON 与比对同值都推迟到边界那次写入，热路径上每 token 不再多一次解析。
    pub(super) fn stage_replay_copy(&self, event: &RuntimeEvent, payload: String) {
        *self
            .pending_replay_copy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PendingReplayCopy {
            event_type: event.event_type.clone(),
            payload_json: payload,
        });
    }

    /// 把暂存的读数落库：一段 provider 流只写一条，同值不重复写。
    ///
    /// 这是一次边界写入，不在每帧路径上：流收口、语义事件前、轮次边界各调用一次。
    /// 写失败只告警，不阻断本轮投递。
    pub(super) fn flush_replay_copy(&self)
    where
        S: RuntimeStore + Send + Sync,
        C: RuntimeClock,
    {
        let Some(pending) = self
            .pending_replay_copy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        // 同一条流里后面几帧会覆盖前面几帧，因此这里解析的是这条流最后一条读数。
        let body = usage_payload(&pending.payload_json);
        if let Some(body) = &body {
            let same = self
                .last_committed_usage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(|last| last == body);
            if same {
                return;
            }
        }
        match self.store.append_interaction_events(
            self.session_id,
            self.host,
            self.interaction_id,
            self.lease_token,
            &[(pending.event_type, pending.payload_json)],
            self.clock.now_ms(),
        ) {
            Ok(_) => {
                // 只有落库成功才记下这份快照：写失败时后续同值帧仍然值得再试一次。
                if let Some(body) = body {
                    *self
                        .last_committed_usage
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(body);
                }
            }
            Err(error) => {
                axon_logger::warning(&format!("用量帧持久化失败（不影响本轮）: {error:?}"));
            }
        }
    }
}
