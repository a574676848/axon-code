//! 编辑引入解析失败的检测与最小肇事区定位。
//!
//! 与 omp 的 auto-repair 同问题域，但**不含模型调用**：这里只做确定性判定：
//! 用既有 tree-sitter 语法（[`LanguageRegistry`]）判断一次编辑是否把「可解析」变成
//! 「不可解析」，定位最小肇事区，并提供修复接受判据（必须重新解析成功，且不得是整段回退）。
//! 真正的修复动作与模型接线由上层决定；本模块不写文件、不改会话状态。

use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::language_registry::LanguageRegistry;

/// 「编辑引入解析失败」告警的统一前缀；回执与上层据此识别解析回归。
pub const EDIT_PARSE_REGRESSION_PREFIX: &str = "[解析回归] ";
/// 肇事区两侧额外保留的上下文行数。
const REPAIR_CONTEXT_LINES: usize = 6;
/// 肇事区行数上限；超过时退回到错误节点自身的原始范围。
const REPAIR_MAX_REGION_LINES: usize = 150;

/// 最小肇事区（1-based，闭区间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairRegion {
    /// 首行行号（1-based）。
    pub start_line: usize,
    /// 末行行号（1-based，含）。
    pub end_line: usize,
}

/// 判定文本在该路径对应语法下是否解析成功。
///
/// 返回 `None` 表示没有可用语法（例如未知扩展名），调用方应**跳过**判定而不是当作失败。
#[must_use]
pub fn source_parses(path: &str, text: &str) -> Option<bool> {
    let tree = parse_tree(path, text)?;
    Some(!tree.root_node().has_error())
}

/// 定位引入解析失败的最小肇事区：所有 ERROR / MISSING 节点的行范围并集，再向两侧扩上下文。
///
/// 文本可解析或没有可用语法时返回 `None`。
#[must_use]
pub fn isolate_culprit_region(path: &str, text: &str) -> Option<RepairRegion> {
    let tree = parse_tree(path, text)?;
    let root = tree.root_node();
    if !root.has_error() {
        return None;
    }
    let (raw_start, raw_end) = error_span(root)?;
    Some(expand_region(raw_start, raw_end, text.lines().count()))
}

/// 生成「编辑引入解析失败」的定向告警。
///
/// 只有「编辑前可解析、编辑后不可解析」才算回归；其余组合（含无可用语法）返回 `None`，
/// 保证该告警只在真正由本次编辑引入时出现。
#[must_use]
pub fn parse_regression_warning(path: &str, previous: &str, next: &str) -> Option<String> {
    if !source_parses(path, previous)? {
        return None;
    }
    if source_parses(path, next)? {
        return None;
    }
    Some(match isolate_culprit_region(path, next) {
        Some(region) => format!(
            "{EDIT_PARSE_REGRESSION_PREFIX}{path} 由本次编辑从「可解析」变为「解析失败」\
             （约行 {}-{}）。请检查该区域的定界符、缩进与括号配对；必要时整段重写该区域。",
            region.start_line, region.end_line
        ),
        None => format!(
            "{EDIT_PARSE_REGRESSION_PREFIX}{path} 由本次编辑从「可解析」变为「解析失败」。\
             请检查本次改动区域的定界符、缩进与括号配对；必要时整段重写该区域。"
        ),
    })
}

/// 修复接受判据：候选内容重新解析成功，且**不是**整段回退到编辑前的文本。
///
/// 两条判据缺一不可：只要求「能解析」会让模型用删除整段的方式蒙混过关；只要求「不等于回退」
/// 则可能接受仍然解析失败的候选。
///
/// 调用方（模型自愈回路）经裁决暂缓接线，因此该判据目前只在测试中被使用；
/// 这里显式标注而不是删除：判据本身是本次移植的交付物之一。
#[cfg_attr(not(test), allow(dead_code))]
#[must_use]
pub fn accepts_repair(path: &str, candidate: &str, previous: &str) -> bool {
    source_parses(path, candidate) == Some(true) && candidate != previous
}

/// 原始错误行范围按上下文与上限展开为最终肇事区。
fn expand_region(raw_start: usize, raw_end: usize, total_lines: usize) -> RepairRegion {
    let raw_start = raw_start.max(1);
    let raw_end = raw_end.max(raw_start);
    let start = raw_start.saturating_sub(REPAIR_CONTEXT_LINES).max(1);
    let end = raw_end
        .saturating_add(REPAIR_CONTEXT_LINES)
        .min(total_lines.max(raw_end));
    if end.saturating_sub(start).saturating_add(1) > REPAIR_MAX_REGION_LINES {
        // 错误跨度本身就超限（例如整个文件被括号破坏）：退回错误节点自身范围，
        // 宁可在提示里给出更大区域，也不把上下文扩到无意义的规模。
        return RepairRegion {
            start_line: raw_start,
            end_line: raw_end.min(total_lines.max(raw_end)),
        };
    }
    RepairRegion {
        start_line: start,
        end_line: end.max(start),
    }
}

fn parse_tree(path: &str, text: &str) -> Option<tree_sitter::Tree> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let grammar = LanguageRegistry::resolve(Path::new(path))?.grammar()?;
    let language = grammar.tree_sitter_language();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    parser.parse(text, None)
}

/// 迭代遍历语法树，收集 ERROR / MISSING 节点覆盖的行范围（并集）。
fn error_span(root: Node<'_>) -> Option<(usize, usize)> {
    let mut span: Option<(usize, usize)> = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            let start = node.start_position().row.saturating_add(1);
            let end = node.end_position().row.saturating_add(1);
            span = Some(match span {
                Some((current_start, current_end)) => {
                    (current_start.min(start), current_end.max(end))
                }
                None => (start, end),
            });
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    span
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_RUST: &str = "fn main() {\n    let value = 1;\n}\n";
    const BROKEN_RUST: &str = "fn main() {\n    let value = 1;\n";

    #[test]
    fn parses_known_languages_and_skips_unknown_extensions() {
        assert_eq!(source_parses("src/lib.rs", VALID_RUST), Some(true));
        assert_eq!(source_parses("src/lib.rs", BROKEN_RUST), Some(false));
        assert_eq!(
            source_parses("notes.unknownext", BROKEN_RUST),
            None,
            "未知扩展名必须跳过判定，而不是当作失败"
        );
    }

    #[test]
    fn isolates_the_error_region_with_context() {
        let region = isolate_culprit_region("src/lib.rs", BROKEN_RUST).expect("应定位到肇事区");

        // 该文本只有 2 行，未闭合的括号被报在文件末尾，因此肇事区覆盖到末行并向前扩上下文。
        assert_eq!((region.start_line, region.end_line), (1, 2), "{region:?}");
        assert!(isolate_culprit_region("src/lib.rs", VALID_RUST).is_none());
    }

    #[test]
    fn expands_regions_with_context_and_caps_oversized_spans() {
        let region = expand_region(50, 52, 200);
        assert_eq!((region.start_line, region.end_line), (44, 58));

        let region = expand_region(1, 2, 200);
        assert_eq!(region.start_line, 1, "首行不应下溢");

        let capped = expand_region(10, 400, 500);
        assert_eq!(
            (capped.start_line, capped.end_line),
            (10, 400),
            "超过上限时退回错误节点原始范围，而不是无界扩张上下文"
        );
    }

    #[test]
    fn warns_only_when_the_edit_introduced_the_parse_failure() {
        let warning = parse_regression_warning("src/lib.rs", VALID_RUST, BROKEN_RUST)
            .expect("可解析 → 不可解析必须告警");
        assert!(
            warning.starts_with(EDIT_PARSE_REGRESSION_PREFIX),
            "{warning}"
        );
        assert!(warning.contains("src/lib.rs"), "{warning}");

        assert!(
            parse_regression_warning("src/lib.rs", VALID_RUST, VALID_RUST).is_none(),
            "未引入失败不得告警"
        );
        assert!(
            parse_regression_warning("src/lib.rs", BROKEN_RUST, BROKEN_RUST).is_none(),
            "编辑前就不可解析时不算回归"
        );
        assert!(
            parse_regression_warning("notes.unknown", VALID_RUST, BROKEN_RUST).is_none(),
            "无可用语法时不得告警"
        );
    }

    #[test]
    fn accepts_only_repairs_that_parse_and_are_not_a_plain_revert() {
        let fixed = "fn main() {\n    let value = 2;\n}\n";

        assert!(accepts_repair("src/lib.rs", fixed, VALID_RUST));
        assert!(
            !accepts_repair("src/lib.rs", BROKEN_RUST, VALID_RUST),
            "仍然解析失败的候选必须拒绝"
        );
        assert!(
            !accepts_repair("src/lib.rs", VALID_RUST, VALID_RUST),
            "整段回退到编辑前内容必须拒绝"
        );
    }

    #[test]
    fn missing_delimiters_are_reported_as_errors() {
        let broken = "export function run() {\n  return 1;\n";
        assert_eq!(source_parses("src/app.ts", broken), Some(false));
        assert!(isolate_culprit_region("src/app.ts", broken).is_some());
    }
}
