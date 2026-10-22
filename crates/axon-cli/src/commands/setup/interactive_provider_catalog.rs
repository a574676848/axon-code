use super::{
    setup_interactive::ProviderInteractiveInput,
    setup_interactive_canvas::{
        ESCAPE_CHOICE, menu_rows, read_single_column_menu_choice, render_setup_frame,
    },
    setup_interactive_provider_model::edit_provider_input,
    setup_interactive_style::{SetupStyle, paint},
};
use crate::SetupCommand;
use axon_config::AxonConfig;
use std::path::Path;

const NEW_PROVIDER_CHOICE: &str = "N";
const DEFAULT_NEW_PROVIDER_NAME: &str = "new-provider";

pub(super) fn collect_provider_input(
    command: &SetupCommand,
    config: &AxonConfig,
    env_path: &Path,
    input: &mut ProviderInteractiveInput,
) {
    loop {
        let mut labels = config
            .providers
            .iter()
            .enumerate()
            .map(|(index, provider)| {
                format!("[{}] {} / {}", index + 1, provider.name, provider.protocol)
            })
            .collect::<Vec<_>>();
        labels.push("[N] 新增供应商".to_owned());

        let values = (1..=config.providers.len())
            .map(|index| index.to_string())
            .chain(std::iter::once(NEW_PROVIDER_CHOICE.to_owned()))
            .collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| render_provider_catalog(&labels, prompt, focus, selected),
            "选择供应商",
            &values,
            0,
            false,
        );
        match choice.as_str() {
            NEW_PROVIDER_CHOICE => {
                *input = ProviderInteractiveInput {
                    resolved_provider_name: DEFAULT_NEW_PROVIDER_NAME.to_owned(),
                    ..ProviderInteractiveInput::default()
                };
                edit_provider_input(command, config, env_path, input);
            }
            "" | ESCAPE_CHOICE => break,
            _ => {
                let Some(index) = choice
                    .parse::<usize>()
                    .ok()
                    .and_then(|value| value.checked_sub(1))
                else {
                    continue;
                };
                let Some(provider) = config.providers.get(index) else {
                    continue;
                };
                *input = ProviderInteractiveInput {
                    resolved_provider_name: provider.name.clone(),
                    ..ProviderInteractiveInput::default()
                };
                edit_provider_input(command, config, env_path, input);
            }
        }
    }
}

fn render_provider_catalog(values: &[String], prompt: &str, focus: usize, selected: Option<usize>) {
    let mut lines = vec![
        paint(SetupStyle::Muted, "主菜单  ›  Providers"),
        String::new(),
    ];
    lines.extend(menu_rows(values, focus, selected));
    lines.extend([
        String::new(),
        paint(SetupStyle::Muted, "方向键移动；回车确认；Esc 返回主菜单"),
        paint(SetupStyle::Accent, prompt),
    ]);
    render_setup_frame(&paint(SetupStyle::Accent, "PROVIDERS CATALOG"), &lines);
}
