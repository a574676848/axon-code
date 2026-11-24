//! Hashline 精确编辑核心。
//!
//! 本模块只做会话快照门禁、stale anchor 恢复和批量编辑计算，
//! 文本解析与语法块解析分别位于独立模块，不接触文件系统、审批或 UI。

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

use crate::WorkspaceFileSnapshot;

#[path = "hashline_edit_block.rs"]
mod hashline_edit_block;
#[path = "hashline_edit_parser.rs"]
mod hashline_edit_parser;
#[path = "hashline_edit_recovery.rs"]
mod hashline_edit_recovery;
#[path = "hashline_edit_semantics.rs"]
mod hashline_edit_semantics;

use hashline_edit_block::resolve_block_ops;
use hashline_edit_parser::{HashlineSection, parse_sections};
use hashline_edit_recovery::recover_ops;
use hashline_edit_semantics::normalize_edit_semantics;

/// Hashline 快照标签长度。
///
/// 6 位十六进制 → 空间 16 777 216。这是「标签空间充足」与「模型输出 token 成本」
/// 的平衡点：再翻倍到 8 位收益递减，而恢复路径的歧义由调用方的内容比对兜底，
/// 不再单纯依赖位数。
pub const HASHLINE_TAG_LEN: usize = 6;

/// 旧格式（位数提升前）的 Hashline 快照标签长度。
///
/// **冻结常量**：仅用于识别过期快照并给出可执行的迁移提示，
/// **不参与任何宽松解析**：该长度的标签一律拒绝（见 `INV-5`）。
const HASHLINE_LEGACY_TAG_LEN: usize = 4;

/// 自动修复类 warning 的统一前缀。
///
/// 边界修复会**改变模型提交的原文**。若它与普通 warning 混在一起，
/// 模型可能以为文件是按原文改动的：这正是「静默修复」风险。
/// 回执层据此前缀识别修复并追加复核指引，因此两侧必须共用同一常量。
pub const HASHLINE_REPAIR_WARNING_PREFIX: &str = "[修复] ";
/// Hashline 缺少快照标签时给模型的稳定提示。
pub const HASHLINE_MISSING_TAG_HINT: &str = "缺少 Hashline 快照标签；请使用最新 read/search 输出里的 [path#TAG]。创建文件时请在同一个 edit 中提交完整 Codex patch。";

/// 编辑未产生任何内容变化（字节级空改动）时的稳定失败标记。
///
/// 回执构造方（CLI 执行器）与工具循环守卫共用同一常量：守卫据此把「重复提交空改动」
/// 从普通工具失败中分离出来，给出针对性的止损指引；两侧若各自硬编码文案会失配。
pub const HASHLINE_NOOP_CHANGE_MARKER: &str = "[builtin.edit 执行失败: NOOP_CHANGE]";

/// 旧格式快照标签的迁移提示。
///
/// 优于泛化的「标签无效」文案：模型能据此直接执行正确动作（重新读取），
/// 而不必反复猜测位数。位数一律取自常量，避免文案与实际校验脱节。
fn hashline_legacy_tag_hint() -> String {
    format!(
        "Hashline 快照标签为旧格式（{HASHLINE_LEGACY_TAG_LEN} 位）；\
         请重新调用 builtin.read_file 获取 {HASHLINE_TAG_LEN} 位新标签"
    )
}

/// 多文件 Hashline 应用请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlineApplyRequest {
    /// Hashline 多文件输入文本。
    pub input: String,
    /// 输入中每个文件对应的当前内容与会话快照。
    pub files: Vec<HashlineApplyFile>,
}

/// 单个 Hashline 文件上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlineApplyFile {
    /// 工具路径。
    pub path: String,
    /// 应用前的当前文件内容。
    pub current_content: String,
    /// 模型读取时保存的完整快照。
    pub snapshot: WorkspaceFileSnapshot,
}

/// Hashline 文件级操作。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HashlineFileOperation {
    /// 原路径原地更新。
    #[default]
    Update,
    /// 删除原文件。
    Delete,
    /// 将最终内容移动到目标路径。
    Move {
        /// 移动目标路径。
        destination: String,
    },
}

/// Hashline BLK 操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashlineBlockOperation {
    /// 替换语法块。
    Replace,
    /// 删除语法块。
    Delete,
    /// 插入到语法块之后。
    InsertAfter,
}

impl HashlineBlockOperation {
    /// 返回稳定回执名称。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Delete => "delete",
            Self::InsertAfter => "insert_after",
        }
    }
}

/// 一个 BLK anchor 解析出的真实 1-based 行范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlineBlockResolution {
    /// 原始 BLK anchor 行。
    pub anchor_line: usize,
    /// 解析后的首行。
    pub start: usize,
    /// 解析后的末行。
    pub end: usize,
    /// BLK 操作类型。
    pub operation: HashlineBlockOperation,
}

/// 多文件 Hashline 预检结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlineApplyReport {
    /// 全部文件的计算结果。
    pub files: Vec<HashlineFileApplyReport>,
}

/// 单文件 Hashline 计算结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlineFileApplyReport {
    /// 工具源路径。
    pub path: String,
    /// 文件级操作。
    pub operation: HashlineFileOperation,
    /// 应用前内容。
    pub previous_content: String,
    /// 应用后的最终内容；删除时保留删除前内容用于回执。
    pub next_content: String,
    /// 是否产生实际文件系统变化。
    pub changed: bool,
    /// 解析、恢复或应用阶段产生的警告。
    pub warnings: Vec<String>,
    /// 未发生 stale recovery 时可回显的 BLK 解析范围。
    pub block_resolutions: Vec<HashlineBlockResolution>,
    /// 写后仍可证明模型已观察的 1-based 行号。
    pub seen_lines: BTreeSet<usize>,
}

impl HashlineFileApplyReport {
    /// 返回提交后的目标路径。
    #[must_use]
    pub fn result_path(&self) -> &str {
        match &self.operation {
            HashlineFileOperation::Move { destination } => destination,
            HashlineFileOperation::Update | HashlineFileOperation::Delete => &self.path,
        }
    }
}

/// Hashline header 的公开只读投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashlineHeader {
    /// 工具路径。
    pub path: String,
    /// 会话快照标签。
    pub tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HashlineOp {
    Swap {
        start: usize,
        end: usize,
        lines: Vec<String>,
    },
    Delete {
        start: usize,
        end: usize,
    },
    InsertBefore {
        line: usize,
        lines: Vec<String>,
    },
    InsertAfter {
        line: usize,
        lines: Vec<String>,
        block_start: Option<usize>,
    },
    InsertHead {
        lines: Vec<String>,
    },
    InsertTail {
        lines: Vec<String>,
    },
    SwapBlock {
        line: usize,
        lines: Vec<String>,
    },
    DeleteBlock {
        line: usize,
    },
    InsertAfterBlock {
        line: usize,
        lines: Vec<String>,
    },
}

/// 计算 Hashline 快照标签。
#[must_use]
pub fn hashline_snapshot_tag(content: &str) -> String {
    axon_crypto::sha256_hex_prefix(content.as_bytes(), HASHLINE_TAG_LEN).to_ascii_uppercase()
}

/// 解析输入中所有文件 header。
pub fn hashline_headers(input: &str) -> Result<Vec<HashlineHeader>, String> {
    parse_sections(input).map(|sections| {
        sections
            .into_iter()
            .map(|section| HashlineHeader {
                path: section.path,
                tag: section.tag,
            })
            .collect()
    })
}

/// 提取 Hashline 将写入的全部源路径和移动目标路径。
pub fn hashline_write_paths(input: &str) -> Result<Vec<String>, String> {
    let mut paths = BTreeSet::new();
    for section in parse_sections(input)? {
        paths.insert(section.path);
        if let HashlineFileOperation::Move { destination } = section.file_operation {
            paths.insert(destination);
        }
    }
    Ok(paths.into_iter().collect())
}

#[path = "hashline_edit_preview.rs"]
mod hashline_edit_preview;

pub use hashline_edit_preview::{
    HashlinePreviewFile, HashlinePreviewOperation, hashline_preview_files,
};

/// 对全部文件执行无副作用预检和编辑计算。
pub fn apply_hashline_edit(request: HashlineApplyRequest) -> Result<HashlineApplyReport, String> {
    let sections = parse_sections(&request.input)?;
    validate_file_operations(&sections)?;
    let mut files = request
        .files
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect::<HashMap<_, _>>();
    if files.len() != sections.len() {
        return Err("edit 的文件上下文与 Hashline section 数量不一致".to_owned());
    }
    let mut reports = Vec::with_capacity(sections.len());
    for section in sections {
        let file = files
            .remove(&section.path)
            .ok_or_else(|| format!("edit 缺少 {} 的文件上下文", section.path))?;
        reports.push(apply_section(section, file)?);
    }
    if !files.is_empty() {
        return Err("edit 收到输入中不存在的额外文件上下文".to_owned());
    }
    Ok(HashlineApplyReport { files: reports })
}

fn validate_file_operations(sections: &[HashlineSection]) -> Result<(), String> {
    let source_paths = sections
        .iter()
        .map(|section| section.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut move_destinations = BTreeSet::new();
    for section in sections {
        let HashlineFileOperation::Move { destination } = &section.file_operation else {
            continue;
        };
        if destination == &section.path {
            return Err(format!("edit 的 MV 目标与源路径相同：{}", section.path));
        }
        if !move_destinations.insert(destination.as_str()) {
            return Err(format!("edit 的多个 MV 使用了同一目标路径：{destination}"));
        }
        if source_paths.contains(destination.as_str()) {
            return Err(format!(
                "edit 的 MV 目标同时是同批次源路径：{destination}；请拆分为独立调用"
            ));
        }
    }
    Ok(())
}

fn apply_section(
    section: HashlineSection,
    file: HashlineApplyFile,
) -> Result<HashlineFileApplyReport, String> {
    if !section.tag.eq_ignore_ascii_case(file.snapshot.tag()) {
        return Err(format!(
            "Hashline 快照标签 {} 不属于当前会话内 {} 的可用版本；请重新读取文件",
            section.tag, section.path
        ));
    }
    let expected = HashlineText::parse(file.snapshot.content());
    let current = HashlineText::parse(&file.current_content);
    let recovered = file.current_content != file.snapshot.content();
    let (resolved_ops, block_resolutions, mut warnings) =
        resolve_block_ops(&section.path, file.snapshot.content(), section.ops)?;
    warnings.extend(section.warnings);
    ensure_seen_lines(&section.path, &resolved_ops, file.snapshot.seen_lines())?;
    let ops = if recovered {
        recover_ops(&expected.lines, &current.lines, &resolved_ops)?
    } else {
        resolved_ops
    };
    let (mut ops, semantic_warnings) = normalize_edit_semantics(&current.lines, ops)?;
    warnings.extend(semantic_warnings);
    let mut seen_lines = if recovered {
        ops.iter()
            .flat_map(HashlineOp::anchor_lines)
            .collect::<BTreeSet<_>>()
    } else {
        file.snapshot.seen_lines().clone()
    };
    ensure_non_overlapping(&ops)?;
    sort_ops_for_application(&mut ops);
    let mut lines = current.lines.clone();
    for op in ops {
        apply_op(&mut lines, &mut seen_lines, op)?;
    }
    let next_content = current.render(&lines);
    if recovered {
        warnings.push(
            "文件在读取后发生变化；已将全部 Hashline anchor 映射到未改变的当前内容".to_owned(),
        );
    }
    let operation = section.file_operation;
    let changed =
        next_content != file.current_content || !matches!(operation, HashlineFileOperation::Update);
    // 解析回归观测：编辑把「可解析」变成「不可解析」时给出定向告警（不阻断写入、
    // 不自动改写文件；修复决策留给上层与模型）。
    if changed
        && let Some(warning) = crate::edit_repair::parse_regression_warning(
            &file.path,
            &file.current_content,
            &next_content,
        )
    {
        warnings.push(warning);
    }
    Ok(HashlineFileApplyReport {
        path: file.path,
        operation,
        previous_content: file.current_content,
        next_content,
        changed,
        warnings,
        block_resolutions: if recovered {
            Vec::new()
        } else {
            block_resolutions
        },
        seen_lines,
    })
}

fn ensure_seen_lines(path: &str, ops: &[HashlineOp], seen: &BTreeSet<usize>) -> Result<(), String> {
    let missing = ops
        .iter()
        .flat_map(HashlineOp::anchor_lines)
        .filter(|line| !seen.contains(line))
        .collect::<BTreeSet<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    let lines = missing
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Err(format!(
        "Hashline 拒绝编辑模型未完整读取的行：{path} 行 {lines}；请先分页读取这些行"
    ))
}

fn ensure_non_overlapping(ops: &[HashlineOp]) -> Result<(), String> {
    let mut occupied = BTreeSet::new();
    let mut ranges = Vec::new();
    for op in ops {
        let (start, end) = match op {
            HashlineOp::Swap { start, end, .. } | HashlineOp::Delete { start, end } => {
                (*start, *end)
            }
            _ => continue,
        };
        for line in start..=end {
            if !occupied.insert(line) {
                return Err(format!(
                    "Hashline 同一批次存在重叠 range：第 {line} 行；请合并为一个操作"
                ));
            }
        }
        ranges.push((start, end));
    }
    for op in ops {
        let invalid = match op {
            HashlineOp::InsertBefore { line, .. } => ranges
                .iter()
                .any(|(start, end)| (*start..=*end).contains(line) && line != start),
            HashlineOp::InsertAfter { line, .. } => ranges
                .iter()
                .any(|(start, end)| (*start..=*end).contains(line) && line != end),
            _ => false,
        };
        if invalid {
            return Err(format!(
                "Hashline 插入 anchor 位于 replacement range 内部：第 {} 行；请锚定 range 边界",
                op.anchor_position()
            ));
        }
    }
    Ok(())
}

fn sort_ops_for_application(ops: &mut Vec<HashlineOp>) {
    let mut indexed = std::mem::take(ops)
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    indexed.sort_by(|(left_index, left), (right_index, right)| {
        std::cmp::Reverse(left.anchor_position())
            .cmp(&std::cmp::Reverse(right.anchor_position()))
            .then_with(|| {
                left.same_anchor_priority()
                    .cmp(&right.same_anchor_priority())
            })
            .then_with(|| match (left, right) {
                (HashlineOp::InsertTail { .. }, HashlineOp::InsertTail { .. }) => {
                    left_index.cmp(right_index)
                }
                _ => right_index.cmp(left_index),
            })
    });
    ops.extend(indexed.into_iter().map(|(_, op)| op));
}

impl HashlineOp {
    fn same_anchor_priority(&self) -> SameAnchorOrder {
        match self {
            Self::InsertAfter { .. } => SameAnchorOrder::InsertAfter,
            Self::Swap { .. } | Self::Delete { .. } => SameAnchorOrder::Destructive,
            Self::InsertBefore { .. } => SameAnchorOrder::InsertBefore,
            Self::InsertHead { .. } | Self::InsertTail { .. } => SameAnchorOrder::Unanchored,
            Self::SwapBlock { .. } | Self::DeleteBlock { .. } | Self::InsertAfterBlock { .. } => {
                SameAnchorOrder::UnresolvedBlock
            }
        }
    }

    fn is_block(&self) -> bool {
        matches!(
            self,
            Self::SwapBlock { .. } | Self::DeleteBlock { .. } | Self::InsertAfterBlock { .. }
        )
    }

    fn anchor_lines(&self) -> Vec<usize> {
        match self {
            Self::Swap { start, end, .. } | Self::Delete { start, end } => {
                (*start..=*end).collect()
            }
            Self::InsertBefore { line, .. }
            | Self::InsertAfter { line, .. }
            | Self::SwapBlock { line, .. }
            | Self::DeleteBlock { line }
            | Self::InsertAfterBlock { line, .. } => vec![*line],
            Self::InsertHead { .. } | Self::InsertTail { .. } => Vec::new(),
        }
    }

    fn anchor_position(&self) -> usize {
        match self {
            Self::Swap { start, .. } | Self::Delete { start, .. } => *start,
            Self::InsertBefore { line, .. }
            | Self::InsertAfter { line, .. }
            | Self::SwapBlock { line, .. }
            | Self::DeleteBlock { line }
            | Self::InsertAfterBlock { line, .. } => *line,
            Self::InsertHead { .. } => 0,
            Self::InsertTail { .. } => usize::MAX,
        }
    }
}

fn split_hashline_lines(content: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    content
        .strip_suffix('\n')
        .unwrap_or(content)
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HashlineText {
    lines: Vec<String>,
    line_ending: &'static str,
    trailing_newline: bool,
    bom: bool,
}

impl HashlineText {
    fn parse(content: &str) -> Self {
        let (bom, text) = content
            .strip_prefix('\u{feff}')
            .map_or((false, content), |text| (true, text));
        Self {
            lines: split_hashline_lines(text),
            line_ending: if text.contains("\r\n") { "\r\n" } else { "\n" },
            trailing_newline: text.ends_with('\n'),
            bom,
        }
    }

    fn render(&self, lines: &[String]) -> String {
        let mut content = lines.join(self.line_ending);
        if self.trailing_newline {
            content.push_str(self.line_ending);
        }
        if self.bom {
            content.insert(0, '\u{feff}');
        }
        content
    }
}

fn apply_op(
    lines: &mut Vec<String>,
    seen_lines: &mut BTreeSet<usize>,
    op: HashlineOp,
) -> Result<(), String> {
    match op {
        HashlineOp::Swap {
            start,
            end,
            lines: new_lines,
        } => {
            validate_range(lines, start, end)?;
            let new_len = new_lines.len();
            lines.splice(start - 1..end, new_lines);
            transform_seen_lines(seen_lines, start, end, new_len, true);
        }
        HashlineOp::Delete { start, end } => {
            validate_range(lines, start, end)?;
            lines.drain(start - 1..end);
            transform_seen_lines(seen_lines, start, end, 0, false);
        }
        HashlineOp::InsertBefore {
            line,
            lines: new_lines,
        } => {
            validate_anchor(lines, line)?;
            let new_len = new_lines.len();
            lines.splice(line - 1..line - 1, new_lines);
            transform_seen_lines(seen_lines, line, line - 1, new_len, true);
        }
        HashlineOp::InsertAfter {
            line,
            lines: new_lines,
            ..
        } => {
            validate_anchor(lines, line)?;
            let new_len = new_lines.len();
            lines.splice(line..line, new_lines);
            transform_seen_lines(seen_lines, line + 1, line, new_len, true);
        }
        HashlineOp::InsertHead { lines: new_lines } => {
            let new_len = new_lines.len();
            lines.splice(0..0, new_lines);
            transform_seen_lines(seen_lines, 1, 0, new_len, true);
        }
        HashlineOp::InsertTail { lines: new_lines } => {
            let start = lines.len() + 1;
            let new_len = new_lines.len();
            lines.extend(new_lines);
            transform_seen_lines(seen_lines, start, start - 1, new_len, true);
        }
        HashlineOp::SwapBlock { .. }
        | HashlineOp::DeleteBlock { .. }
        | HashlineOp::InsertAfterBlock { .. } => {
            return Err("Hashline BLK 操作在应用前未完成解析".to_owned());
        }
    }
    Ok(())
}

fn transform_seen_lines(
    seen_lines: &mut BTreeSet<usize>,
    start: usize,
    end: usize,
    replacement_len: usize,
    mark_replacement_seen: bool,
) {
    let removed_len = end
        .saturating_sub(start)
        .saturating_add(usize::from(end >= start));
    let delta = isize::try_from(replacement_len).unwrap_or(isize::MAX)
        - isize::try_from(removed_len).unwrap_or(isize::MAX);
    let mut transformed = BTreeSet::new();
    for line in seen_lines.iter().copied() {
        if line < start {
            transformed.insert(line);
        } else if line > end {
            transformed.insert(line.saturating_add_signed(delta));
        }
    }
    if mark_replacement_seen {
        transformed.extend(start..start.saturating_add(replacement_len));
    }
    *seen_lines = transformed;
}

fn validate_range(lines: &[String], start: usize, end: usize) -> Result<(), String> {
    validate_anchor(lines, start)?;
    validate_anchor(lines, end)
}

fn validate_anchor(lines: &[String], line: usize) -> Result<(), String> {
    if line == 0 || line > lines.len() {
        return Err(format!(
            "Hashline 行号 {line} 越界，当前文件共 {} 行",
            lines.len()
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SameAnchorOrder {
    InsertAfter,
    Destructive,
    InsertBefore,
    Unanchored,
    UnresolvedBlock,
}

#[cfg(test)]
#[path = "hashline_edit_tests.rs"]
mod tests;
