//! TUI 状态一致性审计的公共数据模型。

use std::fmt;

/// TUI transaction 的状态切面名称。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TuiStateSlice {
    Viewport,
    Composer,
    Cells,
    Bridge,
}

impl TuiStateSlice {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Viewport => "viewport",
            Self::Composer => "composer",
            Self::Cells => "cells",
            Self::Bridge => "bridge",
        }
    }
}

/// 单个一致性违规。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TuiInvariantViolation {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) slices: Vec<TuiStateSlice>,
}

impl TuiInvariantViolation {
    pub(crate) fn new(
        code: &'static str,
        message: impl Into<String>,
        slices: impl Into<Vec<TuiStateSlice>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            slices: slices.into(),
        }
    }
}

/// transaction 前后的五段状态摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TuiInvariantStateSnapshot {
    pub(crate) header: String,
    pub(crate) viewport: String,
    pub(crate) composer: String,
    pub(crate) cells: String,
    pub(crate) bridge: String,
}

/// 可定位 action / event / sub-state diff 的 violation dump。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TuiInvariantViolationDump {
    pub(crate) transaction: &'static str,
    pub(crate) before: TuiInvariantStateSnapshot,
    pub(crate) after: TuiInvariantStateSnapshot,
    pub(crate) violations: Vec<TuiInvariantViolation>,
    pub(crate) state_dump_path: Option<String>,
}

impl TuiInvariantViolationDump {
    pub(crate) fn new(
        transaction: &'static str,
        before: TuiInvariantStateSnapshot,
        after: TuiInvariantStateSnapshot,
        violations: Vec<TuiInvariantViolation>,
    ) -> Self {
        Self {
            transaction,
            before,
            after,
            violations,
            state_dump_path: None,
        }
    }

    pub(crate) fn with_state_dump_path(mut self, path: impl Into<String>) -> Self {
        self.state_dump_path = Some(path.into());
        self
    }
}

impl fmt::Display for TuiInvariantViolationDump {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "TUI state invariant violation: {}",
            self.transaction
        )?;
        for violation in &self.violations {
            let slices = violation
                .slices
                .iter()
                .map(|slice| slice.as_str())
                .collect::<Vec<_>>()
                .join(",");
            writeln!(
                formatter,
                "- {} [{}]: {}",
                violation.code, slices, violation.message
            )?;
        }
        if let Some(path) = &self.state_dump_path {
            writeln!(formatter, "state dump: {path}")?;
        }
        writeln!(formatter, "before: {:?}", self.before)?;
        write!(formatter, "after: {:?}", self.after)
    }
}
