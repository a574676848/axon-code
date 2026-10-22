//! LSP 不可用时的正则过滤回退。

use regex::Regex;
use std::{borrow::Cow, sync::LazyLock};

const ESTIMATED_CHARS_PER_TOKEN: usize = 4;
const MAX_SAVINGS_PCT: u8 = 100;
const AGGRESSIVE_PLACEHOLDER: &str = "// ...";

/// 输出过滤支持的源码语言。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// JavaScript。
    JavaScript,
    /// TypeScript。
    TypeScript,
    /// Python。
    Python,
    /// Rust。
    Rust,
    /// Java。
    Java,
    /// C#。
    CSharp,
    /// Go。
    Go,
    /// 未识别语言。
    #[default]
    Unknown,
}

impl Language {
    /// 根据文件扩展名识别语言。
    #[must_use]
    pub fn from_extension(extension: &str) -> Self {
        match extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str()
        {
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "ts" | "tsx" => Self::TypeScript,
            "py" | "pyw" => Self::Python,
            "rs" => Self::Rust,
            "java" => Self::Java,
            "cs" => Self::CSharp,
            "go" => Self::Go,
            _ => Self::Unknown,
        }
    }

    /// 返回当前语言的注释规则。
    #[must_use]
    pub const fn comment_patterns(self) -> CommentPatterns {
        match self {
            Self::Python => CommentPatterns {
                line: Some("#"),
                block_start: Some("\"\"\""),
                block_end: Some("\"\"\""),
            },
            Self::JavaScript
            | Self::TypeScript
            | Self::Rust
            | Self::Java
            | Self::CSharp
            | Self::Go
            | Self::Unknown => CommentPatterns {
                line: Some("//"),
                block_start: Some("/*"),
                block_end: Some("*/"),
            },
        }
    }
}

/// 源码语言注释规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommentPatterns {
    /// 单行注释起始符。
    pub line: Option<&'static str>,
    /// 块注释起始符。
    pub block_start: Option<&'static str>,
    /// 块注释结束符。
    pub block_end: Option<&'static str>,
}

/// 正则过滤策略。
pub trait FilterStrategy {
    /// 过滤源码内容。
    #[must_use]
    fn filter(&self, content: &str, language: Language) -> String;
}

/// 正则过滤回退器。
#[derive(Debug, Default, Clone, Copy)]
pub struct RegexFallbackFilter;

impl RegexFallbackFilter {
    /// 执行 Minimal 过滤：移除注释并压缩连续空行。
    #[must_use]
    pub fn filter_minimal(&self, content: &str, language: Language) -> String {
        MinimalFilter.filter(content, language)
    }

    /// 执行 Aggressive 过滤：保留导入、类型与函数签名，移除实现细节。
    #[must_use]
    pub fn filter_aggressive(&self, content: &str, language: Language) -> String {
        AggressiveFilter.filter(content, language)
    }
}

/// 不做过滤。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFilter;

impl FilterStrategy for NoFilter {
    fn filter(&self, content: &str, _language: Language) -> String {
        content.to_owned()
    }
}

/// 轻量过滤：移除注释并压缩空行。
#[derive(Debug, Default, Clone, Copy)]
pub struct MinimalFilter;

impl FilterStrategy for MinimalFilter {
    fn filter(&self, content: &str, language: Language) -> String {
        let without_comments = strip_comments(content, language.comment_patterns());
        normalize_blank_lines(&without_comments)
    }
}

/// 激进过滤：保留结构化上下文，隐藏实现体。
#[derive(Debug, Default, Clone, Copy)]
pub struct AggressiveFilter;

impl FilterStrategy for AggressiveFilter {
    fn filter(&self, content: &str, language: Language) -> String {
        let minimal = MinimalFilter.filter(content, language);
        let filtered = match language {
            Language::Python => filter_python_aggressive(&minimal),
            Language::JavaScript
            | Language::TypeScript
            | Language::Rust
            | Language::Java
            | Language::CSharp
            | Language::Go
            | Language::Unknown => filter_brace_aggressive(&minimal),
        };
        normalize_blank_lines(&filtered)
    }
}

/// 判断是否为导入语句。
#[must_use]
pub fn is_import_line(line: &str) -> bool {
    static IMPORT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        // Invariant: this literal regex is covered by unit tests and contains no runtime input.
        Regex::new(r"^(use\s+|import\s+|from\s+\S+\s+import\s+|require\(|package\s+)")
            .expect("导入语句正则必须有效")
    });
    IMPORT_PATTERN.is_match(line.trim())
}

/// 判断是否为函数或方法签名。
#[must_use]
pub fn is_function_signature(line: &str) -> bool {
    static FUNCTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        // Invariant: this literal regex is covered by unit tests and contains no runtime input.
        Regex::new(
            r"^((pub|private|protected|static|async|export|final|override)\s+)*(fn|def|function|func)\s+[A-Za-z_][A-Za-z0-9_]*",
        )
        .expect("函数签名正则必须有效")
    });
    FUNCTION_PATTERN.is_match(line.trim())
}

/// 判断是否为类型定义。
#[must_use]
pub fn is_type_definition(line: &str) -> bool {
    static TYPE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        // Invariant: this literal regex is covered by unit tests and contains no runtime input.
        Regex::new(
            r"^((pub|private|protected|export|abstract|sealed|final)\s+)*(class|struct|enum|trait|interface|type|impl)\s+[A-Za-z_][A-Za-z0-9_]*",
        )
        .expect("类型定义正则必须有效")
    });
    TYPE_PATTERN.is_match(line.trim())
}

/// 估算 token 数。
#[must_use]
pub fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(ESTIMATED_CHARS_PER_TOKEN)
}

/// 计算过滤节省比例。
#[must_use]
pub fn calculate_savings(original: &str, filtered: &str) -> u8 {
    let original_tokens = estimate_tokens(original);
    if original_tokens == 0 {
        return 0;
    }
    let filtered_tokens = estimate_tokens(filtered);
    let saved_tokens = original_tokens.saturating_sub(filtered_tokens);
    let pct = saved_tokens.saturating_mul(usize::from(MAX_SAVINGS_PCT)) / original_tokens;
    u8::try_from(pct).unwrap_or(MAX_SAVINGS_PCT)
}

fn strip_comments(content: &str, patterns: CommentPatterns) -> String {
    let mut output = String::with_capacity(content.len());
    let mut remaining = content;
    let mut in_block = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while !remaining.is_empty() {
        if in_block {
            let Some(end) = patterns.block_end else {
                return output;
            };
            if remaining.starts_with(end) {
                remaining = &remaining[end.len()..];
                in_block = false;
                continue;
            }
            let Some(ch) = remaining.chars().next() else {
                break;
            };
            if ch == '\n' {
                output.push('\n');
            }
            remaining = &remaining[ch.len_utf8()..];
            continue;
        }

        let Some(ch) = remaining.chars().next() else {
            break;
        };

        if let Some(current_quote) = quote {
            output.push(ch);
            remaining = &remaining[ch.len_utf8()..];
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == current_quote {
                quote = None;
            }
            continue;
        }

        if starts_with_pattern(remaining, patterns.block_start) {
            let Some(block_start) = patterns.block_start else {
                output.push(ch);
                remaining = &remaining[ch.len_utf8()..];
                continue;
            };
            in_block = true;
            remaining = &remaining[block_start.len()..];
            continue;
        }

        if starts_with_pattern(remaining, patterns.line) {
            let Some(next_line_index) = remaining.find('\n') else {
                break;
            };
            output.push('\n');
            remaining = &remaining[next_line_index + 1..];
            continue;
        }

        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
        output.push(ch);
        remaining = &remaining[ch.len_utf8()..];
    }
    output
}

fn starts_with_pattern(value: &str, pattern: Option<&str>) -> bool {
    pattern.is_some_and(|pattern| value.starts_with(pattern))
}

fn normalize_blank_lines(content: &str) -> String {
    static MULTIPLE_BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| {
        // Invariant: this literal regex is covered by unit tests and contains no runtime input.
        Regex::new(r"\n{3,}").expect("空行正则必须有效")
    });
    static BLANK_LINE_WITH_SPACES: LazyLock<Regex> = LazyLock::new(|| {
        // Invariant: this literal regex is covered by unit tests and contains no runtime input.
        Regex::new(r"(?m)^[ \t]+$").expect("空白行正则必须有效")
    });
    static TRAILING_WHITESPACE: LazyLock<Regex> = LazyLock::new(|| {
        // Invariant: this literal regex is covered by unit tests and contains no runtime input.
        Regex::new(r"(?m)[ \t]+$").expect("行尾空白正则必须有效")
    });
    let content = TRAILING_WHITESPACE.replace_all(content.trim(), "");
    let content = BLANK_LINE_WITH_SPACES.replace_all(&content, "");
    MULTIPLE_BLANK_LINES
        .replace_all(&content, "\n\n")
        .into_owned()
}

fn filter_brace_aggressive(content: &str) -> String {
    let mut output = Vec::new();
    let mut skipping_body = false;
    let mut depth = 0isize;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if skipping_body {
            depth += brace_delta(trimmed);
            if depth <= 0 {
                output.push(closing_brace_line(line));
                skipping_body = false;
            }
            continue;
        }

        if is_import_line(trimmed) || is_type_definition(trimmed) {
            output.push(Cow::Borrowed(line));
            continue;
        }

        if is_function_signature(trimmed) {
            output.push(Cow::Borrowed(line));
            if trimmed.contains('{') {
                output.push(Cow::Owned(format!(
                    "{}    {AGGRESSIVE_PLACEHOLDER}",
                    indent(line)
                )));
                depth = brace_delta(trimmed);
                skipping_body = depth > 0;
                if !skipping_body {
                    output.push(closing_brace_line(line));
                }
            }
            continue;
        }

        if is_constant_definition(trimmed) {
            output.push(Cow::Borrowed(line));
        }
    }

    output.join("\n")
}

fn filter_python_aggressive(content: &str) -> String {
    let mut output = Vec::new();
    let mut skip_indent: Option<usize> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let current_indent = leading_spaces(line);

        if let Some(block_indent) = skip_indent {
            if current_indent > block_indent {
                continue;
            }
            skip_indent = None;
        }

        if is_import_line(trimmed) || is_type_definition(trimmed) {
            output.push(line.to_owned());
            continue;
        }

        if is_function_signature(trimmed) {
            output.push(line.to_owned());
            output.push(format!("{}    ...", indent(line)));
            skip_indent = Some(current_indent);
            continue;
        }

        if is_constant_definition(trimmed) {
            output.push(line.to_owned());
        }
    }

    output.join("\n")
}

fn is_constant_definition(line: &str) -> bool {
    static CONSTANT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        // Invariant: this literal regex is covered by unit tests and contains no runtime input.
        Regex::new(
            r"^((pub|public|private|protected)\s+)?(const|static|final|let|var)\s+[A-Za-z_][A-Za-z0-9_]*",
        )
            .expect("常量定义正则必须有效")
    });
    CONSTANT_PATTERN.is_match(line.trim())
}

fn brace_delta(line: &str) -> isize {
    let opened = line.matches('{').count();
    let closed = line.matches('}').count();
    isize::try_from(opened).unwrap_or_default() - isize::try_from(closed).unwrap_or_default()
}

fn closing_brace_line(reference: &str) -> Cow<'_, str> {
    Cow::Owned(format!("{}}}", indent(reference)))
}

fn indent(line: &str) -> &str {
    let end = line
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
        .unwrap_or(line.len());
    &line[..end]
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}

#[cfg(test)]
mod tests {
    use super::{Language, RegexFallbackFilter};

    #[test]
    fn language_detection_supports_roadmap_extensions() {
        assert_eq!(Language::from_extension("js"), Language::JavaScript);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("java"), Language::Java);
        assert_eq!(Language::from_extension("cs"), Language::CSharp);
        assert_eq!(Language::from_extension("go"), Language::Go);
    }

    #[test]
    fn minimal_filter_removes_line_and_block_comments() {
        let content =
            "// module note\nfn main() {\n    /* noisy */\n    run(); // inline note\n}\n";

        let filtered = RegexFallbackFilter.filter_minimal(content, Language::Rust);

        assert_eq!(filtered, "fn main() {\n\n    run();\n}");
    }

    #[test]
    fn minimal_filter_handles_python_hash_and_docstring_blocks() {
        let content = "# note\ndef run():\n    \"\"\"doc\"\"\"\n    return 1\n";

        let filtered = RegexFallbackFilter.filter_minimal(content, Language::Python);

        assert_eq!(filtered, "def run():\n\n    return 1");
    }

    #[test]
    fn minimal_filter_removes_c_style_comments_for_supported_languages() {
        for language in [
            Language::JavaScript,
            Language::TypeScript,
            Language::Java,
            Language::CSharp,
            Language::Go,
        ] {
            let content = "// note\nfunction run() {\n  /* noisy */\n  call();\n}\n";

            let filtered = RegexFallbackFilter.filter_minimal(content, language);

            assert_eq!(filtered, "function run() {\n\n  call();\n}");
        }
    }

    #[test]
    fn minimal_filter_keeps_comment_markers_inside_strings() {
        let content = "fn main() {\n    let url = \"https://example.com/*path*/\";\n}\n";

        let filtered = RegexFallbackFilter.filter_minimal(content, Language::Rust);

        assert!(filtered.contains("https://example.com/*path*/"));
    }

    #[test]
    fn aggressive_filter_keeps_rust_structure_without_body() {
        let content = r"
use std::path::Path;

pub struct Runner {
    name: String,
}

pub fn run(path: &Path) -> usize {
    let value = path.display().to_string();
    value.len()
}
";

        let filtered = RegexFallbackFilter.filter_aggressive(content, Language::Rust);

        assert_eq!(
            filtered,
            "use std::path::Path;\npub struct Runner {\npub fn run(path: &Path) -> usize {\n    // ...\n}"
        );
    }

    #[test]
    fn aggressive_filter_keeps_python_signatures() {
        let content = r#"
import os

class Runner:
    def __init__(self):
        self.name = "axon"

def run():
    return os.getcwd()
"#;

        let filtered = RegexFallbackFilter.filter_aggressive(content, Language::Python);

        assert_eq!(
            filtered,
            "import os\nclass Runner:\n    def __init__(self):\n        ...\ndef run():\n    ..."
        );
    }
}
