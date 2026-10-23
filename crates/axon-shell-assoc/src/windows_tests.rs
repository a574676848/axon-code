//! Windows 适配器的路径形态验证。
//!
//! 界面实测回归：`resolve_within` 交给 Shell 的是 `canonicalize` 之后的 `\\?\C:\...`，
//! 而 `SHCreateItemFromParsingName` 对这个前缀返回 `E_INVALIDARG`（`0x80070057`，
//! 界面上即「无法为文件建立 Shell 项：参数错误」）。本组用例锁死还原规则：任何一条
//! 回退成直接传原路径，点击「用其他应用打开」就会重新炸掉。
//!
//! 与 `shell_assoc_tests` 分文件是为了守住单文件 700 行上限：那批用例已经接近上限，
//! 而这里的断言只服务 Windows 适配器，放在哪个文件里读起来都不模糊。

use std::path::Path;

use crate::windows::shell_parsing_name;

/// 断言还原结果，并顺手确认它是以 NUL 结尾、内部无 NUL 的宽字符串。
fn parsed(path: &str) -> String {
    let units = shell_parsing_name(Path::new(path)).expect("路径应能还原成 Shell 解析名");
    assert_eq!(units.last(), Some(&0), "解析名必须以 NUL 结尾");
    assert!(
        !units[..units.len() - 1].contains(&0),
        "解析名内部不得出现 NUL"
    );
    String::from_utf16(&units[..units.len() - 1]).expect("测试路径都是合法 UTF-16")
}

#[test]
fn extended_length_drive_paths_are_reduced_for_the_shell() {
    assert_eq!(
        parsed(r"\\?\C:\Temp\toolbox-learning-cleanup.png"),
        r"C:\Temp\toolbox-learning-cleanup.png"
    );
    assert_eq!(parsed(r"\\?\C:\工具\报告.png"), r"C:\工具\报告.png");
}

#[test]
fn extended_length_shared_paths_keep_their_share_head() {
    assert_eq!(
        parsed(r"\\?\UNC\server\share\a.txt"),
        r"\\server\share\a.txt"
    );
}

#[test]
fn extended_length_paths_written_with_forward_slashes_are_reduced_too() {
    assert_eq!(parsed("//?/C:/Temp/a.txt"), r"C:\Temp\a.txt");
}

#[test]
fn ordinary_paths_reach_the_shell_unchanged() {
    assert_eq!(parsed(r"C:\Temp\a.txt"), r"C:\Temp\a.txt");
    assert_eq!(parsed(r"\\server\share\a.txt"), r"\\server\share\a.txt");
}

#[test]
fn volume_paths_report_a_readable_error_instead_of_repeating_the_raw_hr() {
    let error = shell_parsing_name(Path::new(
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\a.txt",
    ))
    .expect_err("卷 GUID 路径无法还原成盘符或共享路径，必须如实报错");
    assert!(
        error.contains("扩展长度路径"),
        "错误信息应说明路径形态而不是裸 HRESULT：{error}"
    );
}
