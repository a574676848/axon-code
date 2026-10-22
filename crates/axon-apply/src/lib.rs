//! Protocol-independent workspace patch execution.

mod apply_patch;

pub use apply_patch::{
    ApplyPatchError, ApplyPatchExecutor, ApplyPatchFileReport, ApplyPatchReport,
    normalize_relative_path, read_workspace_file, transactionally_materialize,
    workspace_target_exists,
};
