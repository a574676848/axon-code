//! Agent TUI 运行设置里需要落盘的部分（skill 开关）持久化到当前项目配置。
//!
//! 模型与推理强度都是会话内的显式选择，随每轮下发给运行侧，不写配置文件：写盘会把
//! 一次临时切换变成所有会话的默认值，测试时还会改动真实配置。

use std::path::Path;

use axon_config::{AxonConfig, ConfigLoader};
use axon_tui_runtime::AgentTuiSettingsChange;

/// 将 TUI 中需要落盘的设置（skill 开关）写回配置文件。
pub(super) fn persist_tui_settings_change(
    config_path: &Path,
    change: &AgentTuiSettingsChange,
) -> Result<(), String> {
    let mut loaded = ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取配置失败：{error:?}"))?;
    apply_tui_settings_change(&mut loaded.config, change);
    ConfigLoader::save_to_path(config_path, &loaded.config)
        .map_err(|error| format!("写入配置失败：{error:?}"))
}

/// 在 TUI 提交进入 Daemon 前同步必要的运行设置（skill 开关）。
pub(super) fn persist_tui_settings_change_if_needed(
    config_path: &Path,
    change: &AgentTuiSettingsChange,
) -> Result<(), String> {
    if !tui_settings_change_requires_persist(change) {
        return Ok(());
    }
    persist_tui_settings_change(config_path, change)
}

/// 只有 skill 开关会落盘：没有开关变化就不必读写配置文件。
fn tui_settings_change_requires_persist(change: &AgentTuiSettingsChange) -> bool {
    !change.skill_states.is_empty()
}

fn apply_tui_settings_change(config: &mut AxonConfig, change: &AgentTuiSettingsChange) {
    // 模型与推理强度只作用于当前会话：宿主把它们随轮下发给运行侧，这里不写
    // `agent.default_model` 与 `agent.reasoning_effort`，全局默认值由设置入口维护。
    for skill in &change.skill_states {
        if skill.enabled {
            config.skills.disabled.remove(&skill.name);
        } else {
            config.skills.disabled.insert(skill.name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use axon_config::AxonConfig;
    use axon_tui_runtime::AgentTuiSettingsChange;

    use super::{apply_tui_settings_change, tui_settings_change_requires_persist};

    /// 切模型与切强度都不得改写配置：它们只随本轮下发，写盘会污染真实环境。
    #[test]
    fn turn_model_and_reasoning_effort_never_touch_the_config() {
        let mut config = AxonConfig::default();
        config.agent.default_model = "balanced".to_owned();
        config.agent.reasoning_effort = "low".to_owned();
        let change = AgentTuiSettingsChange {
            model: Some("upstream-fast".to_owned()),
            model_explicit: true,
            reasoning_effort: Some("high".to_owned()),
            skill_states: Vec::new(),
        };

        apply_tui_settings_change(&mut config, &change);

        assert_eq!(config.agent.default_model, "balanced");
        assert_eq!(config.agent.reasoning_effort, "low");
        assert!(
            !tui_settings_change_requires_persist(&change),
            "没有 skill 开关变化时不得读写配置文件"
        );
    }

    #[test]
    fn skill_state_change_updates_disabled_skills() {
        let mut config = AxonConfig::default();
        config.skills.disabled.insert("imagegen".to_owned());
        let change = AgentTuiSettingsChange {
            model: None,
            model_explicit: false,
            reasoning_effort: None,
            skill_states: vec![
                axon_tui_runtime::AgentTuiSkillStateChange {
                    name: "review".to_owned(),
                    enabled: false,
                },
                axon_tui_runtime::AgentTuiSkillStateChange {
                    name: "imagegen".to_owned(),
                    enabled: true,
                },
            ],
        };

        apply_tui_settings_change(&mut config, &change);

        assert!(config.skills.disabled.contains("review"));
        assert!(!config.skills.disabled.contains("imagegen"));
        assert!(tui_settings_change_requires_persist(&change));
    }
}
