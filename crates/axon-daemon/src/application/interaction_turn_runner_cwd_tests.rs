//! 主 turn 工作目录回落：会话行留空时必须取所属 Workspace 的根目录。
//!
//! 网关固定会话不绑定项目工作区，历史行因此可能没有 `cwd`；Provider 拿到空目录会直接
//! 以「工作目录不可访问」失败，所以运行路径必须补齐，而不是把空串原样传下去。

use super::effective_cwd;

#[test]
fn session_cwd_wins_when_present() {
    assert_eq!(effective_cwd("/repo/a", "/root"), "/repo/a");
}

#[test]
fn empty_or_blank_session_cwd_falls_back_to_the_workspace_root() {
    // 网关固定会话的真实形态：建行时 cwd 留空。
    assert_eq!(
        effective_cwd("", "C:/Users/tester/.axon"),
        "C:/Users/tester/.axon"
    );
    assert_eq!(effective_cwd("   ", "/root"), "/root");
}

#[test]
fn fallback_keeps_the_workspace_root_verbatim() {
    // 根目录只做回落，不在这一层归一：路径口径由 WorkspaceResolver 一处决定。
    assert_eq!(effective_cwd("", "E:/repo/axon/"), "E:/repo/axon/");
}
