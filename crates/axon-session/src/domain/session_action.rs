//! Session Action 的领域优先级策略。

/// Action 类型的默认优先级。
#[must_use]
pub fn default_action_priority(kind: &str) -> i32 {
    match kind {
        "cancel_turn" | "steer" | "remote_control" => 1_000,
        "user_input" => 700,
        "compaction_continuation" => 500,
        "heartbeat_follow_up" => 400,
        "cron" => 100,
        _ => 0,
    }
}

/// 计算写入队列的有效优先级。
#[must_use]
pub fn effective_action_priority(kind: &str, requested: i32) -> i32 {
    let default = default_action_priority(kind);
    if requested > default {
        requested
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::{default_action_priority, effective_action_priority};

    #[test]
    fn control_and_continuation_priorities_are_ordered() {
        assert!(default_action_priority("cancel_turn") > default_action_priority("user_input"));
        assert!(
            default_action_priority("user_input")
                > default_action_priority("compaction_continuation")
        );
        assert!(
            default_action_priority("compaction_continuation")
                > default_action_priority("heartbeat_follow_up")
        );
    }

    #[test]
    fn explicit_priority_can_raise_but_not_lower_policy_floor() {
        assert_eq!(effective_action_priority("user_input", 0), 700);
        assert_eq!(effective_action_priority("cron", 900), 900);
    }
}
