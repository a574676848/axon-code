//! Linux 平台适配器：按 XDG 规范枚举「用其他应用打开」候选。
//!
//! 数据来源与优先级（MIME Applications Associations Specification）：
//!
//! 1. **桌面条目**：先 `$XDG_DATA_HOME/applications`（默认 `~/.local/share/applications`），
//!    再按顺序读 `$XDG_DATA_DIRS/applications`（默认 `/usr/local/share:/usr/share`）。
//!    同名条目以**先出现的目录**为准，因此用户对系统条目的覆盖副本才生效。
//! 2. **文件的 MIME 类型**：由各数据目录 `mime/globs2`（缺则 `mime/globs`）的 glob 规则判定，
//!    与 `xdg-mime query filetype` 同源；纯读文件，不启动子进程。
//! 3. **默认项**：按 `$XDG_CONFIG_HOME/mimeapps.list` → `$XDG_CONFIG_DIRS/mimeapps.list` →
//!    各数据目录 `applications/mimeapps.list` 的顺序读 `[Default Applications]`，取其中
//!    **第一个确实存在的候选**，等同于 `xdg-mime query default`。
//!
//! 解析逻辑全部在 `desktop_entry` 与 `mime_glob` 两个纯模块里（因此能在任意平台单测）；
//! 本文件只负责目录展开与文件读取。
//!
//! # 已知缺口
//!
//! - 只扫描 `applications/` 的**顶层**文件，不递归子目录。
//! - 不处理 `Name[zh_CN]` 这类本地化键（见 `desktop_entry` 的说明）。
//! - 图标按图标主题的常规目录查找（`hicolor/<尺寸>/apps` 与 `pixmaps`），不解析
//!   `index.theme` 的继承链；主题里只有非常规布局时会取不到图标，此时候选照常在。

use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    FileApplication,
    desktop_entry::{self, DesktopEntry},
    linux_icon, mime_glob,
};

/// 相对数据目录的桌面条目子目录。
const APPLICATIONS_DIR: &str = "applications";
/// 桌面条目的文件名后缀。
const DESKTOP_SUFFIX: &str = ".desktop";
/// 相对数据目录的 MIME 数据库子目录。
const MIME_DIR: &str = "mime";
/// 新格式 glob 数据库文件名（带权重）。
const GLOBS2_FILE: &str = "globs2";
/// 旧格式 glob 数据库文件名。
const GLOBS_FILE: &str = "globs";
/// 默认打开方式登记文件名。
const MIMEAPPS_FILE: &str = "mimeapps.list";
/// `$XDG_DATA_DIRS` 未设置时的默认值（XDG 规范规定）。
const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";
/// `$XDG_CONFIG_DIRS` 未设置时的默认值（XDG 规范规定）。
const DEFAULT_CONFIG_DIRS: &str = "/etc/xdg";
/// `Type=` 里表示「能打开文件的应用」的取值；其他类型（`Link` / `Directory`）不能作候选。
const APPLICATION_TYPE: &str = "Application";

/// 枚举某个文件的候选打开方式。
///
/// 拿不到文件名、判不出 MIME 类型、或没有任何桌面条目声明支持该类型时，都返回空表：
/// 这些都不是错误，只是「系统没有为它登记打开方式」。
pub(crate) fn applications_for(path: &Path) -> Result<Vec<FileApplication>, String> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(Vec::new());
    };
    let data_dirs = xdg_data_dirs();
    let Some(mime_type) = resolve_mime_type(&data_dirs, file_name) else {
        return Ok(Vec::new());
    };
    let entries = collect_desktop_entries(&data_dirs, &mime_type);
    let default_id = default_entry_id(&data_dirs, &mime_type, &entries);

    let mut candidates: Vec<FileApplication> = entries
        .into_iter()
        .map(|(id, entry)| {
            let name = entry
                .name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| desktop_entry::display_name_from_exec(&entry.exec, &id));
            let default = default_id.as_deref() == Some(id.as_str());
            let icon = linux_icon::icon_data_url(&data_dirs, entry.icon.as_deref());
            FileApplication {
                id,
                name,
                icon,
                default,
            }
        })
        .collect();

    /*
     * 默认项排在最前，其余保持扫描顺序：这正是 `gio mime` 打印出来的顺序
     * （默认项一行，其余按系统登记顺序），因此菜单的第一条与「双击打开」的那个应用一致。
     */
    if let Some(position) = candidates.iter().position(|candidate| candidate.default) {
        let preferred = candidates.remove(position);
        candidates.insert(0, preferred);
    }
    Ok(candidates)
}

/// 用某个桌面条目打开文件。
///
/// 标识就是 [`applications_for`] 给出的 `id`（`applications/` 下去掉 `.desktop` 的文件名）：
/// 按同一套「先出现的目录为准」的顺序把它还原成桌面文件，再交给 `gio launch`：与 DSH 的
/// Linux 适配器同一条路径，绝不自行解析 `Exec=` 拼命令行。
pub(crate) fn open_with(path: &Path, application_id: &str) -> Result<(), String> {
    let Some(desktop_file) = locate_desktop_file(&xdg_data_dirs(), application_id) else {
        return Err(format!(
            "系统没有为这类文件登记这个打开方式：{application_id}"
        ));
    };
    let status = std::process::Command::new("gio")
        .arg("launch")
        .arg(&desktop_file)
        .arg(path)
        .status()
        .map_err(|error| format!("无法启动 gio：{error}"))?;
    if !status.success() {
        return Err(format!("用 {application_id} 打开失败"));
    }
    Ok(())
}

/// 把桌面条目 id 还原成磁盘上的桌面文件；两个数据目录都有同名条目时取先出现的那个。
fn locate_desktop_file(data_dirs: &[PathBuf], application_id: &str) -> Option<PathBuf> {
    for dir in data_dirs {
        let candidate = dir
            .join(APPLICATIONS_DIR)
            .join(format!("{application_id}{DESKTOP_SUFFIX}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 该 MIME 类型的默认桌面条目 id：`mimeapps.list` 里**第一个确实存在的候选**。
///
/// 按文件优先级顺序读遍所有 `mimeapps.list`，先出现者先比较；同一文件里 `键=值` 的多个候选
/// 也按登记顺序比较。列表里的 id 一个都不存在时返回 `None`（没有默认项）。
fn default_entry_id(
    data_dirs: &[PathBuf],
    mime_type: &str,
    entries: &[(String, DesktopEntry)],
) -> Option<String> {
    mimeapps_files(data_dirs)
        .into_iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .flat_map(|content| desktop_entry::parse_mimeapps_defaults(&content))
        .find_map(|(declared, id)| {
            (declared == mime_type && entries.iter().any(|(candidate, _)| *candidate == id))
                .then_some(id)
        })
}

/// `mimeapps.list` 的读取顺序：配置目录在前，数据目录在后（mime-apps 规范）。
fn mimeapps_files(data_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = xdg_config_home() {
        files.push(home.join(MIMEAPPS_FILE));
    }
    for dir in xdg_config_dirs() {
        files.push(dir.join(MIMEAPPS_FILE));
    }
    for dir in data_dirs {
        files.push(dir.join(APPLICATIONS_DIR).join(MIMEAPPS_FILE));
    }
    files
}

/// 扫描各数据目录下的 `applications/*.desktop`，挑出声明支持该 MIME 类型的条目。
///
/// 桌面条目 id 按规范取 `applications/` 下的文件名；**同名条目整体以先出现的目录为准**
/// （即使高优先级那份没有声明该 MIME 类型，也会把低优先级的同名条目挡住：这正是 XDG
/// 的「同名文件覆盖」语义）。结果按 id 排序，保证同一份系统状态每次产出同一张列表。
fn collect_desktop_entries(data_dirs: &[PathBuf], mime_type: &str) -> Vec<(String, DesktopEntry)> {
    let mut entries: Vec<(String, DesktopEntry)> = Vec::new();
    let mut shadowed: HashSet<String> = HashSet::new();
    for dir in data_dirs {
        let Ok(items) = std::fs::read_dir(dir.join(APPLICATIONS_DIR)) else {
            // 目录不存在是常态：`$XDG_DATA_DIRS` 的默认值里常常有目录并不存在。
            continue;
        };
        for item in items.flatten() {
            let file_name = item.file_name();
            let Some(id) = file_name
                .to_str()
                .and_then(|name| name.strip_suffix(DESKTOP_SUFFIX))
            else {
                continue;
            };
            if id.is_empty() || !shadowed.insert(id.to_owned()) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(item.path()) else {
                // 单个条目读不出来属于单项失败：跳过它，不影响其余候选。
                continue;
            };
            let entry = desktop_entry::parse_desktop_entry(&content);
            if is_candidate(&entry, mime_type) {
                entries.push((id.to_owned(), entry));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// 判断一条桌面条目是否可作为该 MIME 类型的打开方式。
fn is_candidate(entry: &DesktopEntry, mime_type: &str) -> bool {
    // `Type` 缺失时按规范默认是 `Application`；其他类型（`Link` / `Directory`）打不开文件。
    let wrong_type = entry
        .kind
        .as_deref()
        .is_some_and(|kind| !kind.eq_ignore_ascii_case(APPLICATION_TYPE));
    // `Hidden=true` 表示条目已被用户删除，必须整体忽略。
    // `NoDisplay=true` 则**不**排除：规范明确说这类条目正是「关联了 MIME 类型但不出现在菜单里」。
    // `Exec` 为空的条目没有可执行的命令，打开不了任何东西。
    !wrong_type
        && !entry.hidden
        && !entry.exec.is_empty()
        && entry
            .mime_types
            .iter()
            .any(|declared| declared.eq_ignore_ascii_case(mime_type))
}

/// 用共享 MIME 数据库的 glob 规则判断文件名对应的 MIME 类型；结果统一折叠成小写，
/// 便于与桌面条目的 `MimeType=` 及 `mimeapps.list` 的键比较。
fn resolve_mime_type(data_dirs: &[PathBuf], file_name: &str) -> Option<String> {
    let mut rules = Vec::new();
    for dir in data_dirs {
        let mime_dir = dir.join(MIME_DIR);
        // 新格式优先；同一目录里只有缺新格式时才读旧格式，避免同一批规则被登记两次。
        if let Ok(content) = std::fs::read_to_string(mime_dir.join(GLOBS2_FILE)) {
            rules.extend(mime_glob::parse_globs2(&content));
        } else if let Ok(content) = std::fs::read_to_string(mime_dir.join(GLOBS_FILE)) {
            rules.extend(mime_glob::parse_globs(&content));
        }
    }
    mime_glob::resolve_mime_type(&rules, file_name).map(str::to_ascii_lowercase)
}

/// 按 XDG 规范展开数据目录：`$XDG_DATA_HOME` 优先，随后是 `$XDG_DATA_DIRS`，保持顺序并去重。
fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match environment_path("XDG_DATA_HOME") {
        Some(home) => push_unique(&mut dirs, home),
        None => {
            if let Some(home) = home_dir() {
                push_unique(&mut dirs, home.join(".local").join("share"));
            }
        }
    }
    extend_path_list(&mut dirs, "XDG_DATA_DIRS", DEFAULT_DATA_DIRS);
    dirs
}

/// `$XDG_CONFIG_HOME`（默认 `~/.config`）。
fn xdg_config_home() -> Option<PathBuf> {
    environment_path("XDG_CONFIG_HOME").or_else(|| home_dir().map(|home| home.join(".config")))
}

/// `$XDG_CONFIG_DIRS`（默认 `/etc/xdg`）。
fn xdg_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    extend_path_list(&mut dirs, "XDG_CONFIG_DIRS", DEFAULT_CONFIG_DIRS);
    dirs
}

/// 读一个「以路径分隔符分隔的目录列表」环境变量并追加到 `dirs`；未设置或为空串时用 `fallback`。
///
/// 用 `std::env::split_paths` 而不是硬编码 `:`，语义与平台一致（空段落自动忽略）。
fn extend_path_list(dirs: &mut Vec<PathBuf>, name: &str, fallback: &str) {
    let configured = std::env::var_os(name).filter(|value| !value.is_empty());
    let raw = configured.unwrap_or_else(|| OsString::from(fallback));
    for entry in std::env::split_paths(&raw) {
        if !entry.as_os_str().is_empty() {
            push_unique(dirs, entry);
        }
    }
}

/// 读一个路径类环境变量的值；未设置或为空串时返回 `None`（XDG 规范把空值等同于未设置）。
fn environment_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// 追加一个尚未出现过的目录，保持 XDG 规范要求的优先级顺序。
fn push_unique(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// 用户主目录。
///
/// 只读 `$HOME`：`std::env::home_dir()` 已被弃用（新版工具链上会触发 `deprecated` 警告，
/// 而本仓把警告当错误），Linux 桌面会话里 `$HOME` 必定存在。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}
