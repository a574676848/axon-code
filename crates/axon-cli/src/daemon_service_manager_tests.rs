use super::*;

#[test]
fn service_definitions_enable_restart_and_escape_paths() {
    let exe = Path::new("/tmp/Axon & Tool/axon");
    let config = Path::new("/tmp/user config.toml");
    let runtime = Path::new("/tmp/runtime");
    let plist = launchd_plist(exe, config, runtime);
    assert!(plist.contains("<key>KeepAlive</key><true/>"));
    assert!(plist.contains("Axon &amp; Tool"));
    let unit = systemd_unit(exe, config, runtime);
    assert!(unit.contains("Restart=always"));
    assert!(unit.contains("RestartSec=5"));
    let launcher = Path::new("C:\\Axon & Tool\\axon-launcher.exe");
    let root = Path::new("C:\\Users\\tester\\.axon");
    let config = root.join("config.toml");
    let powershell = Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe");
    let xml = windows_task_xml(
        powershell,
        launcher,
        &config,
        &root.join("data"),
        "S-1-5-21-1-2-3-1001",
    );
    assert_eq!(WINDOWS_TASK_NAME, "AxonDaemon");
    assert!(xml.contains("<LogonTrigger>"));
    assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
    assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
    assert!(xml.contains("<RestartOnFailure>"));
    assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
    assert!(xml.contains(r"<Command>C:\Program Files\PowerShell\7\pwsh.exe</Command>"));
    assert!(xml.contains("-WindowStyle Hidden -EncodedCommand"));
}

#[test]
fn managed_release_service_uses_current_entry() {
    let current = Path::new("/home/tester/.axon/bin/versions/1.2.3-target/axon");
    let expected = if cfg!(windows) {
        Path::new("/home/tester/.axon/bin/launcher/axon-launcher.exe")
    } else {
        Path::new("/home/tester/.axon/bin/launcher/axon-launcher")
    };
    assert_eq!(stable_daemon_executable_from(current), expected);
}

#[test]
fn unmanaged_release_service_keeps_current_executable() {
    let current = Path::new("/usr/local/bin/axon");
    assert_eq!(stable_daemon_executable_from(current), current);
}

#[test]
fn install_initializes_default_soul_without_overwriting_user_content() {
    let temp = axon_test_support::test_temp_dir();
    let runtime = temp.path().join(".axon/data");
    ensure_default_soul(&runtime).unwrap();
    let soul = temp.path().join(".axon/SOUL.md");
    assert_eq!(fs::read_to_string(&soul).unwrap(), DEFAULT_SOUL_CONTENT);

    fs::write(&soul, "custom soul\n").unwrap();
    ensure_default_soul(&runtime).unwrap();
    assert_eq!(fs::read_to_string(soul).unwrap(), "custom soul\n");
}

#[cfg(unix)]
#[test]
fn process_alive_rejects_zero_and_missing_pid() {
    assert!(!process_alive(0));
    assert!(!process_alive(u32::MAX));
    assert!(process_alive(std::process::id()));
}

#[cfg(windows)]
#[test]
fn process_liveness_rejects_zero_and_missing_pid() {
    assert!(!process_alive(0));
    assert!(!process_alive(u32::MAX));
    assert!(process_alive(std::process::id()));
}

/// 强制结束只允许落在 Axon 进程上：PID 被回收给无关进程时绝不能下手。
#[cfg(windows)]
#[test]
fn forced_stop_guard_accepts_axon_and_rejects_strangers() {
    // 测试二进制自身的映像名含 axon，必须被认定为 Axon 进程。
    assert!(super::windows_definition::process_image_is_axon(
        std::process::id()
    ));
    // 不存在的 PID 必须被拒绝，否则 taskkill /T 会带走无关进程树。
    assert!(!super::windows_definition::process_image_is_axon(u32::MAX));
}

#[test]
fn whoami_output_yields_current_user_sid() {
    let output = "\"test-host\\tester\",\"S-1-5-21-1-2-3-1001\"\r\n";
    assert_eq!(
        parse_windows_user_sid(output).as_deref(),
        Some("S-1-5-21-1-2-3-1001")
    );
    assert_eq!(parse_windows_user_sid("whoami 执行失败"), None);
    assert_eq!(parse_windows_user_sid("\"user\",\"not-a-sid\""), None);
    assert_eq!(parse_windows_user_sid("\"user\",\"S-1-5-21--1001\""), None);
}
