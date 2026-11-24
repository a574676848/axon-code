//! Axon setup 的纯配置规划与文件补丁原语。
//!
//! 这里不依赖 CLI、终端或 Axon runtime；CLI 只负责把参数转换成这些数据结构，
//! bootstrap/Axon runtime 再负责提交事务。

mod intent;
mod patch;
mod planner;
mod validator;

#[cfg(test)]
mod tests;

pub use intent::{
    ExplicitEntry, SetupActions, SetupExplicitEntries, SetupIntent, SetupPlannerOptions,
    SetupSource,
};
pub use patch::{
    EnvKeyChange, SETUP_SECRET_MISSING, SETUP_SECRET_PRESENT, render_env_from, secret_status,
};
pub use planner::{SetupPlan, SetupPlanner};
