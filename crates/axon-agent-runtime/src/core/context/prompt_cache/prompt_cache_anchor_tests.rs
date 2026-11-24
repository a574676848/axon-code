use super::prompt_cache_tests::{anchor_input, dynamic_context};
use super::{
    PROMPT_CACHE_HINT_COUNT, PromptCacheAnchorRuntime, PromptCacheHintTarget,
    PromptCacheStablePrefixKind,
};

#[test]
fn prompt_cache_anchor_ignores_dynamic_context_in_cache_key() {
    let runtime = PromptCacheAnchorRuntime;
    let first = runtime
        .plan(anchor_input(dynamic_context("terminal://pane/1", "dyn-a")))
        .unwrap();
    let second = runtime
        .plan(anchor_input(dynamic_context("terminal://pane/2", "dyn-b")))
        .unwrap();

    assert_eq!(first.cache_key, second.cache_key);
    assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
    assert!(!first.dynamic_context_in_cache_key);
    assert_eq!(first.dynamic_context_audit.len(), 1);
    assert_eq!(first.cache_hints.len(), PROMPT_CACHE_HINT_COUNT);
    assert!(
        first
            .cache_hints
            .iter()
            .any(|hint| hint.target == PromptCacheHintTarget::LastToolSchema)
    );
    assert!(first.stable_prefix_manifest.iter().any(|entry| {
        entry.kind == PromptCacheStablePrefixKind::ToolSchema && !entry.hash.is_empty()
    }));
}

#[test]
fn prompt_cache_anchor_sorts_tools_before_hashing() {
    let runtime = PromptCacheAnchorRuntime;
    let mut left = anchor_input(dynamic_context("terminal://pane/1", "dyn-a"));
    left.tool_schema_hashes = vec!["tool-b".to_owned(), "tool-a".to_owned()];
    let mut right = anchor_input(dynamic_context("terminal://pane/1", "dyn-a"));
    right.tool_schema_hashes = vec!["tool-a".to_owned(), "tool-b".to_owned()];

    let left_anchor = runtime.plan(left).unwrap();
    let right_anchor = runtime.plan(right).unwrap();

    assert_eq!(left_anchor.tool_schema_hash, right_anchor.tool_schema_hash);
    assert_eq!(left_anchor.cache_key, right_anchor.cache_key);
}

#[test]
fn prompt_cache_anchor_reports_only_stable_prefix_invalidation() {
    let runtime = PromptCacheAnchorRuntime;
    let previous = runtime
        .plan(anchor_input(dynamic_context("terminal://pane/1", "dyn-a")))
        .unwrap();
    let mut next_input = anchor_input(dynamic_context("terminal://pane/99", "dyn-changed"));
    next_input.previous_anchor = Some(previous);
    next_input.tool_schema_hashes = vec!["tool-schema-v2".to_owned()];
    next_input.model_profile_hash = "model-profile-v2".to_owned();

    let next = runtime.plan(next_input).unwrap();

    assert_eq!(
        next.invalidation_reasons,
        vec!["tool_schema_changed", "model_profile_changed"]
    );
    assert!(
        !next
            .invalidation_reasons
            .iter()
            .any(|reason| reason.contains("dynamic"))
    );
}

#[test]
fn prompt_cache_anchor_reports_only_user_agents_invalidation() {
    let runtime = PromptCacheAnchorRuntime;
    let previous = runtime
        .plan(anchor_input(dynamic_context("terminal://pane/1", "dyn-a")))
        .unwrap();
    let mut next_input = anchor_input(dynamic_context("terminal://pane/1", "dyn-a"));
    next_input.previous_anchor = Some(previous);
    next_input.user_agents_hash = "user-agents-v2".to_owned();

    let next = runtime.plan(next_input).unwrap();

    assert_eq!(next.invalidation_reasons, vec!["user_agents_changed"]);
    assert!(next.stable_prefix_manifest.iter().any(|entry| {
        entry.kind == PromptCacheStablePrefixKind::UserAgents
            && entry.label == "user_agents"
            && entry.hash == "user-agents-v2"
    }));
}

#[test]
fn prompt_cache_anchor_keeps_cross_workspace_stable_context_in_manifest() {
    let anchor = PromptCacheAnchorRuntime
        .plan(anchor_input(dynamic_context(
            "workspace://dynamic",
            "workspace-a",
        )))
        .unwrap();

    for (kind, label) in [
        (PromptCacheStablePrefixKind::Soul, "soul"),
        (PromptCacheStablePrefixKind::UserAgents, "user_agents"),
        (
            PromptCacheStablePrefixKind::PersonalMemory,
            "personal_memory",
        ),
        (PromptCacheStablePrefixKind::Config, "config"),
    ] {
        assert!(
            anchor.stable_prefix_manifest.iter().any(|entry| {
                entry.kind == kind && entry.label == label && !entry.hash.is_empty()
            })
        );
    }
}

#[test]
fn prompt_cache_anchor_rejects_missing_hash_or_dynamic_source_ref() {
    let runtime = PromptCacheAnchorRuntime;
    let mut missing_hash = anchor_input(dynamic_context("terminal://pane/1", "dyn-a"));
    missing_hash.system_prompt_hash.clear();
    let mut missing_ref = anchor_input(dynamic_context("", "dyn-a"));
    missing_ref.system_prompt_hash = "system-v1".to_owned();

    assert!(runtime.plan(missing_hash).is_err());
    assert!(runtime.plan(missing_ref).is_err());
}
