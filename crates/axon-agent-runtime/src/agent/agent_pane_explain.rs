use super::{AgentPaneCard, AgentPaneProjection};

impl AgentPaneProjection {
    /// 生成人类可读的中文解释摘要。
    #[must_use]
    pub fn explain(&self) -> String {
        let mut parts = Vec::new();
        for card in &self.cards {
            parts.push(explain_card(card));
        }
        parts.join("")
    }
}

fn explain_card(card: &AgentPaneCard) -> String {
    match card {
        AgentPaneCard::PaneContext {
            cwd,
            shell_kind,
            backend_badge,
            permission_snapshot_id,
        } => format!(
            "当前 pane 使用 {shell_kind}，cwd 为 {cwd}，后端状态 {backend_badge}，权限快照 {}。",
            permission_snapshot_id.0
        ),
        AgentPaneCard::GitState { branch, dirty } => {
            let state = if *dirty {
                "存在未提交变更"
            } else {
                "工作区干净"
            };
            format!("git 分支为 {branch}，{state}。")
        }
        AgentPaneCard::RecentCommand { command, exit_code } => format!(
            "最近命令为 `{command}`，退出码为 {}。",
            exit_code.map_or_else(|| "未知".to_owned(), |code| code.to_string())
        ),
        AgentPaneCard::CommandEvent {
            command,
            exit_code,
            summary,
            source_ref,
            remote_full_content_allowed: _,
        } => format!(
            "命令事件来自 `{command}`，退出码为 {}，来源 {source_ref}，摘要：{summary}。",
            exit_code.map_or_else(|| "未知".to_owned(), |code| code.to_string())
        ),
        AgentPaneCard::RuntimeCell {
            status,
            event_count,
        } => format!("当前 RuntimeCell 状态为 {status}，已记录 {event_count} 个事件。"),
        AgentPaneCard::ChatResponse {
            cell_id,
            preview,
            truncated,
            source_ref,
            remote_full_response_allowed: _,
        } => {
            let suffix = if *truncated { "，预览已截断" } else { "" };
            format!(
                "RuntimeCell {cell_id} 已生成模型回答{suffix}，来源 {source_ref}，预览：{preview}"
            )
        }
        AgentPaneCard::ApplyExecution {
            cell_id,
            path,
            execution_request_id,
            status,
            summary,
            source_ref,
            remote_trigger_allowed: _,
            remote_full_content_allowed: _,
        } => format!(
            "RuntimeCell {cell_id} apply 执行状态为 {status:?}，目标 {path}，执行请求 {execution_request_id}，来源 {source_ref}，摘要：{summary}。"
        ),
    }
}
