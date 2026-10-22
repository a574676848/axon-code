//! Context fragment 抽象层。
//!
//! 本 crate 定义了 context fragment 的核心 trait，用于模块化组装模型输入上下文。

#[path = "fragments/fragment.rs"]
mod fragment;

pub use fragment::FragmentRegistration;
pub use fragment::FragmentRegistrationProxy;
pub use fragment::PromptFragment;
pub use fragment::PromptFragmentBudget;
pub use fragment::PromptFragmentLifecycle;
pub use fragment::PromptFragmentMetadata;
pub use fragment::PromptFragmentRole;
pub use fragment::PromptFragmentStability;
