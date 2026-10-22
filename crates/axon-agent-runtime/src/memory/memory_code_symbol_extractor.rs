use std::{collections::BTreeSet, path::Path};

use axon_crypto::sha256_hex;

use crate::memory_code_symbol_types::{MemoryCodeSymbolKind, MemoryCodeSymbolRecord};

type SymbolLineExtractor = fn(&str, &str, u64) -> Option<MemoryCodeSymbolRecord>;

pub(super) fn extract_symbols_for_path(
    path_ref: &str,
    source: &str,
    now_ms: u64,
) -> Vec<MemoryCodeSymbolRecord> {
    let extractor: Option<SymbolLineExtractor> = if path_ref.ends_with(".rs") {
        Some(rust_symbol_from_line)
    } else if path_ref.ends_with(".ts")
        || path_ref.ends_with(".tsx")
        || path_ref.ends_with(".js")
        || path_ref.ends_with(".jsx")
    {
        Some(ts_symbol_from_line)
    } else if path_ref.ends_with(".py") {
        Some(python_symbol_from_line)
    } else if path_ref.ends_with(".go") {
        Some(go_symbol_from_line)
    } else {
        None
    };
    let mut records: Vec<MemoryCodeSymbolRecord> = if let Some(extract) = extractor {
        source
            .lines()
            .filter_map(|line| extract(path_ref, line, now_ms))
            .collect()
    } else {
        Vec::new()
    };
    if records.is_empty()
        && let Some(symbol_name) = config_entry_symbol_name(path_ref)
    {
        records.push(symbol_record(
            path_ref,
            symbol_name,
            MemoryCodeSymbolKind::EntryPoint,
            now_ms,
        ));
    }
    let dependency_refs = dependency_refs_for_source(source);
    if !dependency_refs.is_empty() {
        for record in &mut records {
            record.dependency_refs.clone_from(&dependency_refs);
        }
    }
    let reference_refs = reference_refs_for_source(path_ref, source);
    if !reference_refs.is_empty() {
        for record in &mut records {
            record.reference_refs.clone_from(&reference_refs);
        }
    }
    records
}

fn dependency_refs_for_source(source: &str) -> Vec<String> {
    let mut refs = source
        .lines()
        .filter_map(dependency_ref_from_line)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    refs.sort();
    refs
}

fn dependency_ref_from_line(line: &str) -> Option<String> {
    let visible = line.trim();
    let dep = if let Some((_, tail)) = visible.split_once(" from ") {
        tail
    } else if let Some(tail) = visible.strip_prefix("from ") {
        tail
    } else {
        visible
            .strip_prefix("use ")
            .or_else(|| visible.strip_prefix("mod "))
            .or_else(|| visible.strip_prefix("import "))
            .or_else(|| visible.strip_prefix("package "))?
    };
    let token = dep
        .split(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    ';' | ':' | '{' | '}' | '(' | ')' | ',' | '"' | '\'' | '`'
                )
        })
        .find(|part| !part.is_empty())?;
    let dependency = token.trim_start_matches('.').trim_matches('/');
    (!dependency.is_empty()).then(|| format!("dep:{dependency}"))
}

fn reference_refs_for_source(path_ref: &str, source: &str) -> Vec<String> {
    let mut refs = source
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|token| token.chars().next().is_some_and(char::is_uppercase))
        .map(|token| format!("ref:{token}"))
        .collect::<BTreeSet<_>>();
    if is_test_path(path_ref) {
        refs.insert(format!("test-cover:{}", path_ref.replace(['/', '\\'], ":")));
    }
    refs.into_iter().collect()
}

fn is_test_path(path_ref: &str) -> bool {
    path_ref.contains("/tests/")
        || path_ref.ends_with("_test.rs")
        || path_ref.ends_with(".test.ts")
        || path_ref.ends_with(".spec.ts")
        || path_ref.ends_with("_test.py")
}

fn ts_symbol_from_line(path_ref: &str, line: &str, now_ms: u64) -> Option<MemoryCodeSymbolRecord> {
    let visible = line
        .trim_start()
        .strip_prefix("export default ")
        .or_else(|| line.trim_start().strip_prefix("export "))
        .unwrap_or_else(|| line.trim_start())
        .strip_prefix("async ")
        .unwrap_or_else(|| {
            line.trim_start()
                .strip_prefix("export ")
                .unwrap_or_else(|| line.trim_start())
        });
    let (kind, tail) = if let Some(tail) = visible.strip_prefix("function ") {
        (MemoryCodeSymbolKind::Function, tail)
    } else if let Some(tail) = visible.strip_prefix("class ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else if let Some(tail) = visible.strip_prefix("interface ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else if let Some(tail) = visible.strip_prefix("type ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else if let Some(tail) = visible.strip_prefix("enum ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else {
        let tail = visible
            .strip_prefix("const ")
            .or_else(|| visible.strip_prefix("let "))
            .or_else(|| visible.strip_prefix("var "))
            .filter(|tail| tail.contains("=>") || tail.contains("function"))?;
        (MemoryCodeSymbolKind::Function, tail)
    };
    symbol_record_from_tail(path_ref, tail, kind, now_ms)
}

fn python_symbol_from_line(
    path_ref: &str,
    line: &str,
    now_ms: u64,
) -> Option<MemoryCodeSymbolRecord> {
    let visible = line.trim_start();
    if visible.starts_with("if __name__") && visible.contains("__main__") {
        return Some(symbol_record(
            path_ref,
            "main".to_owned(),
            MemoryCodeSymbolKind::EntryPoint,
            now_ms,
        ));
    }
    let (kind, tail) = if let Some(tail) = visible
        .strip_prefix("async def ")
        .or_else(|| visible.strip_prefix("def "))
    {
        (MemoryCodeSymbolKind::Function, tail)
    } else {
        let tail = visible.strip_prefix("class ")?;
        (MemoryCodeSymbolKind::Type, tail)
    };
    symbol_record_from_tail(path_ref, tail, kind, now_ms)
}

fn go_symbol_from_line(path_ref: &str, line: &str, now_ms: u64) -> Option<MemoryCodeSymbolRecord> {
    let visible = line.trim_start();
    let (kind, tail) = if let Some(tail) = visible.strip_prefix("func ") {
        (MemoryCodeSymbolKind::Function, go_function_tail(tail))
    } else if let Some(tail) = visible.strip_prefix("type ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else {
        let tail = visible.strip_prefix("package ")?;
        (MemoryCodeSymbolKind::Module, tail)
    };
    symbol_record_from_tail(path_ref, tail, kind, now_ms)
}

fn go_function_tail(tail: &str) -> &str {
    if tail.trim_start().starts_with('(') {
        tail.split_once(')')
            .map_or(tail, |(_, rest)| rest.trim_start())
    } else {
        tail
    }
}

fn rust_symbol_from_line(
    path_ref: &str,
    line: &str,
    now_ms: u64,
) -> Option<MemoryCodeSymbolRecord> {
    let trimmed = line.trim_start();
    let visible = trimmed
        .strip_prefix("pub ")
        .or_else(|| trimmed.strip_prefix("pub(crate) "))
        .unwrap_or(trimmed);
    let (kind, tail) = if let Some(tail) = visible.strip_prefix("fn ") {
        (MemoryCodeSymbolKind::Function, tail)
    } else if let Some(tail) = visible.strip_prefix("struct ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else if let Some(tail) = visible.strip_prefix("enum ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else if let Some(tail) = visible.strip_prefix("trait ") {
        (MemoryCodeSymbolKind::Type, tail)
    } else {
        let tail = visible.strip_prefix("mod ")?;
        (MemoryCodeSymbolKind::Module, tail)
    };
    symbol_record_from_tail(path_ref, tail, kind, now_ms)
}

fn symbol_record_from_tail(
    path_ref: &str,
    tail: &str,
    kind: MemoryCodeSymbolKind,
    now_ms: u64,
) -> Option<MemoryCodeSymbolRecord> {
    Some(symbol_record(
        path_ref,
        symbol_name_from_tail(tail)?,
        kind,
        now_ms,
    ))
}

fn symbol_record(
    path_ref: &str,
    symbol_name: String,
    kind: MemoryCodeSymbolKind,
    now_ms: u64,
) -> MemoryCodeSymbolRecord {
    MemoryCodeSymbolRecord {
        symbol_ref: format!("symbol:{path_ref}:{symbol_name}"),
        source_summary_hash: source_symbol_hash(path_ref, &symbol_name, kind),
        symbol_name,
        kind,
        path_ref: path_ref.to_owned(),
        dependency_refs: Vec::new(),
        reference_refs: Vec::new(),
        updated_at_ms: now_ms,
    }
}

fn config_entry_symbol_name(path_ref: &str) -> Option<String> {
    let file_name = Path::new(path_ref).file_name()?.to_str()?;
    let is_config = matches!(
        file_name,
        "package.json" | "pyproject.toml" | "go.mod" | "Dockerfile"
    ) || file_name.contains(".config.")
        || path_ref.starts_with(".github/workflows/");
    is_config.then(|| file_name.to_owned())
}

fn symbol_name_from_tail(tail: &str) -> Option<String> {
    let name = tail
        .trim_start()
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .next()?
        .trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn source_symbol_hash(path_ref: &str, symbol_name: &str, kind: MemoryCodeSymbolKind) -> String {
    sha256_hex(format!("{path_ref}:{symbol_name}:{kind:?}").as_bytes())
}
