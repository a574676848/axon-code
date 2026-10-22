use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolProgressInfo, ToolRuntimeEvent, ToolRuntimeObserver,
};

pub(crate) fn emit_file_progress(
    runtime_observer: &mut Option<&mut ToolRuntimeObserver<'_>>,
    request: &ToolExecutionRequest,
    phase: &str,
    completed: usize,
    total: usize,
    current_item: Option<&str>,
    message: Option<&str>,
) {
    let Some(observer) = runtime_observer.as_deref_mut() else {
        return;
    };
    observer(ToolRuntimeEvent::Progress(ToolProgressInfo {
        tool_call_id: request.tool_call_id,
        part_id: request.part_id.clone(),
        provider_call_id: request.tool_call.id.clone(),
        tool_name: request.tool_call.dispatch_name().to_owned(),
        kind: request.kind,
        phase: phase.to_owned(),
        items_completed: Some(u64::try_from(completed).unwrap_or(u64::MAX)),
        items_total: Some(u64::try_from(total).unwrap_or(u64::MAX)),
        current_item: current_item.map(ToOwned::to_owned),
        elapsed_ms: None,
        message: message.map(ToOwned::to_owned),
    }));
}
