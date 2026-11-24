//! XDG 桌面条目与 `mimeapps.list` 的**纯文本解析**（不触碰文件系统）。
//!
//! 与 `linux.rs` 分开是为了可验证性：这些函数只吃字符串，因此能在任意平台（含开发机上的
//! Windows 与 CI）用内联 fixture 直接单测；`linux.rs` 里的目录遍历与环境变量读取才是真正
//! 只能在 Linux 上跑的部分。
//!
//! 参考规范：Desktop Entry Specification 与 MIME Applications Associations Specification。

use std::path::Path;

/// 主组名；只有这个组里的键才是应用定义。
const MAIN_GROUP: &str = "Desktop Entry";

/// `mimeapps.list` 里登记默认打开方式的组名。
const DEFAULTS_GROUP: &str = "Default Applications";

/// 一个 `[Desktop Entry]` 组里本 crate 用到的键。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DesktopEntry {
    /// `Name=` 的取值。**已知缺口**：不处理 `Name[zh_CN]` 这类本地化变体，只认无后缀的键。
    pub(crate) name: Option<String>,
    /// `Icon=` 的取值：图标主题名（`org.gnome.TextEditor`）或绝对路径。
    pub(crate) icon: Option<String>,
    /// `Exec=` 解析后的参数表：字段码已剥离、引号已处理。
    pub(crate) exec: Vec<String>,
    /// `MimeType=` 列出的 MIME 类型。
    pub(crate) mime_types: Vec<String>,
    /// `Type=` 的取值；缺失时按规范默认是 `Application`（由调用方判定）。
    pub(crate) kind: Option<String>,
    /// `Hidden=true`：条目已被删除，必须整体忽略。
    pub(crate) hidden: bool,
}

/// 解析一个 `.desktop` 文件的内容，只取 `[Desktop Entry]` 组。
///
/// 规则按桌面条目规范：`#` 起头是注释，`[组名]` 切换当前组，组内才是 `键=值`。
/// 同一个键重复出现时以**第一次**为准：这样后面的脏行不会覆盖前面已解析出来的值。
pub(crate) fn parse_desktop_entry(content: &str) -> DesktopEntry {
    let mut entry = DesktopEntry::default();
    let mut in_main_group = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_main_group = group == MAIN_GROUP;
            continue;
        }
        if !in_main_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "Name" if entry.name.is_none() => entry.name = Some(value.trim().to_owned()),
            "Icon" if entry.icon.is_none() => entry.icon = Some(value.trim().to_owned()),
            // `Exec` 解析成空表（例如整条命令只有一个 `%f`）时视为还没解析到，允许后面的键补上。
            "Exec" if entry.exec.is_empty() => entry.exec = exec_arguments(value),
            "MimeType" if entry.mime_types.is_empty() => entry.mime_types = split_list(value),
            "Type" if entry.kind.is_none() => entry.kind = Some(value.trim().to_owned()),
            "Hidden" => entry.hidden |= is_true(value),
            _ => {}
        }
    }
    entry
}

/// 从 `mimeapps.list` 内容里取出 `[Default Applications]`，展平成 (MIME 类型, 桌面条目 id) 序列。
///
/// 规范允许一个 MIME 类型后面跟一串候选（`text/plain=a.desktop;b.desktop;`）：第一个是默认项，
/// 其余是「默认项不可用时」的替补。这里保留原顺序并展平，由调用方取「第一个确实存在的候选」
/// 作为默认项：这正是 `xdg-mime query default` 在缺少 `mimeinfo.cache` 时的等价规则。
/// MIME 类型按 RFC 折叠成小写后再比较。
pub(crate) fn parse_mimeapps_defaults(content: &str) -> Vec<(String, String)> {
    let mut defaults = Vec::new();
    let mut in_defaults = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_defaults = group == DEFAULTS_GROUP;
            continue;
        }
        if !in_defaults {
            continue;
        }
        let Some((mime_type, ids)) = line.split_once('=') else {
            continue;
        };
        let mime_type = mime_type.trim().to_ascii_lowercase();
        if mime_type.is_empty() {
            continue;
        }
        for id in split_list(ids) {
            defaults.push((mime_type.clone(), id));
        }
    }
    defaults
}

/// `Name=` 缺失时的回退展示名：`Exec` 里程序名的文件名部分，再退不到就用桌面条目 id。
///
/// 与 Windows 侧「`GetUIName` 取不到就回退可执行文件名」同一口径，保证任何平台上的候选
/// 都有一个可展示的名字。
pub(crate) fn display_name_from_exec(exec: &[String], id: &str) -> String {
    exec.first()
        .map(String::as_str)
        .filter(|program| !program.is_empty())
        .and_then(|program| Path::new(program).file_name())
        .map_or_else(|| id.to_owned(), |name| name.to_string_lossy().into_owned())
}

/// 解析 `Exec=` 的命令行：先按规范切词，再剥离字段码。
pub(crate) fn exec_arguments(value: &str) -> Vec<String> {
    split_exec_words(value)
        .iter()
        .filter_map(|word| strip_field_codes(word))
        .collect()
}

/// 按桌面条目规范切分 `Exec=` 的单词。
///
/// 规范只定义了一种引号：双引号内的空格与制表符属于同一个参数；引号内的 `\` 是转义符。
/// **单引号不是**桌面条目的引号（这点与 shell 不同），按普通字符处理；引号外的 `\` 同样是
/// 普通字符。引号本身不进入结果，`""` 产生一个空参数。
fn split_exec_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => {
                if let Some(escaped) = characters.next() {
                    current.push(escaped);
                }
                started = true;
            }
            character if character.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            character => {
                current.push(character);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

/// 剥离一个参数里的字段码；剥完什么都不剩时返回 `None`（该参数整体消失）。
///
/// 规范里的字段码分两类，本函数一律**整段丢弃**：
/// - 占位符（`%f` `%F` `%u` `%U` `%i` `%c` `%k`）：由启动器在调用时替换成本次打开的文件、
///   图标或应用名。本 crate 只负责枚举候选、不负责启动进程，因此没有东西可替换。
/// - 已废弃的码（`%d` `%D` `%n` `%N` `%v` `%m`）：规范要求忽略。
///
/// `%%` 是字面量百分号，还原成单个 `%`；字符串末尾孤立的 `%` 同样按字面量 `%` 处理。
fn strip_field_codes(argument: &str) -> Option<String> {
    let mut stripped = String::with_capacity(argument.len());
    let mut characters = argument.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            stripped.push(character);
            continue;
        }
        match characters.next() {
            Some('%') | None => stripped.push('%'),
            Some(_) => {}
        }
    }
    (!stripped.is_empty()).then_some(stripped)
}

/// 拆开分号分隔的列表（`MimeType=a;b;` 允许结尾分号，空项直接丢掉）。
fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 判断键值是否为布尔真；规范要求写成小写 `true`，这里放宽到不区分大小写。
fn is_true(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true")
}
