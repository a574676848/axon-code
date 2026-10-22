use std::collections::BTreeSet;

use crate::{RepoIndexSnapshot, RepoSourceTag, RepoStructureSourceKind, RepoSymbol};

pub(super) fn bounded_summary(snapshot: &RepoIndexSnapshot, query_text: &str) -> String {
    let modules = snapshot
        .modules
        .iter()
        .map(|module| module.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let entrypoints = snapshot.workspace.entrypoint_refs.to_vec().join(", ");
    let symbols = snapshot
        .symbols
        .iter()
        .map(|symbol| {
            format!(
                "{} kind={:?} path={} source={}",
                symbol.symbol_name,
                symbol.kind,
                symbol.path_ref,
                stable_sources(&symbol.sources)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let summary = format!(
        "repo_query={query_text}\nworkspace={}\nrefresh_state={}\nmodules={}\nlanguages={}\nentrypoints={}\nsymbols=\n{}",
        snapshot.workspace.workspace_root,
        if snapshot.partial { "partial" } else { "fresh" },
        if modules.is_empty() { "none" } else { &modules },
        if snapshot.workspace.primary_languages.is_empty() {
            "none".to_owned()
        } else {
            snapshot.workspace.primary_languages.join(", ")
        },
        if entrypoints.is_empty() {
            "none"
        } else {
            &entrypoints
        },
        if symbols.is_empty() {
            "none".to_owned()
        } else {
            symbols
        },
    );
    summary
}

pub(super) fn summary_source_refs(snapshot: &RepoIndexSnapshot) -> Vec<String> {
    let mut refs = BTreeSet::new();
    refs.insert(format!("repo://{}", snapshot.workspace.workspace_root));
    for path_ref in &snapshot.workspace.entrypoint_refs {
        refs.insert(format!("file:{path_ref}"));
    }
    for symbol in &snapshot.symbols {
        refs.insert(symbol.symbol_ref.clone());
    }
    refs.into_iter().collect()
}

pub(super) fn stable_sources(sources: &[RepoSourceTag]) -> String {
    sources
        .iter()
        .map(|source| format!("{:?}:{}", source.kind, source.detail))
        .collect::<Vec<_>>()
        .join("|")
}

pub(super) fn primary_symbol_source(symbol: &RepoSymbol) -> RepoStructureSourceKind {
    symbol
        .sources
        .first()
        .map_or(RepoStructureSourceKind::StaticScan, |source| source.kind)
}
