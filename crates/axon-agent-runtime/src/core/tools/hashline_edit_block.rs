//! Tree-sitter 驱动的 Hashline BLK 解析。

use std::{
    collections::VecDeque,
    path::Path,
    sync::{Mutex, OnceLock},
};

use tree_sitter::{Node, Parser, Point};

use crate::language_registry::LanguageRegistry;

use super::{HashlineBlockOperation, HashlineBlockResolution, HashlineOp};

const BLOCK_RESOLUTION_CACHE_MAX: usize = 512;
const BLOCK_ERROR_CONTEXT_RADIUS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockSpan {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockCacheKey {
    content_hash: String,
    content_len: usize,
    line: usize,
    path: String,
}

type BlockResolution = (Vec<HashlineOp>, Vec<HashlineBlockResolution>, Vec<String>);
type BlockResolutionResult = Result<BlockResolution, String>;
type BlockCache = Mutex<VecDeque<(BlockCacheKey, Option<BlockSpan>)>>;

pub(super) fn resolve_block_ops(
    path: &str,
    text: &str,
    ops: Vec<HashlineOp>,
) -> BlockResolutionResult {
    if !ops.iter().any(HashlineOp::is_block) {
        return Ok((ops, Vec::new(), Vec::new()));
    }
    let mut resolved_ops = Vec::with_capacity(ops.len());
    let mut resolutions = Vec::new();
    let mut warnings = Vec::new();
    for op in ops {
        match op {
            HashlineOp::SwapBlock { line, lines } => {
                let span = required_block_span(path, text, line, "SWAP.BLK")?;
                resolutions.push(HashlineBlockResolution {
                    anchor_line: line,
                    start: span.start,
                    end: span.end,
                    operation: HashlineBlockOperation::Replace,
                });
                resolved_ops.push(HashlineOp::Swap {
                    start: span.start,
                    end: span.end,
                    lines,
                });
            }
            HashlineOp::DeleteBlock { line } => {
                let span = required_block_span(path, text, line, "DEL.BLK")?;
                resolutions.push(HashlineBlockResolution {
                    anchor_line: line,
                    start: span.start,
                    end: span.end,
                    operation: HashlineBlockOperation::Delete,
                });
                resolved_ops.push(HashlineOp::Delete {
                    start: span.start,
                    end: span.end,
                });
            }
            HashlineOp::InsertAfterBlock { line, lines } => {
                match cached_block_range_at(path, text, line) {
                    Some(span) if span.start != span.end => {
                        resolutions.push(HashlineBlockResolution {
                            anchor_line: line,
                            start: span.start,
                            end: span.end,
                            operation: HashlineBlockOperation::InsertAfter,
                        });
                        resolved_ops.push(HashlineOp::InsertAfter {
                            line: span.end,
                            lines,
                            block_start: Some(span.start),
                        });
                    }
                    Some(_) => {
                        return Err(format!(
                            "INS.BLK.POST {line} 解析为单行节点；该行不是多行语法块起点，请改用 INS.POST {line}:"
                        ));
                    }
                    None => {
                        warnings.push(format!(
                            "无法解析从第 {line} 行开始的语法块；已按普通 INS.POST {line}: 应用"
                        ));
                        resolved_ops.push(HashlineOp::InsertAfter {
                            line,
                            lines,
                            block_start: None,
                        });
                    }
                }
            }
            concrete => resolved_ops.push(concrete),
        }
    }
    Ok((resolved_ops, resolutions, warnings))
}

fn required_block_span(
    path: &str,
    text: &str,
    line: usize,
    operation: &str,
) -> Result<BlockSpan, String> {
    let Some(span) = cached_block_range_at(path, text, line) else {
        let preview = nearby_context(text, line);
        return Err(format!(
            "{operation} 无法解析从第 {line} 行开始的语法块{preview}"
        ));
    };
    if span.start == span.end {
        return Err(format!(
            "{operation} {line} 解析为单行节点；该行不是多行语法块起点，请改用普通行范围操作"
        ));
    }
    Ok(span)
}

fn cached_block_range_at(path: &str, text: &str, line: usize) -> Option<BlockSpan> {
    let key = BlockCacheKey {
        content_hash: axon_crypto::sha256_hex_prefix(text.as_bytes(), 16),
        content_len: text.len(),
        line,
        path: path.to_owned(),
    };
    let cache = block_cache();
    if let Some(result) = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(candidate, _)| candidate == &key)
        .map(|(_, result)| *result)
    {
        return result;
    }
    let result = block_range_at(path, text, line);
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() >= BLOCK_RESOLUTION_CACHE_MAX {
        cache.pop_front();
    }
    cache.push_back((key, result));
    result
}

fn block_cache() -> &'static BlockCache {
    static CACHE: OnceLock<BlockCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn block_range_at(path: &str, text: &str, line: usize) -> Option<BlockSpan> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if line == 0 || text.is_empty() {
        return None;
    }
    let grammar = LanguageRegistry::resolve(Path::new(path))?.grammar()?;
    let row = line - 1;
    let column = first_content_column(text, row)?;
    let language = grammar.tree_sitter_language();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(text, None)?;
    let root = tree.root_node();
    let point = Point::new(row, column);
    let point_end = Point::new(row, column.saturating_add(1));
    let leaf = root.named_descendant_for_point_range(point, point_end)?;
    if leaf.start_position().row != row {
        return None;
    }

    let mut node = leaf;
    while let Some(parent) = node.parent() {
        if parent.id() == root.id() || parent.start_position().row != row {
            break;
        }
        node = parent;
    }
    if node.has_error() {
        return None;
    }
    Some(BlockSpan {
        start: node_start_line(node),
        end: node_content_end_line(node),
    })
}

fn first_content_column(text: &str, row: usize) -> Option<usize> {
    text.split('\n')
        .nth(row)?
        .bytes()
        .position(|byte| byte != b' ' && byte != b'\t')
}

fn node_start_line(node: Node<'_>) -> usize {
    node.start_position().row.saturating_add(1)
}

fn node_content_end_line(node: Node<'_>) -> usize {
    let position = node.end_position();
    if position.column == 0 && position.row > 0 {
        position.row
    } else {
        position.row.saturating_add(1)
    }
}

fn nearby_context(text: &str, anchor: usize) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    if anchor == 0 || anchor > lines.len() {
        return String::new();
    }
    let start = anchor.saturating_sub(BLOCK_ERROR_CONTEXT_RADIUS + 1);
    let end = anchor
        .saturating_add(BLOCK_ERROR_CONTEXT_RADIUS)
        .min(lines.len());
    let preview = (start..end)
        .map(|index| {
            let line = index + 1;
            let marker = if line == anchor { '*' } else { ' ' };
            format!("{marker}{line}:{}", lines[index])
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\n{preview}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_outermost_named_block_starting_on_anchor_line() {
        let text = "function f() {\n  if (a) {\n    g();\n  }\n}\n";
        assert_eq!(
            block_range_at("f.ts", text, 2),
            Some(BlockSpan { start: 2, end: 4 })
        );
        assert_eq!(block_range_at("f.ts", text, 4), None);
    }

    #[test]
    fn resolves_first_line_block_after_utf8_bom() {
        let text = "\u{feff}function f() {\n  g();\n}\n";

        assert_eq!(
            block_range_at("f.ts", text, 1),
            Some(BlockSpan { start: 1, end: 3 })
        );
    }

    #[test]
    fn rejects_invalid_block_anchors_and_unknown_language() {
        assert_eq!(
            block_range_at("broken.ts", "function f() {\n  if (a) {\n}\n", 1),
            None
        );
        assert_eq!(block_range_at("notes.md", "# Title\n\nbody\n", 2), None);
        let text = "function f() {\n  g();\n}\n";
        assert_eq!(block_range_at("f.ts", text, 3), None);
        assert_eq!(block_range_at("f.unknown", text, 1), None);
    }

    #[test]
    fn resolves_markdown_section_to_next_peer_heading() {
        let text =
            "# H1\nintro\n\n## H2 alpha\nbody a\n\n### H3 deep\ndeep body\n\n## H2 beta\nbody b\n";

        assert_eq!(
            block_range_at("plan.md", text, 4),
            Some(BlockSpan { start: 4, end: 9 })
        );
        assert_eq!(
            block_range_at("plan.md", text, 7),
            Some(BlockSpan { start: 7, end: 9 })
        );
    }

    #[test]
    fn required_block_operations_reject_single_line_nodes() {
        let error = resolve_block_ops(
            "f.ts",
            "const value = 1;\n",
            vec![HashlineOp::SwapBlock {
                line: 1,
                lines: vec!["const value = 2;".to_owned()],
            }],
        )
        .expect_err("单行节点不能作为 SWAP.BLK");

        assert!(error.contains("单行节点"));
    }

    #[test]
    fn insert_after_block_falls_back_to_plain_insert_with_warning() {
        let (ops, resolutions, warnings) = resolve_block_ops(
            "notes.unknown",
            "alpha\nbeta\n",
            vec![HashlineOp::InsertAfterBlock {
                line: 1,
                lines: vec!["inserted".to_owned()],
            }],
        )
        .expect("INS.BLK.POST 无法解析时应降级");

        assert_eq!(
            ops,
            vec![HashlineOp::InsertAfter {
                line: 1,
                lines: vec!["inserted".to_owned()],
                block_start: None,
            }]
        );
        assert!(resolutions.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("INS.POST 1"));
    }
}
