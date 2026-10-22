//! 补丁数据模型与纯计算：文本解析、模糊匹配、变更计划与文本格式继承。
//!
//! 本 crate 只承载**无 I/O、无文件系统访问、无可变全局状态**的补丁模型与纯计算，
//! 由「补丁应用引擎」`axon-apply`、「Agent 运行时」`axon-agent-runtime` 与
//! `axon-cli` 共同依赖，从而消除「应用引擎反向依赖 Agent 运行时」的依赖倒置。
//!
//! 判定标准：任何需要读写文件系统、感知工作区快照或持有运行态资源的逻辑
//! 都不属于本 crate，应留在 `axon-apply`。

mod patch_apply;
mod patch_parser;
mod patch_seek;
mod text_file_format;

pub use patch_apply::{
    MatchLevel, PatchApplyError, PatchFileChange, PatchFileOperation, PatchPlan, WorkspaceSnapshot,
    compute_plan,
};
pub use patch_parser::{PatchHunk, PatchParseError, UpdateFileChunk, parse_patch};
pub use patch_seek::find_best_matching_window;
pub use text_file_format::{
    inherit_text_file_format, text_change_visible_lines, text_file_line_count,
};
