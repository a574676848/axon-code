//! Axon 人格系统的领域层。
//!
//! 本 crate 承载四层人格架构中除接线以外的全部纯逻辑：
//!
//! - 人格内核：`SOUL.md` 的解析、校验与规范化渲染输入。
//! - 感知适配：场景帧与用户情绪枚举。
//! - 决策调度：人格状态机与表达策略。
//! - 动态表达：表达指令与限频发布的习得风格输入。
//!
//! 领域层零 IO、零框架依赖，全部判定都是纯函数，可由单测完整覆盖。
//! 持久化与提示接线分别由基础设施层与应用层承担，见 [`domain`]。

pub mod application;
pub mod constants;
pub mod domain;
pub use application::{
    exceeds_hard_limit, exceeds_hard_limit_len, exceeds_soft_limit, exceeds_soft_limit_len,
    render_dynamic, render_stable,
};
pub mod infrastructure;

pub use domain::{
    Audience, Channel, Daypart, ExpressionDirective, HumorLevel, Identity, PersonaCore,
    PersonaError, PersonaEvent, PersonaMode, PersonaState, RuleCondition, SceneFrame, SceneRule,
    SoulDocument, SoulError, Transition, TurnFinalStatus, TurnSettledInput, UserAffect, Verbosity,
    Voice, VoicePatch, persona_state::FatigueLevel, resolve_expression, resolve_mode, transition,
};
