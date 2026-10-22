//! 基于 LSP documentSymbol 的代码符号过滤。

#[path = "lsp_symbol_filter/cache.rs"]
mod cache;
#[path = "lsp_symbol_filter/formatter.rs"]
mod formatter;
#[path = "lsp_symbol_filter/parser.rs"]
mod parser;

use axon_domain::{AxonError, AxonResult, ToolError};
use serde::Serialize;
use serde_json::Value;

use self::{
    cache::{cached_document_symbols, store_document_symbols},
    formatter::format_symbols,
    parser::parse_first_symbol_hit,
};

const ESTIMATED_CHARS_PER_TOKEN: usize = 4;
const MAX_SAVINGS_PCT: u8 = 100;
pub(super) const LSP_DOCUMENT_SYMBOL_EMPTY_ERROR: &str = "LSP documentSymbol 未返回可用符号";
pub(super) const LSP_DOCUMENT_SYMBOL_INVALID_ERROR: &str = "LSP documentSymbol 响应结构无效";

/// LSP 符号过滤级别。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LspFilterLevel {
    /// 不过滤，原样返回。
    #[default]
    None,
    /// 轻量过滤，后续用于移除注释与低价值空白。
    Minimal,
    /// 激进过滤，后续用于提取符号签名与结构。
    Aggressive,
}

impl LspFilterLevel {
    /// 返回 provider/tool schema 使用的稳定字符串。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Aggressive => "aggressive",
        }
    }

    /// 从 provider/tool 参数解析过滤级别。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "aggressive" => Some(Self::Aggressive),
            _ => None,
        }
    }
}

/// Token 节省统计。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokenSavings {
    /// 原始内容估算 token 数。
    pub original_tokens: usize,
    /// 过滤后内容估算 token 数。
    pub filtered_tokens: usize,
    /// 估算节省比例。
    pub savings_pct: u8,
}

/// LSP 符号过滤结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LspFilterResult {
    /// 过滤后的内容。
    pub filtered_content: String,
    /// 提取出的符号列表。
    pub symbols: Vec<ExtractedSymbol>,
    /// 过滤后内容的估算 token 数。
    pub tokens: usize,
    /// 相对原始内容的估算节省比例。
    pub savings_pct: u8,
    /// Token 节省明细。
    pub token_savings: TokenSavings,
    /// 本次过滤是否真实使用了 LSP。
    pub lsp_used: bool,
}

/// 从源码中提取出的符号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractedSymbol {
    /// 符号名称。
    pub name: String,
    /// 符号类型。
    pub kind: String,
    /// 符号签名。
    pub signature: String,
    /// 符号源码范围。
    pub range: SymbolRange,
    /// 子符号列表。
    pub children: Vec<ExtractedSymbol>,
}

/// 符号源码范围，行列均采用 1-based 坐标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SymbolRange {
    /// 起始行。
    pub start_line: u32,
    /// 起始列。
    pub start_character: u32,
    /// 结束行。
    pub end_line: u32,
    /// 结束列。
    pub end_character: u32,
}

/// documentSymbol 查询入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocumentSymbolQuery {
    /// 工作区根目录。
    pub workspace_root: String,
    /// 目标文档 URI。
    pub text_document_uri: String,
}

/// 单个 LSP server 返回的 documentSymbol 响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocumentSymbolHit {
    /// 命中对应的 server。
    pub server_id: String,
    /// 原始 JSON-RPC 响应。
    pub response: Value,
}

/// documentSymbol 查询端口，由运行时适配真实 LSP 实现。
pub trait LspDocumentSymbolProvider: std::fmt::Debug {
    /// 查询目标文档的符号列表。
    ///
    /// # Errors
    ///
    /// 当 LSP 不可用、启动失败或返回错误响应时返回错误。
    fn query_document_symbols(
        &self,
        query: &LspDocumentSymbolQuery,
    ) -> AxonResult<Vec<LspDocumentSymbolHit>>;
}

/// LSP 符号过滤器。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspSymbolFilter;

impl LspSymbolFilter {
    /// 原样返回源码内容。
    #[must_use]
    pub fn filter_none(&self, content: &str) -> LspFilterResult {
        filter_none(content)
    }

    /// 使用已完成的 Minimal 内容生成过滤结果。
    #[must_use]
    pub fn filter_minimal_content(
        &self,
        content: &str,
        filtered_content: String,
    ) -> LspFilterResult {
        let token_savings = token_savings(content, &filtered_content);
        LspFilterResult {
            tokens: estimate_tokens(&filtered_content),
            savings_pct: token_savings.savings_pct,
            token_savings,
            filtered_content,
            symbols: Vec::new(),
            lsp_used: false,
        }
    }

    /// 通过注入的 LSP provider 查询并解析 documentSymbol。
    ///
    /// # Errors
    ///
    /// 当 provider 不可用、无命中或响应结构不可解析时返回错误。
    pub fn query_document_symbols(
        &self,
        provider: &(impl LspDocumentSymbolProvider + ?Sized),
        query: &LspDocumentSymbolQuery,
    ) -> AxonResult<Vec<ExtractedSymbol>> {
        if let Some(symbols) = cached_document_symbols(query) {
            return Ok(symbols);
        }
        let hits = provider.query_document_symbols(query)?;
        let symbols = parse_first_symbol_hit(&hits)?;
        store_document_symbols(query, &symbols);
        Ok(symbols)
    }

    /// 使用已提取符号生成 aggressive 过滤结果。
    #[must_use]
    pub fn filter_aggressive_symbols(
        &self,
        content: &str,
        symbols: Vec<ExtractedSymbol>,
    ) -> LspFilterResult {
        let filtered_content = format_symbols(&symbols);
        let token_savings = token_savings(content, &filtered_content);
        LspFilterResult {
            tokens: estimate_tokens(&filtered_content),
            savings_pct: token_savings.savings_pct,
            token_savings,
            filtered_content,
            symbols,
            lsp_used: true,
        }
    }

    /// 解析 JSON-RPC documentSymbol 响应。
    ///
    /// # Errors
    ///
    /// 当响应缺少 result 或符号范围字段不完整时返回错误。
    pub fn parse_document_symbols(response: &Value) -> AxonResult<Vec<ExtractedSymbol>> {
        let Some(result) = response.get("result") else {
            return Err(AxonError::Tool(ToolError::InvalidDefinition(
                LSP_DOCUMENT_SYMBOL_INVALID_ERROR.to_owned(),
            )));
        };
        parser::parse_symbol_items(result)
    }
}

fn filter_none(content: &str) -> LspFilterResult {
    let tokens = estimate_tokens(content);
    let token_savings = token_savings(content, content);
    LspFilterResult {
        filtered_content: content.to_owned(),
        symbols: Vec::new(),
        tokens,
        savings_pct: token_savings.savings_pct,
        token_savings,
        lsp_used: false,
    }
}

fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(ESTIMATED_CHARS_PER_TOKEN)
}

fn calculate_savings(original: &str, filtered: &str) -> u8 {
    let original_tokens = estimate_tokens(original);
    if original_tokens == 0 {
        return 0;
    }
    let filtered_tokens = estimate_tokens(filtered);
    let saved_tokens = original_tokens.saturating_sub(filtered_tokens);
    let pct = saved_tokens.saturating_mul(usize::from(MAX_SAVINGS_PCT)) / original_tokens;
    u8::try_from(pct).unwrap_or(MAX_SAVINGS_PCT)
}

fn token_savings(original: &str, filtered: &str) -> TokenSavings {
    TokenSavings {
        original_tokens: estimate_tokens(original),
        filtered_tokens: estimate_tokens(filtered),
        savings_pct: calculate_savings(original, filtered),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::PathBuf;

    use axon_domain::{AxonError, AxonResult, ToolError};
    use serde_json::json;

    const TEST_WORKSPACE_PREFIX: &str = "axon-output-filter-symbol-cache";

    use super::{
        LspDocumentSymbolHit, LspDocumentSymbolProvider, LspDocumentSymbolQuery, LspSymbolFilter,
        parser::map_lsp_symbol_kind,
    };

    #[derive(Debug)]
    struct StaticProvider {
        result: AxonResult<Vec<LspDocumentSymbolHit>>,
    }

    impl LspDocumentSymbolProvider for StaticProvider {
        fn query_document_symbols(
            &self,
            _query: &LspDocumentSymbolQuery,
        ) -> AxonResult<Vec<LspDocumentSymbolHit>> {
            self.result.clone()
        }
    }

    #[derive(Debug)]
    struct CountingProvider {
        calls: Cell<usize>,
    }

    impl LspDocumentSymbolProvider for CountingProvider {
        fn query_document_symbols(
            &self,
            _query: &LspDocumentSymbolQuery,
        ) -> AxonResult<Vec<LspDocumentSymbolHit>> {
            self.calls.set(self.calls.get().saturating_add(1));
            Ok(vec![LspDocumentSymbolHit {
                server_id: "rust-analyzer".to_owned(),
                response: json!({
                    "result": [{
                        "name": "main",
                        "kind": 12,
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 9 }
                        }
                    }]
                }),
            }])
        }
    }

    #[test]
    fn none_level_returns_original_content_without_lsp() {
        let result = LspSymbolFilter.filter_none("fn main() {}");

        assert_eq!(result.filtered_content, "fn main() {}");
        assert!(result.symbols.is_empty());
        assert!(!result.lsp_used);
        assert_eq!(result.savings_pct, 0);
        assert_eq!(result.token_savings.savings_pct, 0);
    }

    #[test]
    fn minimal_content_returns_filtered_text_without_lsp() {
        let result = LspSymbolFilter
            .filter_minimal_content("// note\nfn main() {}", "fn main() {}".to_owned());

        assert_eq!(result.filtered_content, "fn main() {}");
        assert!(result.symbols.is_empty());
        assert!(!result.lsp_used);
        assert!(result.tokens > 0);
    }

    #[test]
    fn parses_nested_document_symbols_with_one_based_range() {
        let response = json!({
            "result": [{
                "name": "Service",
                "kind": 5,
                "detail": "struct Service",
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 10, "character": 1 }
                },
                "children": [{
                    "name": "run",
                    "kind": 6,
                    "range": {
                        "start": { "line": 2, "character": 4 },
                        "end": { "line": 4, "character": 5 }
                    }
                }]
            }]
        });

        let symbols = LspSymbolFilter::parse_document_symbols(&response).unwrap();

        assert_eq!(symbols[0].name, "Service");
        assert_eq!(symbols[0].signature, "struct Service");
        assert_eq!(symbols[0].kind, "class");
        assert_eq!(symbols[0].range.start_line, 1);
        assert_eq!(symbols[0].children[0].name, "run");
        assert_eq!(symbols[0].children[0].signature, "method run");
        assert_eq!(symbols[0].children[0].range.start_character, 5);
    }

    #[test]
    fn aggressive_symbols_format_signatures_with_placeholders() {
        let symbols = LspSymbolFilter::parse_document_symbols(&json!({
            "result": [{
                "name": "Service",
                "kind": 5,
                "detail": "struct Service",
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 8, "character": 1 }
                },
                "children": [{
                    "name": "run",
                    "kind": 12,
                    "detail": "fn run(&self)",
                    "range": {
                        "start": { "line": 2, "character": 4 },
                        "end": { "line": 4, "character": 5 }
                    }
                }]
            }]
        }))
        .unwrap();

        let result = LspSymbolFilter.filter_aggressive_symbols(
            "struct Service { fn run(&self) { println!(\"axon\"); } }",
            symbols,
        );

        assert!(result.lsp_used);
        assert_eq!(
            result.filtered_content,
            "struct Service {\n    fn run(&self) {\n        ...\n    }\n}\n"
        );
        assert_eq!(result.symbols.len(), 1);
    }

    #[test]
    fn maps_lsp_symbol_kind_numbers_to_stable_labels() {
        assert_eq!(map_lsp_symbol_kind(Some(12)), "function");
        assert_eq!(map_lsp_symbol_kind(Some(23)), "struct");
        assert_eq!(map_lsp_symbol_kind(None), "unknown");
    }

    #[test]
    fn query_document_symbols_propagates_provider_error() {
        let provider = StaticProvider {
            result: Err(AxonError::Tool(ToolError::InvalidDefinition(
                "LSP 不可用".to_owned(),
            ))),
        };

        let result = LspSymbolFilter.query_document_symbols(
            &provider,
            &LspDocumentSymbolQuery {
                workspace_root: "E:/repo".to_owned(),
                text_document_uri: "file:///E:/repo/src/lib.rs".to_owned(),
            },
        );

        assert!(matches!(
            result,
            Err(AxonError::Tool(ToolError::InvalidDefinition(message))) if message == "LSP 不可用"
        ));
    }

    #[test]
    fn query_document_symbols_uses_first_non_empty_hit() {
        let provider = StaticProvider {
            result: Ok(vec![
                LspDocumentSymbolHit {
                    server_id: "empty".to_owned(),
                    response: json!({ "result": [] }),
                },
                LspDocumentSymbolHit {
                    server_id: "rust-analyzer".to_owned(),
                    response: json!({
                        "result": [{
                            "name": "main",
                            "kind": 12,
                            "location": {
                                "uri": "file:///E:/repo/src/main.rs",
                                "range": {
                                    "start": { "line": 1, "character": 0 },
                                    "end": { "line": 1, "character": 9 }
                                }
                            }
                        }]
                    }),
                },
            ]),
        };

        let symbols = LspSymbolFilter
            .query_document_symbols(
                &provider,
                &LspDocumentSymbolQuery {
                    workspace_root: "E:/repo".to_owned(),
                    text_document_uri: "file:///E:/repo/src/main.rs".to_owned(),
                },
            )
            .unwrap();

        assert_eq!(symbols[0].name, "main");
        assert_eq!(symbols[0].range.start_line, 2);
    }

    #[test]
    fn query_document_symbols_reuses_cache_until_file_hash_changes() {
        let workspace = temp_workspace();
        let src = workspace.join("src");
        fs::create_dir_all(&src).unwrap();
        let file = src.join("lib.rs");
        fs::write(&file, "fn main() {}\n").unwrap();
        let query = LspDocumentSymbolQuery {
            workspace_root: workspace.display().to_string(),
            text_document_uri: format!("file:///{}", file.to_string_lossy().replace('\\', "/")),
        };
        let provider = CountingProvider {
            calls: Cell::new(0),
        };

        let first = LspSymbolFilter
            .query_document_symbols(&provider, &query)
            .unwrap();
        let second = LspSymbolFilter
            .query_document_symbols(&provider, &query)
            .unwrap();
        fs::write(&file, "fn main() {}\nfn changed() {}\n").unwrap();
        let third = LspSymbolFilter
            .query_document_symbols(&provider, &query)
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(third[0].name, "main");
        assert_eq!(provider.calls.get(), 2);
        fs::remove_dir_all(workspace).unwrap();
    }

    fn temp_workspace() -> PathBuf {
        let path = axon_test_support::test_temp_root()
            .join(format!("{TEST_WORKSPACE_PREFIX}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }
}
