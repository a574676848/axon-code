//! Repo Structure Model 静态扫描。

use std::{collections::BTreeMap, fs, path::Path};

use axon_domain::{AxonError, AxonResult};
use axon_fs_walk::{EntryKind, Walk, WalkAction, WalkOpts};

use crate::repo_structure_model_types::{
    RepoFile, RepoFileRole, RepoModule, RepoModuleKind, RepoScanAccumulator, RepoScanContext,
    RepoSourceTag, RepoStructureSourceKind, RepoWorkspace, RepoWorkspaceScanState,
};

const ROOT_MARKER_FILES: [&str; 4] = ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"];
const ENTRYPOINT_FILE_NAMES: [&str; 8] = [
    "main.rs",
    "lib.rs",
    "mod.rs",
    "main.ts",
    "main.tsx",
    "index.ts",
    "index.tsx",
    "main.py",
];

pub(super) fn scan_workspace(
    workspace_root: &Path,
    max_files: usize,
) -> AxonResult<RepoWorkspaceScanState> {
    validate_workspace_root(workspace_root)?;
    let refreshed_at_ms = current_time_ms();
    let root = workspace_root
        .canonicalize()
        .map_err(|_| AxonError::runtime_failure("仓库根目录不可读"))?;
    let mut accumulator = RepoScanAccumulator {
        file_count: 0,
        truncated: false,
        modules: vec![RepoModule {
            module_ref: "repo:workspace".to_owned(),
            display_name: root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("workspace")
                .to_owned(),
            kind: RepoModuleKind::WorkspaceRoot,
            path_ref: ".".to_owned(),
            manifest_path_ref: None,
            entrypoint_path_refs: Vec::new(),
            updated_at_ms: refreshed_at_ms,
            sources: vec![source_tag(
                RepoStructureSourceKind::StaticScan,
                "workspace_root",
            )],
        }],
        files: Vec::new(),
    };
    let context = RepoScanContext {
        root: &root,
        max_files,
        refreshed_at_ms,
    };
    scan_repo_entries(&context, &root, &mut accumulator)?;
    let workspace = RepoWorkspace {
        workspace_ref: "repo:workspace".to_owned(),
        workspace_root: root.display().to_string(),
        module_refs: accumulator
            .modules
            .iter()
            .map(|module| module.module_ref.clone())
            .collect(),
        primary_languages: primary_languages(&accumulator.files),
        entrypoint_refs: accumulator
            .files
            .iter()
            .filter(|file| file.role == RepoFileRole::EntryPoint)
            .map(|file| file.path_ref.clone())
            .collect(),
        scanned_files: accumulator.files.len(),
        truncated: accumulator.truncated,
        updated_at_ms: refreshed_at_ms,
        sources: vec![source_tag(
            RepoStructureSourceKind::StaticScan,
            "workspace_scan",
        )],
    };
    Ok(RepoWorkspaceScanState {
        workspace,
        modules: accumulator.modules,
        files: accumulator.files,
        refreshed_at_ms,
        partial: accumulator.truncated,
    })
}

fn scan_repo_entries(
    context: &RepoScanContext<'_>,
    current_dir: &Path,
    accumulator: &mut RepoScanAccumulator,
) -> AxonResult<()> {
    let opts = WalkOpts::default();
    let mut scan_error = None;
    let summary = Walk::run(current_dir, &opts, |entry| {
        if accumulator.file_count >= context.max_files {
            accumulator.truncated = true;
            return WalkAction::Stop;
        }
        match entry.kind {
            EntryKind::Dir => {
                let manifest_path = manifest_path_for_dir(&entry.path);
                let _ = register_module(
                    context.root,
                    &entry.path,
                    manifest_path.as_deref(),
                    &mut accumulator.modules,
                    context.refreshed_at_ms,
                );
            }
            EntryKind::File => {
                if let Err(error) = register_file_entry(context, &entry.path, accumulator) {
                    scan_error = Some(error);
                    return WalkAction::Stop;
                }
            }
            EntryKind::Symlink | EntryKind::Other => {}
        }
        WalkAction::Continue
    })?;
    if let Some(error) = scan_error {
        return Err(error);
    }
    accumulator.truncated |= summary.truncated;
    Ok(())
}

fn register_file_entry(
    context: &RepoScanContext<'_>,
    path: &Path,
    accumulator: &mut RepoScanAccumulator,
) -> AxonResult<()> {
    let parent = path.parent().unwrap_or(context.root);
    let manifest_path = manifest_path_for_dir(parent);
    let module_ref = register_module(
        context.root,
        parent,
        manifest_path.as_deref(),
        &mut accumulator.modules,
        context.refreshed_at_ms,
    );
    let path_ref = relative_path_ref(context.root, path)?;
    let role = file_role(path);
    accumulator.files.push(RepoFile {
        file_ref: format!("file:{path_ref}"),
        path_ref: path_ref.clone(),
        module_ref: module_ref.clone(),
        language: detect_language(path),
        role,
        updated_at_ms: context.refreshed_at_ms,
        sources: vec![source_tag(
            RepoStructureSourceKind::StaticScan,
            "workspace_scan",
        )],
    });
    if let Some(module) = accumulator
        .modules
        .iter_mut()
        .find(|item| item.module_ref == module_ref)
        && role == RepoFileRole::EntryPoint
    {
        module.entrypoint_path_refs.push(path_ref);
    }
    accumulator.file_count += 1;
    Ok(())
}

fn manifest_path_for_dir(dir: &Path) -> Option<std::path::PathBuf> {
    ROOT_MARKER_FILES
        .iter()
        .map(|marker| dir.join(marker))
        .find(|path| path.is_file())
}

fn register_module(
    root: &Path,
    current_dir: &Path,
    manifest_path: Option<&Path>,
    modules: &mut Vec<RepoModule>,
    refreshed_at_ms: u64,
) -> String {
    let path_ref = relative_path_ref(root, current_dir).unwrap_or_else(|_| ".".to_owned());
    if path_ref == "." {
        return "repo:workspace".to_owned();
    }
    let module_ref = format!("module:{path_ref}");
    if modules.iter().any(|module| module.module_ref == module_ref) {
        return module_ref;
    }
    modules.push(RepoModule {
        module_ref: module_ref.clone(),
        display_name: current_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("module")
            .to_owned(),
        kind: module_kind(manifest_path),
        path_ref,
        manifest_path_ref: manifest_path.and_then(|path| relative_path_ref(root, path).ok()),
        entrypoint_path_refs: Vec::new(),
        updated_at_ms: refreshed_at_ms,
        sources: vec![source_tag(
            RepoStructureSourceKind::StaticScan,
            "workspace_scan",
        )],
    });
    module_ref
}

pub(super) fn validate_workspace_root(path: &Path) -> AxonResult<()> {
    let metadata =
        fs::metadata(path).map_err(|_| AxonError::runtime_failure("仓库根目录不可读"))?;
    if !metadata.is_dir() {
        return Err(AxonError::runtime_failure("仓库根目录必须是目录"));
    }
    Ok(())
}

fn is_root_marker(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| ROOT_MARKER_FILES.contains(&name))
}

fn file_role(path: &Path) -> RepoFileRole {
    if is_root_marker(path) {
        return RepoFileRole::Manifest;
    }
    if path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| ENTRYPOINT_FILE_NAMES.contains(&name))
    {
        return RepoFileRole::EntryPoint;
    }
    match path.extension().and_then(|value| value.to_str()) {
        Some("rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go") => RepoFileRole::Source,
        Some("toml" | "json" | "yaml" | "yml") => RepoFileRole::Config,
        _ => RepoFileRole::Other,
    }
}

fn detect_language(path: &Path) -> String {
    match path.extension().and_then(|value| value.to_str()) {
        Some("rs") => "rust".to_owned(),
        Some("ts" | "tsx") => "typescript".to_owned(),
        Some("js" | "jsx") => "javascript".to_owned(),
        Some("py") => "python".to_owned(),
        Some("go") => "go".to_owned(),
        Some("toml") => "toml".to_owned(),
        Some("json") => "json".to_owned(),
        _ => "other".to_owned(),
    }
}

fn module_kind(manifest_path: Option<&Path>) -> RepoModuleKind {
    match manifest_path
        .and_then(|path| path.file_name())
        .and_then(|value| value.to_str())
    {
        Some("Cargo.toml") => RepoModuleKind::RustCrate,
        Some("package.json") => RepoModuleKind::NodePackage,
        Some("pyproject.toml") => RepoModuleKind::PythonProject,
        Some("go.mod") => RepoModuleKind::GoModule,
        _ => RepoModuleKind::Directory,
    }
}

fn primary_languages(files: &[RepoFile]) -> Vec<String> {
    let mut counts = BTreeMap::new();
    for file in files {
        if file.language == "other" {
            continue;
        }
        *counts.entry(file.language.clone()).or_insert(0usize) += 1;
    }
    let mut pairs = counts.into_iter().collect::<Vec<_>>();
    pairs.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    pairs.into_iter().map(|(language, _)| language).collect()
}

pub(super) fn relative_path_ref(root: &Path, path: &Path) -> AxonResult<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AxonError::runtime_failure("仓库路径越界"))?;
    Ok(if relative.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        relative.to_string_lossy().replace('\\', "/")
    })
}

pub(super) fn uri_to_path_ref(uri: &str) -> Option<String> {
    uri.strip_prefix("file:///")
        .map(|value| value.trim_start_matches('/').replace('\\', "/"))
}

pub(super) fn file_stem_name(path_ref: &str) -> String {
    Path::new(path_ref)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("entrypoint")
        .to_owned()
}

pub(super) fn source_tag(kind: RepoStructureSourceKind, detail: &str) -> RepoSourceTag {
    RepoSourceTag {
        kind,
        detail: detail.to_owned(),
    }
}

pub(super) fn current_time_ms() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
