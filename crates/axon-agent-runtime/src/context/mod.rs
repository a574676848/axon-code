//! Context 管理模块。
//!
//! 本模块负责 context fragments 的组装、压缩、缓存和截断。

pub mod fragment_builder;
pub mod fragments;
pub mod prompt_fragment_pipeline;

pub use fragment_builder::{FragmentBuilder, FragmentBuilderConfig};
pub use prompt_fragment_pipeline::PromptFragmentPipeline;
