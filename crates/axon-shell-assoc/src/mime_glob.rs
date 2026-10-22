//! 共享 MIME 数据库（`shared-mime-info`）的 glob 规则解析与匹配。
//!
//! Linux 上 `xdg-mime query filetype` 最终查的就是各数据目录下的 `mime/globs2`（新格式，
//! 带权重）与 `mime/globs`（旧格式）。这里按同样的规则自己匹配，才能在**不新增依赖、
//! 不启动子进程**的前提下拿到与系统一致的 MIME 类型，进而用桌面条目的 `MimeType=` 关联。
//!
//! 本模块只吃字符串，因此可在任意平台单测（见 `tests`）。

/// `mime/globs`（旧格式）没有权重字段，`shared-mime-info` 按中位权重对待。
const GLOBS_LEGACY_WEIGHT: i32 = 50;

/// 一条 glob 规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlobRule {
    /// 规则对应的 MIME 类型。
    pub(crate) mime_type: String,
    /// 匹配**文件名**（不含目录）的 glob 模式。
    pub(crate) pattern: String,
    /// 优先级权重；越大越优先。
    pub(crate) weight: i32,
}

/// 解析 `mime/globs2`：每行 `权重:MIME类型:标志:glob`，`#` 起头是注释。
///
/// 字段数不足、权重不是整数的行直接跳过：数据文件来自系统，局部脏行不应让整张表作废。
pub(crate) fn parse_globs2(content: &str) -> Vec<GlobRule> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // `splitn(4, ..)` 让 glob 里可能出现的 `:` 完整留在第四个字段。
            let mut fields = line.splitn(4, ':');
            let weight = fields.next()?.trim().parse::<i32>().ok()?;
            let mime_type = fields.next()?.trim();
            let _flags = fields.next()?;
            let pattern = fields.next()?.trim();
            (!mime_type.is_empty() && !pattern.is_empty()).then(|| GlobRule {
                mime_type: mime_type.to_owned(),
                pattern: pattern.to_owned(),
                weight,
            })
        })
        .collect()
}

/// 解析旧版 `mime/globs`：每行 `MIME类型:glob[:标志]`，统一按 [`GLOBS_LEGACY_WEIGHT`] 计权。
pub(crate) fn parse_globs(content: &str) -> Vec<GlobRule> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut fields = line.splitn(3, ':');
            let mime_type = fields.next()?.trim();
            let pattern = fields.next()?.trim();
            (!mime_type.is_empty() && !pattern.is_empty()).then(|| GlobRule {
                mime_type: mime_type.to_owned(),
                pattern: pattern.to_owned(),
                weight: GLOBS_LEGACY_WEIGHT,
            })
        })
        .collect()
}

/// 在规则表里找权重最高的匹配；权重相同时保留**先出现**的规则。
///
/// 这正是共享 MIME 数据库的裁决顺序：数据目录按 XDG 优先级依次读入，先出现的赢。
pub(crate) fn resolve_mime_type<'a>(rules: &'a [GlobRule], file_name: &str) -> Option<&'a str> {
    let mut best: Option<&GlobRule> = None;
    for rule in rules {
        if !glob_matches(&rule.pattern, file_name) {
            continue;
        }
        if best.is_none_or(|best| rule.weight > best.weight) {
            best = Some(rule);
        }
    }
    best.map(|rule| rule.mime_type.as_str())
}

/// 判断文件名是否匹配 glob 模式。
///
/// 支持 `*`（任意长度）、`?`（单个字符）、`[...]`（字符集合，含 `[a-z]` 范围与 `[!...]` /
/// `[^...]` 取反）与 `\` 转义。**不做大小写折叠**：数据文件把大小写变体（`*.json` 与
/// `*.JSON`）登记成分开的规则，原样匹配才与系统一致。
pub(crate) fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    matches_from(&pattern, &name, 0, 0)
}

/// 从 `pattern[start..]` 与 `name[from..]` 起匹配。
///
/// `*` 是唯一需要回溯的分支，其余分支每轮至少前进一个字符，因此递归深度只由 `*` 的个数
/// 决定（现实数据里不超过个位数）。
fn matches_from(pattern: &[char], name: &[char], mut start: usize, mut from: usize) -> bool {
    while start < pattern.len() {
        match pattern[start] {
            '*' => {
                // 连续的 `*` 等价于一个。
                while start < pattern.len() && pattern[start] == '*' {
                    start += 1;
                }
                if start == pattern.len() {
                    return true;
                }
                // 回溯：让 `*` 依次吃掉 0..=剩余 个字符。
                return (from..=name.len()).any(|skip| matches_from(pattern, name, start, skip));
            }
            '?' => {
                if from >= name.len() {
                    return false;
                }
                start += 1;
                from += 1;
            }
            '[' => match character_class(pattern, start, name.get(from).copied()) {
                Some((true, next)) => {
                    start = next;
                    from += 1;
                }
                Some((false, _)) => return false,
                None => {
                    // 未闭合的 `[` 按普通字符处理，与 shell 的通配行为一致。
                    if name.get(from) != Some(&'[') {
                        return false;
                    }
                    start += 1;
                    from += 1;
                }
            },
            '\\' if start + 1 < pattern.len() => {
                start += 1;
                if name.get(from) != Some(&pattern[start]) {
                    return false;
                }
                start += 1;
                from += 1;
            }
            literal => {
                if name.get(from) != Some(&literal) {
                    return false;
                }
                start += 1;
                from += 1;
            }
        }
    }
    from == name.len()
}

/// 匹配 `pattern[open]` 处的 `[...]` 字符集合。
///
/// 返回 `Some((是否命中, 集合之后的下标))`；`[...]` 未闭合时返回 `None`。名字已经到末尾时
/// 返回「未命中但集合合法」，由调用方按普通不匹配处理。
fn character_class(
    pattern: &[char],
    open: usize,
    candidate: Option<char>,
) -> Option<(bool, usize)> {
    let mut cursor = open + 1;
    let negated = matches!(pattern.get(cursor), Some('!' | '^'));
    if negated {
        cursor += 1;
    }
    // 集合里的第一个 `]` 是普通字符（POSIX 约定），因此先定位结束位置再逐项判定。
    let body_start = cursor;
    let mut closing = None;
    let mut first = true;
    while cursor < pattern.len() {
        if pattern[cursor] == ']' && !first {
            closing = Some(cursor);
            break;
        }
        first = false;
        cursor += 1;
    }
    let closing = closing?;
    let mut hit = false;
    let mut cursor = body_start;
    while cursor < closing {
        let current = pattern[cursor];
        let range_end = if pattern.get(cursor + 1) == Some(&'-') && cursor + 2 < closing {
            pattern.get(cursor + 2)
        } else {
            None
        };
        match range_end {
            Some(end) => {
                hit |= candidate.is_some_and(|candidate| candidate >= current && candidate <= *end);
                cursor += 3;
            }
            None => {
                hit |= candidate == Some(current);
                cursor += 1;
            }
        }
    }
    // 名字已到末尾时 `hit` 必为 false，取反类也不该命中。
    Some((candidate.is_some() && hit != negated, closing + 1))
}
