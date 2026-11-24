use super::*;

#[test]
fn detects_pure_sleep_placeholders_across_shells() {
    for script in [
        "sleep 10",
        "/bin/sleep 5s",
        "Start-Sleep -Seconds 15",
        "start-sleep -m 2500",
        "Start-Sleep 3",
        "timeout /t 10 /nobreak",
        "timeout /t:5",
        "ping -n 10 127.0.0.1 > nul",
        "ping -n 5 localhost > $null",
        "ping -n 5 ::1 | Out-Null",
        "ping -c 4 127.0.0.1 >/dev/null 2>&1",
        "[Threading.Thread]::Sleep(5000)",
        "[System.Threading.Thread]::Sleep(1500)",
        "Wait-Event -Timeout 10",
        r#"python -c "import time; time.sleep(10)""#,
        r#"python3 -c "from time import sleep; sleep(5)""#,
        r#"node -e "setTimeout(() => {}, 5000)""#,
        r#"node -e "await new Promise(r => setTimeout(r, 3000))""#,
        "echo waiting; sleep 10",
        "# wait\nWrite-Host 'waiting'\nStart-Sleep -Seconds 10",
        "WAIT_SEC=5; sleep $WAIT_SEC",
    ] {
        assert!(
            detect_pure_sleep_placeholder(script).is_some(),
            "纯休眠占位必须被拦截：{script}"
        );
    }
}

#[test]
fn allows_legitimate_scripts_and_non_pure_sleep_combinations() {
    for script in [
        "sleep 2 && cargo test",
        "Start-Sleep -Seconds 2; Invoke-RestMethod http://localhost:8080/health",
        "timeout 30 cargo test --workspace",
        "ping -n 4 8.8.8.8",
        "ping -n 4 127.0.0.1",
        "Wait-Event -SourceIdentifier MyEvent -Timeout 10",
        "for i in 1 2 3; do curl -s localhost:8080 && break; sleep 1; done",
        "while (!(Test-Path ready.flag)) { Start-Sleep -Milliseconds 200 }",
        r#"grep -rn "Start-Sleep" crates/"#,
        r#"echo "Start-Sleep is not running""#,
        r#"python -c "import time; time.sleep(1); print('ok')""#,
        r#"node -e "setTimeout(() => console.log('hi'), 1000)""#,
        "sleep 5 | cat",
        "sleep 1 > keepalive.txt",
        "cargo test --workspace",
        "command -v sleep",
    ] {
        assert!(
            detect_pure_sleep_placeholder(script).is_none(),
            "正常业务脚本不得被误杀：{script}"
        );
    }
}

#[test]
fn extracts_raw_script_from_canonical_powershell_and_posix_commands() {
    let pwsh = vec![
        "pwsh".to_owned(),
        "-NoLogo".to_owned(),
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-Command".to_owned(),
        format!("{POWERSHELL_UTF8_PREFIX}Start-Sleep -Seconds 10"),
    ];
    assert!(detect_pure_sleep_in_command(&pwsh).is_some());

    let bash = vec![
        "/bin/bash".to_owned(),
        "-lc".to_owned(),
        "sleep 2 && cargo test".to_owned(),
    ];
    assert!(detect_pure_sleep_in_command(&bash).is_none());
}
