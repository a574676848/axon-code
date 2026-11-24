//! 自动换窗失败退避的回归测试。

use crate::AgentModelMessage;
use crate::insert_then_compress::{
    CompressionOutcome, CompressionPolicy, WindowRolloverPolicyRuntime,
};

/// 自动换窗连续失败必须退避：同一读数不得逐轮重试。
///
/// 失败原因通常在同一上下文读数上持续存在，逐轮重试会让每轮都付一次整份语料的
/// 归档与回滚成本；而读数单调增长，退避线最终一定会被越过，自救不会永久放弃。
#[test]
fn repeated_auto_rollover_failures_back_off_until_context_grows() {
    let mut runtime = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: 1_000_000,
        context_window: 2_000_000,
    });
    let messages = vec![
        AgentModelMessage::text("user", "目标"),
        AgentModelMessage::text("assistant", "进展"),
    ];
    // 强制线是 trigger 加回退缓冲，这里取一个确定越线的读数。
    let failed_at = 1_100_000_u64;
    assert!(
        runtime.should_compress(&messages, Some(failed_at)),
        "越过强制线应触发自动换窗"
    );

    runtime.note_auto_rollover_failure(failed_at);
    assert!(
        !runtime.should_compress(&messages, Some(failed_at)),
        "同一读数不得立即重试"
    );
    assert!(
        !runtime.should_compress(&messages, Some(failed_at + 4_095)),
        "未涨过一个退避步长仍不重试"
    );
    assert!(
        runtime.should_compress(&messages, Some(failed_at + 4_096)),
        "涨过退避线后必须再试"
    );
}

/// 换窗成功后失败计数清零，新窗口不受旧窗口的退避影响。
#[test]
fn successful_rollover_clears_failure_backoff() {
    let mut runtime = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: 1_000_000,
        context_window: 2_000_000,
    });
    let messages = vec![
        AgentModelMessage::text("user", "目标"),
        AgentModelMessage::text("assistant", "进展"),
    ];
    let failed_at = 1_100_000_u64;
    runtime.note_auto_rollover_failure(failed_at);
    assert!(!runtime.should_compress(&messages, Some(failed_at)));

    runtime.record_outcome(CompressionOutcome::Compressed);
    assert!(
        runtime.should_compress(&messages, Some(failed_at)),
        "换窗成功后失败计数必须清零"
    );
}
