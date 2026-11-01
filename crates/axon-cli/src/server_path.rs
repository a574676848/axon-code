//! 服务端路径归一化：把绝对路径或「根 + 相对路径」收敛成一个可用路径，或给出拒绝结论。
//!
//! 为什么自写一套：仓库既有的两份路径工具都不适合服务端。桌面端那份 `resolve_within`
//! 明确放行 `~` 与整个用户主目录，授权完全交给调用方；工作区那份越界时甚至返回 Ok。
//! 服务端的口径相反：凡是能判定的越界与非法形态一律**拒绝**，不把判定责任推给调用方。
//!
//! 归一化口径与合同 §6.3.1 一致：
//!
//! - 先做纯字符串归一化，再与根按**路径组件**比对，绝不用字符串前缀比较，否则
//!   `/a/bc` 会命中 `/a/b`；
//! - Windows 大小写不敏感；8.3 短名、目录联接与符号链接由 [`std::fs::canonicalize`]
//!   展开成真实长名路径；`\\?\` 前缀只在对外输出里剥掉，不接受带该前缀的入参；
//! - 拒绝而非放行的形态：`..` 跳出、绝对路径跳出、NTFS 数据流、设备命名空间、
//!   UNC、驱动器相对路径、尾随点与空格被系统剥离的变体、8.3 短名与上级跳转的组合。
//!
//! 明确不承诺的事：本模块不引入根集合白名单。任何持有效凭据者本来就能注册任意路径，
//! 白名单不构成 containment（合同 §6.3.1 与 §12 R25 已登记为已知残余风险）。
//!
//! 纯函数优先：形态判定与组件比对都接受 [`PathDialect`] 参数，不读运行平台，
//! 因此 POSIX 语义可以在 Windows 上逐条断言；只有走到文件系统的那两个函数需要
//! 宿主平台的真实方言。

use std::path::{Path, PathBuf};

/// `\\?\` 长路径前缀，仅用于从 `canonicalize` 的产物里剥离。
const VERBATIM_PREFIX: &str = "\\\\?\\";
/// 附件目录在运行时根下的名字；该目录自成一类根，不得被当作工作区根。
const ATTACHMENTS_DIR_NAME: &str = "attachments";
/// 单个路径段里允许出现的最长字节数，与 NTFS 单段上限同量级，超出必然是构造出来的。
const MAX_COMPONENT_BYTES: usize = 255;

/// 平台路径方言，决定分隔符、大小写敏感性与 Windows 专有形态的判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathDialect {
    /// Windows 语义：反斜杠与正斜杠都是分隔符，大小写不敏感，存在盘符与 NTFS 专有形态。
    Windows,
    /// POSIX 语义：只有正斜杠是分隔符，大小写敏感。
    Posix,
}

impl PathDialect {
    /// 当前编译目标的真实方言。
    ///
    /// 只有真的落到文件系统上时才用得上；纯判定一律由调用方显式传入方言。
    pub(crate) const fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }

    /// 该字符在此方言下是否为路径分隔符。
    const fn is_separator(self, value: char) -> bool {
        match self {
            Self::Windows => value == '\\' || value == '/',
            Self::Posix => value == '/',
        }
    }
}

/// 归一化拒绝原因。
///
/// 每一条都自带中文结论与对应的 HTTP 拒绝码，路由层因此不必再维护第二份映射表：
/// 越界与非法形态是 403，形态本身就不成立是 400，解析不到目标是 404。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejection {
    /// 路径为空。
    Empty,
    /// 路径包含空字符。
    NullByte,
    /// 路径包含控制字符。
    ControlCharacter,
    /// 单个路径段过长。
    ComponentTooLong,
    /// 设备命名空间，例如 `\\.\PhysicalDrive0`。
    DeviceNamespace,
    /// 长路径前缀，例如 `\\?\C:\Windows\win.ini`。
    VerbatimPrefix,
    /// UNC 网络共享，例如 `\\server\share`。
    Unc,
    /// 驱动器相对路径，例如 `C:..\..\Windows` 或 `\Windows`。
    DriveRelative,
    /// NTFS 备用数据流，例如 `file.txt::$DATA`。
    DataStream,
    /// 以点或空格结尾的路径段，Windows 会把这些字符剥掉。
    TrailingDotOrSpace,
    /// 8.3 短名与上级跳转的组合，例如 `PROGRA~1\..`。
    ShortNameFallback,
    /// 上级跳转越出根目录。
    ParentEscape,
    /// 路径落在根目录之外。
    OutsideRoot,
    /// 要求绝对路径却拿到了相对路径。
    NotAbsolute,
    /// 根目录不存在或不可访问。
    RootUnavailable,
    /// 目标不存在或不可访问。
    ResolveFailed,
}

impl Rejection {
    /// 中文拒绝结论，直接进响应正文。
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Empty => "路径不能为空",
            Self::NullByte => "路径包含空字符",
            Self::ControlCharacter => "路径包含控制字符",
            Self::ComponentTooLong => "路径段过长",
            Self::DeviceNamespace => "不接受设备命名空间路径（\\\\.\\ 形态）",
            Self::VerbatimPrefix => "不接受长路径前缀（\\\\?\\ 形态），请使用普通绝对路径",
            Self::Unc => "不接受 UNC 网络共享路径（\\\\server\\share 形态）",
            Self::DriveRelative => {
                "不接受驱动器相对路径（C:xxx 或 \\xxx 形态），必须给出带盘符的绝对路径"
            }
            Self::DataStream => "不接受 NTFS 备用数据流路径（file.txt::$DATA 形态）",
            Self::TrailingDotOrSpace => "不接受以点或空格结尾的路径段，Windows 会把它们剥掉",
            Self::ShortNameFallback => "不接受 8.3 短名与上级跳转的组合（PROGRA~1\\.. 形态）",
            Self::ParentEscape => "上级跳转越出根目录",
            Self::OutsideRoot => "路径越出注册的根目录",
            Self::NotAbsolute => "必须给出绝对路径",
            Self::RootUnavailable => "根目录不存在或不可访问",
            Self::ResolveFailed => "目标不存在或不可访问",
        }
    }

    /// 对应的 HTTP 拒绝码。
    pub(crate) const fn denied_status(self) -> u16 {
        match self {
            Self::Empty | Self::NotAbsolute => 400,
            Self::RootUnavailable | Self::ResolveFailed => 404,
            _ => 403,
        }
    }
}

/// 归一化后的绝对路径，以路径组件序列表示。
///
/// 组件序列是比对与拼接的唯一形态：整条路径从不以字符串形式参与比较，因此
/// 分隔符混用、`\\?\` 前缀与大小写都不会影响判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedPath {
    /// 组件序列；Windows 盘符是第一个组件（形如 `C:`），POSIX 不含前导空组件。
    components: Vec<String>,
}

impl NormalizedPath {
    /// 组件序列。
    pub(crate) fn components(&self) -> &[String] {
        &self.components
    }

    /// 重组成该方言下的绝对路径字符串。
    pub(crate) fn display(&self, dialect: PathDialect) -> String {
        match dialect {
            PathDialect::Windows => self.display_windows(),
            PathDialect::Posix => format!("/{}", self.components.join("/")),
        }
    }

    /// 转成可交给文件系统 API 的路径。
    ///
    /// 只对宿主方言成立：POSIX 形态的路径在 Windows 上没有盘符，无法落盘。
    pub(crate) fn to_path_buf(&self, dialect: PathDialect) -> PathBuf {
        PathBuf::from(self.display(dialect))
    }

    /// Windows 重建：组件之间恰好一个分隔符；只有盘符一个组件时必须补尾分隔符，
    /// 否则 `C:` 是驱动器相对路径而不是盘根。
    fn display_windows(&self) -> String {
        let mut text = String::new();
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                text.push('\\');
            }
            text.push_str(component);
        }
        let is_bare_drive = self.components.len() == 1
            && self
                .components
                .first()
                .is_some_and(|component| is_drive_component(component));
        if is_bare_drive {
            text.push('\\');
        }
        text
    }
}

/// 剥掉 `\\?\` 长路径前缀。
pub(crate) fn strip_verbatim_prefix(value: &str) -> &str {
    value.strip_prefix(VERBATIM_PREFIX).unwrap_or(value)
}

/// 判断组件是否为 Windows 盘符（`C:` 形态）。
fn is_drive_component(component: &str) -> bool {
    let bytes = component.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// 纯函数：判断入参在此方言下是否已经是绝对路径。
pub(crate) fn is_absolute_input(input: &str, dialect: PathDialect) -> bool {
    match dialect {
        PathDialect::Windows => {
            let bytes = input.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && dialect.is_separator(bytes[2] as char)
        }
        PathDialect::Posix => input.starts_with('/'),
    }
}

/// 纯函数：把绝对路径归一化成组件序列。
pub(crate) fn normalize_absolute(
    input: &str,
    dialect: PathDialect,
) -> Result<NormalizedPath, Rejection> {
    if input.is_empty() {
        return Err(Rejection::Empty);
    }
    check_characters(input)?;
    let (base, rest) = split_absolute_root(input, dialect)?;
    normalize_into(&base, rest, dialect)
}

/// 纯函数：把「根 + 相对路径」归一化成组件序列。
///
/// 相对路径里的 `..` 可以回退，但不得越过根；绝对路径与驱动器形态一律拒绝，
/// 调用方判断是绝对路径时应当先走 [`normalize_absolute`] 再做包含性判定。
pub(crate) fn normalize_relative(
    base: &NormalizedPath,
    relative: &str,
    dialect: PathDialect,
) -> Result<NormalizedPath, Rejection> {
    if relative.is_empty() {
        return Ok(base.clone());
    }
    check_characters(relative)?;
    if dialect == PathDialect::Windows {
        check_windows_relative_form(relative)?;
    }
    normalize_into(&base.components, relative, dialect)
}

/// 纯函数：按组件判定 `child` 是否位于 `root` 之内。
///
/// 前缀比较按组件进行，因此 `/a/bc` 不命中 `/a/b`：这是本函数存在的全部理由。
pub(crate) fn is_within(
    child: &NormalizedPath,
    root: &NormalizedPath,
    dialect: PathDialect,
) -> bool {
    let root_components = root.components();
    let child_components = child.components();
    if child_components.len() < root_components.len() {
        return false;
    }
    root_components
        .iter()
        .zip(child_components.iter())
        .all(|(left, right)| components_equal(left, right, dialect))
}

/// 逐段比较两个组件；Windows 大小写不敏感。
fn components_equal(left: &str, right: &str, dialect: PathDialect) -> bool {
    match dialect {
        PathDialect::Windows => left.to_lowercase() == right.to_lowercase(),
        PathDialect::Posix => left == right,
    }
}

/// 运行时根下的附件目录。
pub(crate) fn attachments_dir(axon_root: &Path) -> PathBuf {
    axon_root
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(ATTACHMENTS_DIR_NAME)
}

/// 判断目标路径是否就是附件目录或位于其下。
///
/// 附件目录自成一类根，不得被当作工作区根或可浏览目录，否则附件会被当成源码浏览
/// （合同 §6.3.1）。目录还不存在时同样要给结论，因此这里走的是宽松解析。
pub(crate) fn is_attachments_path(path: &Path, axon_root: &Path, dialect: PathDialect) -> bool {
    let Ok(directory) = resolve_absolute(&attachments_dir(axon_root).to_string_lossy(), dialect)
    else {
        return false;
    };
    let (Ok(target), Ok(directory)) = (
        normalize_absolute(strip_verbatim_prefix(&path.to_string_lossy()), dialect),
        normalize_absolute(strip_verbatim_prefix(&directory.to_string_lossy()), dialect),
    ) else {
        return false;
    };
    is_within(&target, &directory, dialect)
}

/// 解析一条绝对路径：归一化后落到真实路径，目标不存在时给出最近存在祖先的等价路径。
pub(crate) fn resolve_absolute(input: &str, dialect: PathDialect) -> Result<PathBuf, Rejection> {
    let normalized = normalize_absolute(input, dialect)?;
    canonicalize_lenient(&normalized.to_path_buf(dialect), dialect)
}

/// 以 `root` 为界解析 `path`：`path` 可以是相对路径，也可以是落在根内的绝对路径。
///
/// 两道包含性判定缺一不可：先按组件判一次拦住 `..` 与绝对路径跳出（此时还没碰
/// 文件系统，结论最干净），再对**解析后的真实路径**判一次拦住目录联接与符号链接
/// 跳出（此时才知道链接指向哪里）。
pub(crate) fn resolve_within(
    root: &Path,
    path: &str,
    dialect: PathDialect,
) -> Result<PathBuf, Rejection> {
    let root_text = root.to_string_lossy().into_owned();
    let root_normalized = normalize_absolute(strip_verbatim_prefix(&root_text), dialect)?;
    let root_resolved = std::fs::canonicalize(root_normalized.to_path_buf(dialect))
        .map_err(|_| Rejection::RootUnavailable)?;
    let canonical_root = normalize_absolute(
        strip_verbatim_prefix(&root_resolved.to_string_lossy()),
        dialect,
    )?;

    let candidate = if path.is_empty() {
        canonical_root.clone()
    } else if is_absolute_input(path, dialect) {
        normalize_absolute(path, dialect)?
    } else {
        normalize_relative(&canonical_root, path, dialect)?
    };
    if !is_within(&candidate, &canonical_root, dialect) {
        return Err(Rejection::OutsideRoot);
    }

    let resolved = canonicalize_lenient(&candidate.to_path_buf(dialect), dialect)?;
    let resolved_normalized =
        normalize_absolute(strip_verbatim_prefix(&resolved.to_string_lossy()), dialect)?;
    if !is_within(&resolved_normalized, &canonical_root, dialect) {
        return Err(Rejection::OutsideRoot);
    }
    Ok(resolved)
}

/// 把路径解析成真实路径；目标不存在时退到最近的既有祖先再拼回尾部组件。
///
/// 「目标不存在」必须显式处理而不是当成错误：列表与内容路由的合法输入里，
/// 未创建的目录与拼错的路径都要能给出 404 而不是 500。尾部组件都是普通名字，
/// 无法再跳出，因此解析结果仍然可信，但调用方必须对结果重做包含性判定。
fn canonicalize_lenient(path: &Path, dialect: PathDialect) -> Result<PathBuf, Rejection> {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return Ok(strip_verbatim(&resolved, dialect));
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path.to_path_buf();
    loop {
        let Some(parent) = cursor.parent().map(Path::to_path_buf) else {
            return Err(Rejection::ResolveFailed);
        };
        let Some(name) = cursor.file_name().map(std::ffi::OsStr::to_os_string) else {
            return Err(Rejection::ResolveFailed);
        };
        if parent.as_os_str().is_empty() {
            return Err(Rejection::ResolveFailed);
        }
        tail.push(name);
        match std::fs::canonicalize(&parent) {
            Ok(resolved) => {
                let mut result = strip_verbatim(&resolved, dialect);
                for component in tail.iter().rev() {
                    result.push(component);
                }
                return Ok(result);
            }
            Err(_) => cursor = parent,
        }
    }
}

/// 剥掉 `\\?\` 前缀后再转回 `PathBuf`，避免前缀泄漏到对外输出。
fn strip_verbatim(path: &Path, dialect: PathDialect) -> PathBuf {
    if dialect != PathDialect::Windows {
        return path.to_path_buf();
    }
    PathBuf::from(strip_verbatim_prefix(&path.to_string_lossy()).to_owned())
}

/// 拒绝空字符与控制字符；两平台都不接受这两种输入。
fn check_characters(input: &str) -> Result<(), Rejection> {
    if input.contains('\0') {
        return Err(Rejection::NullByte);
    }
    if input.chars().any(char::is_control) {
        return Err(Rejection::ControlCharacter);
    }
    Ok(())
}

/// 拆分绝对路径的根与前缀之后的剩余部分。
fn split_absolute_root(
    input: &str,
    dialect: PathDialect,
) -> Result<(Vec<String>, &str), Rejection> {
    match dialect {
        PathDialect::Windows => split_windows_root(input),
        PathDialect::Posix => match input.strip_prefix('/') {
            Some(rest) => Ok((Vec::new(), rest)),
            None => Err(Rejection::NotAbsolute),
        },
    }
}

/// 拆分 Windows 绝对路径：只接受 `X:\` 一种盘符绝对形态。
fn split_windows_root(input: &str) -> Result<(Vec<String>, &str), Rejection> {
    let bytes = input.as_bytes();
    if input.starts_with("\\\\") || input.starts_with("//") {
        let third = bytes.get(2).copied().unwrap_or(b'\0');
        return Err(match third {
            b'?' => Rejection::VerbatimPrefix,
            b'.' => Rejection::DeviceNamespace,
            _ => Rejection::Unc,
        });
    }
    if is_absolute_input(input, PathDialect::Windows) {
        let drive = input.get(..2).unwrap_or_default().to_owned();
        return Ok((vec![drive], input.get(3..).unwrap_or_default()));
    }
    // 形如 `C:..\..\Windows`：盘符在，但紧跟的不是分隔符。
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(Rejection::DriveRelative);
    }
    // 单分隔符开头是「当前盘的根」，依赖进程工作目录，同样不接受。
    if bytes
        .first()
        .is_some_and(|first| *first == b'\\' || *first == b'/')
    {
        return Err(Rejection::DriveRelative);
    }
    Err(Rejection::NotAbsolute)
}

/// 相对路径的 Windows 形态检查：这些形态一旦拼到根上就不再是「根之内的相对路径」。
fn check_windows_relative_form(relative: &str) -> Result<(), Rejection> {
    if relative.starts_with("\\\\") || relative.starts_with("//") {
        let third = relative.as_bytes().get(2).copied().unwrap_or(b'\0');
        return Err(match third {
            b'?' => Rejection::VerbatimPrefix,
            b'.' => Rejection::DeviceNamespace,
            _ => Rejection::Unc,
        });
    }
    if relative.starts_with('\\') || relative.starts_with('/') {
        return Err(Rejection::DriveRelative);
    }
    let bytes = relative.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(Rejection::DriveRelative);
    }
    if relative.contains(':') {
        return Err(Rejection::DataStream);
    }
    Ok(())
}

/// 把剩余部分逐段并入组件序列，折叠 `.`、回退 `..`，并逐段做形态检查。
fn normalize_into(
    base: &[String],
    rest: &str,
    dialect: PathDialect,
) -> Result<NormalizedPath, Rejection> {
    let mut components: Vec<String> = base.to_vec();
    let floor = base.len();
    let segments: Vec<&str> = rest
        .split(|value: char| dialect.is_separator(value))
        .collect();
    for (index, segment) in segments.iter().enumerate() {
        if segment.is_empty() || *segment == "." {
            continue;
        }
        if *segment == ".." {
            if components.len() <= floor {
                return Err(Rejection::ParentEscape);
            }
            components.pop();
            continue;
        }
        if segment.len() > MAX_COMPONENT_BYTES {
            return Err(Rejection::ComponentTooLong);
        }
        if dialect == PathDialect::Windows {
            check_windows_segment(segment)?;
            if looks_like_short_name(segment) && next_significant(&segments, index) == Some("..") {
                return Err(Rejection::ShortNameFallback);
            }
        }
        components.push((*segment).to_owned());
    }
    Ok(NormalizedPath { components })
}

/// 单个 Windows 路径段的形态检查。
fn check_windows_segment(segment: &str) -> Result<(), Rejection> {
    if segment.contains(':') {
        return Err(Rejection::DataStream);
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return Err(Rejection::TrailingDotOrSpace);
    }
    Ok(())
}

/// 下一个有效段（跳过空段与 `.`），用于识别 `PROGRA~1\..` 这类组合。
fn next_significant<'a>(segments: &[&'a str], index: usize) -> Option<&'a str> {
    segments
        .iter()
        .skip(index + 1)
        .find(|segment| !segment.is_empty() && **segment != ".")
        .copied()
}

/// 判断某个段是否为 8.3 短名形态（`PROGRA~1`、`SECRET~1.TXT`）。
fn looks_like_short_name(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes
        .iter()
        .position(|value| *value == b'~')
        .is_some_and(|index| index > 0 && bytes.get(index + 1).is_some_and(u8::is_ascii_digit))
}

/// 读取查询串里的单个路径参数。
///
/// 路径参数可能带百分号编码（Windows 空格、中文目录名都常见），因此统一走
/// `application/x-www-form-urlencoded` 解码，与其它既有适配器同一实现。
/// 缺省与空值同义，由调用方决定是不是必填。
pub(crate) fn query_parameter(query: Option<&str>, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

#[cfg(test)]
#[path = "server_path_tests.rs"]
mod server_path_tests;
