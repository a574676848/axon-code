//! Memory surface 用户可见性规则。

const PROVIDER_VISIBLE_MEMORY_SURFACE_MARKERS: &[&str] = &[
    "memory-ref:",
    "memory-tool:",
    "memory-guidance:",
    "memory_tool_guidance=",
    "memory_verify_candidate=",
    "memory_citation_",
    "<runtime-note",
    "</runtime-note>",
    "<conversation-checkpoint>",
    "</conversation-checkpoint>",
    "[Axon Runtime Constraints]",
];

/// 判断 memory surface 文本是否可面向用户展示。
#[must_use]
pub fn memory_surface_text_is_user_visible(value: &str) -> bool {
    let normalized = value.trim();
    !normalized.is_empty()
        && !PROVIDER_VISIBLE_MEMORY_SURFACE_MARKERS
            .iter()
            .any(|marker| normalized.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::memory_surface_text_is_user_visible;

    #[test]
    fn memory_tool_refs_are_not_user_visible() {
        assert!(!memory_surface_text_is_user_visible("memory-tool:8d2e2d"));
        assert!(!memory_surface_text_is_user_visible(
            "内部引用 memory-tool:8d2e2d"
        ));
        assert!(memory_surface_text_is_user_visible("用户偏好使用简洁输出"));
    }
}
