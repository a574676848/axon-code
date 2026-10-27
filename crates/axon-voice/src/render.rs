//! 朗读稿渲染：把助手回复变成可朗读的人话（WP-04 六条规则）。
//!
//! 纯函数 + 有状态分块器：`render` 处理完整文本，`SpeechStream` 处理流式增量。
//! TTS 只接收本模块输出；代码块、表格、绝对路径与令牌不会进入合成。

use crate::domain::profile::Verbosity;
use crate::port::SpeechRenderer;

/// 单句上限（4 KiB）。
const MAX_SENTENCE_BYTES: usize = 4 * 1024;
/// 合并短句的最小汉字数。
const MIN_CJK_CHARS: usize = 6;

/// 完整回复渲染为句子列表。
/// 完整回复渲染为句子列表。
#[must_use]
pub fn render_sentences(markdown: &str, verbosity: Verbosity) -> Vec<String> {
    render_sentences_with_override(markdown, None, verbosity)
}

/// 带「专门朗读内容」的渲染：模型若提供了口语版，以口语版为准（仍做脱敏）。
///
/// 与 Codex 实时语音一致：正文给眼睛看，朗读稿给耳朵听。
#[must_use]
pub fn render_sentences_with_override(
    markdown: &str,
    spoken: Option<&str>,
    verbosity: Verbosity,
) -> Vec<String> {
    let source = spoken.unwrap_or(markdown);
    let cleaned = strip_think(source);
    let blocks = clean_blocks(&cleaned);
    let sentences = split_sentences(&blocks);
    select_by_verbosity(sentences, verbosity)
}

/// 渲染为一段可直接合成的文本。
#[must_use]
pub fn render_for_speech(markdown: &str, verbosity: Verbosity) -> String {
    render_for_speech_with_override(markdown, None, verbosity)
}

/// 带专门朗读内容的合成文本。
#[must_use]
pub fn render_for_speech_with_override(
    markdown: &str,
    spoken: Option<&str>,
    verbosity: Verbosity,
) -> String {
    render_sentences_with_override(markdown, spoken, verbosity).join("，")
}

/// 有状态分块器：流式增量进来，攒够一句就吐一句。
#[derive(Debug, Default)]
pub struct SpeechStream {
    buffer: String,
}

impl SpeechStream {
    /// 新建。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加增量，返回本次可朗读的完整句子。
    pub fn feed(&mut self, delta: &str) -> Vec<String> {
        self.buffer.push_str(delta);
        let cleaned = strip_think(&self.buffer);
        let mut sentences = Vec::new();
        if let Some(end) = find_boundary(&cleaned) {
            let head: String = cleaned[..end].to_owned();
            sentences.extend(split_sentences(&clean_blocks(&head)));
            self.buffer = cleaned[end..].to_owned();
        }
        sentences
    }

    /// 收尾。
    pub fn flush(&mut self) -> Vec<String> {
        let cleaned = strip_think(&self.buffer);
        self.buffer.clear();
        split_sentences(&clean_blocks(&cleaned))
    }
}

impl SpeechRenderer for SpeechStream {
    fn feed(&mut self, delta: &str) -> Vec<String> {
        SpeechStream::feed(self, delta)
    }

    fn flush(&mut self) -> Option<String> {
        let sentences = SpeechStream::flush(self);
        (!sentences.is_empty()).then(|| sentences.join("，"))
    }
}

fn find_boundary(text: &str) -> Option<usize> {
    text.char_indices()
        .find(|(_, ch)| matches!(ch, '。' | '！' | '？' | '\n'))
        .map(|(index, ch)| index + ch.len_utf8())
}

/// 剥离 `think` 标签与工具过程行。
fn strip_think(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(" think") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some(end) = after.find("<｜end▁of▁thinking｜>") {
            rest = &after[end + "<｜end▁of▁thinking｜>".len()..];
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    let out = out
        .replace(" thinking", "")
        .replace(" response", "")
        .replace("think>", "")
        .replace("</think>", "");
    out.lines()
        .filter(|line| !is_process_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_process_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("🛠")
        || trimmed.starts_with("⏵")
        || trimmed.starts_with("tool:")
        || trimmed.starts_with("[tool]")
        || trimmed.starts_with("运行工具：")
        || trimmed.starts_with("调用工具：")
}

/// 处理代码块、diff、表格、路径、URL 与 markdown 标记。
fn clean_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence: Option<String> = None;
    let mut table_rows = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            match in_fence.take() {
                None => {
                    let lang = trimmed.trim_start_matches("```").trim().to_lowercase();
                    out.push('\n');
                    in_fence = Some(lang);
                }
                Some(lang) => out.push_str(code_summary(&lang)),
            }
            continue;
        }
        if in_fence.is_some() {
            continue;
        }
        if trimmed.starts_with('|') {
            table_rows += 1;
            continue;
        }
        if table_rows > 0 {
            out.push_str("表格已省略，详见正文。\n");
            table_rows = 0;
        }
        let cleaned = clean_inline(trimmed);
        if !cleaned.is_empty() {
            out.push_str(&cleaned);
            out.push('\n');
        }
    }
    if table_rows > 0 {
        out.push_str("表格已省略，详见正文。\n");
    }
    out
}

fn code_summary(lang: &str) -> &'static str {
    if lang == "diff" || lang == "patch" {
        "本轮修改了若干文件。"
    } else {
        "代码已写入文件，详见正文。"
    }
}

fn clean_inline(line: &str) -> String {
    let mut text = line.to_owned();
    for marker in [
        "######", "#####", "####", "###", "##", "#", ">", "**", "__", "*", "_", "`", "- [ ]",
        "- [x]", "- ", "* ", "+ ",
    ] {
        text = text.replace(marker, "");
    }
    // 去掉有序列表前缀 "1. "。
    if let Some((head, _)) = text.split_once(". ")
        && head.chars().all(|ch| ch.is_ascii_digit())
    {
        text = text[(head.len() + 2)..].to_owned();
    }
    text = replace_urls(&text);
    text = replace_paths(&text);
    text = redact(&text);
    text.trim().to_owned()
}

fn replace_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("http://").or_else(|| rest.find("https://")) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        let end = tail
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '，' | '。' | '）' | ')'))
            .unwrap_or(tail.len());
        let url = &tail[..end];
        let domain = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("");
        out.push_str(if domain.is_empty() { "链接" } else { domain });
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn replace_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for token in text.split_whitespace() {
        let cleaned =
            token.trim_matches(|ch: char| matches!(ch, '（' | '）' | '(' | ')' | '，' | '。'));
        if is_absolute_path(cleaned) {
            let name = cleaned
                .rsplit(['/', '\\'])
                .find(|part| !part.is_empty())
                .unwrap_or("文件");
            out.push_str(name);
        } else {
            out.push_str(token);
        }
        out.push(' ');
    }
    out.trim_end().to_owned()
}

fn is_absolute_path(token: &str) -> bool {
    (token.starts_with('/') && token.len() > 1 && !token.starts_with("//"))
        || (token.len() > 2
            && token.as_bytes()[1] == b':'
            && matches!(token.as_bytes()[2], b'\\' | b'/'))
        || token.starts_with("\\\\")
}

/// 令牌、邮箱、绝对路径统一替换为占位词。
fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for token in text.split_whitespace() {
        let cleaned =
            token.trim_matches(|ch: char| matches!(ch, '（' | '）' | '(' | ')' | '，' | '。'));
        if looks_like_token(cleaned) {
            out.push_str("[令牌]");
        } else if looks_like_email(cleaned) {
            out.push_str("[邮箱]");
        } else if is_absolute_path(cleaned) {
            out.push_str("[路径]");
        } else {
            out.push_str(token);
        }
        out.push(' ');
    }
    out.trim_end().to_owned()
}

fn looks_like_token(token: &str) -> bool {
    let prefixes = ["sk-", "ghp_", "xoxb-", "xoxp-", "AKIA"];
    prefixes.iter().any(|prefix| token.starts_with(prefix)) && token.len() >= 12
}

fn looks_like_email(token: &str) -> bool {
    token.contains('@') && token.contains('.') && !token.starts_with('@')
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();
    // 上一段已结束：段落内的短句不再跨段合并。
    let mut closed_paragraph = true;
    for ch in text.chars() {
        current.push(ch);
        if ch == '\n' {
            let sentence = current.trim().to_owned();
            current.clear();
            if !sentence.is_empty() && sentence != "。" && sentence != "！" && sentence != "？" {
                sentences.push(sentence);
            }
            closed_paragraph = true;
        } else if matches!(ch, '。' | '！' | '？' | '；') {
            let sentence = current.trim().trim_end_matches('；').trim().to_owned();
            current.clear();
            if sentence.is_empty() {
                continue;
            }
            if closed_paragraph {
                sentences.push(sentence);
                closed_paragraph = false;
            } else {
                push_sentence(&mut sentences, sentence);
            }
        }
    }
    let tail = current.trim().to_owned();
    if !tail.is_empty() {
        push_sentence(&mut sentences, tail);
    }
    sentences
}

fn push_sentence(sentences: &mut Vec<String>, sentence: String) {
    if sentence.len() > MAX_SENTENCE_BYTES {
        for chunk in sentence.as_bytes().chunks(MAX_SENTENCE_BYTES) {
            if let Ok(text) = std::str::from_utf8(chunk) {
                sentences.push(text.to_owned());
            }
        }
        return;
    }
    if let Some(last) = sentences.last_mut()
        && cjk_count(last) < MIN_CJK_CHARS
    {
        last.push('，');
        last.push_str(&sentence);
        return;
    }
    if cjk_count(&sentence) < MIN_CJK_CHARS
        && let Some(last) = sentences.last_mut()
    {
        last.push('，');
        last.push_str(&sentence);
        return;
    }
    sentences.push(sentence);
}

fn cjk_count(text: &str) -> usize {
    text.chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count()
}

fn select_by_verbosity(sentences: Vec<String>, verbosity: Verbosity) -> Vec<String> {
    match verbosity {
        Verbosity::Detailed => sentences,
        Verbosity::Concise => {
            let filtered: Vec<String> = sentences
                .into_iter()
                .filter(|sentence| {
                    !sentence.contains("代码已写入") && !sentence.contains("本轮修改了")
                })
                .collect();
            match filtered.len() {
                0..=2 => filtered,
                len => vec![filtered[0].clone(), filtered[len - 1].clone()],
            }
        }
        Verbosity::Minimal => {
            let mut tail: Vec<String> = sentences.iter().rev().take(1).cloned().collect();
            tail.reverse();
            tail
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SpeechStream, render_for_speech, render_sentences};
    use crate::domain::profile::Verbosity;

    const SAMPLE: &str = r#"## 结论
把网关改成流式后，首包从 900 ms 降到 220 ms。

```rust
fn main() { println!("secret"); }
```

| 指标 | 旧 | 新 |
| --- | --- | --- |
| 首包 | 900 | 220 |

详见 https://example.com/docs/start 与 C:\Users\me\axon\src\gateway.rs。
令牌 sk-abcdefghijklmnop 不能朗读，邮箱 dev@example.com 也不能。
"#;

    #[test]
    fn strips_code_blocks_tables_and_markdown() {
        let text = render_for_speech(SAMPLE, Verbosity::Detailed);
        assert!(!text.contains("fn main"), "代码块内容不应出现");
        assert!(!text.contains("println"), "代码块内容不应出现");
        assert!(!text.contains("| 指标"), "表格不应逐格朗读");
        assert!(text.contains("首包"), "结论应保留");
    }

    #[test]
    fn keeps_only_domain_for_urls() {
        let text = render_for_speech(
            "文档在 https://example.com/docs/start 。",
            Verbosity::Detailed,
        );
        assert!(text.contains("example.com"));
        assert!(!text.contains("docs/start"));
    }

    #[test]
    fn reads_basename_for_absolute_paths() {
        let text = render_for_speech(
            "改的是 C:\\Users\\me\\axon\\src\\gateway.rs 。",
            Verbosity::Detailed,
        );
        assert!(!text.contains("C:\\Users"), "绝对路径不应朗读");
        assert!(!text.contains("[路径]"), "正文路径应降级为文件名");
    }

    #[test]
    fn redacts_tokens_and_emails() {
        let text = render_for_speech(
            "令牌 sk-abcdefghijklmnop 和 dev@example.com 。",
            Verbosity::Detailed,
        );
        assert!(!text.contains("sk-abcdefghijklmnop"));
        assert!(!text.contains("dev@example.com"));
        assert!(text.contains("[令牌]"));
        assert!(text.contains("[邮箱]"));
    }

    #[test]
    fn think_blocks_are_removed() {
        let text = render_for_speech(
            " think 内部推理 <｜end▁of▁thinking｜> 对外结论。",
            Verbosity::Detailed,
        );
        assert!(!text.contains("内部推理"));
        assert!(text.contains("对外结论"));
    }

    #[test]
    fn process_lines_are_removed() {
        let text = render_for_speech("🛠 调用 read_file\n结论是好的。", Verbosity::Detailed);
        assert!(!text.contains("read_file"));
        assert!(text.contains("结论"));
    }

    #[test]
    fn minimal_is_shorter_than_detailed() {
        let detailed = render_for_speech(SAMPLE, Verbosity::Detailed);
        let minimal = render_for_speech(SAMPLE, Verbosity::Minimal);
        assert!(
            minimal.chars().count() * 100 <= detailed.chars().count() * 60,
            "Minimal 应不超过 Detailed 的 60%：{} vs {}",
            minimal.chars().count(),
            detailed.chars().count()
        );
    }

    #[test]
    fn short_sentences_are_merged() {
        let sentences = render_sentences("好。很好。非常好。", Verbosity::Detailed);
        assert_eq!(sentences.len(), 1);
        assert!(sentences[0].contains("非常好"));
    }

    #[test]
    fn semicolon_does_not_split_sentence() {
        let sentences = render_sentences("先做这个；再做那个；最后验收。", Verbosity::Detailed);
        assert!(sentences.iter().all(|sentence| !sentence.starts_with('；')));
    }

    #[test]
    fn inline_code_backticks_are_removed() {
        let text = render_for_speech("调用 `gateway.submit` 即可。", Verbosity::Detailed);
        assert!(text.contains("gateway.submit"));
        assert!(!text.contains('`'));
    }

    #[test]
    fn stream_emits_complete_sentences() {
        let mut stream = SpeechStream::new();
        assert!(stream.feed("第一句还没结束").is_empty());
        let sentences = stream.feed("。第二句开始");
        assert_eq!(sentences.len(), 1);
        assert!(sentences[0].contains("第一句"));
        let rest = stream.flush();
        assert!(rest.iter().any(|sentence| sentence.contains("第二句")));
    }

    #[test]
    fn renderer_trait_flushes_to_string() {
        use crate::port::SpeechRenderer;
        let mut stream = SpeechStream::new();
        let _ = stream.feed("只有一句没有标点");
        assert!(<SpeechStream as SpeechRenderer>::flush(&mut stream).is_some());
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(render_sentences("", Verbosity::Detailed).is_empty());
        assert_eq!(render_for_speech("   ", Verbosity::Detailed), "");
    }
}

#[cfg(test)]
mod override_tests {
    use super::{render_for_speech_with_override, render_sentences_with_override};
    use crate::domain::profile::Verbosity;

    #[test]
    fn spoken_override_replaces_body() {
        let body = "## 结论\n\n```rust\nfn main() {}\n```\n\n正文细节很长。";
        let spoken = "已经改好了，首包降到二百二十毫秒。";
        let text = render_for_speech_with_override(body, Some(spoken), Verbosity::Detailed);
        assert!(text.contains("二百二十毫秒"));
        assert!(!text.contains("fn main"));
        assert!(!text.contains("正文细节"));
    }

    #[test]
    fn spoken_override_is_still_redacted() {
        let text = render_for_speech_with_override(
            "正文",
            Some("令牌 sk-abcdefghijklmnop 不能播。"),
            Verbosity::Detailed,
        );
        assert!(!text.contains("sk-abcdefghijklmnop"));
        assert!(text.contains("[令牌]"));
    }

    #[test]
    fn concise_keeps_headline_and_conclusion() {
        let body = "结论：首包降低。\n\n中间细节一。\n\n中间细节二。\n\n下一步：压测。";
        let sentences = render_sentences_with_override(body, None, Verbosity::Concise);
        assert_eq!(
            sentences.len(),
            2,
            "Concise 只保留开头与结尾：{sentences:?}"
        );
        assert!(sentences[0].contains("首包"));
        assert!(sentences[1].contains("压测"));
    }
}
