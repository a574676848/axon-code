use super::{
    setup_interactive_canvas::{
        ESCAPE_CHOICE, menu_rows, read_frame_choice, read_single_column_menu_choice,
        render_setup_frame,
    },
    setup_interactive_style::{SetupStyle, paint},
    setup_option_help::option_detail,
};

pub(super) const CHOICE_EMPTY: &str = "留空";
pub(super) const CHOICE_KEEP: &str = "保留原值";

pub(super) fn prompt_string(label: &str, description: &str, default: &str) -> String {
    let prompt = prompt_with_default(label, default);
    let value = prompt_in_frame(label, description, &[], &prompt);
    if value == ESCAPE_CHOICE {
        return default.to_owned();
    }
    if value.trim().is_empty() {
        default.to_owned()
    } else {
        value.trim().to_owned()
    }
}

pub(super) fn prompt_choice(
    label: &str,
    description: &str,
    options: &[&str],
    default: &str,
) -> String {
    prompt_choice_in_frame(label, description, options, default)
}

pub(super) fn prompt_optional_choice(
    label: &str,
    description: &str,
    options: &[&str],
    current: Option<&str>,
) -> Option<String> {
    let mut choices = vec![CHOICE_KEEP, CHOICE_EMPTY];
    choices.extend_from_slice(options);
    let default = current.unwrap_or(CHOICE_KEEP);
    let choice = prompt_choice(label, description, &choices, default);
    match choice.as_str() {
        ESCAPE_CHOICE | CHOICE_KEEP | CHOICE_EMPTY => None,
        _ => Some(choice),
    }
}

pub(super) fn prompt_optional_secret(label: &str, description: &str) -> Option<String> {
    let value = prompt_in_frame(
        label,
        description,
        &[],
        &format!("{label}（留空保持现状） ❯"),
    );
    (!value.trim().is_empty() && value != ESCAPE_CHOICE).then(|| value.trim().to_owned())
}

pub(super) fn prompt_yes_no(label: &str, default: bool) -> bool {
    let default_choice = if default { "是" } else { "否" };
    match prompt_choice(
        label,
        "方向键移动；空格选中；回车确认。",
        &["是", "否"],
        default_choice,
    )
    .as_str()
    {
        "是" => true,
        "否" => false,
        _ => default,
    }
}

pub(super) fn prompt_u64(label: &str, description: &str, default: u64) -> u64 {
    prompt_string(label, description, &default.to_string())
        .parse::<u64>()
        .unwrap_or(default)
}

pub(super) fn prompt_usize(label: &str, description: &str, default: usize) -> usize {
    prompt_string(label, description, &default.to_string())
        .parse::<usize>()
        .unwrap_or(default)
}

pub(super) fn prompt_i32(label: &str, description: &str, default: i32) -> i32 {
    prompt_string(label, description, &default.to_string())
        .parse::<i32>()
        .unwrap_or(default)
}

pub(super) fn prompt_u32(label: &str, description: &str, default: u32) -> u32 {
    prompt_string(label, description, &default.to_string())
        .parse::<u32>()
        .unwrap_or(default)
}

fn prompt_with_default(label: &str, default: &str) -> String {
    if default.is_empty() {
        format!("{label} ❯")
    } else {
        format!("{label} [{default}]（回车保留） ❯")
    }
}

fn prompt_in_frame(label: &str, description: &str, options: &[String], prompt: &str) -> String {
    read_frame_choice(
        |input| {
            let mut lines = vec![
                paint(SetupStyle::Muted, "主菜单  ›  编辑配置"),
                String::new(),
            ];
            if !description.is_empty() {
                lines.push(paint(SetupStyle::Muted, description));
            }
            if !options.is_empty() {
                lines.extend([String::new(), paint(SetupStyle::Muted, "可选值：")]);
                lines.extend(options.iter().cloned());
            }
            lines.extend([String::new(), paint(SetupStyle::Accent, input)]);
            render_setup_frame(&paint(SetupStyle::Accent, label), &lines);
        },
        prompt,
    )
}

fn prompt_choice_in_frame(
    label: &str,
    description: &str,
    options: &[&str],
    default: &str,
) -> String {
    let rows = options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            option_detail(label, option).map_or_else(
                || format!("[{}] {option}", index + 1),
                |detail| format!("[{}] {option}：{detail}", index + 1),
            )
        })
        .collect::<Vec<_>>();
    let initial_focus = options
        .iter()
        .position(|option| *option == default)
        .unwrap_or(0);
    read_single_column_menu_choice(
        |prompt, focus, selected| {
            let mut lines = vec![
                paint(SetupStyle::Muted, "主菜单  ›  编辑配置"),
                String::new(),
            ];
            if !description.is_empty() {
                lines.push(paint(SetupStyle::Muted, description));
            }
            lines.extend([
                String::new(),
                paint(
                    SetupStyle::Muted,
                    "方向键移动；空格选中；回车确认；Esc 返回",
                ),
            ]);
            lines.extend(menu_rows(&rows, focus, selected));
            lines.extend([String::new(), paint(SetupStyle::Accent, prompt)]);
            render_setup_frame(&paint(SetupStyle::Accent, label), &lines);
        },
        &prompt_with_default(label, default),
        &options
            .iter()
            .map(|option| (*option).to_owned())
            .collect::<Vec<_>>(),
        initial_focus,
        true,
    )
}
