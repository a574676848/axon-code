use axon_protocol::wire::{WireMemorySurfaceEntry, memory_surface_text_is_user_visible};

const MEMORY_SURFACED_LIMIT: usize = 3;
const MEMORY_SURFACED_TITLE_MAX_CHARS: usize = 72;

pub(super) fn surfaced_memory_entries(
    metadata: Option<&serde_json::Value>,
) -> Vec<WireMemorySurfaceEntry> {
    let Some(trace) = metadata
        .and_then(|value| value.get("trace"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    trace
        .iter()
        .filter(|item| {
            item.get("promptInjected")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or_default()
        })
        .filter_map(surfaced_memory_entry)
        .take(MEMORY_SURFACED_LIMIT)
        .collect()
}

fn surfaced_memory_entry(value: &serde_json::Value) -> Option<WireMemorySurfaceEntry> {
    let content_ref = value.get("contentRef")?.as_str()?.trim();
    if !memory_surface_text_is_user_visible(content_ref) {
        return None;
    }
    let score_basis_points = value
        .get("scoreBasisPoints")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default()
        .min(10_000) as u16;
    let scope = value
        .get("scopeMatch")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .unwrap_or("unscoped");
    Some(WireMemorySurfaceEntry {
        id: content_ref.to_owned(),
        title: bounded_single_line(content_ref, MEMORY_SURFACED_TITLE_MAX_CHARS),
        summary: format!("相关度 {}% · {scope}", score_basis_points / 100),
        relevance_score: Some(f32::from(score_basis_points) / 10_000.0),
    })
}

fn bounded_single_line(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>()
        + "…"
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn surfaced_memory_entries_omit_provider_visible_guidance_refs() {
        let metadata = json!({
            "trace": [{
                "contentRef": "memory-ref:tool-guidance:use builtin.glob",
                "scoreBasisPoints": 9300,
                "scopeMatch": "same_repo",
                "promptInjected": true
            }, {
                "contentRef": "memory_tool_guidance=memory-guidance: run fmt",
                "scoreBasisPoints": 9100,
                "scopeMatch": "same_repo",
                "promptInjected": true
            }, {
                "contentRef": "memory://workspace/visible-preference",
                "scoreBasisPoints": 8700,
                "scopeMatch": "same_workspace",
                "promptInjected": true
            }]
        });

        let entries = surfaced_memory_entries(Some(&metadata));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "memory://workspace/visible-preference");
    }
}
