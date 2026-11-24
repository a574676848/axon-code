//! 人格系统的应用层：把领域对象渲染为提示片段，并校验习得风格候选。

pub mod render;
pub mod style_candidate;

pub use render::{
    exceeds_hard_limit, exceeds_hard_limit_len, exceeds_soft_limit, exceeds_soft_limit_len,
    render_dynamic, render_stable,
};
pub use style_candidate::{StyleCandidateError, validate_style_candidate};
