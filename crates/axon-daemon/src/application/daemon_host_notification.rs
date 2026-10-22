//! Daemon Host 的显式飞书通知配置。

use super::{DaemonHost, DaemonHostError};
use crate::ports::ProcessIdentity;
use axon_session::RuntimeStore;
use axon_session_runtime::{EventPublisher, RuntimeClock, TurnExecutor};

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 更新飞书消息通知目标；清空时关闭后台飞书通知。
    pub fn set_feishu_notification_chat_id(
        &self,
        chat_id: Option<&str>,
    ) -> Result<(), DaemonHostError> {
        let target = chat_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        *self
            .feishu_notification_chat_id
            .write()
            .map_err(|_| DaemonHostError::Worker("飞书通知配置锁不可用".to_owned()))? = target;
        Ok(())
    }

    pub(super) fn feishu_notification_target(&self) -> Result<Option<String>, String> {
        self.feishu_notification_chat_id
            .read()
            .map(|target| target.clone())
            .map_err(|_| "飞书通知配置锁不可用".to_owned())
    }
}
