//! Agent TUI 能力保留矩阵。

use serde::Serialize;

#[cfg(test)]
pub(crate) const MIN_SMOKE_COVERED_ABILITIES: usize = 80;

/// Agent TUI 能力保留矩阵的启动报告快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiAbilityMatrixSnapshot {
    /// 能力总数。
    pub total_count: usize,
    /// 被 smoke 报告覆盖的能力数。
    pub smoke_covered_count: usize,
    /// 按能力类别聚合的数量摘要。
    pub categories: Vec<AgentTuiAbilityCategorySnapshot>,
    /// 逐项能力保留入口。
    pub abilities: Vec<AgentTuiAbilityEntry>,
}

/// Agent TUI 能力类别摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiAbilityCategorySnapshot {
    /// 类别稳定名称。
    pub category: &'static str,
    /// 类别能力总数。
    pub total_count: usize,
    /// 类别内被 smoke 报告覆盖的能力数。
    pub smoke_covered_count: usize,
}

/// 单项 TUI 能力的入口覆盖映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiAbilityEntry {
    /// 能力稳定编号。
    pub id: &'static str,
    /// 能力类别。
    pub category: &'static str,
    /// 现有能力入口或用户场景。
    pub old_entry: &'static str,
    /// 目标态入口。
    pub target_entry: &'static str,
    /// 自动化验证入口名称。
    pub automated_verification: &'static str,
    /// 是否被 smoke 报告覆盖。
    pub smoke_covered: bool,
    /// 指向真实 registry 或目标态能力的稳定 token。
    pub target_token: &'static str,
}

#[must_use]
pub fn agent_tui_ability_matrix_snapshot() -> AgentTuiAbilityMatrixSnapshot {
    let abilities = agent_tui_ability_entries().to_vec();
    let categories = category_snapshots(&abilities);
    let smoke_covered_count = abilities
        .iter()
        .filter(|ability| ability.smoke_covered)
        .count();
    AgentTuiAbilityMatrixSnapshot {
        total_count: abilities.len(),
        smoke_covered_count,
        categories,
        abilities,
    }
}

pub(crate) fn agent_tui_ability_entries() -> &'static [AgentTuiAbilityEntry] {
    ABILITIES
}

fn category_snapshots(abilities: &[AgentTuiAbilityEntry]) -> Vec<AgentTuiAbilityCategorySnapshot> {
    CATEGORY_ORDER
        .iter()
        .filter_map(|category| {
            let total_count = abilities
                .iter()
                .filter(|ability| ability.category == *category)
                .count();
            if total_count == 0 {
                return None;
            }
            let smoke_covered_count = abilities
                .iter()
                .filter(|ability| ability.category == *category && ability.smoke_covered)
                .count();
            Some(AgentTuiAbilityCategorySnapshot {
                category,
                total_count,
                smoke_covered_count,
            })
        })
        .collect()
}

const CATEGORY_INPUT: &str = "input";
const CATEGORY_ATTACHMENT: &str = "attachment_mention";
const CATEGORY_QUEUE: &str = "queue_steering";
const CATEGORY_SLASH: &str = "slash_command";
const CATEGORY_POPUP: &str = "composer_popup";
const CATEGORY_DOCK: &str = "dock";
const CATEGORY_CELL: &str = "transcript_cell";
const CATEGORY_RUNTIME: &str = "runtime_projection";
const CATEGORY_LIVE_CELL: &str = "live_cell_mutation";
const CATEGORY_SYSTEM: &str = "system_catalog";

const CATEGORY_ORDER: &[&str] = &[
    CATEGORY_INPUT,
    CATEGORY_ATTACHMENT,
    CATEGORY_QUEUE,
    CATEGORY_SLASH,
    CATEGORY_POPUP,
    CATEGORY_DOCK,
    CATEGORY_CELL,
    CATEGORY_RUNTIME,
    CATEGORY_LIVE_CELL,
    CATEGORY_SYSTEM,
];

macro_rules! ability {
    ($id:literal, $category:expr, $old:literal, $target:literal, $verification:literal, $token:expr) => {
        AgentTuiAbilityEntry {
            id: $id,
            category: $category,
            old_entry: $old,
            target_entry: $target,
            automated_verification: $verification,
            smoke_covered: true,
            target_token: $token,
        }
    };
}

mod ability_matrix_entries;

use ability_matrix_entries::ABILITIES;

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use super::super::commands::{EXIT_COMMAND_ALIASES, builtin_command_tokens};
    use super::super::run_output::AgentTuiRuntimePayload;
    use super::*;
    use crate::app_shell::composer_dock::registered_dock_kinds;
    use crate::surface::cell::RENDER_CELL_REGISTRY;
    use crate::ui_workflow::action_kind::UiActionKind;
    use crate::ui_workflow::shortcuts::UI_SHORTCUT_BINDINGS;

    #[test]
    fn ability_matrix_has_target_entries_for_all_capabilities() {
        let snapshot = agent_tui_ability_matrix_snapshot();
        assert!(snapshot.total_count >= 95);
        assert!(snapshot.smoke_covered_count >= MIN_SMOKE_COVERED_ABILITIES);
        assert_eq!(snapshot.total_count, snapshot.abilities.len());

        let mut ids = HashSet::new();
        for ability in &snapshot.abilities {
            assert!(ids.insert(ability.id), "重复能力编号：{}", ability.id);
            assert!(
                !ability.old_entry.trim().is_empty(),
                "{} 缺少旧入口",
                ability.id
            );
            assert!(
                !ability.target_entry.trim().is_empty(),
                "{} 缺少目标态入口",
                ability.id
            );
            assert!(
                !ability.automated_verification.trim().is_empty(),
                "{} 缺少自动化验证入口",
                ability.id
            );
            assert!(ability.smoke_covered, "{} 缺少 smoke 覆盖", ability.id);
        }

        let counts = category_counts(&snapshot.abilities);
        assert_eq!(counts[CATEGORY_INPUT], 14);
        assert_eq!(counts[CATEGORY_ATTACHMENT], 5);
        assert_eq!(counts[CATEGORY_QUEUE], 5);
        assert_eq!(counts[CATEGORY_SLASH], 9);
        assert_eq!(counts[CATEGORY_POPUP], 3);
        assert_eq!(counts[CATEGORY_DOCK], 7);
        assert_eq!(counts[CATEGORY_CELL], 15);
        assert_eq!(counts[CATEGORY_RUNTIME], 14);
        assert_eq!(counts[CATEGORY_LIVE_CELL], 5);
    }

    #[test]
    fn ability_matrix_validates_required_gestures() {
        for (gesture, action) in [
            ("Alt+V", UiActionKind::PasteImageFromSystemClipboard),
            ("Command+V", UiActionKind::PasteImageFromSystemClipboard),
            ("Ctrl+L/Esc", UiActionKind::ClearDraft),
            ("Alt+E", UiActionKind::RestoreInterruptInput),
            ("Shift+Enter/Alt+Enter", UiActionKind::InsertNewline),
            ("Ctrl+R", UiActionKind::ToggleHistorySearch),
        ] {
            assert!(
                UI_SHORTCUT_BINDINGS
                    .iter()
                    .any(|binding| binding.gesture == gesture && binding.action == action),
                "{gesture} 未映射到 {action:?}"
            );
        }
        assert!(
            UI_SHORTCUT_BINDINGS
                .iter()
                .all(|binding| binding.gesture != "Ctrl+C")
        );
    }

    #[test]
    fn ability_matrix_validates_shortcut_targets() {
        let shortcut_targets = UI_SHORTCUT_BINDINGS
            .iter()
            .map(|binding| binding.action.as_str())
            .collect::<HashSet<_>>();
        for ability in agent_tui_ability_entries().iter().filter(|ability| {
            ability.automated_verification == "ability_matrix_validates_shortcut_targets"
        }) {
            assert!(
                shortcut_targets.contains(ability.target_token),
                "{} 指向未登记 action：{}",
                ability.id,
                ability.target_token
            );
        }
    }

    #[test]
    fn ability_matrix_validates_command_targets() {
        let mut command_targets = builtin_command_tokens().collect::<HashSet<_>>();
        command_targets.extend(EXIT_COMMAND_ALIASES.iter().copied());
        for ability in agent_tui_ability_entries().iter().filter(|ability| {
            ability.automated_verification == "ability_matrix_validates_command_targets"
        }) {
            assert!(
                command_targets.contains(ability.target_token),
                "{} 指向未登记 command：{}",
                ability.id,
                ability.target_token
            );
        }
    }

    #[test]
    fn ability_matrix_validates_dock_targets() {
        let dock_targets = registered_dock_kinds()
            .iter()
            .map(|kind| kind.as_str())
            .collect::<HashSet<_>>();
        for ability in agent_tui_ability_entries().iter().filter(|ability| {
            ability.automated_verification == "ability_matrix_validates_dock_targets"
        }) {
            assert!(
                dock_targets.contains(ability.target_token),
                "{} 指向未登记 dock：{}",
                ability.id,
                ability.target_token
            );
        }

        for dock in ["ToolOutputViewer", "ShortcutHelp"] {
            assert!(dock_targets.contains(dock), "{dock} 未覆盖旧 overlay 场景");
        }
    }

    #[test]
    fn ability_matrix_validates_cell_targets() {
        let cell_targets = RENDER_CELL_REGISTRY
            .iter()
            .map(|entry| entry.kind.as_str())
            .collect::<HashSet<_>>();
        for ability in agent_tui_ability_entries().iter().filter(|ability| {
            ability.automated_verification == "ability_matrix_validates_cell_targets"
        }) {
            assert!(
                cell_targets.contains(ability.target_token),
                "{} 指向未登记 cell：{}",
                ability.id,
                ability.target_token
            );
        }
    }

    #[test]
    fn ability_matrix_validates_runtime_projection_targets() {
        let runtime_targets = AgentTuiRuntimePayload::ALL_KINDS
            .iter()
            .map(|kind| kind.as_str())
            .collect::<HashSet<_>>();
        let projection_abilities = agent_tui_ability_entries()
            .iter()
            .filter(|ability| ability.category == CATEGORY_RUNTIME)
            .collect::<Vec<_>>();
        assert_eq!(projection_abilities.len(), 14);

        for ability in projection_abilities {
            assert!(
                runtime_targets.contains(ability.target_token),
                "{} 指向未登记 runtime payload：{}",
                ability.id,
                ability.target_token
            );
        }
    }

    #[test]
    fn ability_matrix_validates_runtime_inventory_keeps_resume_and_control_out_of_projection_count()
    {
        let projection_tokens = agent_tui_ability_entries()
            .iter()
            .filter(|ability| ability.category == CATEGORY_RUNTIME)
            .map(|ability| ability.target_token)
            .collect::<HashSet<_>>();
        for token in [
            AgentTuiRuntimePayload::KIND_WIRE_RESUME_SNAPSHOT.as_str(),
            AgentTuiRuntimePayload::KIND_WIRE_RUNTIME_ERROR.as_str(),
            AgentTuiRuntimePayload::KIND_WATCHER.as_str(),
            AgentTuiRuntimePayload::KIND_CANCEL_REQUESTED.as_str(),
            AgentTuiRuntimePayload::KIND_INTERRUPT_REQUESTED.as_str(),
        ] {
            assert!(
                !projection_tokens.contains(token),
                "{token} 应作为系统专项能力，不计入 runtime projection 能力"
            );
        }
    }

    fn category_counts(abilities: &[AgentTuiAbilityEntry]) -> BTreeMap<&'static str, usize> {
        let mut counts = BTreeMap::new();
        for ability in abilities {
            *counts.entry(ability.category).or_insert(0) += 1;
        }
        counts
    }
}
