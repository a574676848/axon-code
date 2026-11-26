//! 自动换窗阈值：触发线、fallback 缓冲，以及提示发出后的阈值降级。

use super::*;

#[test]
fn test_should_compress() {
    let runtime = WindowRolloverPolicyRuntime::new(policy(100));
    let system = AgentModelMessage::context("system", "x".repeat(4_000));

    // 前缀之外不足 2 条可压缩消息：不触发。
    let only_prefix = vec![system.clone(), message_with_chars("user", 4_000)];
    assert!(!runtime.should_compress(&only_prefix, Some(100)));

    // 达到 trigger 后先进入 fallback 缓冲，直到完整窗口边界才强制轮转。
    let compressible = vec![
        system,
        message_with_chars("user", 200),
        message_with_chars("assistant", 200),
    ];
    assert!(!runtime.should_compress(&compressible, Some(100)));
    assert!(!runtime.should_compress(&compressible, Some(199)));
    assert!(runtime.should_compress(&compressible, Some(200)));

    // 未达到阈值：不触发。
    let below = vec![
        message_with_chars("system", 10),
        message_with_chars("user", 10),
        message_with_chars("assistant", 10),
    ];
    assert!(!runtime.should_compress(&below, Some(99)));
    assert!(!runtime.should_compress(&compressible, Some(0)));
    assert!(!runtime.should_compress(&compressible, None));
}

/// 发过「窗口已耗尽」提示后阈值降回触发线：下一迭代直接换窗，不等模型自觉调用工具。
///
/// 真机现场：模型写完便签只在答复里说「现在换窗继续」，没有调用 builtin.new_context，
/// 本轮正常收口，任务半途停下。
#[test]
fn fallback_notice_lowers_the_rollover_threshold() {
    let mut runtime = WindowRolloverPolicyRuntime::new(policy(100));
    let compressible = vec![
        AgentModelMessage::context("system", "x".repeat(4_000)),
        message_with_chars("user", 200),
        message_with_chars("assistant", 200),
    ];

    // 提示未发：199 只是越过触发线，仍留在 fallback 缓冲里。
    assert!(!runtime.should_compress(&compressible, Some(199)));

    runtime.mark_fallback_emitted_for_test();
    assert!(
        runtime.should_compress(&compressible, Some(199)),
        "提示已发过就必须由运行时自动换窗"
    );
}
