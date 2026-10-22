//! Thread 标题本地生成与持久化。

use std::path::Path;

use axon_session::{JsonlThreadEventStore, NameThreadCommand, ThreadService};

use super::{RunSessionRecord, run_execution::now_ms, run_session_store::resolved_axon_root};

const TITLE_MAX_CHARS: usize = 32;
const TITLE_FALLBACK: &str = "新会话";
const ATTACHMENT_FILE_PREFIX: &str = "[文件:";
const ATTACHMENT_IMAGE_PREFIX: &str = "[图片:";

pub(super) fn derive_thread_title(prompt: &str) -> String {
    let source = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !is_attachment_metadata(line))
        .unwrap_or(TITLE_FALLBACK);
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let content = normalized
        .trim_start_matches(['#', '>', '-', '*', '+'])
        .trim();
    let clause = content
        .split(['。', '！', '？', '!', '?'])
        .next()
        .unwrap_or(content)
        .trim();
    let title = clause.chars().take(TITLE_MAX_CHARS).collect::<String>();
    if title.is_empty() {
        TITLE_FALLBACK.to_owned()
    } else {
        title
    }
}

pub(super) fn mirror_thread_title(
    config_path: &Path,
    record: &RunSessionRecord,
    title: &str,
) -> Result<(), String> {
    let root = resolved_axon_root(config_path)?;
    ThreadService::new(JsonlThreadEventStore).name_thread(
        &root,
        NameThreadCommand {
            thread_id: record.session_id.clone(),
            recorded_at_ms: now_ms(),
            name: Some(title.to_owned()),
        },
    )
}

fn is_attachment_metadata(line: &str) -> bool {
    line.starts_with(ATTACHMENT_FILE_PREFIX) || line.starts_with(ATTACHMENT_IMAGE_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_title_from_first_meaningful_prompt_clause() {
        assert_eq!(
            derive_thread_title("  ## 修复 Provider 延迟。后续说明\n第二行"),
            "修复 Provider 延迟"
        );
    }

    #[test]
    fn ignores_attachment_metadata_and_limits_title_length() {
        let title = derive_thread_title(
            "[图片: screen.png]\n这是一个明确超过三十二个字符并且需要被稳定截断的会话标题示例内容用于验证边界",
        );

        assert_eq!(title.chars().count(), TITLE_MAX_CHARS);
        assert!(title.starts_with("这是一个"));
    }

    #[test]
    fn empty_prompt_uses_stable_fallback() {
        assert_eq!(derive_thread_title("  \n\t"), TITLE_FALLBACK);
    }
}
