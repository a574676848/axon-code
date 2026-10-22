//! 后台作业运行时：与 Shell 交互式会话解耦的执行、读数、终止与回收。
//!
//! Axon 原先把「后台长任务」寄生在 Shell 会话上，导致并发策略的爆炸半径过大：
//! `write_stdin` 声明为 `Exclusive`，而 `tool_calls_can_run_in_parallel`
//! 只要看到**一个** `Exclusive` 就让**整批**工具调用退化为串行：包括同批中
//! 本来完全独立的只读工具。
//!
//! 根因是「交互式会话」与「后台长任务」是两种**不同**的资源，被压缩进了同一个抽象。
//! 本模块把后者独立出来，并按合同 AXON-DSH-JOB-001 §5.1.1 抽出统一的生产者内核：
//! Shell 后台作业与一次性后台子代理注册进**同一**注册表，共用同一套身份分配、
//! owner 校验、读取/等待、终止、终态保留、归档与墓碑回收。
//!
//! | 文件 | 职责 |
//! |---|---|
//! | [`registry`] | 注册表、身份分配、owner 校验、保留与墓碑、回收线程 |
//! | [`producer`] | 生产者接口、执行体接口与统一终态投影 |
//! | [`shell_producer`] | Shell 后台作业（统一生命周期的第一个实现） |
//! | [`subagent_producer`] | 一次性后台子代理（结果落盘与引用回传） |
//!
//! **不存在双通道**：作业注册表与会话注册表相互独立，作业 ID 永远不会被
//! `write_stdin` 接受，`session_id` 也永远不会被 `job_output` 接受。
//! 两者由 `shell_command` 的 `run_in_background` 与 `tty` **硬互斥**分流。
//!
//! 双层生命周期边界（合同 §4 不变量 8）：临时 Job 是进程内、单 run、随进程消亡的
//! 事实源；Goal/Execution 由 Work Ledger 拥有。本模块**不写入** `jarvis_goals` 或
//! `jarvis_executions`，也不承载跨轮任务状态。

mod notice;
mod producer;
mod registry;
mod shell_producer;
mod subagent_producer;

pub use notice::{
    CompletionNotice, CompletionNoticeBatch, CompletionNoticeHandler, CompletionNoticeSubscription,
    publish_completion_notice, publish_completion_notices, subscribe_completion_notices,
};
pub use producer::{
    JOB_REASON_BUDGET_EXHAUSTED, JOB_REASON_USER_STOP, JobProducerKind, JobTerminalKind,
};
pub use registry::{
    JobId, JobOutput, JobRuntime, JobStatus, bounded_job_label, job_reference, runtime_epoch,
};
pub use shell_producer::JobSpec;
pub use subagent_producer::{
    SubagentJobHandle, SubagentJobInterrupt, SubagentJobOutcome, SubagentJobSpec,
    submit_subagent_job,
};
