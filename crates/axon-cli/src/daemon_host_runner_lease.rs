//! 主 turn 租约丢失钩子的装配。
//!
//! 单独成篇：组合根已经接近行数硬限，而这里只是一条回调接线。

use crate::run_commands::run_steer::ActivePrimaryTurnRegistry;

/// 用运行侧的中断入口构造租约丢失钩子。
///
/// 主 turn 的租约一旦被证明不在自己手上，就按与用户按 ESC 同一条取消令牌让本轮在
/// 检查点收口。不接线时这一轮会跑到收尾，然后因为事件写入被租约围栏拒绝而整段输出
/// 落不了地，用户那边只看到一条永远收不了口的轮次。
pub(super) fn lease_lost_hook(
    active_primary_turns: ActivePrimaryTurnRegistry,
) -> axon_daemon::LeaseLostHook {
    axon_daemon::LeaseLostHook::new(move |session_id: &axon_domain::DaemonSessionId| {
        let affinity_key = ActivePrimaryTurnRegistry::affinity_key(session_id.as_str());
        match active_primary_turns.interrupt(&affinity_key) {
            Ok(Some(interaction_id)) => axon_logger::warning(&format!(
                "主 turn 租约丢失，已请求取消: session={}, interaction={}",
                session_id.as_str(),
                interaction_id.as_str()
            )),
            Ok(None) => axon_logger::warning(&format!(
                "主 turn 租约丢失，但该车道没有活跃轮次可取消: session={}",
                session_id.as_str()
            )),
            Err(error) => axon_logger::error(&format!(
                "主 turn 租约丢失后请求取消失败: session={}, error={error}",
                session_id.as_str()
            )),
        }
    })
}
