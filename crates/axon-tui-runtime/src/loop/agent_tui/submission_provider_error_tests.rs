use super::test_support::*;
use super::*;
use crate::surface::{
    cell::RenderCellData,
    error_model::{TuiError, TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity},
};

#[test]
fn provider_tool_call_failure_finishes_as_structured_error_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("继续写入文档"));

    let error = TuiError {
        severity: TuiErrorSeverity::Recoverable,
        action: TuiErrorAction::Retry,
        cell_kind: TuiErrorCellKind::Runtime,
        message: "模型服务返回了不完整的工具调用，本次未执行该调用。".to_owned(),
        error_type: "provider_transport".to_owned(),
        error_code: Some("provider_tool_call_incomplete".to_owned()),
        next_step: Some("从当前会话继续，或切换模型后重新提交。".to_owned()),
        details: vec![
            "代码 provider_tool_call_incomplete".to_owned(),
            "工具调用索引 2".to_owned(),
        ],
    };

    state.finish_submit(Err(RuntimeError::from(error)));

    let cell = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Error(cell)) => Some(cell),
            _ => None,
        })
        .expect("provider tool-call 失败必须进入 ErrorCell");

    assert_eq!(cell.cell_kind, TuiErrorCellKind::Runtime);
    assert_eq!(cell.action, TuiErrorAction::Retry);
    assert!(
        cell.details
            .iter()
            .any(|detail| detail.contains("provider_tool_call_incomplete"))
    );
}
