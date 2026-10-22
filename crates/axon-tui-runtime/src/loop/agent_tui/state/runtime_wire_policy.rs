use axon_protocol::wire::WireEventKind;

pub(super) fn wire_event_transaction_name(kind: &WireEventKind) -> &'static str {
    match kind {
        WireEventKind::TurnLifecycle(_) => "runtime_event:wire:TurnLifecycle",
        WireEventKind::ProviderStatus(_) => "runtime_event:wire:ProviderStatus",
        WireEventKind::AssistantMessage(_) => "runtime_event:wire:AssistantMessage",
        WireEventKind::ReasoningStart(_) => "runtime_event:wire:ReasoningStart",
        WireEventKind::ReasoningDelta(_) => "runtime_event:wire:ReasoningDelta",
        WireEventKind::ReasoningEnd(_) => "runtime_event:wire:ReasoningEnd",
        WireEventKind::UserMessage(_) => "runtime_event:wire:UserMessage",
        WireEventKind::ToolCall(_) => "runtime_event:wire:ToolCall",
        WireEventKind::ToolResult(_) => "runtime_event:wire:ToolResult",
        WireEventKind::FileChange(_) => "runtime_event:wire:FileChange",
        WireEventKind::PlanUpdate(_) => "runtime_event:wire:PlanUpdate",
        WireEventKind::ReviewStatus(_) => "runtime_event:wire:ReviewStatus",
        WireEventKind::MemoryStatus(_) => "runtime_event:wire:MemoryStatus",
        WireEventKind::SkillStatus(_) => "runtime_event:wire:SkillStatus",
        WireEventKind::VerifyStatus(_) => "runtime_event:wire:VerifyStatus",
        WireEventKind::GovernanceStatus(_) => "runtime_event:wire:GovernanceStatus",
        WireEventKind::UsageUpdate(_) => "runtime_event:wire:UsageUpdate",
        WireEventKind::ResumeSnapshot(_) => "runtime_event:wire:ResumeSnapshot",
        WireEventKind::RuntimeError(_) => "runtime_event:wire:RuntimeError",
        WireEventKind::TokenBudgetReminder(_) => "runtime_event:wire:TokenBudgetReminder",
        WireEventKind::ContextRollover(_) => "runtime_event:wire:ContextRollover",
        WireEventKind::UserQuestion(_) => "runtime_event:wire:UserQuestion",
    }
}
