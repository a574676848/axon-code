use std::time::{Duration, Instant};

use axon_display::format_duration_millis_compact;
use axon_output_filter::{
    OutputFilter, lsp_symbol_filter::LspFilterLevel, regex_fallback::Language,
};

const COMMON_LANGUAGE_COUNT: usize = 7;
const MIN_LSP_SUPPORT_PERCENT: usize = 80;
const LSP_FILTER_MAX: Duration = Duration::from_millis(100);
const REGEX_FILTER_MAX: Duration = Duration::from_millis(5);

fn main() {
    let report = AcceptanceReport {
        lsp_support_percent: lsp_support_percent(),
        regex_language_count: regex_language_count(),
        aggressive_read_savings: aggressive_read_savings(),
        lsp_filter_elapsed: timed(|| {
            let _ = OutputFilter
                .filter_code(
                    rust_large_sample(),
                    Language::Rust,
                    LspFilterLevel::Aggressive,
                )
                .expect("aggressive 过滤应成功");
        }),
        regex_filter_elapsed: timed(|| {
            let _ = axon_output_filter::regex_fallback::RegexFallbackFilter
                .filter_minimal(rust_large_sample(), Language::Rust);
        }),
    };
    report.assert_passed();
    println!("{}", report.summary());
}

struct AcceptanceReport {
    lsp_support_percent: usize,
    regex_language_count: usize,
    aggressive_read_savings: u8,
    lsp_filter_elapsed: Duration,
    regex_filter_elapsed: Duration,
}

impl AcceptanceReport {
    fn assert_passed(&self) {
        assert!(self.lsp_support_percent > MIN_LSP_SUPPORT_PERCENT);
        assert!(self.regex_language_count >= COMMON_LANGUAGE_COUNT);
        assert!((60..=90).contains(&self.aggressive_read_savings));
        assert!(self.lsp_filter_elapsed < LSP_FILTER_MAX);
        assert!(self.regex_filter_elapsed < REGEX_FILTER_MAX);
    }

    fn summary(&self) -> String {
        format!(
            "output-filter acceptance OK: lsp_support={}%, regex_languages={}, read_savings={}%, lsp={}, regex={}",
            self.lsp_support_percent,
            self.regex_language_count,
            self.aggressive_read_savings,
            format_elapsed(self.lsp_filter_elapsed),
            format_elapsed(self.regex_filter_elapsed)
        )
    }
}

fn format_elapsed(duration: Duration) -> String {
    format_duration_millis_compact(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

fn lsp_support_percent() -> usize {
    let supported = language_cases()
        .into_iter()
        .filter(|case| {
            !OutputFilter
                .filter_code(case.source, case.language, LspFilterLevel::Aggressive)
                .expect("aggressive 过滤应成功")
                .filtered_content
                .is_empty()
        })
        .count();
    supported * 100 / COMMON_LANGUAGE_COUNT
}

fn regex_language_count() -> usize {
    language_cases()
        .into_iter()
        .filter(|case| Language::from_extension(case.extension) == case.language)
        .count()
}

fn aggressive_read_savings() -> u8 {
    OutputFilter
        .filter_code(
            rust_large_sample(),
            Language::Rust,
            LspFilterLevel::Aggressive,
        )
        .expect("aggressive 过滤应成功")
        .savings_pct
}

fn timed(operation: impl FnOnce()) -> Duration {
    let start = Instant::now();
    operation();
    start.elapsed()
}

struct LanguageCase {
    extension: &'static str,
    language: Language,
    source: &'static str,
}

fn language_cases() -> [LanguageCase; COMMON_LANGUAGE_COUNT] {
    [
        LanguageCase {
            extension: "js",
            language: Language::JavaScript,
            source: "import x from 'x';\nfunction run() { console.log(x); }\n",
        },
        LanguageCase {
            extension: "ts",
            language: Language::TypeScript,
            source: "export type User = { id: string };\nexport function run() { return 1; }\n",
        },
        LanguageCase {
            extension: "py",
            language: Language::Python,
            source: "import os\nclass Runner:\n    def run(self):\n        return os.getcwd()\n",
        },
        LanguageCase {
            extension: "rs",
            language: Language::Rust,
            source: "use std::path::Path;\npub struct Runner;\npub fn run() {}\n",
        },
        LanguageCase {
            extension: "java",
            language: Language::Java,
            source: "package demo;\npublic class Runner { public void run() {} }\n",
        },
        LanguageCase {
            extension: "cs",
            language: Language::CSharp,
            source: "public class Runner { public void Run() {} }\n",
        },
        LanguageCase {
            extension: "go",
            language: Language::Go,
            source: "package main\nfunc run() { println(\"ok\") }\n",
        },
    ]
}

fn rust_large_sample() -> &'static str {
    "use std::path::Path;\n\npub struct Runner {\n    name: String,\n}\n\npub fn run(path: &Path) -> usize {\n    let value = path.display().to_string();\n    let doubled = format!(\"{}{}\", value, value);\n    let normalized = doubled.replace('\\\\', \"/\");\n    normalized.len()\n}\n\npub fn collect() -> Vec<String> {\n    vec![\"alpha\".to_owned(), \"beta\".to_owned(), \"gamma\".to_owned()]\n}\n"
}
