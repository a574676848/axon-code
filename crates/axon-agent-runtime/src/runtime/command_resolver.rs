//! 可执行命令发现与补充 PATH 规则。

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const WINDOWS_WRAPPER_EXTENSIONS: [&str; 4] = [".cmd", ".ps1", ".exe", ".bat"];
const PORTABLE_GO_PREFIX: &str = "go-portable-";
const NPM_CONFIG_PREFIX_KEYS: [&str; 2] = ["NPM_CONFIG_PREFIX", "npm_config_prefix"];
const NPM_USER_CONFIG_FILE: &str = ".npmrc";
const NPM_PREFIX_KEY: &str = "prefix";

static NPM_GLOBAL_PREFIX: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 判断命令当前是否可执行。
#[must_use]
pub fn command_exists(binary: &str) -> bool {
    command_path(binary).is_some()
}

/// 返回命令可执行路径。
///
/// 解析全程只做文件系统探测（直接路径 / PATH / 常见 npm 目录 / portable go），
/// 不启动 `npm`、`cmd` 等外部进程。可用性探测会按 catalog 维度批量调用本函数，
/// 一旦退化成进程探测，单条命令就会付出秒级开销。
#[must_use]
pub fn command_path(binary: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(binary);
    if direct.is_file() {
        return Some(direct);
    }
    if let Some(found) = find_command_on_path(binary) {
        return Some(found);
    }
    resolve_global_command(binary)
}

/// Windows 场景下补充 npm global / APPDATA npm / portable go 的发现逻辑。
#[must_use]
pub fn resolve_global_command(binary: &str) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    if is_gopls(binary)
        && let Some(candidate) = portable_go_tool("gopls.exe")
        && candidate.is_file()
    {
        return Some(candidate);
    }
    global_command_candidates(binary)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// 基于 portable go 补充 PATH。
#[must_use]
pub fn extra_go_env(current_path: &str) -> Vec<(String, String)> {
    let Some(portable_go) = portable_go_bin_dir() else {
        return Vec::new();
    };
    let portable = portable_go.to_string_lossy().to_string();
    vec![("PATH".to_owned(), format!("{portable};{current_path}"))]
}

/// npm global / APPDATA npm / portable wrapper 兜底候选。
#[must_use]
pub fn global_command_candidates(binary: &str) -> Vec<PathBuf> {
    let mut candidates = cheap_global_command_candidates(binary);
    if let Some(prefix) = npm_global_prefix() {
        for name in command_name_candidates(binary) {
            candidates.push(prefix.join(name));
        }
    }
    candidates
}

/// 无需额外配置解析的兜底候选：APPDATA npm 与显式配置的 npm prefix。
fn cheap_global_command_candidates(binary: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let names = command_name_candidates(binary);
    if let Some(home) = env::var_os("APPDATA").map(PathBuf::from) {
        for name in &names {
            candidates.push(home.join("npm").join(name));
        }
    }
    for key in NPM_CONFIG_PREFIX_KEYS {
        if let Some(prefix) = env::var_os(key).map(PathBuf::from) {
            for name in &names {
                candidates.push(prefix.join(name));
            }
        }
    }
    candidates
}

/// 当前系统上的 npm global prefix：读取 npm 用户配置，不启动 npm CLI。
#[must_use]
pub fn npm_global_prefix() -> Option<PathBuf> {
    NPM_GLOBAL_PREFIX
        .get_or_init(resolve_npm_global_prefix_from_config)
        .clone()
}

fn resolve_npm_global_prefix_from_config() -> Option<PathBuf> {
    let config_path = env::var_os("USERPROFILE")
        .map(PathBuf::from)?
        .join(NPM_USER_CONFIG_FILE);
    let content = fs::read_to_string(config_path).ok()?;
    content.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix(NPM_PREFIX_KEY)?
            .trim_start()
            .strip_prefix('=')?
            .trim();
        (!value.is_empty()).then(|| PathBuf::from(value))
    })
}

/// portable go 的 `go/bin` 目录。
#[must_use]
pub fn portable_go_bin_dir() -> Option<PathBuf> {
    let temp = env::var_os("TEMP").map(PathBuf::from)?;
    for entry in fs::read_dir(temp).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(PORTABLE_GO_PREFIX) {
            continue;
        }
        let go_bin = entry.path().join("go").join("bin");
        if go_bin.join("go.exe").is_file() {
            return Some(go_bin);
        }
    }
    None
}

/// portable go 中的真实工具路径。
#[must_use]
pub fn portable_go_tool(binary: &str) -> Option<PathBuf> {
    let temp = env::var_os("TEMP").map(PathBuf::from)?;
    for entry in fs::read_dir(temp).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(PORTABLE_GO_PREFIX) {
            continue;
        }
        let candidate = entry.path().join("bin").join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_command_on_path(binary: &str) -> Option<PathBuf> {
    let path_value = env::var_os("PATH")?;
    let path_entries = env::split_paths(&path_value).collect::<Vec<_>>();
    let extensions = executable_extensions();
    for directory in path_entries {
        for candidate in candidate_paths(&directory, binary, &extensions) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn command_name_candidates(binary: &str) -> Vec<String> {
    let path = Path::new(binary);
    let raw = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(binary);
    if !cfg!(windows) {
        return vec![raw.to_owned()];
    }
    if raw.contains('.') {
        return vec![raw.to_owned()];
    }
    let mut names = vec![raw.to_owned()];
    for extension in WINDOWS_WRAPPER_EXTENSIONS {
        names.push(format!("{raw}{extension}"));
    }
    names
}

fn executable_extensions() -> Vec<String> {
    if cfg!(windows) {
        return env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|item| !item.trim().is_empty())
                    .map(str::to_ascii_lowercase)
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty())
            .unwrap_or_else(|| vec![".exe".to_owned(), ".cmd".to_owned(), ".bat".to_owned()]);
    }
    vec![String::new()]
}

fn candidate_paths(directory: &Path, binary: &str, extensions: &[String]) -> Vec<PathBuf> {
    let base = directory.join(binary);
    if base.extension().is_some() || !cfg!(windows) {
        return vec![base];
    }
    let mut candidates = vec![base.clone()];
    for extension in extensions {
        let trimmed = extension.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.trim_start_matches('.');
        candidates.push(base.with_extension(normalized));
    }
    candidates
}

fn is_gopls(binary: &str) -> bool {
    matches!(binary, "gopls" | "gopls.exe")
}
