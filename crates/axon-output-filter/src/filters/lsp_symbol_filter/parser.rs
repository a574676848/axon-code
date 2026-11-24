//! LSP documentSymbol 响应解析。

use axon_domain::{AxonError, AxonResult, ToolError};
use serde_json::Value;

use super::{
    ExtractedSymbol, LSP_DOCUMENT_SYMBOL_INVALID_ERROR, LspDocumentSymbolHit, LspSymbolFilter,
    SymbolRange,
};

pub(super) fn parse_first_symbol_hit(
    hits: &[LspDocumentSymbolHit],
) -> AxonResult<Vec<ExtractedSymbol>> {
    for hit in hits {
        let symbols = LspSymbolFilter::parse_document_symbols(&hit.response)?;
        if !symbols.is_empty() {
            return Ok(symbols);
        }
    }
    Err(AxonError::Tool(ToolError::InvalidDefinition(
        super::LSP_DOCUMENT_SYMBOL_EMPTY_ERROR.to_owned(),
    )))
}

pub(super) fn parse_symbol_items(value: &Value) -> AxonResult<Vec<ExtractedSymbol>> {
    let Some(items) = value.as_array() else {
        return Err(AxonError::Tool(ToolError::InvalidDefinition(
            LSP_DOCUMENT_SYMBOL_INVALID_ERROR.to_owned(),
        )));
    };
    items.iter().map(parse_symbol_item).collect()
}

fn parse_symbol_item(item: &Value) -> AxonResult<ExtractedSymbol> {
    let name = string_field(item, "name")?;
    let kind = map_lsp_symbol_kind(item.get("kind").and_then(Value::as_u64));
    let signature = extract_signature(item, &name, &kind);
    let range = parse_symbol_range(item)?;
    let children = item
        .get("children")
        .map(parse_symbol_items)
        .transpose()?
        .unwrap_or_default();
    Ok(ExtractedSymbol {
        name,
        kind,
        signature,
        range,
        children,
    })
}

fn extract_signature(item: &Value, name: &str, kind: &str) -> String {
    item.get("detail")
        .and_then(Value::as_str)
        .filter(|detail| !detail.trim().is_empty())
        .map_or_else(|| fallback_signature(kind, name), str::to_owned)
}

fn fallback_signature(kind: &str, name: &str) -> String {
    if kind.is_empty() || kind == "unknown" {
        return name.to_owned();
    }
    format!("{kind} {name}")
}

pub(super) fn map_lsp_symbol_kind(kind: Option<u64>) -> String {
    match kind {
        Some(1) => "file",
        Some(2) => "module",
        Some(3) => "namespace",
        Some(4) => "package",
        Some(5) => "class",
        Some(6) => "method",
        Some(7) => "property",
        Some(8) => "field",
        Some(9) => "constructor",
        Some(10) => "enum",
        Some(11) => "interface",
        Some(12) => "function",
        Some(13) => "variable",
        Some(14) => "constant",
        Some(15) => "string",
        Some(16) => "number",
        Some(17) => "boolean",
        Some(18) => "array",
        Some(19) => "object",
        Some(20) => "key",
        Some(21) => "null",
        Some(22) => "enum_member",
        Some(23) => "struct",
        Some(24) => "event",
        Some(25) => "operator",
        Some(26) => "type_parameter",
        _ => "unknown",
    }
    .to_owned()
}

fn parse_symbol_range(item: &Value) -> AxonResult<SymbolRange> {
    let range = item
        .get("range")
        .or_else(|| {
            item.get("location")
                .and_then(|location| location.get("range"))
        })
        .ok_or_else(|| {
            AxonError::Tool(ToolError::InvalidDefinition(
                LSP_DOCUMENT_SYMBOL_INVALID_ERROR.to_owned(),
            ))
        })?;
    Ok(SymbolRange {
        start_line: position_value(range, "start", "line")?.saturating_add(1),
        start_character: position_value(range, "start", "character")?.saturating_add(1),
        end_line: position_value(range, "end", "line")?.saturating_add(1),
        end_character: position_value(range, "end", "character")?.saturating_add(1),
    })
}

fn position_value(range: &Value, endpoint: &str, field: &str) -> AxonResult<u32> {
    range
        .get(endpoint)
        .and_then(|position| position.get(field))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            AxonError::Tool(ToolError::InvalidDefinition(
                LSP_DOCUMENT_SYMBOL_INVALID_ERROR.to_owned(),
            ))
        })
}

fn string_field(item: &Value, field: &str) -> AxonResult<String> {
    item.get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            AxonError::Tool(ToolError::InvalidDefinition(
                LSP_DOCUMENT_SYMBOL_INVALID_ERROR.to_owned(),
            ))
        })
}
