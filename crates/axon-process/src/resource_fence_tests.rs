//! 资源围栏的单元验证：只覆盖不启动进程的纯函数与不变量。
//!
//! 需要真实子进程才能观测的行为（内核接受了什么限额、超限写入是否被终止）
//! 见 `tests/resource_fence_probe.rs`。

use std::path::PathBuf;

use super::*;
// 限额解析与 PTY 包装解析已移入子模块（INV-7 拆分），测试按模块路径引入。
use super::pty_wrapper::{find_program, pty_limit_arguments};
use super::resource_fence_limits::{apply_environment_override, parse_environment_limit};

/// 本 crate 源码根目录。
const SOURCE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

/// 唯一允许出现 `unsafe` 的源文件。
const UNSAFE_SOURCE_FILE: &str = "resource_fence.rs";

/// 判断源码是否**实际声明**了不安全代码，而不是仅在文档中提到该关键字。
///
/// 只认三种形式：`unsafe` 块、`unsafe impl`、`unsafe fn`，且关键字必须出现在
/// 代码行而非文档注释行，避免 `/// ... 唯一允许 unsafe 的模块` 这类说明造成假阳性。
fn declares_unsafe_code(source: &str) -> bool {
    source.lines().any(|line| {
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with('*') {
            return false;
        }
        ["unsafe {", "unsafe impl ", "unsafe fn ", "unsafe extern "]
            .iter()
            .any(|marker| code.contains(marker))
    })
}

/// 不属于生产源码的目录名。
const NON_SOURCE_DIRECTORIES: &[&str] = &["bin"];

#[test]
fn deployment_defaults_are_constant_and_finite_where_required() {
    let limits = ProcessResourceLimits::deployment_default();
    assert_eq!(limits.max_file_bytes, Some(DEFAULT_RLIMIT_FSIZE_BYTES));
    assert_eq!(limits.max_processes, Some(DEFAULT_RLIMIT_NPROC));
    assert_eq!(limits.max_open_files, Some(DEFAULT_RLIMIT_NOFILE));
    assert_eq!(limits.max_address_space_bytes, DEFAULT_RLIMIT_AS_BYTES);
    assert!(!limits.is_off(), "部署默认值必须真的设限，否则围栏形同虚设");
}

#[test]
fn interactive_defaults_are_wider_than_deployment_defaults() {
    let interactive = ProcessResourceLimits::interactive_default();
    let deployment = ProcessResourceLimits::deployment_default();
    assert!(
        interactive.max_processes > deployment.max_processes,
        "PTY 交互式会话的进程数上限必须比一次性命令更宽"
    );
    assert!(interactive.max_file_bytes > deployment.max_file_bytes);
    assert!(interactive.max_open_files > deployment.max_open_files);
    assert_eq!(
        interactive.max_address_space_bytes,
        deployment.max_address_space_bytes
    );
}

#[test]
fn off_disables_every_dimension() {
    let limits = ProcessResourceLimits::off();
    assert!(limits.is_off());
    assert_eq!(limits, ProcessResourceLimits::default());
}

#[test]
fn environment_values_parse_off_zero_and_numbers() {
    assert_eq!(parse_environment_limit("off"), Some(None));
    assert_eq!(parse_environment_limit("OFF"), Some(None));
    assert_eq!(parse_environment_limit(" 0 "), Some(None));
    assert_eq!(parse_environment_limit("4096"), Some(Some(4_096)));
    assert_eq!(parse_environment_limit("-1"), None);
    assert_eq!(parse_environment_limit("many"), None);
}

#[test]
fn environment_overrides_replace_deployment_defaults_per_dimension() {
    let fallback = Some(DEFAULT_RLIMIT_NOFILE);
    assert_eq!(
        apply_environment_override(Some("1234"), fallback),
        Some(1_234),
        "环境变量必须覆盖部署默认值，故障排查时无需重新编译"
    );
    assert_eq!(
        apply_environment_override(Some(RESOURCE_LIMIT_DISABLED_VALUE), fallback),
        None,
        "off 档位必须能完全放下该维度"
    );
    assert_eq!(
        apply_environment_override(Some("not-a-number"), fallback),
        fallback,
        "非法取值必须回落到部署默认值，而不是静默取消围栏"
    );
    assert_eq!(
        apply_environment_override(None, fallback),
        fallback,
        "未设置时必须使用部署默认值"
    );
}

#[test]
fn program_lookup_prefers_candidates_and_reports_absence() {
    let absent = ["/usr/bin/axon-process-absent", "/bin/axon-process-absent"];
    assert_eq!(
        find_program("axon-process-absent", &absent, &[]),
        None,
        "候选路径必须逐个按文件校验，不存在时返回 None 而不是给出一条假路径"
    );
    assert_eq!(
        find_program("axon-process-absent", &absent, &["/usr/bin".to_owned()]),
        None,
        "PATH 目录里也没有该程序时必须返回 None"
    );
    assert_eq!(
        find_program("/usr/bin/axon-process-absent", &absent, &[]),
        None,
        "绝对程序名同样要按文件校验"
    );
}

#[test]
#[cfg(unix)]
fn program_lookup_falls_back_to_path_entries() {
    assert_eq!(
        find_program("/bin/sh", &[], &["/bin".to_owned()]),
        Some("/bin/sh".to_owned()),
        "候选为空时必须回退到 PATH 目录查找"
    );
}

#[test]
fn pty_wrapper_prefixes_limits_and_preserves_the_command() {
    let mut command = vec![
        "/bin/bash".to_owned(),
        "-lc".to_owned(),
        "echo hi".to_owned(),
    ];
    let wrapper = fence_pty_command(&mut command, ProcessResourceLimits::interactive_default());
    let Some(wrapper) = wrapper else {
        return;
    };
    let expected = vec![
        wrapper,
        format!("--fsize={PTY_RLIMIT_FSIZE_BYTES}"),
        format!("--nproc={PTY_RLIMIT_NPROC}"),
        format!("--nofile={PTY_RLIMIT_NOFILE}"),
        "--".to_owned(),
        "/bin/bash".to_owned(),
        "-lc".to_owned(),
        "echo hi".to_owned(),
    ];
    assert_eq!(
        command, expected,
        "包装必须前置限额参数、以 -- 终止选项，并把原始命令原样留在之后"
    );
}

#[test]
fn pty_wrapper_is_skipped_when_limits_are_off() {
    let original = vec![
        "/bin/bash".to_owned(),
        "-lc".to_owned(),
        "echo hi".to_owned(),
    ];
    let mut command = original.clone();
    assert_eq!(
        fence_pty_command(&mut command, ProcessResourceLimits::off()),
        None
    );
    assert_eq!(command, original, "关闭围栏时不得改写命令");
}

#[test]
fn pty_limit_arguments_cover_only_the_set_dimensions() {
    let arguments = pty_limit_arguments(ProcessResourceLimits {
        max_file_bytes: Some(1),
        max_processes: Some(2),
        max_open_files: Some(3),
        max_address_space_bytes: None,
    });
    assert_eq!(arguments, ["--fsize=1", "--nproc=2", "--nofile=3"]);
    assert!(pty_limit_arguments(ProcessResourceLimits::off()).is_empty());
}

#[test]
fn unsafe_code_is_confined_to_the_single_fenced_module() {
    let sources = production_source_files();
    assert!(
        !sources.is_empty(),
        "未扫描到任何生产源文件，路径断言会静默变绿"
    );
    let mut offenders = Vec::new();
    let mut fenced_module_seen = false;
    for path in sources {
        let body = std::fs::read_to_string(&path).expect("生产源文件必须可读");
        if path.file_name().and_then(|name| name.to_str()) == Some(UNSAFE_SOURCE_FILE) {
            fenced_module_seen = true;
            // 反向对照：路径或扫描逻辑一旦失效，这里必须失败而不是静默通过。
            assert!(
                declares_unsafe_code(&body),
                "{UNSAFE_SOURCE_FILE} 必须真的声明 unsafe；否则正向断言失去意义"
            );
            continue;
        }
        if declares_unsafe_code(&body) {
            offenders.push(path);
        }
    }
    assert!(
        fenced_module_seen,
        "扫描结果中缺失 {UNSAFE_SOURCE_FILE}，扫描范围可能已失效"
    );
    assert!(
        offenders.is_empty(),
        "unsafe 只允许存在于 {UNSAFE_SOURCE_FILE}，违规文件：{offenders:?}"
    );
}

/// 列出 `crates/axon-process/src` 下的生产源文件，排除测试与二进制入口。
fn production_source_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_sources(&PathBuf::from(SOURCE_ROOT), &mut files);
    files
}

fn collect_rust_sources(directory: &PathBuf, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skip = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| NON_SOURCE_DIRECTORIES.contains(&name));
            if !skip {
                collect_rust_sources(&path, files);
            }
            continue;
        }
        let is_test_source = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("_tests.rs"));
        if path.extension().is_some_and(|value| value == "rs") && !is_test_source {
            files.push(path);
        }
    }
}

/// R2 §5.2 回归：Job 句柄关闭必须终止受围栏的子进程。
///
/// 缺了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，Axon 进程退出（正常或崩溃）后作业子进程
/// 会变成孤儿：实测父进程已退出而 `Start-Sleep` 包装进程仍在运行，且作业注册表随之
/// 消失，回执只能报「Job 不存在」，用户与模型都无法回收它。
///
/// 本用例以「关掉句柄后子进程是否真的退出」测行为，而不是断言常量，避免把标志位写错
/// 却仍然通过。
#[cfg(windows)]
#[test]
fn closing_the_fence_handle_terminates_the_fenced_child() {
    let mut child = std::process::Command::new("cmd.exe")
        .args(["/c", "ping -n 60 127.0.0.1 > nul"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("受围栏子进程应可启动");
    let fence =
        fence_spawned_child(&child, ProcessResourceLimits::off()).expect("围栏绑定不应返回错误");
    assert!(
        fence.is_some(),
        "Windows 必须返回 Job Object 句柄，否则围栏与 kill-on-close 都从未生效"
    );
    drop(fence);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut exited = false;
    while std::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !exited {
        let _ = child.kill();
    }
    // 无论哪条路径都回收子进程，避免留下僵尸句柄。
    let _ = child.wait();
    assert!(
        exited,
        "Job 句柄关闭后子进程必须被终止；否则父进程退出会留下无法回收的孤儿进程"
    );
}
