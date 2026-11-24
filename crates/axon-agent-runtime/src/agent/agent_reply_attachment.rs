//! Agent 最终回复中的本地附件识别。

use std::path::PathBuf;

const AGENT_REPLY_MEDIA_PREFIX: &str = "MEDIA:";

/// Agent 回复附件识别 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentReplyAttachmentRuntime;

impl AgentReplyAttachmentRuntime {
    /// 提取最终回复中以 `MEDIA:` 标记、确实存在的绝对文件路径。
    #[must_use]
    pub fn existing_files(reply: &str) -> Vec<PathBuf> {
        reply
            .lines()
            .filter_map(existing_file_on_line)
            .fold(Vec::new(), |mut files, path| {
                if !files.iter().any(|existing| existing == &path) {
                    files.push(path);
                }
                files
            })
    }

    /// 判断回复是否包含附件指令。
    #[must_use]
    pub fn has_media_directive(reply: &str) -> bool {
        reply.lines().any(is_media_line)
    }

    /// 移除附件指令后返回可展示正文。
    #[must_use]
    pub fn visible_text(reply: &str) -> String {
        reply
            .lines()
            .filter(|line| !is_media_line(line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn existing_file_on_line(line: &str) -> Option<PathBuf> {
    let value = media_value_on_line(line)?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    (path.is_absolute() && path.is_file()).then_some(path)
}

fn is_media_line(line: &str) -> bool {
    media_value_on_line(line).is_some()
}

fn media_value_on_line(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix(AGENT_REPLY_MEDIA_PREFIX)
        .map(str::trim)
        .map(|value| value.trim_matches(|character| matches!(character, '`' | '"' | '\'')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_attachment_extracts_existing_absolute_paths_once() {
        let root = axon_test_support::test_temp_dir();
        let file = root.path().join("report.txt");
        std::fs::write(&file, "report").expect("测试文件应可写入");
        let reply = format!(
            "文件已准备：\nMEDIA:`{}`\nMEDIA:{}",
            file.display(),
            file.display()
        );

        assert_eq!(
            AgentReplyAttachmentRuntime::existing_files(&reply),
            vec![file]
        );
    }

    #[test]
    fn reply_attachment_ignores_inline_relative_and_missing_paths() {
        let root = axon_test_support::test_temp_dir();
        let file = root.path().join("report.txt");
        std::fs::write(&file, "report").expect("测试文件应可写入");
        let reply = format!(
            "文件位于 {}\nreport.txt\n{}",
            file.display(),
            root.path().join("missing.txt").display()
        );

        assert!(AgentReplyAttachmentRuntime::existing_files(&reply).is_empty());
    }

    #[test]
    fn reply_attachment_hides_media_directives_from_visible_text() {
        let reply = "说明\nMEDIA:C:\\temp\\report.txt\n结论";
        assert!(AgentReplyAttachmentRuntime::has_media_directive(reply));
        assert_eq!(
            AgentReplyAttachmentRuntime::visible_text(reply),
            "说明\n结论"
        );
    }
}
