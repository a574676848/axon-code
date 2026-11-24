//! 切档重启机制的断言：参数消费、launcher 专用解析与失败记录的消费语义。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    FAILURE_FILE, FailureRecord, HANDLED_FILE, is_launchable, launcher_relative_path,
    resolve_launcher_at, take_relaunch_argument,
};

fn os(value: &str) -> OsString {
    OsString::from(value)
}

#[test]
fn relaunch_argument_is_consumed_and_never_forwarded() {
    let args = vec![os("--relaunch-after=4242"), os("--other")];
    let (pid, kept) = take_relaunch_argument(args);
    assert_eq!(pid, Some(4242));
    assert_eq!(kept, vec![os("--other")], "该参数必须被吃掉，不得透传");

    let (pid, kept) = take_relaunch_argument(vec![os("--relaunch-after=abc")]);
    assert_eq!(pid, None, "非数字取值不得被当成 pid");
    assert_eq!(kept.len(), 1, "非法取值原样保留，不被静默丢弃");

    let (pid, kept) = take_relaunch_argument(vec![os("--serve")]);
    assert_eq!(pid, None);
    assert_eq!(kept, vec![os("--serve")]);
}

#[test]
fn launcher_path_points_at_the_launcher_directory() {
    let relative = launcher_relative_path();
    assert_eq!(
        relative.parent().and_then(Path::file_name),
        Some(std::ffi::OsStr::new("launcher"))
    );
    let name = relative
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    if cfg!(windows) {
        assert_eq!(name, "axon-launcher.exe");
    } else {
        assert_eq!(name, "axon-launcher");
    }
}

#[test]
fn launcher_resolution_rejects_a_directory_and_a_missing_file() {
    let root = temp_dir("missing");
    let error = resolve_launcher_at(&root).expect_err("缺少 launcher 必须失败");
    assert!(error.contains("稳定入口不可用"), "{error}");

    let directory = root.join(launcher_relative_path());
    fs::create_dir_all(&directory).expect("应可建目录");
    assert!(!is_launchable(&directory), "目录不得被当作 launcher");
    assert!(resolve_launcher_at(&root).is_err());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn launcher_resolution_does_not_fall_back_to_other_candidates() {
    // 同目录下同时放 axon.exe 与 axon.cmd：专用解析只认 launcher，因此仍然失败。
    let root = temp_dir("mixed");
    fs::create_dir_all(root.join("launcher")).expect("目录应可创建");
    fs::write(root.join("axon.cmd"), b"@echo off").expect("axon.cmd 应可写入");
    fs::write(root.join("axon.exe"), b"binary").expect("axon.exe 应可写入");

    let error = resolve_launcher_at(&root).expect_err("混合候选表不得让检查通过");
    assert!(error.contains("axon-launcher"), "{error}");

    // 放上真正的 launcher 之后才成立。
    let launcher = root.join(launcher_relative_path());
    fs::write(&launcher, b"launcher").expect("launcher 应可写入");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(&launcher).expect("元数据").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&launcher, permissions).expect("应可设置可执行位");
    }
    assert!(resolve_launcher_at(&root).is_ok());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn failure_record_round_trips_with_camel_case_keys() {
    let record = FailureRecord {
        target_profile_id: "3f2b".to_owned(),
        previous_profile_id: Some("local".to_owned()),
        reason: "launcher 不可用".to_owned(),
        at_ms: 1760000000000,
    };
    let text = serde_json::to_string(&record).expect("应可序列化");
    assert!(text.contains("\"targetProfileId\""), "{text}");
    assert!(text.contains("\"previousProfileId\""), "{text}");
    let parsed: FailureRecord = serde_json::from_str(&text).expect("应可反序列化");
    assert_eq!(parsed, record);
}

#[test]
fn failure_files_are_named_as_documented() {
    assert_eq!(FAILURE_FILE, "relaunch-failed.json");
    assert_eq!(HANDLED_FILE, "relaunch-failed.handled.json");
}

/// 造一个只属于本次用例的临时目录。
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "axon-relaunch-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
    ));
    fs::create_dir_all(&dir).expect("临时目录应可创建");
    dir
}
