//! Fragment 构造器：基于运行时状态动态构造 context fragments。

use std::path::{Path, PathBuf};

use axon_context_fragments::PromptFragment;
use axon_output_filter::{
    ExtractedSymbol, LspDocumentSymbolProvider, LspDocumentSymbolQuery, LspSymbolFilter,
};

use crate::{
    AgentContextSnapshot, AgentPromptContext, LspDocumentLocationRuntime,
    context::fragments::DocumentSymbolContextFragment,
};

/// Fragment 构造器配置。
#[derive(Debug, Clone)]
pub struct FragmentBuilderConfig<'a> {
    /// 上下文快照。
    pub context_snapshot: &'a AgentContextSnapshot,
    /// Prompt 工程上下文。
    pub prompt_context: &'a AgentPromptContext,
    /// 运行时约束字符串（可选）。
    pub runtime_constraints: Option<&'a str>,
    /// 用户原始 prompt，用于提取 `path:line` 这类精确源码引用。
    pub user_prompt: Option<&'a str>,
    /// documentSymbol 查询 provider；None 时使用默认 LSP runtime。
    pub document_symbol_provider: Option<&'a dyn LspDocumentSymbolProvider>,
}

/// Fragment 构造器。
#[derive(Debug, Default, Clone, Copy)]
pub struct FragmentBuilder;

impl FragmentBuilder {
    /// 基于配置构造所有需要注入的 fragments。
    #[must_use]
    pub fn build_fragments(
        &self,
        config: &FragmentBuilderConfig<'_>,
    ) -> Vec<Box<dyn PromptFragment>> {
        document_symbol_context_fragments(config)
    }
}

fn document_symbol_context_fragments(
    config: &FragmentBuilderConfig<'_>,
) -> Vec<Box<dyn PromptFragment>> {
    let Some(user_prompt) = config.user_prompt else {
        return Vec::new();
    };
    let references = parse_prompt_line_references(user_prompt);
    if references.is_empty() {
        return Vec::new();
    }
    let workspace_root = PathBuf::from(&config.context_snapshot.cwd);
    let runtime_provider = LspDocumentLocationRuntime;
    let provider = config.document_symbol_provider.unwrap_or(&runtime_provider);
    references
        .into_iter()
        .filter_map(|reference| {
            resolve_symbol_context(provider, &workspace_root, &reference).map(|symbol| {
                Box::new(DocumentSymbolContextFragment::new(
                    reference.path,
                    reference.line,
                    symbol.kind,
                    symbol.name,
                    symbol.signature,
                    symbol.range.start_line,
                    symbol.range.end_line,
                )) as Box<dyn PromptFragment>
            })
        })
        .collect()
}

fn resolve_symbol_context(
    provider: &dyn LspDocumentSymbolProvider,
    workspace_root: &Path,
    reference: &PromptLineReference,
) -> Option<ExtractedSymbol> {
    let target = workspace_root.join(&reference.path);
    if !target.is_file() {
        return None;
    }
    let query = LspDocumentSymbolQuery {
        workspace_root: workspace_root.display().to_string(),
        text_document_uri: path_to_file_uri(&target),
    };
    let symbols = LspSymbolFilter
        .query_document_symbols(provider, &query)
        .ok()?;
    find_symbol_for_line(&symbols, reference.line)
}

fn find_symbol_for_line(symbols: &[ExtractedSymbol], line: u32) -> Option<ExtractedSymbol> {
    let mut best = None;
    for symbol in symbols {
        if symbol.range.start_line <= line && line <= symbol.range.end_line {
            best = find_symbol_for_line(&symbol.children, line).or_else(|| Some(symbol.clone()));
        }
    }
    best
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptLineReference {
    path: String,
    line: u32,
}

fn parse_prompt_line_references(prompt: &str) -> Vec<PromptLineReference> {
    let mut references = Vec::new();
    for token in prompt.split_whitespace() {
        let token = trim_prompt_token(token);
        let Some((path, line)) = token.rsplit_once(':') else {
            continue;
        };
        if !looks_like_source_path(path) {
            continue;
        }
        let Ok(line) = line.parse::<u32>() else {
            continue;
        };
        if line == 0 {
            continue;
        }
        let path = path.replace('\\', "/");
        if references
            .iter()
            .any(|item: &PromptLineReference| item.path == path && item.line == line)
        {
            continue;
        }
        references.push(PromptLineReference { path, line });
    }
    references
}

fn trim_prompt_token(token: &str) -> &str {
    token.trim_matches(|value: char| {
        matches!(
            value,
            '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
    })
}

fn looks_like_source_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    if path.starts_with('/')
        || path.contains("://")
        || path.contains("..")
        || path.as_bytes().get(1).is_some_and(|value| *value == b':')
    {
        return false;
    }
    path.contains('/')
        && path.rsplit('/').next().is_some_and(|file| {
            file.contains('.') && !file.starts_with('.') && !file.ends_with('.')
        })
}

fn path_to_file_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axon_domain::{AxonResult, PaneId, TokenBudget};
    use axon_output_filter::LspDocumentSymbolHit;
    use serde_json::json;

    use super::*;
    use crate::{AgentContextRuntime, AgentContextSnapshotInput};

    #[derive(Debug)]
    struct StaticDocumentSymbolProvider;

    impl LspDocumentSymbolProvider for StaticDocumentSymbolProvider {
        fn query_document_symbols(
            &self,
            _query: &LspDocumentSymbolQuery,
        ) -> AxonResult<Vec<LspDocumentSymbolHit>> {
            Ok(vec![LspDocumentSymbolHit {
                server_id: "rust-analyzer".to_owned(),
                response: json!({
                    "result": [{
                        "name": "Service",
                        "kind": 23,
                        "detail": "struct Service",
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 9, "character": 1 }
                        },
                        "children": [{
                            "name": "run",
                            "kind": 6,
                            "detail": "fn run(&self)",
                            "range": {
                                "start": { "line": 3, "character": 4 },
                                "end": { "line": 6, "character": 5 }
                            }
                        }]
                    }]
                }),
            }])
        }
    }

    #[test]
    fn fragment_builder_expands_prompt_line_reference_with_parent_symbol() {
        let workspace = axon_test_support::test_temp_dir();
        let src = workspace.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.rs"), "struct Service;\n").unwrap();
        let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
            pane_id: PaneId(1),
            cwd: workspace.path().display().to_string(),
            git_state: None,
            shell_history: Vec::new(),
            command_activity: None,
            apply_history: Vec::new(),
            chat_history: Vec::new(),
            total_limit: TokenBudget::new(2048).unwrap(),
            reserved_for_tools: TokenBudget::new(256).unwrap(),
        });
        let provider = StaticDocumentSymbolProvider;
        let prompt_context = AgentPromptContext::default();

        let fragments = FragmentBuilder.build_fragments(&FragmentBuilderConfig {
            context_snapshot: &snapshot,
            prompt_context: &prompt_context,
            runtime_constraints: None,
            user_prompt: Some("请查看 `src/lib.rs:5` 的报错"),
            document_symbol_provider: Some(&provider),
        });
        let rendered = fragments
            .into_iter()
            .map(|fragment| fragment.render())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("<document-symbol-context>"));
        assert!(rendered.contains("path=\"src/lib.rs\" line=\"5\""));
        assert!(rendered.contains("name=\"run\""));
        assert!(rendered.contains("fn run(&self)"));
    }

    #[test]
    fn fragment_builder_expands_all_explicit_prompt_line_references() {
        let workspace = axon_test_support::test_temp_dir();
        let src = workspace.path().join("src");
        fs::create_dir_all(&src).unwrap();
        for name in ["a.rs", "b.rs", "c.rs", "d.rs"] {
            fs::write(src.join(name), "struct Service;\n").unwrap();
        }
        let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
            pane_id: PaneId(1),
            cwd: workspace.path().display().to_string(),
            git_state: None,
            shell_history: Vec::new(),
            command_activity: None,
            apply_history: Vec::new(),
            chat_history: Vec::new(),
            total_limit: TokenBudget::new(2048).unwrap(),
            reserved_for_tools: TokenBudget::new(256).unwrap(),
        });
        let provider = StaticDocumentSymbolProvider;
        let prompt_context = AgentPromptContext::default();

        let fragments = FragmentBuilder.build_fragments(&FragmentBuilderConfig {
            context_snapshot: &snapshot,
            prompt_context: &prompt_context,
            runtime_constraints: None,
            user_prompt: Some("src/a.rs:5 src/b.rs:5 src/c.rs:5 src/d.rs:5"),
            document_symbol_provider: Some(&provider),
        });

        assert_eq!(fragments.len(), 4);
    }

    #[test]
    fn prompt_line_reference_parser_ignores_absolute_and_parent_paths() {
        let references = parse_prompt_line_references(
            "src/lib.rs:5 E:/repo/src/lib.rs:6 ../src/lib.rs:7 src/lib.rs:0",
        );

        assert_eq!(
            references,
            vec![PromptLineReference {
                path: "src/lib.rs".to_owned(),
                line: 5,
            }]
        );
    }
}
