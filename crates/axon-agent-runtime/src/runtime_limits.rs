//! Agent runtime 内部运行限制。

use std::time::Duration;

/// Shell 命令默认超时。
pub const DEFAULT_SHELL_TIMEOUT_MS: u64 = 10_000;
/// Shell 命令最小请求超时。
pub const MIN_SHELL_TIMEOUT_MS: u64 = 1_000;
/// 网络查询默认传输超时。
pub const DEFAULT_NETWORK_SEARCH_TIMEOUT_MS: u64 = 10_000;
/// MCP stdio 握手超时。
pub const MCP_HANDSHAKE_TIMEOUT_SECS: u64 = 10;
/// MCP 会话池重启退避基础窗口。
pub const MCP_RESTART_BACKOFF_BASE_MS: u64 = 1_000;
/// MCP 会话池重启退避最大窗口。
pub const MCP_RESTART_BACKOFF_MAX_MS: u64 = 30_000;
/// MCP 会话池重启退避指数上限。
pub const MCP_RESTART_BACKOFF_MAX_EXPONENT: u32 = 5;
/// LSP stdio 单次响应超时。
pub const LSP_RESPONSE_TIMEOUT_SECS: u64 = 30;
/// LSP stdio reader 有界消息队列容量。
pub const LSP_MESSAGE_QUEUE_CAPACITY: usize = 128;
/// 文件工具后台触达 LSP 的有界队列容量。
pub const LSP_BACKGROUND_QUEUE_CAPACITY: usize = 64;
/// LSP push diagnostics 最大等待毫秒数。
pub const LSP_PUSH_DIAGNOSTICS_WAIT_MS: u64 = 2_000;
/// edit 写后 diagnostics 前台等待毫秒数。
pub const LSP_WRITE_DIAGNOSTICS_INLINE_WAIT_MS: u64 = 500;
/// edit 写后 diagnostics 延迟回注最大等待毫秒数。
pub const LSP_WRITE_DIAGNOSTICS_DEFERRED_WAIT_MS: u64 = 12_000;
/// LSP push diagnostics settle 等待毫秒数。
pub const LSP_PUSH_DIAGNOSTICS_SETTLE_MS: u64 = 50;
/// LSP client 空闲回收窗口。
pub const LSP_CLIENT_IDLE_TIMEOUT_SECS: u64 = 120;
/// LSP client 失败退避窗口。
pub const LSP_CLIENT_BROKEN_BACKOFF_SECS: u64 = 20;
/// LSP executable 正负探测缓存窗口。
pub const LSP_EXECUTABLE_CACHE_TTL_SECS: u64 = 30;
/// LSP workspace coverage 缓存容量。
pub const LSP_WORKSPACE_COVERAGE_CACHE_MAX_ENTRIES: usize = 32;
/// LSP workspace coverage 外部变化自愈窗口。
pub const LSP_WORKSPACE_COVERAGE_CACHE_TTL_SECS: u64 = 30;
/// Shell 输出读取块大小。
pub const SHELL_COMMAND_OUTPUT_CHUNK_BYTES: usize = 8 * 1024;
/// Shell 终态收口时等待输出读取线程排干管道的上限。
///
/// 子进程退出信号**早于**管道 EOF，读取线程此刻可能尚未把残余字节写进共享缓冲；
/// 终态回执必须等它排干，否则会发出 `exit_code=0` 却零输出的回执。
/// 上限只约束「读取线程迟迟不结束」的情形（后台守护进程继承了管道），
/// 正常命令在毫秒内返回。
pub const SHELL_COMMAND_OUTPUT_DRAIN_TIMEOUT_MS: u64 = 2_000;
/// Shell 命令完整输出归档的字节上限；达到上限后停止落盘并标记截断。
///
/// 无上限时 `yes`、无限日志等命令会持续写入归档直至磁盘耗尽，
/// 而归档与 Axon 自身日志、会话数据同盘，属难以恢复的故障。
pub const SHELL_ARTIFACT_MAX_BYTES: usize = 64 * 1024 * 1024;
/// Shell 完整输出归档所在的子目录名。
///
/// 归档目录位于运行态日志根之下（`<data>/logs/tee`）；未注入日志根时
/// `create_artifact` 回退到进程临时目录下的同名子目录。
/// **清理侧必须复用本常量**：两处各自硬编码字符串会导致归档永不回收。
pub const SHELL_ARTIFACT_DIR_NAME: &str = "tee";
/// Shell 完整输出归档的文件后缀；清理侧据此识别本工具产生的文件，
/// 从而不触碰同目录下属他人的内容。
pub const SHELL_ARTIFACT_FILE_SUFFIX: &str = ".shell.log";
/// Shell 首次执行与交互调用的最大让出窗口。
pub const MAX_SHELL_INTERACTION_YIELD_MS: u64 = 30_000;
/// 后台终端空输入等待的最小让出窗口。
pub const MIN_BACKGROUND_TERMINAL_WAIT_MS: u64 = 5_000;
/// 后台终端空输入等待的最大让出窗口。
pub const MAX_BACKGROUND_TERMINAL_WAIT_MS: u64 = 300_000;
/// Shell 协作式取消的初始检查间隔。
pub const SHELL_COMMAND_CANCEL_POLL_INTERVAL_MS: u64 = 100;
/// Shell 协作式取消检查间隔的退避上限。
pub const SHELL_COMMAND_CANCEL_POLL_MAX_INTERVAL_MS: u64 = 5_000;
/// Shell 协作式取消检查间隔的退避倍率。
pub const SHELL_COMMAND_CANCEL_POLL_BACKOFF_FACTOR: u32 = 2;
/// 独立 `axon exec` 子进程的状态轮询间隔。
pub const SHELL_COMMAND_POLL_INTERVAL_MS: u64 = 20;
/// Shell / exec 超时退出码。
pub const SHELL_COMMAND_TIMEOUT_EXIT_CODE: i32 = 124;
/// 工具默认重试次数。
pub const DEFAULT_TOOL_RETRY_LIMIT: u8 = 0;
/// 网络查询默认重试次数。
pub const DEFAULT_NETWORK_SEARCH_RETRY_LIMIT: u8 = 1;

// ---------- 临时 Job 生命周期阈值（SSOT 见 docs/tool-schema.md） ----------

/// 作业终态在注册表中的保留时长；窗口内仍可读取终态与退出码。
pub const JOB_TERMINAL_RETENTION: Duration = Duration::from_secs(5 * 60);
/// 已回收作业墓碑的保留时长；须长于 `JOB_TERMINAL_RETENTION`。
pub const JOB_TOMBSTONE_RETENTION: Duration = Duration::from_secs(30 * 60);
/// 墓碑容量上界；超出后按 FIFO 淘汰最旧的记录。
pub const JOB_TOMBSTONE_CAPACITY: usize = 256;
/// 回收线程的推进间隔。
pub const JOB_REAPER_INTERVAL: Duration = Duration::from_secs(1);
/// 单次读取等待的上界，避免模型用超大 yield 卡死整批工具；也是空闲超时判定值。
pub const JOB_MAX_READ_YIELD: Duration = Duration::from_secs(30 * 60);
/// 无输出时的检查间隔。
pub const JOB_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 作业标签的最大保留字符数；仅用于回执展示。
pub const JOB_LABEL_MAX_CHARS: usize = 200;
/// 结果摘要阈值；超过则落盘 artifact 并只回传引用（合同 §5.1.3）。
pub const JOB_SUMMARY_MAX_CHARS: usize = 4_096;
/// 退出原因的有界字符数（合同 §5.1.4）。
pub const JOB_EXIT_REASON_MAX_CHARS: usize = 200;

// ---------- 完成通知阈值（合同 AXON-DSH-JOB-001 §5.1.4） ----------

/// 通知内有界摘要上限。
pub const COMPLETION_NOTICE_SUMMARY_MAX_CHARS: usize = 1_024;
/// 通知内退出原因上限。
pub const COMPLETION_NOTICE_REASON_MAX_CHARS: usize = 200;
/// 单条通知正文上限；超限必须截断，且先丢摘要正文、保留终态与引用。
pub const COMPLETION_NOTICE_MAX_CHARS: usize = 4_096;
/// 单次投递的通知条数上限；超出合并为一条计数摘要。
pub const COMPLETION_NOTICE_MAX_PER_TURN: usize = 8;
/// 临时 Job 通知的保留窗口；同时也是去重键的存活窗口。
pub const COMPLETION_NOTICE_RETENTION: Duration = Duration::from_secs(30 * 60);
/// 去重键窗口的容量上界；超出按 FIFO 淘汰最旧的键。
pub const COMPLETION_NOTICE_DEDUP_CAPACITY: usize = 256;
/// pre-final 守卫至多延迟轮数（合同 §5.1.6）。
pub const PRE_FINAL_GUARD_MAX_DELAY_ROUNDS: u32 = 1;

// ---------- 前台会话回收通知阈值 ----------

/// 会话回收通知的命令与工作目录展示上限。
pub const SHELL_RECLAIM_NOTICE_COMMAND_MAX_CHARS: usize = 300;
/// 单条会话回收通知正文上限；超限截断正文，保留 `session_id` 与终态。
pub const SHELL_RECLAIM_NOTICE_MAX_CHARS: usize = 2_048;
/// 会话回收通知的保留窗口；同时也是去重键的存活窗口。
pub const SHELL_RECLAIM_NOTICE_RETENTION: Duration = Duration::from_secs(30 * 60);
/// 无订阅者时单 owner 暂存的回收通知条数上限；超出按 FIFO 淘汰最旧的记录，
/// 避免 run 拆除后长时间无消费者导致无界增长。
pub const SHELL_RECLAIM_NOTICE_MAX_PENDING_PER_OWNER: usize = 8;
/// 会话回收通知去重键窗口的容量上界；超出按 FIFO 淘汰最旧的键。
pub const SHELL_RECLAIM_NOTICE_DEDUP_CAPACITY: usize = 256;

// ---------- 跨轮 mailbox 与 continuation 阈值（合同 AXON-DSH-JOB-001 §5.3） ----------

/// mailbox 消息正文上限；超限稳定拒绝，避免无界正文进入持久表（合同 §5.3.1）。
pub const MAILBOX_MESSAGE_MAX_CHARS: usize = 4_096;
/// mailbox 消息保留窗口；与完成通知「隔天未读自动失效」口径一致（合同 §5.3.2）。
pub const MAILBOX_MESSAGE_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
/// 单次 mailbox 恢复的条数上限，保证恢复期的读取有界（合同 §5.3.1）。
pub const MAILBOX_HYDRATE_BATCH_LIMIT: u32 = 64;
/// 单次投递重试扫描的条数上限（合同 §5.3.2）。
pub const MAILBOX_DELIVERY_BATCH_LIMIT: u32 = 32;
/// 投递失败的最大尝试次数；达到上限的消息不再重试（合同 §5.3.2）。
pub const MAILBOX_DELIVERY_MAX_ATTEMPTS: u32 = 8;
/// 投递失败后的首次重试退避窗口（合同 §5.3.2）。
pub const MAILBOX_DELIVERY_RETRY_BACKOFF: Duration = Duration::from_secs(30);
/// 指数退避的上限，防止退避窗口无界增长（合同 §5.3.2）。
pub const MAILBOX_DELIVERY_RETRY_MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
/// continuation 终态摘要上限；超阈值的结果只经 artifact 路径引用（合同 §4 不变量 6）。
pub const CONTINUATION_SUMMARY_MAX_CHARS: usize = 1_024;
/// 单次恢复装配注入的 continuation 条数上限（合同 §5.3.3）。
pub const CONTINUATION_ASSEMBLY_BATCH_LIMIT: u32 = 8;
/// 单次装配注入正文的总上限，超限截断（合同 §4 不变量 6）。
pub const CONTINUATION_ASSEMBLY_MAX_CHARS: usize = 4_096;
/// continuation 保留窗口；跨轮恢复窗口须显著长于消息保留窗口（合同 §5.3.3）。
pub const CONTINUATION_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
