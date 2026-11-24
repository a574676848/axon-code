//! LSP 符号过滤结果格式化。

use super::ExtractedSymbol;

const SYMBOL_INDENT_SPACES: usize = 4;
const SYMBOL_PLACEHOLDER: &str = "...";

pub(super) fn format_symbols(symbols: &[ExtractedSymbol]) -> String {
    let mut output = String::new();
    for (index, symbol) in symbols.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        format_symbol(symbol, 0, &mut output);
    }
    output
}

fn format_symbol(symbol: &ExtractedSymbol, depth: usize, output: &mut String) {
    let indent = " ".repeat(depth.saturating_mul(SYMBOL_INDENT_SPACES));
    output.push_str(&indent);
    output.push_str(&symbol.signature);
    output.push_str(" {\n");
    if symbol.children.is_empty() {
        output.push_str(&indent);
        output.push_str(&" ".repeat(SYMBOL_INDENT_SPACES));
        output.push_str(SYMBOL_PLACEHOLDER);
        output.push('\n');
    } else {
        for child in &symbol.children {
            format_symbol(child, depth.saturating_add(1), output);
        }
    }
    output.push_str(&indent);
    output.push('}');
    output.push('\n');
}
