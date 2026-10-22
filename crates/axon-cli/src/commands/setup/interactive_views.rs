use std::{collections::BTreeMap, path::Path};

use axon_config::setup::{SETUP_SECRET_PRESENT, secret_status};
use axon_config::{AxonConfig, ConfigLoader};
use axon_display::display_path;

use super::{
    setup_interactive::SetupResetInteractiveInput,
    setup_interactive_canvas::{
        ESCAPE_CHOICE, menu_rows, read_frame_choice, read_single_column_menu_choice,
        render_setup_frame, two_column_rows,
    },
    setup_interactive_config::collect_module_config_values,
    setup_interactive_prompts::{prompt_string, prompt_yes_no},
    setup_interactive_style::{SetupStyle, paint},
};

pub(super) fn confirm_setup_commit(
    config_path: &Path,
    env_path: &Path,
    changes: &[String],
    config_changed: bool,
    env_changed: bool,
) -> bool {
    let mut lines = vec![
        paint(SetupStyle::Muted, "以下变更将在确认后写入。"),
        String::new(),
    ];
    if config_changed {
        lines.push(paint(
            SetupStyle::Success,
            format!("[UPDATED] {}", display_path(config_path)),
        ));
    }
    if env_changed {
        lines.push(paint(
            SetupStyle::Success,
            format!("[UPDATED] {} (密钥值已隐藏)", display_path(env_path)),
        ));
    }
    lines.extend(
        changes
            .iter()
            .map(|change| paint(SetupStyle::Success, format!("+ {change}"))),
    );
    let choice = read_frame_choice(
        |prompt| {
            let mut frame_lines = lines.clone();
            frame_lines.extend([String::new(), paint(SetupStyle::Warning, prompt)]);
            render_setup_frame(
                &paint(SetupStyle::Accent, "CONFIG CHANGE PREVIEW"),
                &frame_lines,
            );
        },
        "Enter/Y 保存；Esc/N 返回 ❯",
    );
    is_commit_choice(&choice)
}

fn is_commit_choice(choice: &str) -> bool {
    matches!(
        choice.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes" | "是"
    )
}

pub(super) fn print_dashboard(
    config: &AxonConfig,
    env_path: &Path,
    prompt: &str,
    focus: usize,
    selected: Option<usize>,
) {
    let user_cfg_path = ConfigLoader::default_user_config_path()
        .map(|p| display_path(&p))
        .unwrap_or_else(|_| "~/.axon/config.toml".to_owned());
    let cfg_status_desc = if ConfigLoader::default_user_config_path().is_ok_and(|p| p.exists()) {
        format!("{user_cfg_path} [已就绪]")
    } else {
        format!("{user_cfg_path} (不存在时延后创建)")
    };
    let env_path_display = format!("{} [已绑定]", display_path(env_path));

    let prov_count = config.providers.len();
    let prov_summary = config
        .providers
        .first()
        .map(|provider| provider.name.as_str())
        .unwrap_or("未配置");
    let model_count = config.models.len();
    let model_summary = if !config.agent.default_model.is_empty() {
        config.agent.default_model.as_str()
    } else {
        config
            .models
            .first()
            .map(|model| model.name.as_str())
            .unwrap_or("未绑定")
    };
    let key_env = config
        .providers
        .first()
        .and_then(|provider| provider.api_key_env.as_deref())
        .unwrap_or("OPENAI_API_KEY");
    let (key_style, key_display) = if secret_status(env_path, key_env) == SETUP_SECRET_PRESENT {
        (
            SetupStyle::Success,
            format!("[PRESENT] (已从 {key_env} 就绪)"),
        )
    } else {
        (SetupStyle::Warning, format!("[MISSING] (需配置 {key_env})"))
    };

    let mut lines = vec![
        paint(SetupStyle::Muted, format!("配置文件: {cfg_status_desc}")),
        paint(SetupStyle::Muted, format!("密钥文件: {env_path_display}")),
        String::new(),
        paint(SetupStyle::Accent, "系统当前状态"),
        format!("  Providers : {prov_count} 已配置 ({prov_summary})"),
        format!("  Models    : {model_count} 已绑定 ({model_summary})"),
        format!("  API Key   : {}", paint(key_style, key_display)),
        String::new(),
        paint(SetupStyle::Muted, "方向键移动；回车确认；Esc 放弃退出"),
    ];
    let menu = menu_rows(
        &[
            "[1] Providers & Gateways".to_owned(),
            "[2] Models & Context".to_owned(),
            "[3] Agent Behaviors".to_owned(),
            "[4] Remote".to_owned(),
            "[5] Advanced Config".to_owned(),
            paint(SetupStyle::Warning, "[R] Reset"),
            paint(SetupStyle::Success, "[S] Save & Preview"),
            paint(SetupStyle::Error, "[Q] Cancel"),
        ],
        focus,
        selected,
    );
    lines.extend(two_column_rows(&menu));
    lines.push(paint(SetupStyle::Accent, prompt));
    render_setup_frame(&paint(SetupStyle::Accent, "AXON SETUP WIZARD"), &lines);
}

pub(super) fn collect_remote_input(
    config: &AxonConfig,
    config_values: &mut BTreeMap<String, String>,
) {
    loop {
        let choices = ["1"].into_iter().map(str::to_owned).collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| {
                let mut lines = vec![paint(SetupStyle::Muted, "主菜单  ›  Remote"), String::new()];
                lines.extend(menu_rows(&["[1] Remote 配置".to_owned()], focus, selected));
                lines.extend([
                    String::new(),
                    paint(SetupStyle::Muted, "方向键移动；回车确认；Esc 返回主菜单"),
                    paint(SetupStyle::Accent, prompt),
                ]);
                render_setup_frame(&paint(SetupStyle::Accent, "REMOTE GATEWAY"), &lines);
            },
            "选择模块",
            &choices,
            0,
            false,
        );
        match choice.to_ascii_uppercase().as_str() {
            "1" => collect_module_config_values(config, "remote", config_values),
            "0" | "" | ESCAPE_CHOICE => break,
            _ => {}
        }
    }
}

pub(super) fn collect_advanced_config_input(
    config: &AxonConfig,
    config_values: &mut BTreeMap<String, String>,
) {
    loop {
        let choices = ["1", "2"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| {
                let mut lines = vec![
                    paint(SetupStyle::Muted, "主菜单  ›  Advanced Config"),
                    String::new(),
                ];
                lines.extend(menu_rows(
                    &["[1] memory".to_owned(), "[2] channel_balancer".to_owned()],
                    focus,
                    selected,
                ));
                lines.extend([
                    String::new(),
                    paint(SetupStyle::Muted, "方向键移动；回车确认；Esc 返回主菜单"),
                    paint(SetupStyle::Accent, prompt),
                ]);
                render_setup_frame(&paint(SetupStyle::Accent, "ADVANCED CONFIG VALUES"), &lines);
            },
            "选择模块",
            &choices,
            0,
            false,
        );
        match choice.to_ascii_uppercase().as_str() {
            "1" => collect_module_config_values(config, "memory", config_values),
            "2" => collect_module_config_values(config, "channel_balancer", config_values),
            "0" | "" | ESCAPE_CHOICE => break,
            _ => {}
        }
    }
}

pub(super) fn collect_reset_menu_input(reset_actions: &mut SetupResetInteractiveInput) {
    loop {
        let choices = ["1", "2"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| {
                let mut lines = vec![paint(SetupStyle::Muted, "主菜单  ›  Reset"), String::new()];
                lines.extend(menu_rows(
                    &[
                        "[1] 删除 Provider 及其模型".to_owned(),
                        "[2] 清空 Agent 模型槽位".to_owned(),
                    ],
                    focus,
                    selected,
                ));
                lines.extend([
                    String::new(),
                    paint(SetupStyle::Muted, "方向键移动；回车确认；Esc 返回主菜单"),
                    paint(SetupStyle::Accent, prompt),
                ]);
                render_setup_frame(
                    &paint(SetupStyle::Accent, "RESET CONFIGURATION SLOT"),
                    &lines,
                );
            },
            "选择操作",
            &choices,
            0,
            false,
        );
        match choice.to_ascii_uppercase().as_str() {
            "1" => {
                let name = prompt_string("Provider 名称", "输入要删除的 Provider 名称", "openai");
                let confirmation = format!(
                    "影响：删除 providers[name={name}] 及其模型，并清理失效 Agent 引用。确认重置 Provider '{name}'?"
                );
                if prompt_yes_no(&confirmation, false) {
                    reset_actions.reset_provider = Some(name);
                }
            }
            "2" => {
                if prompt_yes_no(
                    "影响：清空 default/fast 模型槽位并恢复默认推理强度。确认重置?",
                    false,
                ) {
                    reset_actions.reset_agent_models = true;
                }
            }
            "0" | "" | ESCAPE_CHOICE => break,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_commit_choice;

    #[test]
    fn commit_choice_recognizes_confirm_and_cancel_inputs() {
        assert!(is_commit_choice(""));
        assert!(is_commit_choice("Y"));
        assert!(is_commit_choice("是"));
        assert!(!is_commit_choice("n"));
        assert!(!is_commit_choice("\u{1b}"));
    }
}
