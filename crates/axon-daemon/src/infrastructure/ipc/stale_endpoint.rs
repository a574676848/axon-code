//! POSIX 端点文件回收。
//!
//! Windows 的命名管道随进程一起消失，POSIX 的本地 socket 却是磁盘上的真实文件：
//! 守护进程被强杀或崩溃后，`<runroot>/data/axon-daemon` 会留在原处，下一次 bind
//! 直接返回 `EADDRINUSE`。叠加 systemd 的 `Restart=always`，就变成「拉起即失败」
//! 的死循环，只能靠人工执行 `axon daemon start`（那条路径会先清理）才恢复。
//!
//! 回收必须发生在 bind 之前，且判定顺序不能颠倒：
//! **先尝试连接，连得上说明还有活的监听者，直接返回；只有连不上才删文件。**
//! 反过来先删再连，会把正在运行的守护进程端点拆掉。

use std::path::Path;

use interprocess::local_socket::{Stream, prelude::*};

/// 回收无人监听的端点文件。
///
/// 尽力而为：任何一步失败都只是放弃清理，随后 bind 的返回值才是权威结论，
/// 因此这里不返回错误、也不 panic。
pub(super) fn reclaim(endpoint: &str) {
    let path = Path::new(endpoint);
    // 只有绝对路径才落成文件；命名空间端点（Linux 抽象命名空间）没有文件可删。
    if !path.is_absolute() || !path.exists() {
        return;
    }
    let Ok(name) = super::local_socket_name(endpoint) else {
        return;
    };
    if Stream::connect(name).is_ok() {
        return;
    }
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use interprocess::local_socket::{GenericFilePath, ListenerOptions, prelude::*};

    use super::reclaim;

    /// 一个无人监听的普通文件冒充残留端点：回收后必须消失。
    #[test]
    fn reclaims_endpoint_file_that_nobody_listens_on() {
        let temp = axon_test_support::test_temp_dir();
        let path = temp.path().join("stale");
        std::fs::write(&path, b"stale").expect("应能写入占位端点文件");

        reclaim(&path.to_string_lossy());

        assert!(!path.exists(), "无人监听的残留端点必须被回收");
    }

    /// 有活监听者的端点：回收后文件必须仍在，否则会拆掉运行中的守护进程。
    #[test]
    fn keeps_endpoint_file_that_has_a_live_listener() {
        let temp = axon_test_support::test_temp_dir();
        let path = temp.path().join("live");
        let endpoint = path.to_string_lossy().into_owned();
        let name = endpoint
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .expect("端点应能命名");
        let _listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("测试监听应能绑定端点");

        reclaim(&endpoint);

        assert!(Path::new(&endpoint).exists(), "有活监听者的端点不得被删除");
    }
}
