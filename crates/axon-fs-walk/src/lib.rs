//! fd 风格文件遍历基础设施。
//!
//! 本 crate 只负责文件系统遍历、目录过滤、上限截断与条目投影，不承载
//! LSP、repo structure 或 TUI 的业务规则。

mod entry;
mod error;
mod policy;
mod skip_modes;
mod walker;

pub use self::entry::{EntryKind, WalkEntry};
pub use self::policy::DirPolicy;
pub use self::skip_modes::CaseMode;
pub use self::walker::{Walk, WalkAction, WalkDepth, WalkOpts, WalkSummary};
