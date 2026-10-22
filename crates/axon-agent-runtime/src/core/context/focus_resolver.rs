//! 个人助理当前工作焦点解析。

use axon_domain::{GoalId, GoalRecord, GoalStatus, ScopeRef};

/// 明确要求离开当前话题的文本标记。
const TOPIC_CHANGE_MARKERS: &[&str] = &["换个话题", "换个主题", "不谈这个", "另一个话题"];

/// 一次焦点解析的输入。
#[derive(Debug, Clone, Copy)]
pub struct FocusResolutionInput<'a> {
    /// 当前 Interaction 的自然语言内容。
    pub interaction_content: &'a str,
    /// 当前 Interaction 的作用域。
    pub scope: &'a ScopeRef,
    /// 从 Ledger 有界读取的 Goal 候选。
    pub active_goals: &'a [GoalRecord],
}

/// 焦点解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusResolution {
    /// 当前 Interaction 关联的 Goal；None 表示普通交流或明确切换话题。
    pub goal_id: Option<GoalId>,
    /// 本次解析原因。
    pub reason: FocusResolutionReason,
}

/// 焦点解析原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusResolutionReason {
    /// 用户明确要求切换话题。
    TopicChanged,
    /// Interaction 明确匹配一个 active Goal。
    MatchedActiveGoal,
    /// 当前 Interaction 不属于任何 active Goal。
    NoActiveGoalMatch,
}

/// 当前工作焦点解析器。
#[derive(Debug, Default, Clone, Copy)]
pub struct FocusResolverRuntime;

impl FocusResolverRuntime {
    /// 从当前 Interaction 与同 scope active Goal 中解析唯一焦点。
    #[must_use]
    pub fn resolve(&self, input: FocusResolutionInput<'_>) -> FocusResolution {
        let interaction = normalize(input.interaction_content);
        if is_topic_change(&interaction) {
            return FocusResolution {
                goal_id: None,
                reason: FocusResolutionReason::TopicChanged,
            };
        }

        let goal_id = input
            .active_goals
            .iter()
            .filter(|goal| goal.scope == *input.scope && goal.status == GoalStatus::Active)
            .filter_map(|goal| goal_match_score(&interaction, goal).map(|score| (score, goal)))
            .max_by(|(left_score, left), (right_score, right)| {
                left_score
                    .cmp(right_score)
                    .then_with(|| left.updated_at_ms.cmp(&right.updated_at_ms))
                    .then_with(|| left.goal_id.as_str().cmp(right.goal_id.as_str()))
            })
            .map(|(_, goal)| goal.goal_id.clone());

        FocusResolution {
            reason: if goal_id.is_some() {
                FocusResolutionReason::MatchedActiveGoal
            } else {
                FocusResolutionReason::NoActiveGoalMatch
            },
            goal_id,
        }
    }
}

fn is_topic_change(interaction: &str) -> bool {
    TOPIC_CHANGE_MARKERS
        .iter()
        .any(|marker| interaction.contains(marker))
}

fn goal_match_score(interaction: &str, goal: &GoalRecord) -> Option<u8> {
    let goal_id = normalize(goal.goal_id.as_str());
    if interaction.contains(&goal_id) {
        return Some(3);
    }
    let title = normalize(&goal.title);
    if !title.is_empty() && interaction.contains(&title) {
        return Some(2);
    }
    let objective = normalize(&goal.plan.objective);
    (!objective.is_empty() && interaction.contains(&objective)).then_some(1)
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::GoalPlan;

    fn goal(
        id: &str,
        scope: ScopeRef,
        title: &str,
        objective: &str,
        updated_at_ms: u64,
    ) -> GoalRecord {
        let mut goal = GoalRecord::new(
            GoalId::new(id).unwrap(),
            scope,
            title,
            GoalPlan {
                objective: objective.to_owned(),
                steps: vec!["执行".to_owned()],
            },
            vec!["完成".to_owned()],
            updated_at_ms,
        )
        .unwrap();
        goal.transition(GoalStatus::Active, updated_at_ms).unwrap();
        goal
    }

    #[test]
    fn resolver_selects_active_goal_matched_by_title() {
        let scope = ScopeRef::workspace("workspace-a").unwrap();
        let expected = goal("goal-a", scope.clone(), "依赖升级", "升级依赖版本", 10);
        let resolution = FocusResolverRuntime.resolve(FocusResolutionInput {
            interaction_content: "继续处理依赖升级",
            scope: &scope,
            active_goals: std::slice::from_ref(&expected),
        });

        assert_eq!(resolution.goal_id, Some(expected.goal_id));
        assert_eq!(resolution.reason, FocusResolutionReason::MatchedActiveGoal);
    }

    #[test]
    fn resolver_clears_focus_when_user_changes_topic() {
        let scope = ScopeRef::global();
        let active = goal("goal-a", scope.clone(), "依赖升级", "升级依赖版本", 10);
        let resolution = FocusResolverRuntime.resolve(FocusResolutionInput {
            interaction_content: "换个话题，介绍一下这个仓库",
            scope: &scope,
            active_goals: &[active],
        });

        assert_eq!(resolution.goal_id, None);
        assert_eq!(resolution.reason, FocusResolutionReason::TopicChanged);
    }

    #[test]
    fn resolver_rejects_goal_from_another_scope() {
        let goal_scope = ScopeRef::workspace("workspace-a").unwrap();
        let interaction_scope = ScopeRef::workspace("workspace-b").unwrap();
        let active = goal("goal-a", goal_scope, "依赖升级", "升级依赖版本", 10);
        let resolution = FocusResolverRuntime.resolve(FocusResolutionInput {
            interaction_content: "继续处理依赖升级",
            scope: &interaction_scope,
            active_goals: &[active],
        });

        assert_eq!(resolution.goal_id, None);
        assert_eq!(resolution.reason, FocusResolutionReason::NoActiveGoalMatch);
    }
}
