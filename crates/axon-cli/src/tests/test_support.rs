//! CLI 测试共用的临时根构造。

use std::path::PathBuf;

/// 按用例名派生一个隔离的临时根。
pub(crate) fn test_root(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-cli-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}