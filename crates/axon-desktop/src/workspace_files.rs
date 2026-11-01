//! 工作区文件浏览：右侧栏的文件树与文件查看。
//!
//! 数据来源按生效档案分流（长官裁决：本机档案直读、云档案经网关）：
//!
//! - **本机档案**：直接读本机文件系统。文件列表与读取是热路径，同一台机器上再绕一次
//!   HTTP 只多付一跳延迟，因此保留既有实现；
//! - **云档案**：文件在远端，一律经网关（`/api/v1/files`、`/api/v1/files/content`、
//!   `/api/v1/fs/dirs`）。
//!
//! 本机打开类动作（默认程序、打开方式、文件管理器定位）只能是本机动作：云档案生效时
//! 一律拒绝，不启动任何本机进程（合同 §9 阶段 3）。
//!
//! 本机档案下，路径**严格限定在工作区根目录与用户主目录内**：任何越出范围的相对路径
//! （`..`、绝对路径、经符号链接跳出的真实路径）都被拒绝。边界判断必须自己做，不能
//! 指望调用方守规矩。

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace_files_gateway as gateway;

/// 单次列目录返回的最大条目数。
const MAX_DIRECTORY_ENTRIES: usize = 500;
/// 单个文件的最大读取字节数；超出部分截断并如实标注。
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// 二进制文件（图片、PDF、HTML）的最大读取字节数：这几类动辄上兆，上限比文本宽。
const MAX_BINARY_BYTES: u64 = 12 * 1024 * 1024;
/// 判断二进制文件时检查的前缀字节数。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 云档案下本机动作的统一拒绝文案。
pub(crate) const CLOUD_LOCAL_ACTION_REJECTED: &str = "云档案下不支持在本机打开，请在远端服务上操作";

/// 文件类命令的数据来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileSource {
    /// 本机档案：直读本机文件系统。
    Local,
    /// 云档案：经网关读写远端文件。
    Gateway,
}

/// 只读判定：给定「当前是否为云档案」的结论，选出数据来源。
///
/// 单测直接断言这个函数，不需要起网关，也不依赖开发机上此刻生效的是哪个档案。
#[must_use]
pub(crate) fn file_source(is_cloud: bool) -> FileSource {
    if is_cloud {
        FileSource::Gateway
    } else {
        FileSource::Local
    }
}

/// 当前生效档案的数据来源。
fn current_file_source() -> FileSource {
    file_source(crate::connection::active_is_cloud())
}

/// 只读判定：给定「当前是否为云档案」的结论，本机动作是否被拒绝。
#[must_use]
pub(crate) fn local_action_rejection(is_cloud: bool) -> Option<&'static str> {
    is_cloud.then_some(CLOUD_LOCAL_ACTION_REJECTED)
}

/// 按当前生效档案判定本机动作是否允许。
fn ensure_local_action_allowed() -> Result<(), String> {
    match local_action_rejection(crate::connection::active_is_cloud()) {
        Some(message) => Err(message.to_owned()),
        None => Ok(()),
    }
}

/// 目录项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntryDto {
    /// 文件或目录名。
    pub name: String,
    /// 相对工作区根目录的路径，统一用正斜杠。
    pub path: String,
    /// `dir` 或 `file`。
    pub kind: String,
}

/// 文件内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTextDto {
    /// 相对工作区根目录的路径。
    pub path: String,
    /// 工作区内的绝对路径；查看页的路径栏显示这一份。
    pub absolute_path: String,
    /// 文本内容；二进制文件时为空。
    pub content: String,
    /// 是否因超出上限被截断。
    pub truncated: bool,
    /// 是否为二进制文件。
    pub binary: bool,
}

/// 一个文件的原始字节。
///
/// 图片、PDF、HTML 这类内容必须拿到**未经解码的字节**：按文本读会把非 UTF-8 的
/// 部分替换成占位符，渲染出来的图或文档就是坏的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBytesDto {
    /// 相对工作区根目录的路径。
    pub path: String,
    /// 工作区内的绝对路径。
    pub absolute_path: String,
    /// 由扩展名判定的 MIME 类型；判不出来时是 `application/octet-stream`。
    pub mime: String,
    /// 原始字节的 base64。
    pub data_base64: String,
    /// 是否因超出上限被截断。
    pub truncated: bool,
}

/// 把相对路径规范化为可读取的绝对路径。
///
/// 允许的范围只有两级：**工作区**与**用户主目录**。
/// - 工作区内一律用相对路径（纯相对、不得含 `..`）；
/// - `~` 开头（以及 `~/`、`~\`）解析到用户主目录：助手正文里常写 `~\.qoder`
///   这类路径，芯片点开时若按工作区相对路径拼接（`<工作区>\~\.qoder`）必然
///   找不到文件（表现为 os error 3）；
/// - 绝对路径只有落在主目录或工作区内才接受，`C:\Windows` 这类自由路径仍然拒绝。
///
/// `pub(crate)` 是给「用其他应用打开」共用的：那条路径同样必须先过这一关。
///
/// # Errors
///
/// 云档案生效时返回拒绝文案；本机档案下越界、`..` 跳转或目标不存在时返回原因。
pub(crate) fn resolve_within(root: &str, relative: &str) -> Result<PathBuf, String> {
    // 云档案下本机动作没有意义：远端文件不在本机，拉起本机进程只会打开一个同名的
    // 本地文件。判定放在这里，是因为「用默认程序打开」「用其他应用打开」「枚举打开
    // 方式」「在文件管理器中显示」四条命令都先经过这一关，四条命令因此共用同一句
    // 拒绝文案，也不会各写一份。
    ensure_local_action_allowed()?;
    resolve_within_locally(root, relative)
}

/// 本机档案下的路径解析：工作区与用户主目录两级范围。
fn resolve_within_locally(root: &str, relative: &str) -> Result<PathBuf, String> {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Err(format!("工作区目录不存在：{root}"));
    }
    let canonical_root = root_path
        .canonicalize()
        .map_err(|error| format!("解析工作区目录失败：{error}"))?;
    let home = home_directory();

    let trimmed = relative.trim().replace('\\', "/");
    let (candidate, scope) = if trimmed.is_empty() || trimmed == "." {
        (canonical_root.clone(), canonical_root.clone())
    } else if let Some(rest) = trimmed.strip_prefix('~') {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        let home = home.ok_or_else(|| "无法解析用户主目录，不能打开 `~` 路径".to_string())?;
        (home.join(rest), home)
    } else {
        let relative_path = Path::new(&trimmed);
        if relative_path.is_absolute() {
            /*
             * 先把绝对路径规范化再判范围：Windows 上规范化会带上 `\\?\` 扩展前缀，
             * 拿未规范化的字符串与已规范化的主目录比对必然不相等。
             */
            let canonical = relative_path
                .canonicalize()
                .map_err(|error| format!("读取路径失败：{error}"))?;
            let scope = if canonical.starts_with(&canonical_root) {
                canonical_root.clone()
            } else if home
                .as_deref()
                .is_some_and(|home| canonical.starts_with(home))
            {
                home.clone().unwrap_or_default()
            } else {
                return Err("只接受工作区或用户主目录内的路径".to_string());
            };
            (canonical, scope)
        } else {
            for component in relative_path.components() {
                match component {
                    Component::Normal(_) | Component::CurDir => {}
                    _ => return Err("路径不得包含上级跳转".to_string()),
                }
            }
            (canonical_root.join(relative_path), canonical_root.clone())
        }
    };

    let canonical = candidate
        .canonicalize()
        .map_err(|error| format!("读取路径失败：{error}"))?;
    if !canonical.starts_with(&scope) {
        return Err("路径越出允许范围".to_string());
    }
    Ok(canonical)
}

/// 用户主目录的规范化路径；解析不出来时返回 None（此时 `~` 路径会明确报错）。
fn home_directory() -> Option<PathBuf> {
    axon_config::user_home_dir()
        .ok()
        .and_then(|home| home.canonicalize().ok())
}

/// 把绝对路径还原成展示用的正斜杠路径：工作区内相对工作区，主目录内用 `~/` 形式。
fn relative_display(root: &Path, path: &Path) -> Result<String, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("解析工作区目录失败：{error}"))?;
    if let Ok(relative) = path.strip_prefix(&canonical_root) {
        return Ok(relative.to_string_lossy().replace('\\', "/"));
    }
    if let Some(home) = home_directory()
        && let Ok(relative) = path.strip_prefix(&home)
    {
        return Ok(format!(
            "~/{}",
            relative.to_string_lossy().replace('\\', "/")
        ));
    }
    Err("路径越出工作区范围".to_string())
}

/// 目录在前、名称升序：两种数据来源共用同一套展示顺序，界面不会跳序。
fn sort_entries(entries: &mut [FileEntryDto]) {
    entries.sort_by(|left, right| {
        right
            .kind
            .cmp(&left.kind)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

/// 列出工作区内某个目录的直接子项：目录在前，随后按名称排序。
///
/// # Errors
///
/// 云档案下由网关给出结论；本机档案下目录不存在、越界或读取失败时返回原因。
pub fn list_entries(root: &str, relative: &str) -> Result<Vec<FileEntryDto>, String> {
    if current_file_source() == FileSource::Gateway {
        let mut entries = gateway::list_entries(root, relative)?;
        sort_entries(&mut entries);
        return Ok(entries);
    }
    list_entries_locally(root, relative)
}

/// 本机档案下的目录列表。
fn list_entries_locally(root: &str, relative: &str) -> Result<Vec<FileEntryDto>, String> {
    let root_path = Path::new(root);
    let directory = resolve_within_locally(root, relative)?;
    if !directory.is_dir() {
        return Err("目标不是目录".to_string());
    }
    let mut entries: Vec<FileEntryDto> = Vec::new();
    let reader = std::fs::read_dir(&directory).map_err(|error| format!("读取目录失败：{error}"))?;
    for item in reader.flatten() {
        if entries.len() >= MAX_DIRECTORY_ENTRIES {
            break;
        }
        let Ok(kind) = item.file_type() else {
            continue;
        };
        let name = item.file_name().to_string_lossy().to_string();
        let kind_label = if kind.is_dir() { "dir" } else { "file" };
        let Ok(path) = relative_display(root_path, &item.path()) else {
            continue;
        };
        entries.push(FileEntryDto {
            name,
            path,
            kind: kind_label.to_string(),
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// 读取工作区内一个文本文件。
///
/// 二进制文件不返回内容，只回一个可读结论；超出上限的文件截断到上限，
/// 并如实标注 `truncated`，不假装内容完整。
///
/// # Errors
///
/// 云档案下由网关给出结论；本机档案下越界、目标不是文件或读取失败时返回原因。
pub fn read_text(root: &str, relative: &str) -> Result<FileTextDto, String> {
    if current_file_source() == FileSource::Gateway {
        let (text, truncated) = gateway::fetch_text(root, relative)?;
        // 二进制判定与直读同一口径：前 8 KiB 出现 NUL 即按二进制处理，正文留空。
        let bytes = text.as_bytes();
        let binary = bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0);
        return Ok(FileTextDto {
            path: cloud_display(root, relative),
            absolute_path: cloud_absolute(root, relative),
            content: if binary { String::new() } else { text },
            truncated: truncated && !binary,
            binary,
        });
    }
    read_text_locally(root, relative)
}

/// 本机档案下的文本读取。
fn read_text_locally(root: &str, relative: &str) -> Result<FileTextDto, String> {
    let path = resolve_within_locally(root, relative)?;
    let metadata =
        std::fs::metadata(&path).map_err(|error| format!("读取文件信息失败：{error}"))?;
    if !metadata.is_file() {
        return Err("目标不是文件".to_string());
    }
    let display = relative_display(Path::new(root), &path)?;
    // 规范化后的绝对路径在 Windows 上带 `\\?\` 扩展前缀；那是文件系统的细节，
    // 不该出现在界面上的路径栏里，因此按工作区展示的同一口径去掉它。
    let absolute = crate::workspace::strip_extended_prefix(&path.to_string_lossy());
    let length = metadata.len();
    let read_limit = length.min(MAX_FILE_BYTES);
    let bytes = read_prefix(&path, read_limit as usize)?;

    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return Ok(FileTextDto {
            path: display,
            absolute_path: absolute,
            content: String::new(),
            truncated: false,
            binary: true,
        });
    }
    let content = String::from_utf8_lossy(&bytes).to_string();
    Ok(FileTextDto {
        path: display,
        absolute_path: absolute,
        content,
        truncated: length > read_limit,
        binary: false,
    })
}

/// 读取文件前缀，避免把超大文件整体读进内存。
fn read_prefix(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|error| format!("打开文件失败：{error}"))?;
    let mut buffer = Vec::with_capacity(limit.min(64 * 1024));
    file.take(limit as u64)
        .read_to_end(&mut buffer)
        .map_err(|error| format!("读取文件失败：{error}"))?;
    Ok(buffer)
}

/// Tauri IPC 命令：列出工作区内某个目录的直接子项。
#[tauri::command(async)]
pub fn workspace_list_entries(
    root: String,
    relative_path: Option<String>,
) -> Result<Vec<FileEntryDto>, String> {
    list_entries(&root, relative_path.as_deref().unwrap_or_default())
}

/// Tauri IPC 命令：读取工作区内的一个文本文件。
#[tauri::command(async)]
pub fn workspace_read_file(root: String, relative_path: String) -> Result<FileTextDto, String> {
    read_text(&root, &relative_path)
}

/// 路径探测结果：前端据此决定「按文件预览」还是「按目录树展开」。
#[derive(Debug, Clone, Serialize)]
pub struct PathStatDto {
    /// 是否解析到一个目录。
    pub is_dir: bool,
    /// 规范化后的绝对路径；解析不出来时为空串。
    pub absolute_path: String,
    /// 展示用路径：工作区内相对工作区，主目录内是 `~/` 形式。
    pub display_path: String,
}

/// 探测路径是不是目录。
///
/// 解析不出来（文件不存在、越出允许范围）时只回答「不是目录」：真正的错误由后续的
/// 读取流程给出，这里多一种错误文案只会让同一件事有两种说法。
#[must_use]
pub fn stat_path(root: &str, relative: &str) -> PathStatDto {
    if current_file_source() == FileSource::Gateway {
        return stat_path_cloud(root, relative);
    }
    stat_path_locally(root, relative)
}

/// 本机档案下的路径探测。
fn stat_path_locally(root: &str, relative: &str) -> PathStatDto {
    let Ok(path) = resolve_within_locally(root, relative) else {
        return PathStatDto {
            is_dir: false,
            absolute_path: String::new(),
            display_path: relative.trim().to_string(),
        };
    };
    PathStatDto {
        is_dir: path.is_dir(),
        absolute_path: crate::workspace::strip_extended_prefix(&path.to_string_lossy()),
        display_path: relative_display(Path::new(root), &path)
            .unwrap_or_else(|_| relative.trim().to_string()),
    }
}

/// 云档案下的路径探测：用目录列表与目录路由组合判定。
///
/// 先列父目录，一次就同时得到「在不在」与「是目录还是文件」；列表里没有（父目录不在
/// 已登记的根内、或条目不可见）再用目录路由问一次绝对路径。两处都得不出结论时如实
/// 回报不可判定：`absolute_path` 留空，界面据此不走目录展开，绝不伪造「是文件」。
fn stat_path_cloud(root: &str, relative: &str) -> PathStatDto {
    let text = relative.trim();
    let undecidable = || PathStatDto {
        is_dir: false,
        absolute_path: String::new(),
        display_path: text.to_string(),
    };
    if text.is_empty() || text == "." {
        return PathStatDto {
            is_dir: true,
            absolute_path: cloud_absolute(root, text),
            display_path: String::new(),
        };
    }
    let display = cloud_display(root, text);
    let absolute = cloud_absolute(root, text);
    let (parent, leaf) = match display.rsplit_once('/') {
        Some((parent, leaf)) => (parent.to_string(), leaf.to_string()),
        None => (String::new(), display.clone()),
    };
    if let Ok(entries) = gateway::list_entries(root, &parent)
        && let Some(entry) = entries
            .iter()
            .find(|entry| entry.path == display || entry.name == leaf)
    {
        return PathStatDto {
            is_dir: entry.kind == "dir",
            absolute_path: absolute,
            display_path: display,
        };
    }
    match gateway::is_directory(&absolute) {
        Ok(is_dir) => PathStatDto {
            is_dir,
            absolute_path: absolute,
            display_path: display,
        },
        Err(_) => undecidable(),
    }
}

/// 云档案的展示用相对路径：输入是根内绝对路径时还原成相对形式，否则原样返回。
fn cloud_display(root: &str, relative: &str) -> String {
    let text = relative.trim().replace('\\', "/");
    let base = root.trim().replace('\\', "/");
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        return text;
    }
    let Some(prefix) = text.get(..base.len()) else {
        return text;
    };
    if !prefix.eq_ignore_ascii_case(base) {
        return text;
    }
    // 根之后必须紧跟分隔符或到此为止：`C:/work` 不是 `C:/workspace` 的前缀。
    let rest = text.get(base.len()..).unwrap_or_default();
    if rest.is_empty() || rest.starts_with('/') {
        rest.trim_start_matches('/').to_string()
    } else {
        text
    }
}

/// 云档案的绝对路径：相对输入拼到根下，绝对输入原样返回（统一正斜杠）。
fn cloud_absolute(root: &str, relative: &str) -> String {
    let text = relative.trim().replace('\\', "/");
    let base = root.trim().replace('\\', "/");
    let base = base.trim_end_matches('/');
    if text.is_empty() || text == "." {
        return base.to_string();
    }
    let is_absolute = text.starts_with('/')
        || text
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if is_absolute {
        return text;
    }
    format!("{base}/{}", text.trim_start_matches('/'))
}

/// Tauri IPC 命令：探测工作区内的一个路径是文件还是目录。
#[tauri::command(async)]
pub fn workspace_stat_path(root: String, relative_path: String) -> Result<PathStatDto, String> {
    Ok(stat_path(&root, &relative_path))
}

/// 按扩展名判定 MIME 类型；判不出来时给出通用的二进制类型。
///
/// 渲染器用它决定「这份字节该怎么交给浏览器」：图片走 `data:` 地址、
/// HTML 与 PDF 走 `Blob`。判错不会崩，只是浏览器可能拒绝渲染。
fn mime_of(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// 读取工作区内一个文件的原始字节。
///
/// 与文本读取同一套边界：本机档案下路径必须先过工作区校验，符号链接跳出同样被拒绝；
/// 云档案下由网关按服务端自己的路径边界判定。
///
/// 二进制文件没有「截断后仍然可用」这一说：本机档案超限时如实标注 `truncated`，
/// 由渲染器决定是提示还是拒绝；网关对超出字节上限的文件直接拒绝，不回残缺字节。
///
/// # Errors
///
/// 云档案下由网关给出结论；本机档案下越界、目标不是文件或读取失败时返回原因。
pub fn read_bytes(root: &str, relative: &str) -> Result<FileBytesDto, String> {
    if current_file_source() == FileSource::Gateway {
        let (data_base64, mime) = gateway::fetch_bytes(root, relative)?;
        return Ok(FileBytesDto {
            path: cloud_display(root, relative),
            absolute_path: cloud_absolute(root, relative),
            mime,
            data_base64,
            truncated: false,
        });
    }
    read_bytes_locally(root, relative)
}

/// 本机档案下的原始字节读取。
fn read_bytes_locally(root: &str, relative: &str) -> Result<FileBytesDto, String> {
    let path = resolve_within_locally(root, relative)?;
    let metadata =
        std::fs::metadata(&path).map_err(|error| format!("读取文件信息失败：{error}"))?;
    if !metadata.is_file() {
        return Err("目标不是文件".to_string());
    }
    let display = relative_display(Path::new(root), &path)?;
    let absolute = crate::workspace::strip_extended_prefix(&path.to_string_lossy());
    let length = metadata.len();
    let read_limit = length.min(MAX_BINARY_BYTES);
    let bytes = read_prefix(&path, read_limit as usize)?;
    use base64::Engine as _;
    Ok(FileBytesDto {
        path: display,
        absolute_path: absolute,
        mime: mime_of(&path).to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        truncated: length > read_limit,
    })
}

/// Tauri IPC 命令：读取工作区内的一个文件的原始字节。
#[tauri::command(async)]
pub fn workspace_read_bytes(root: String, relative_path: String) -> Result<FileBytesDto, String> {
    read_bytes(&root, &relative_path)
}

/// 用系统默认程序打开工作区内的文件。
///
/// # Errors
///
/// 云档案下返回拒绝文案；本机档案下越界或系统调用失败时返回原因。
pub fn open_default(root: &str, relative: &str) -> Result<(), String> {
    let path = resolve_within(root, relative)?;
    crate::process::open_path(&path)
}

/// 在文件管理器中定位工作区内的文件。
///
/// # Errors
///
/// 云档案下返回拒绝文案；本机档案下越界或系统调用失败时返回原因。
pub fn reveal(root: &str, relative: &str) -> Result<(), String> {
    let path = resolve_within(root, relative)?;
    crate::process::reveal(&path)
}

/// Tauri IPC 命令：用系统默认程序打开工作区内的文件。
#[tauri::command(async)]
pub fn workspace_open_default(root: String, relative_path: String) -> Result<(), String> {
    open_default(&root, &relative_path)
}

/// Tauri IPC 命令：在文件管理器中定位工作区内的文件。
#[tauri::command(async)]
pub fn workspace_reveal(root: String, relative_path: String) -> Result<(), String> {
    reveal(&root, &relative_path)
}
#[cfg(test)]
#[path = "workspace_files_tests.rs"]
mod tests;
