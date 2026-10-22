use super::{
    setup_interactive::ModelInteractiveInput,
    setup_interactive_canvas::{
        ESCAPE_CHOICE, menu_rows, read_single_column_menu_choice, render_setup_frame,
    },
    setup_interactive_provider_model::edit_model_input,
    setup_interactive_style::{SetupStyle, paint},
};
use crate::SetupCommand;
use axon_config::AxonConfig;

const NEW_MODEL_CHOICE: &str = "N";
const DEFAULT_NEW_MODEL_ID: &str = "gpt-4o";

pub(super) fn collect_model_input(
    command: &SetupCommand,
    config: &AxonConfig,
    staged_provider_name: &str,
    input: &mut ModelInteractiveInput,
) {
    loop {
        let mut labels = config
            .models
            .iter()
            .enumerate()
            .map(|(index, model)| format!("[{}] {} / {}", index + 1, model.id, model.provider))
            .collect::<Vec<_>>();
        labels.push("[N] 新增模型".to_owned());

        let values = (1..=config.models.len())
            .map(|index| index.to_string())
            .chain(std::iter::once(NEW_MODEL_CHOICE.to_owned()))
            .collect::<Vec<_>>();
        let choice = read_single_column_menu_choice(
            |prompt, focus, selected| render_model_catalog(&labels, prompt, focus, selected),
            "选择模型",
            &values,
            0,
            false,
        );
        match choice.as_str() {
            NEW_MODEL_CHOICE => {
                *input = ModelInteractiveInput {
                    model: Some(DEFAULT_NEW_MODEL_ID.to_owned()),
                    model_name: Some(DEFAULT_NEW_MODEL_ID.to_owned()),
                    model_provider: Some(staged_provider_name.to_owned()),
                    configure_model: true,
                    ..ModelInteractiveInput::default()
                };
                edit_model_input(command, config, staged_provider_name, input);
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
                let Some(model) = config.models.get(index) else {
                    continue;
                };
                *input = ModelInteractiveInput {
                    selected_model_id: Some(model.id.clone()),
                    selected_model_provider: Some(model.provider.clone()),
                    ..ModelInteractiveInput::default()
                };
                edit_model_input(command, config, staged_provider_name, input);
            }
        }
    }
}

fn render_model_catalog(values: &[String], prompt: &str, focus: usize, selected: Option<usize>) {
    let mut lines = vec![paint(SetupStyle::Muted, "主菜单  ›  Models"), String::new()];
    lines.extend(menu_rows(values, focus, selected));
    lines.extend([
        String::new(),
        paint(SetupStyle::Muted, "方向键移动；回车确认；Esc 返回主菜单"),
        paint(SetupStyle::Accent, prompt),
    ]);
    render_setup_frame(&paint(SetupStyle::Accent, "MODELS CATALOG"), &lines);
}
