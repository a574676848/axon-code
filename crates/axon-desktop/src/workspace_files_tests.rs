//! [`crate::workspace_files`] 的用例。
//!
//! 单独成篇只为守住单文件 700 行硬限：本机分支的断言与实现放在两个文件里，读代码时
//! 不必在 500 行实现之后再翻 150 行用例。分流判定是纯函数，因此这里不碰真实档案，
//! 也不依赖开发机上此刻生效的是哪个档案。

use std::path::Path;

use super::*;

fn scratch() -> PathBuf {
    let root = axon_test_support::test_temp_dir().path().to_path_buf();
    std::fs::create_dir_all(root.join("nested")).expect("创建测试目录");
    std::fs::write(root.join("nested").join("readme.md"), "内容").expect("写入测试文件");
    root
}

#[test]
fn the_local_profile_reads_the_filesystem_and_the_cloud_profile_uses_the_gateway() {
    // 本机档案直读（再多一跳 HTTP 没有收益）；云档案的文件在远端，只能经网关。
    assert_eq!(file_source(false), FileSource::Local);
    assert_eq!(file_source(true), FileSource::Gateway);
}

#[test]
fn local_open_actions_are_refused_under_a_cloud_profile() {
    assert!(
        local_action_rejection(false).is_none(),
        "本机档案下必须放行"
    );
    let message = local_action_rejection(true).expect("云档案下必须拒绝");
    assert_eq!(message, "云档案下不支持在本机打开，请在远端服务上操作");
    assert!(message.contains("云档案"), "{message}");
}

#[test]
fn listing_puts_directories_first_and_reports_relative_paths() {
    let root = scratch();
    let entries = list_entries_locally(&root.to_string_lossy(), "").expect("列目录");
    assert_eq!(entries[0].kind, "dir");
    assert_eq!(entries[0].path, "nested");
    let nested = list_entries_locally(&root.to_string_lossy(), "nested").expect("列子目录");
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].path, "nested/readme.md");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn escaping_the_workspace_root_is_rejected() {
    let root = scratch();
    for escape in ["..", "../..", "nested/../../etc", "C:/Windows"] {
        assert!(
            resolve_within_locally(&root.to_string_lossy(), escape).is_err(),
            "{escape} 必须被拒绝"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tilde_paths_resolve_to_the_user_home() {
    let root = scratch();
    let home = axon_config::user_home_dir().expect("用户主目录");
    let home = home.canonicalize().expect("主目录可解析");
    // 主目录下的一个真实文件：`~` 前缀与 `~/` 前缀都要落到同一处。
    let marker = home.join("axon-tilde-probe.txt");
    std::fs::write(&marker, "tilde").expect("写入探针文件");
    for value in ["~/axon-tilde-probe.txt", "~"] {
        let resolved = resolve_within_locally(&root.to_string_lossy(), value).expect("解析 ~ 路径");
        let expected = if value == "~" {
            home.clone()
        } else {
            marker.clone()
        };
        assert_eq!(resolved, expected, "{value} 必须解析到主目录");
    }
    // 主目录下的绝对路径同样接受，展示成 `~/` 形式。
    let text = read_text_locally(
        &root.to_string_lossy(),
        &marker.to_string_lossy().replace('\\', "/"),
    )
    .expect("按绝对路径读取主目录文件");
    assert_eq!(text.path, "~/axon-tilde-probe.txt");
    // 主目录之外的自由路径仍然拒绝。
    assert!(
        resolve_within_locally(
            &root.to_string_lossy(),
            "C:/Windows/System32/drivers/etc/hosts"
        )
        .is_err(),
        "主目录之外的绝对路径必须被拒绝"
    );
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn binary_files_report_a_readable_conclusion_instead_of_content() {
    let root = scratch();
    std::fs::write(root.join("blob.bin"), [0_u8, 1, 2, 3]).expect("写入二进制文件");
    let text = read_text_locally(&root.to_string_lossy(), "blob.bin").expect("读取二进制文件");
    assert!(text.binary);
    assert!(text.content.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_text_file_reports_both_the_relative_and_the_absolute_path() {
    let root = scratch();
    let text = read_text_locally(&root.to_string_lossy(), "nested/readme.md").expect("读取文本");
    assert_eq!(text.path, "nested/readme.md", "相对路径走正斜杠");
    assert!(
        Path::new(&text.absolute_path).is_absolute(),
        "绝对路径必须是绝对路径：{}",
        text.absolute_path
    );
    assert!(
        text.absolute_path.ends_with("readme.md"),
        "绝对路径必须指向同一个文件：{}",
        text.absolute_path
    );
    assert!(
        !text.absolute_path.contains(r"\\?\") && !text.absolute_path.contains("//?/"),
        "绝对路径不得残留 Windows 扩展前缀：{}",
        text.absolute_path
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cloud_paths_are_reported_in_the_same_shape_as_local_ones() {
    // 云档案的展示路径与绝对路径只做字符串收敛：不碰本机文件系统。
    assert_eq!(cloud_display("C:/work", "nested/a.md"), "nested/a.md");
    assert_eq!(
        cloud_display("C:/work", "C:\\work\\nested\\a.md"),
        "nested/a.md"
    );
    assert_eq!(cloud_display("C:/work", "D:/other/a.md"), "D:/other/a.md");
    // 前缀必须按路径段比对：`C:/work` 不是 `C:/workspace` 的前缀。
    assert_eq!(
        cloud_display("C:/work", "C:/workspace/a.md"),
        "C:/workspace/a.md"
    );
    assert_eq!(
        cloud_absolute("C:/work", "nested/a.md"),
        "C:/work/nested/a.md"
    );
    assert_eq!(cloud_absolute("C:/work", "D:/other/a.md"), "D:/other/a.md");
    assert_eq!(cloud_absolute("C:/work", ""), "C:/work");
}
