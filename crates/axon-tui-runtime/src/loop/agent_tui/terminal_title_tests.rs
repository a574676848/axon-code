use std::time::{Duration, Instant};

use crossterm::Command;

use super::*;

fn title_interval() -> Duration {
    crate::surface::animation::transcript_motion_tokens().terminal_title
}

fn stable_brand_with(text: &str) -> String {
    format!("{DEFAULT_TERMINAL_TITLE}{TITLE_SEPARATOR}{text}")
}

fn running_brand_with(spinner: &str, text: &str) -> String {
    format!("{spinner}{TITLE_SEPARATOR}{DEFAULT_TERMINAL_TITLE}{TITLE_SEPARATOR}{text}")
}

fn title_context<'a>(
    conversation_title: Option<&'a str>,
    workspace_name: Option<&'a str>,
    activity: TerminalTitleActivity,
) -> TerminalTitleContext<'a> {
    TerminalTitleContext {
        conversation_title,
        workspace_name,
        activity,
        lifecycle_revision: 0,
    }
}

#[test]
fn terminal_title_sanitizes_control_and_invisible_text() {
    let sanitized = sanitize_terminal_title(" Axon\tReady\x1b\x07 \u{202E}Project ");

    assert_eq!(sanitized, "Axon Ready Project");
}

#[test]
fn terminal_title_sanitizes_bidi_marks_and_enforces_length_limit() {
    let long_title = format!(
        "Axon \u{202D}{}\u{2069}\x1b",
        "工作区 ".repeat(MAX_TERMINAL_TITLE_CHARS)
    );

    let sanitized = sanitize_terminal_title(&long_title);

    assert!(!sanitized.contains('\u{202D}'));
    assert!(!sanitized.contains('\u{2069}'));
    assert!(!sanitized.contains('\x1b'));
    assert!(sanitized.chars().count() <= MAX_TERMINAL_TITLE_CHARS);
}

#[test]
fn terminal_title_builds_session_aware_title() {
    let context = title_context(
        Some("修复 TUI 输入区交互"),
        Some("axon-code"),
        TerminalTitleActivity::Idle,
    );

    let now = Instant::now();
    let title = build_terminal_title(&context, now, now);

    assert_eq!(title, stable_brand_with("修复 TUI 输入区交互"));
}

#[test]
fn terminal_title_idle_without_session_uses_workspace_name() {
    let context = title_context(None, Some("axon-code"), TerminalTitleActivity::Idle);

    let now = Instant::now();
    let title = build_terminal_title(&context, now, now);

    assert_eq!(title, stable_brand_with("axon-code"));
}

#[test]
fn terminal_title_idle_without_any_context_uses_default_title() {
    let context = title_context(None, None, TerminalTitleActivity::Idle);

    let now = Instant::now();
    let title = build_terminal_title(&context, now, now);

    assert_eq!(title, DEFAULT_TERMINAL_TITLE);
}

#[test]
fn terminal_title_uses_model_name_without_provider_prefix() {
    let context = title_context(
        Some("生成回复"),
        Some("axon-code"),
        TerminalTitleActivity::Working,
    );

    let now = Instant::now();
    let title = build_terminal_title(&context, now, now);

    assert!(title.contains("生成回复"));
    assert!(title.contains(DEFAULT_TERMINAL_TITLE));
    assert!(!title.contains("gpt"));
}

#[test]
fn terminal_title_idle_after_failure_drops_activity_and_model() {
    let context = title_context(
        Some("失败后继续输入"),
        Some("axon-code"),
        TerminalTitleActivity::Idle,
    );

    let now = Instant::now();
    let title = build_terminal_title(&context, now, now);

    assert_eq!(title, stable_brand_with("失败后继续输入"));
    assert_eq!(next_refresh_interval(&context), None);
}

#[test]
fn terminal_title_command_uses_osc_title_sequence() {
    let mut output = String::new();
    SetWindowTitle("axon-code".to_owned())
        .write_ansi(&mut output)
        .expect("终端标题 ANSI 序列应可编码");

    assert_eq!(output, "\x1b]0;axon-code\x07");
}

#[test]
fn terminal_title_with_activity_uses_spinner_without_status_text() {
    let context = title_context(
        Some("实现新功能"),
        Some("axon-code"),
        TerminalTitleActivity::Working,
    );

    let now = Instant::now();
    let title = build_terminal_title(&context, now, now + title_interval());

    assert_eq!(next_refresh_interval(&context), Some(title_interval()));
    assert_eq!(title, running_brand_with("⠙", "实现新功能"));
    assert!(!title.contains("生成中"));
}

#[test]
fn terminal_title_refresh_keeps_stable_text_inside_motion_interval() {
    let mut manager = TerminalTitleManager::default();
    let mut terminal_output = Vec::new();
    let context = title_context(
        Some("稳定会话标题"),
        Some("axon-code"),
        TerminalTitleActivity::Working,
    );

    let now = Instant::now();
    manager
        .refresh_with_writer(&context, now, &mut terminal_output, true)
        .expect("首次标题刷新应成功");
    let first_len = terminal_output.len();

    manager
        .refresh_with_writer(
            &context,
            now + title_interval() / 2,
            &mut terminal_output,
            true,
        )
        .expect("同一动画帧应可安全跳过");

    assert_eq!(terminal_output.len(), first_len);
}

#[test]
fn terminal_title_refresh_writes_next_spinner_frame_after_interval() {
    let mut manager = TerminalTitleManager::default();
    let mut terminal_output = Vec::new();
    let context = title_context(
        Some("稳定会话标题"),
        Some("axon-code"),
        TerminalTitleActivity::Working,
    );

    let now = Instant::now();
    manager
        .refresh_with_writer(&context, now, &mut terminal_output, true)
        .expect("首次标题刷新应成功");
    let first_len = terminal_output.len();

    manager
        .refresh_with_writer(&context, now + title_interval(), &mut terminal_output, true)
        .expect("下一 spinner 帧应刷新标题");

    assert!(terminal_output.len() > first_len);
}

#[test]
fn terminal_title_finish_keeps_stable_session_title() {
    let mut manager = TerminalTitleManager::default();
    let mut terminal_output = Vec::new();
    let context = title_context(
        Some("实现新功能"),
        Some("axon-code"),
        TerminalTitleActivity::Working,
    );

    let now = Instant::now();
    manager
        .refresh_with_writer(&context, now, &mut terminal_output, true)
        .expect("运行中标题刷新应成功");
    assert!(
        manager
            .last_title
            .as_deref()
            .is_some_and(|title| title.starts_with("⠋ · ⚡ Axon"))
    );

    manager
        .finish_with_writer(&context, &mut terminal_output, true)
        .expect("结束标题收口应成功");

    assert_eq!(
        manager.last_title.as_deref(),
        Some(stable_brand_with("实现新功能").as_str())
    );
    assert_eq!(manager.animation_origin, None);
}

#[test]
fn terminal_title_refresh_stops_animation_after_activity_becomes_idle() {
    let mut manager = TerminalTitleManager::default();
    let mut terminal_output = Vec::new();
    let working = title_context(
        Some("稳定会话标题"),
        Some("axon-code"),
        TerminalTitleActivity::Working,
    );
    let idle = title_context(
        Some("稳定会话标题"),
        Some("axon-code"),
        TerminalTitleActivity::Idle,
    );

    let now = Instant::now();
    assert_eq!(
        manager
            .refresh_with_writer(&working, now, &mut terminal_output, true)
            .expect("运行中标题刷新应成功"),
        Some(title_interval())
    );
    assert!(
        manager
            .last_title
            .as_deref()
            .is_some_and(|title| title == running_brand_with("⠋", "稳定会话标题"))
    );

    assert_eq!(
        manager
            .refresh_with_writer(&idle, now, &mut terminal_output, true)
            .expect("空闲标题刷新应成功"),
        None
    );

    assert_eq!(
        manager.last_title.as_deref(),
        Some(stable_brand_with("稳定会话标题").as_str())
    );
}

#[test]
fn terminal_title_lifecycle_revision_forces_stable_title_rewrite() {
    let mut manager = TerminalTitleManager::default();
    let mut terminal_output = Vec::new();
    let first = title_context(
        Some("完成后的会话"),
        Some("axon-code"),
        TerminalTitleActivity::Idle,
    );
    let next = TerminalTitleContext {
        lifecycle_revision: 1,
        ..first
    };
    let now = Instant::now();

    manager
        .refresh_with_writer(&first, now, &mut terminal_output, true)
        .expect("首次稳定标题应写入");
    let first_len = terminal_output.len();
    manager
        .refresh_with_writer(&next, now, &mut terminal_output, true)
        .expect("新生命周期必须重新声明稳定标题");

    assert!(terminal_output.len() > first_len);
    assert_eq!(
        manager.last_title.as_deref(),
        Some(stable_brand_with("完成后的会话").as_str())
    );
}

#[test]
fn terminal_title_same_revision_deduplicates_stable_title() {
    let mut manager = TerminalTitleManager::default();
    let mut terminal_output = Vec::new();
    let context = title_context(
        Some("稳定会话标题"),
        Some("axon-code"),
        TerminalTitleActivity::Idle,
    );
    let now = Instant::now();

    manager
        .refresh_with_writer(&context, now, &mut terminal_output, true)
        .expect("首次稳定标题应写入");
    let first_len = terminal_output.len();
    manager
        .refresh_with_writer(&context, now, &mut terminal_output, true)
        .expect("相同生命周期的稳定标题应安全去重");

    assert_eq!(terminal_output.len(), first_len);
}
