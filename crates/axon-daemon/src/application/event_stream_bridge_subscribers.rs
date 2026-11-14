//! 实时订阅的注册表：谁在听、听到哪、这一帧要不要投。
//!
//! 与 `event_stream_bridge` 的分发逻辑分开成文件，是为了让「订阅者长什么样、
//! 怎么淘汰」独立于「一帧事件投给谁」演进，桥接器本体只保留投递动作。

use super::event_stream_bridge_live_status::RememberedLiveTable;
use super::event_stream_bridge_notifications::StreamNotification;
use axon_domain::{ConsumerCursor, DaemonSessionId, InteractionId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc::SyncSender};

/// 一个实时订阅的投递端点。
#[derive(Debug)]
pub(super) struct Subscriber {
    pub(super) id: u64,
    /// 订阅者关心的 Interaction；`None` 表示**会话级订阅**（该会话的每条事件都投递）。
    ///
    /// 子代理会话不属于任何 Interaction，它的逐字稿没有 interaction 标注，
    /// 因此必须有一条不按 Interaction 过滤的订阅路径，界面才能像主会话一样实时接收。
    pub(super) interaction_id: Option<InteractionId>,
    pub(super) last_cursor: Option<ConsumerCursor>,
    pub(super) sender: SyncSender<StreamNotification>,
    pub(super) pending_reset: Arc<Mutex<Option<PendingStreamReset>>>,
}

#[derive(Debug, Clone)]
pub(super) struct PendingStreamReset {
    pub(super) cursor: Option<ConsumerCursor>,
    pub(super) reason: &'static str,
}

impl Subscriber {
    pub(super) fn request_reset(&self, reason: &'static str) {
        let mut pending = self
            .pending_reset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.is_none() {
            *pending = Some(PendingStreamReset {
                cursor: self.last_cursor.clone(),
                reason,
            });
        }
    }

    pub(super) fn reset_pending(&self) -> bool {
        self.pending_reset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
}

/// 全部活跃订阅，按 Session 分组。
#[derive(Debug, Default)]
pub(super) struct SubscriberRegistry {
    pub(super) by_session: HashMap<DaemonSessionId, Vec<Subscriber>>,
    /// 订阅建立前发出的瞬态读数；「留什么、留多久」见 `event_stream_bridge_live_status`。
    pub(super) remembered_live: RememberedLiveTable,
    pub(super) total: usize,
}
