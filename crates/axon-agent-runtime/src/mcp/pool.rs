//! 插件 MCP 会话池与重启退避治理。

use axon_domain::{AxonError, AxonResult, PaneSecurityBadge, PaneSecurityBadgeLevel};

use crate::{McpServerHealth, McpServerHealthStatus, McpStdioSessionReport};

/// MCP 会话池 doctor 检查名称。
pub const MCP_POOL_DOCTOR_CHECK_NAME: &str = "mcp.pool";
/// MCP 会话池后台保活间隔。
pub const MCP_KEEPALIVE_INTERVAL_MS: u64 = 15_000;
pub use crate::runtime_limits::{
    MCP_RESTART_BACKOFF_BASE_MS, MCP_RESTART_BACKOFF_MAX_EXPONENT, MCP_RESTART_BACKOFF_MAX_MS,
};
/// 会话复用原因。
pub const MCP_POOL_REASON_SESSION_REUSED: &str = "mcp_pool:session_reused";
/// 并发请求复用现有会话原因。
pub const MCP_POOL_REASON_CONCURRENT_REUSED: &str = "mcp_pool:concurrent_reused";
/// 重启被退避窗口阻断原因。
pub const MCP_POOL_REASON_RESTART_BACKOFF: &str = "mcp_pool:restart_backoff";
/// 本机重启已通过退避治理原因。
pub const MCP_POOL_REASON_RESTART_ALLOWED: &str = "mcp_pool:restart_allowed";
/// 崩溃记录进入池治理原因。
pub const MCP_POOL_REASON_CRASH_RECORDED: &str = "mcp_pool:crash_recorded";
/// 后台保活确认会话仍在运行。
pub const MCP_POOL_REASON_KEEPALIVE_RUNNING: &str = "mcp_pool:keepalive_running";
/// 后台保活发现会话已退出。
pub const MCP_POOL_REASON_KEEPALIVE_EXITED: &str = "mcp_pool:keepalive_exited";
/// 退避遥测处于活跃窗口。
pub const MCP_POOL_REASON_BACKOFF_TELEMETRY: &str = "mcp_pool:backoff_telemetry";
/// doctor 投影显示会话池健康。
pub const MCP_POOL_REASON_DOCTOR_READY: &str = "mcp_pool:doctor_ready";
/// doctor 投影显示会话池降级。
pub const MCP_POOL_REASON_DOCTOR_DEGRADED: &str = "mcp_pool:doctor_degraded";

/// MCP 会话池动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPoolAction {
    /// 注册新的可复用会话。
    Registered,
    /// 复用已有空闲会话。
    ReusedIdle,
    /// 并发请求复用同一会话，但必须串行执行。
    ReusedConcurrent,
    /// 重启被退避窗口阻断。
    RestartBackoffBlocked,
    /// 允许本机 runtime 重启。
    RestartAllowed,
    /// 记录崩溃并打开退避窗口。
    CrashRecorded,
    /// 后台保活已观测会话健康。
    KeepAliveObserved,
}

/// MCP 会话池后台保活状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPoolKeepAliveStatus {
    /// 会话仍在运行。
    Healthy,
    /// 会话已退出并打开退避窗口。
    BackoffOpened,
}

/// MCP 会话池 doctor 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPoolDoctorStatus {
    /// 会话池健康。
    Ok,
    /// 会话池降级但主循环继续。
    Degraded,
}

/// MCP 会话池遥测事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpPoolTelemetryEvent {
    /// 后台保活已观测。
    KeepAliveObserved {
        /// MCP server ID。
        server_id: String,
    },
    /// 退避窗口已观测。
    RestartBackoffObserved {
        /// MCP server ID。
        server_id: String,
        /// 退避截止时间。
        backoff_until_ms: u64,
    },
    /// doctor 投影已更新。
    DoctorProjectionUpdated {
        /// MCP server ID。
        server_id: String,
    },
}

/// MCP 会话池退避遥测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPoolTelemetry {
    /// MCP server ID。
    pub server_id: String,
    /// 活跃会话数量。
    pub active_session_count: usize,
    /// 正忙会话数量。
    pub busy_session_count: usize,
    /// 连续失败次数。
    pub failure_count: u32,
    /// 剩余退避毫秒。
    pub backoff_remaining_ms: Option<u64>,
    /// 是否允许本机重启。
    pub restart_allowed: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 遥测事件。
    pub events: Vec<McpPoolTelemetryEvent>,
}

/// MCP 会话池 doctor 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPoolDoctorProjection {
    /// 检查名称。
    pub check_name: String,
    /// MCP server ID。
    pub server_id: String,
    /// doctor 状态。
    pub status: McpPoolDoctorStatus,
    /// pane / doctor 可复用 badge。
    pub badge: PaneSecurityBadge,
    /// 中文说明。
    pub message: String,
    /// 活跃会话数量。
    pub active_session_count: usize,
    /// 退避截止时间。
    pub backoff_until_ms: Option<u64>,
    /// terminal 主循环是否继续。
    pub terminal_main_loop_continues: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
}

/// MCP 会话池后台保活投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPoolKeepAliveProjection {
    /// MCP server ID。
    pub server_id: String,
    /// 保活状态。
    pub status: McpPoolKeepAliveStatus,
    /// 进程 ID。
    pub process_id: Option<u32>,
    /// 下一次保活检查时间。
    pub next_check_at_ms: Option<u64>,
    /// 会话池决策。
    pub decision: McpSessionPoolDecision,
    /// 退避遥测。
    pub telemetry: McpPoolTelemetry,
    /// doctor 投影。
    pub doctor: McpPoolDoctorProjection,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 插件 MCP 池化会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPooledSession {
    /// MCP server ID。
    pub server_id: String,
    /// 进程 ID。
    pub process_id: u32,
    /// 工具清单。
    pub listed_tools: Vec<String>,
    /// 会话是否正被请求占用。
    pub busy: bool,
    /// 会话代际。
    pub generation: u64,
    /// 最近一次健康状态。
    pub health_status: McpServerHealthStatus,
    /// 注册时间。
    pub registered_at_ms: u64,
}

/// MCP 重启退避状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRestartBackoff {
    /// MCP server ID。
    pub server_id: String,
    /// 连续失败次数。
    pub failure_count: u32,
    /// 下一次允许本机重启的时间。
    pub next_restart_at_ms: u64,
}

/// MCP 会话池决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSessionPoolDecision {
    /// MCP server ID。
    pub server_id: String,
    /// 决策动作。
    pub action: McpPoolAction,
    /// 复用的进程 ID。
    pub process_id: Option<u32>,
    /// 是否复用已有会话。
    pub reused_existing_session: bool,
    /// 是否需要串行等待。
    pub requires_serial_wait: bool,
    /// 是否允许本机重启。
    pub restart_allowed: bool,
    /// 退避截止时间。
    pub backoff_until_ms: Option<u64>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// MCP 会话池。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct McpSessionPool {
    sessions: Vec<McpPooledSession>,
    backoffs: Vec<McpRestartBackoff>,
    next_generation: u64,
}

impl McpSessionPool {
    /// 注册握手成功后的会话。
    pub fn register_session(
        &mut self,
        report: &McpStdioSessionReport,
        now_ms: u64,
    ) -> AxonResult<McpSessionPoolDecision> {
        if !report.discovered
            || !report.tools_listed
            || report.health.status != McpServerHealthStatus::Running
        {
            return Err(AxonError::InvalidMcpDefinition(
                "只有已完成握手且运行中的 MCP 会话能进入池",
            ));
        }
        self.next_generation += 1;
        let session = McpPooledSession {
            server_id: report.server_id.clone(),
            process_id: report.process_id,
            listed_tools: report.listed_tools.clone(),
            busy: false,
            generation: self.next_generation,
            health_status: report.health.status,
            registered_at_ms: now_ms,
        };
        self.sessions
            .retain(|existing| existing.server_id != report.server_id);
        self.sessions.push(session);
        self.backoffs
            .retain(|backoff| backoff.server_id != report.server_id);

        Ok(McpSessionPoolDecision {
            server_id: report.server_id.clone(),
            action: McpPoolAction::Registered,
            process_id: Some(report.process_id),
            reused_existing_session: false,
            requires_serial_wait: false,
            restart_allowed: false,
            backoff_until_ms: None,
            remote_trigger_allowed: false,
            reasons: vec![MCP_POOL_REASON_SESSION_REUSED.to_owned()],
        })
    }

    /// 为请求获取会话；并发请求复用现有会话并要求串行等待。
    pub fn acquire_request(&mut self, server_id: &str) -> AxonResult<McpSessionPoolDecision> {
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.server_id == server_id)
            .ok_or(AxonError::InvalidMcpDefinition("MCP 会话池中没有可用会话"))?;
        let concurrent = session.busy;
        session.busy = true;

        Ok(McpSessionPoolDecision {
            server_id: server_id.to_owned(),
            action: if concurrent {
                McpPoolAction::ReusedConcurrent
            } else {
                McpPoolAction::ReusedIdle
            },
            process_id: Some(session.process_id),
            reused_existing_session: true,
            requires_serial_wait: concurrent,
            restart_allowed: false,
            backoff_until_ms: None,
            remote_trigger_allowed: false,
            reasons: vec![if concurrent {
                MCP_POOL_REASON_CONCURRENT_REUSED.to_owned()
            } else {
                MCP_POOL_REASON_SESSION_REUSED.to_owned()
            }],
        })
    }

    /// 释放请求占用。
    pub fn release_request(&mut self, server_id: &str) -> AxonResult<()> {
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.server_id == server_id)
            .ok_or(AxonError::InvalidMcpDefinition(
                "MCP 会话池中没有可释放会话",
            ))?;
        session.busy = false;
        Ok(())
    }

    /// 记录崩溃并计算退避窗口。
    pub fn record_crash(&mut self, server_id: &str, now_ms: u64) -> McpSessionPoolDecision {
        self.sessions
            .retain(|session| session.server_id != server_id);
        let next_failure_count = self
            .backoffs
            .iter()
            .find(|backoff| backoff.server_id == server_id)
            .map_or(1, |backoff| backoff.failure_count.saturating_add(1));
        let next_restart_at_ms = now_ms + restart_backoff_ms(next_failure_count);
        self.backoffs
            .retain(|backoff| backoff.server_id != server_id);
        self.backoffs.push(McpRestartBackoff {
            server_id: server_id.to_owned(),
            failure_count: next_failure_count,
            next_restart_at_ms,
        });

        McpSessionPoolDecision {
            server_id: server_id.to_owned(),
            action: McpPoolAction::CrashRecorded,
            process_id: None,
            reused_existing_session: false,
            requires_serial_wait: false,
            restart_allowed: false,
            backoff_until_ms: Some(next_restart_at_ms),
            remote_trigger_allowed: false,
            reasons: vec![MCP_POOL_REASON_CRASH_RECORDED.to_owned()],
        }
    }

    /// 判断是否允许本机 runtime 重启。
    #[must_use]
    pub fn restart_decision(&self, server_id: &str, now_ms: u64) -> McpSessionPoolDecision {
        if let Some(backoff) = self
            .backoffs
            .iter()
            .find(|backoff| backoff.server_id == server_id)
            && now_ms < backoff.next_restart_at_ms
        {
            return McpSessionPoolDecision {
                server_id: server_id.to_owned(),
                action: McpPoolAction::RestartBackoffBlocked,
                process_id: None,
                reused_existing_session: false,
                requires_serial_wait: false,
                restart_allowed: false,
                backoff_until_ms: Some(backoff.next_restart_at_ms),
                remote_trigger_allowed: false,
                reasons: vec![MCP_POOL_REASON_RESTART_BACKOFF.to_owned()],
            };
        }
        McpSessionPoolDecision {
            server_id: server_id.to_owned(),
            action: McpPoolAction::RestartAllowed,
            process_id: None,
            reused_existing_session: false,
            requires_serial_wait: false,
            restart_allowed: true,
            backoff_until_ms: None,
            remote_trigger_allowed: false,
            reasons: vec![MCP_POOL_REASON_RESTART_ALLOWED.to_owned()],
        }
    }

    /// 投影后台保活 tick；只消费已知会话的健康快照，不启动远控恢复。
    pub fn project_keepalive(
        &mut self,
        health: &McpServerHealth,
        now_ms: u64,
    ) -> AxonResult<McpPoolKeepAliveProjection> {
        if !self
            .sessions
            .iter()
            .any(|session| session.server_id == health.server_id)
        {
            return Err(AxonError::InvalidMcpDefinition(
                "MCP 会话池中没有可保活会话",
            ));
        }

        if health.status == McpServerHealthStatus::Running {
            let session = self
                .sessions
                .iter_mut()
                .find(|session| session.server_id == health.server_id)
                .ok_or(AxonError::InvalidMcpDefinition(
                    "MCP 会话池中没有可保活会话",
                ))?;
            session.health_status = health.status;
            return Ok(self.keepalive_healthy_projection(health, now_ms));
        }

        let decision = self.record_crash(&health.server_id, now_ms);
        let telemetry = self.telemetry(&health.server_id, now_ms);
        let doctor = self.doctor_projection(&health.server_id, now_ms);
        Ok(McpPoolKeepAliveProjection {
            server_id: health.server_id.clone(),
            status: McpPoolKeepAliveStatus::BackoffOpened,
            process_id: Some(health.process_id),
            next_check_at_ms: decision.backoff_until_ms,
            decision,
            telemetry,
            doctor,
            remote_trigger_allowed: false,
            reasons: vec![MCP_POOL_REASON_KEEPALIVE_EXITED.to_owned()],
        })
    }

    /// 生成 MCP 会话池退避遥测。
    #[must_use]
    pub fn telemetry(&self, server_id: &str, now_ms: u64) -> McpPoolTelemetry {
        let active_session_count = self
            .sessions
            .iter()
            .filter(|session| session.server_id == server_id)
            .count();
        let busy_session_count = self
            .sessions
            .iter()
            .filter(|session| session.server_id == server_id && session.busy)
            .count();
        let backoff = self
            .backoffs
            .iter()
            .find(|backoff| backoff.server_id == server_id);
        let failure_count = backoff.map_or(0, |backoff| backoff.failure_count);
        let backoff_remaining_ms = backoff
            .and_then(|backoff| backoff.next_restart_at_ms.checked_sub(now_ms))
            .filter(|remaining| *remaining > 0);
        let mut events = vec![McpPoolTelemetryEvent::KeepAliveObserved {
            server_id: server_id.to_owned(),
        }];
        if let Some(backoff) = backoff
            && backoff_remaining_ms.is_some()
        {
            events.push(McpPoolTelemetryEvent::RestartBackoffObserved {
                server_id: server_id.to_owned(),
                backoff_until_ms: backoff.next_restart_at_ms,
            });
        }
        events.push(McpPoolTelemetryEvent::DoctorProjectionUpdated {
            server_id: server_id.to_owned(),
        });

        McpPoolTelemetry {
            server_id: server_id.to_owned(),
            active_session_count,
            busy_session_count,
            failure_count,
            backoff_remaining_ms,
            restart_allowed: backoff_remaining_ms.is_none(),
            remote_trigger_allowed: false,
            events,
        }
    }

    /// 生成 MCP 会话池 doctor 投影。
    #[must_use]
    pub fn doctor_projection(&self, server_id: &str, now_ms: u64) -> McpPoolDoctorProjection {
        let telemetry = self.telemetry(server_id, now_ms);
        let backoff = self
            .backoffs
            .iter()
            .find(|backoff| backoff.server_id == server_id);
        let backoff_until_ms = backoff.and_then(|backoff| {
            (backoff.next_restart_at_ms > now_ms).then_some(backoff.next_restart_at_ms)
        });
        let degraded = backoff_until_ms.is_some();
        let status = if degraded {
            McpPoolDoctorStatus::Degraded
        } else {
            McpPoolDoctorStatus::Ok
        };
        let reason = if degraded {
            MCP_POOL_REASON_DOCTOR_DEGRADED
        } else {
            MCP_POOL_REASON_DOCTOR_READY
        };
        let badge_level = if degraded {
            PaneSecurityBadgeLevel::Degraded
        } else {
            PaneSecurityBadgeLevel::Normal
        };
        let badge_label = if degraded { "degraded" } else { "normal" };
        let message = if let Some(until_ms) = backoff_until_ms {
            let remaining_ms = until_ms.saturating_sub(now_ms);
            format!(
                "插件 MCP 会话池退避中，失败 {} 次，重试 {} 后",
                telemetry.failure_count,
                axon_display::format_duration_millis_compact(remaining_ms)
            )
        } else {
            format!(
                "插件 MCP 会话池健康，active_sessions={}，busy_sessions={}",
                telemetry.active_session_count, telemetry.busy_session_count
            )
        };

        McpPoolDoctorProjection {
            check_name: MCP_POOL_DOCTOR_CHECK_NAME.to_owned(),
            server_id: server_id.to_owned(),
            status,
            badge: PaneSecurityBadge {
                level: badge_level,
                label: badge_label.to_owned(),
                reasons: vec![reason.to_owned()],
            },
            message,
            active_session_count: telemetry.active_session_count,
            backoff_until_ms,
            terminal_main_loop_continues: true,
            remote_trigger_allowed: false,
        }
    }

    /// 当前会话数量。
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    fn keepalive_healthy_projection(
        &self,
        health: &McpServerHealth,
        now_ms: u64,
    ) -> McpPoolKeepAliveProjection {
        let telemetry = self.telemetry(&health.server_id, now_ms);
        let doctor = self.doctor_projection(&health.server_id, now_ms);
        McpPoolKeepAliveProjection {
            server_id: health.server_id.clone(),
            status: McpPoolKeepAliveStatus::Healthy,
            process_id: Some(health.process_id),
            next_check_at_ms: Some(now_ms.saturating_add(MCP_KEEPALIVE_INTERVAL_MS)),
            decision: McpSessionPoolDecision {
                server_id: health.server_id.clone(),
                action: McpPoolAction::KeepAliveObserved,
                process_id: Some(health.process_id),
                reused_existing_session: true,
                requires_serial_wait: false,
                restart_allowed: false,
                backoff_until_ms: None,
                remote_trigger_allowed: false,
                reasons: vec![MCP_POOL_REASON_KEEPALIVE_RUNNING.to_owned()],
            },
            telemetry,
            doctor,
            remote_trigger_allowed: false,
            reasons: vec![MCP_POOL_REASON_KEEPALIVE_RUNNING.to_owned()],
        }
    }
}

fn restart_backoff_ms(failure_count: u32) -> u64 {
    let exponent = failure_count
        .saturating_sub(1)
        .min(MCP_RESTART_BACKOFF_MAX_EXPONENT);
    let multiplier = 1_u64 << exponent;
    MCP_RESTART_BACKOFF_BASE_MS
        .saturating_mul(multiplier)
        .min(MCP_RESTART_BACKOFF_MAX_MS)
}
