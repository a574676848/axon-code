//! 默认 TUI 启动前的更新提示。

use std::{
    fs,
    io::{self, IsTerminal, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_RELEASE_MANIFEST_URL, UpgradePlan, fetch_release_manifest, is_newer_version,
    normalize_version,
};
use crate::{CliRunResult, OutputFormat, UpgradeCommand};

const STARTUP_UPDATE_STATE_FILE: &str = "update.json";
const STARTUP_UPDATE_DIR: &str = "data";
const RELEASE_NOTES_URL: &str = "https://github.com/a574676848/axon-code/releases/latest";
const UPDATE_CHOICE_NOW: &str = "1";
const UPDATE_CHOICE_NEXT_SESSION: &str = "2";
const UPDATE_CHOICE_SKIP_VERSION: &str = "3";
const STARTUP_UPDATE_CHECK_INTERVAL_SECS: u64 = 20 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupUpdateGate {
    Continue,
    Exit(CliRunResult),
}

pub(crate) fn run_startup_update_gate(
    axon_root: Option<&Path>,
    prompt: Option<&str>,
    format: OutputFormat,
) -> StartupUpdateGate {
    if should_skip_startup_update(prompt, format) {
        return StartupUpdateGate::Continue;
    }
    let Some(axon_root) = axon_root else {
        return StartupUpdateGate::Continue;
    };
    if !stdin_is_interactive() {
        return StartupUpdateGate::Continue;
    }

    let state_path = startup_update_state_path(axon_root);
    let state = read_startup_update_state(&state_path);
    if state.update_next_session {
        let _ = write_startup_update_state(
            &state_path,
            &StartupUpdateState {
                update_next_session: false,
                ..state.clone()
            },
        );
        return StartupUpdateGate::Exit(apply_startup_update());
    }

    let Some((info, refreshed_state)) = resolve_startup_update_info(&state) else {
        return StartupUpdateGate::Continue;
    };
    if let Some(refreshed_state) = refreshed_state {
        let _ = write_startup_update_state(&state_path, &refreshed_state);
    }
    if state.dismissed_version.as_deref() == Some(info.latest_version.as_str()) {
        return StartupUpdateGate::Continue;
    }

    match prompt_startup_update(&info) {
        StartupUpdateSelection::UpdateNow => StartupUpdateGate::Exit(apply_startup_update()),
        StartupUpdateSelection::UpdateNextSession => {
            let _ = write_startup_update_state(
                &state_path,
                &StartupUpdateState {
                    latest_version: Some(info.latest_version),
                    last_checked_unix_secs: now_unix_secs(),
                    dismissed_version: state.dismissed_version,
                    update_next_session: true,
                },
            );
            StartupUpdateGate::Continue
        }
        StartupUpdateSelection::SkipVersion => {
            let _ = write_startup_update_state(
                &state_path,
                &StartupUpdateState {
                    latest_version: Some(info.latest_version.clone()),
                    last_checked_unix_secs: now_unix_secs(),
                    dismissed_version: Some(info.latest_version),
                    update_next_session: false,
                },
            );
            StartupUpdateGate::Continue
        }
        StartupUpdateSelection::SkipOnce => StartupUpdateGate::Continue,
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
struct StartupUpdateState {
    #[serde(default)]
    latest_version: Option<String>,
    #[serde(default)]
    last_checked_unix_secs: u64,
    #[serde(default)]
    dismissed_version: Option<String>,
    #[serde(default)]
    update_next_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartupUpdateInfo {
    current_version: String,
    latest_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupUpdateSelection {
    UpdateNow,
    UpdateNextSession,
    SkipVersion,
    SkipOnce,
}

fn should_skip_startup_update(prompt: Option<&str>, format: OutputFormat) -> bool {
    format == OutputFormat::Json || prompt.is_some_and(|value| !value.trim().is_empty())
}

fn stdin_is_interactive() -> bool {
    io::stdin().is_terminal()
}

fn resolve_startup_update_info(
    state: &StartupUpdateState,
) -> Option<(StartupUpdateInfo, Option<StartupUpdateState>)> {
    if startup_update_cache_is_fresh(state) {
        let info = startup_update_info_from_latest(state.latest_version.as_deref()?)?;
        return Some((info, None));
    }
    let info = check_latest_version_for_startup()?;
    let refreshed = StartupUpdateState {
        latest_version: Some(info.latest_version.clone()),
        last_checked_unix_secs: now_unix_secs(),
        dismissed_version: state.dismissed_version.clone(),
        update_next_session: state.update_next_session,
    };
    Some((info, Some(refreshed)))
}

fn startup_update_cache_is_fresh(state: &StartupUpdateState) -> bool {
    state.latest_version.is_some()
        && now_unix_secs().saturating_sub(state.last_checked_unix_secs)
            < STARTUP_UPDATE_CHECK_INTERVAL_SECS
}

fn startup_update_info_from_latest(latest_version: &str) -> Option<StartupUpdateInfo> {
    let current_version = env!("CARGO_PKG_VERSION").to_owned();
    is_newer_version(latest_version, &current_version).then_some(StartupUpdateInfo {
        current_version,
        latest_version: latest_version.to_owned(),
    })
}

fn check_latest_version_for_startup() -> Option<StartupUpdateInfo> {
    let manifest = fetch_release_manifest(DEFAULT_RELEASE_MANIFEST_URL).ok()?;
    let latest_version = normalize_version(&manifest.tag_name);
    startup_update_info_from_latest(&latest_version)
}

fn prompt_startup_update(info: &StartupUpdateInfo) -> StartupUpdateSelection {
    let _ = writeln!(
        io::stderr(),
        "Axon 发现新版本：{} -> {}\nRelease notes: {}\n\n  1. 立即更新\n  2. 下一个会话更新\n  3. 不更新当前版本\n\n请选择 [1/2/3]，直接回车跳过本次：",
        info.current_version,
        info.latest_version,
        RELEASE_NOTES_URL
    );
    let _ = io::stderr().flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return StartupUpdateSelection::SkipOnce;
    }
    selection_from_input(&input)
}

fn selection_from_input(input: &str) -> StartupUpdateSelection {
    match input.trim() {
        UPDATE_CHOICE_NOW => StartupUpdateSelection::UpdateNow,
        UPDATE_CHOICE_NEXT_SESSION => StartupUpdateSelection::UpdateNextSession,
        UPDATE_CHOICE_SKIP_VERSION => StartupUpdateSelection::SkipVersion,
        _ => StartupUpdateSelection::SkipOnce,
    }
}

fn apply_startup_update() -> CliRunResult {
    let command = UpgradeCommand {
        manifest_url: None,
        download: false,
        apply: true,
        rollback: false,
        output_dir: None,
        asset: None,
        format: Some(OutputFormat::Text),
    };
    match UpgradePlan::from_command(&command).and_then(UpgradePlan::execute) {
        Ok(report) => startup_update_result(
            &report.current_version,
            &report.latest_version,
            report.update_available,
        ),
        Err(message) => CliRunResult {
            exit_code: 2,
            stdout: String::new(),
            stderr: format!("Axon 更新失败：{message}\n"),
        },
    }
}

fn startup_update_result(
    current_version: &str,
    latest_version: &str,
    update_available: bool,
) -> CliRunResult {
    if update_available {
        return CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon 更新完成：{current_version} -> {latest_version}\n请重新运行 axon 开始新会话。\n"
            ),
            stderr: String::new(),
        };
    }
    CliRunResult {
        exit_code: 0,
        stdout: format!("Axon 已是最新版本：{current_version}\n"),
        stderr: String::new(),
    }
}

fn startup_update_state_path(axon_root: &Path) -> PathBuf {
    axon_root
        .join(STARTUP_UPDATE_DIR)
        .join(STARTUP_UPDATE_STATE_FILE)
}

fn read_startup_update_state(path: &Path) -> StartupUpdateState {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn write_startup_update_state(path: &Path, state: &StartupUpdateState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建更新状态目录失败: {error}"))?;
    }
    let content = serde_json::to_string_pretty(state)
        .map_err(|error| format!("序列化更新状态失败: {error}"))?;
    fs::write(path, format!("{content}\n")).map_err(|error| format!("写入更新状态失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_update_skips_prompt_and_json_mode() {
        assert!(should_skip_startup_update(
            Some("修复构建"),
            OutputFormat::Text
        ));
        assert!(should_skip_startup_update(None, OutputFormat::Json));
        assert!(!should_skip_startup_update(None, OutputFormat::Text));
    }

    #[test]
    fn startup_update_selection_maps_three_explicit_choices() {
        assert_eq!(
            selection_from_input("1\n"),
            StartupUpdateSelection::UpdateNow
        );
        assert_eq!(
            selection_from_input("2"),
            StartupUpdateSelection::UpdateNextSession
        );
        assert_eq!(
            selection_from_input("3"),
            StartupUpdateSelection::SkipVersion
        );
        assert_eq!(selection_from_input(""), StartupUpdateSelection::SkipOnce);
    }

    #[test]
    fn startup_update_state_round_trips_dismiss_and_next_session() {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-startup-update-test-{}", std::process::id()));
        let path = startup_update_state_path(&root);
        let state = StartupUpdateState {
            latest_version: Some("1.2.3".to_owned()),
            last_checked_unix_secs: 123,
            dismissed_version: Some("1.2.3".to_owned()),
            update_next_session: true,
        };

        write_startup_update_state(&path, &state).expect("更新状态应可写入");

        assert_eq!(read_startup_update_state(&path), state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn startup_update_result_distinguishes_latest_from_applied() {
        assert!(
            startup_update_result("1.0.0", "1.1.0", true)
                .stdout
                .contains("更新完成")
        );
        assert!(
            startup_update_result("1.1.0", "1.1.0", false)
                .stdout
                .contains("已是最新版本")
        );
    }

    #[test]
    fn startup_update_uses_fresh_cached_latest_version() {
        let state = StartupUpdateState {
            latest_version: Some("999.0.0".to_owned()),
            last_checked_unix_secs: now_unix_secs(),
            dismissed_version: None,
            update_next_session: false,
        };

        let (info, refreshed) = resolve_startup_update_info(&state).expect("新版本缓存应触发提示");

        assert_eq!(info.latest_version, "999.0.0");
        assert!(refreshed.is_none());
    }
}
