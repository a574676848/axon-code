//! Hashline 应用前的边界修复与插入落点归一化。

use std::collections::BTreeSet;

#[path = "hashline_edit_jsx.rs"]
mod hashline_edit_jsx;
#[path = "hashline_edit_projection.rs"]
mod hashline_edit_projection;

use super::{HASHLINE_REPAIR_WARNING_PREFIX, HashlineOp};
use hashline_edit_jsx::{is_jsx_closer_line, payload_has_opener_for_echo};
use hashline_edit_projection::repair_missing_closers;

/// 修复类 warning 的前缀，供本模块与投影模块共用。
use HASHLINE_REPAIR_WARNING_PREFIX as REPAIR_WARNING_PREFIX;

/// 对 OMP 已验证的常见模型偏差做有证据的修复；不确定时保持字面语义。
pub(super) fn normalize_edit_semantics(
    source: &[String],
    mut ops: Vec<HashlineOp>,
) -> Result<(Vec<HashlineOp>, Vec<String>), String> {
    let mut warnings = repair_replacement_boundaries(source, &mut ops)?;
    warnings.extend(repair_after_insert_landings(source, &mut ops));
    Ok((ops, warnings))
}

fn repair_replacement_boundaries(
    source: &[String],
    ops: &mut Vec<HashlineOp>,
) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    for op in ops.iter_mut() {
        repair_local_boundary(source, op, &mut warnings)?;
    }
    let (repaired, projection_warnings) = repair_missing_closers(source, std::mem::take(ops))?;
    *ops = repaired;
    warnings.extend(projection_warnings);
    Ok(warnings)
}

/// 边界修复规则标识。
///
/// 每种对应一条既有 warning 文案，并用于优先级守护测试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryRepairKind {
    /// 载荷同时重复了范围外的前缀与后缀。
    BothSidesEcho,
    /// 平衡差为零时的单侧边界回声。
    ZeroDeltaOneSidedEcho,
    /// 后缀恰好解释平衡差。
    BalancesDuplicateSuffix,
    /// 前缀恰好解释平衡差。
    BalancesDuplicatePrefix,
}

impl BoundaryRepairKind {
    /// 判重用的稳定编码；`PartialEq` 在 `const` 上下文不可调用，故改用整数比较。
    const fn code(self) -> u8 {
        match self {
            Self::BothSidesEcho => 0,
            Self::ZeroDeltaOneSidedEcho => 1,
            Self::BalancesDuplicateSuffix => 2,
            Self::BalancesDuplicatePrefix => 3,
        }
    }
}

/// 单条规则的判定结果。
///
/// 三态是必需的：原判据链中「双侧回声」的判据不成立时会**继续**尝试后续规则，
/// 而「单侧回声」一旦接管就**无论是否产出修复都终止**链。
/// 二者无法用「首个匹配者独占」这一种语义同时表达。
enum RuleOutcome {
    /// 产出修复；链终止。
    Repaired(BoundaryRepair),
    /// 本条规则接管但证据不足：保持字面语义，链终止。
    KeepLiteral,
    /// 本条规则不适用：交给下一条规则。
    NotApplicable,
}

/// 一次边界修复的动作。
///
/// 五种修复最终都只做「削头」与「截尾」两件事；用两个计数统一表达，
/// 由 [`apply_boundary_repair`] 统一应用，避免每条规则各自操作载荷。
struct BoundaryRepair {
    /// 从载荷头部丢弃的行数。
    drop_prefix: usize,
    /// 从载荷尾部丢弃的行数。
    drop_suffix: usize,
    /// 写入 warnings 的说明文案。
    warning: String,
}

/// 修复规则的只读判据上下文。
struct RepairContext<'a> {
    source: &'a [String],
    start: usize,
    end: usize,
    /// 当前载荷；规则只读取，动作以「丢弃计数」表达。
    lines: &'a [String],
    leading: usize,
    trailing: usize,
    delta: DelimiterBalance,
}

/// 一条边界修复规则。
struct BoundaryRepairRule {
    /// 规则标识：用于编译期判重与优先级守护测试，保证重构不会悄悄改变顺序。
    kind: BoundaryRepairKind,
    /// 判据与动作；`Err` 表示证据冲突、必须让模型修正输入。
    evaluate: fn(&RepairContext<'_>) -> Result<RuleOutcome, String>,
}

/// 边界修复规则表。
///
/// **数组顺序即优先级**，与原 `if/else` 判据链逐位对应。改动顺序会改变修复语义，
/// 由 `boundary_repair_rule_priority_is_locked` 守护。
const BOUNDARY_REPAIR_RULES: &[BoundaryRepairRule] = &[
    BoundaryRepairRule {
        kind: BoundaryRepairKind::BothSidesEcho,
        evaluate: evaluate_both_sides_echo,
    },
    BoundaryRepairRule {
        kind: BoundaryRepairKind::ZeroDeltaOneSidedEcho,
        evaluate: evaluate_zero_delta_one_sided_echo,
    },
    BoundaryRepairRule {
        kind: BoundaryRepairKind::BalancesDuplicateSuffix,
        evaluate: evaluate_balances_duplicate_suffix,
    },
    BoundaryRepairRule {
        kind: BoundaryRepairKind::BalancesDuplicatePrefix,
        evaluate: evaluate_balances_duplicate_prefix,
    },
];

/// 编译期守护：规则标识两两不同，避免重构把两条规则折叠成同一个判据。
///
/// 放在 `const` 上下文而非只交给测试断言有两个好处：判据复用会在**编译期**就失败；
/// `kind` 在任何构建配置下都被真实读取，因此不需要任何编译期 lint 抑制。
const _: () = {
    let mut i = 0;
    while i < BOUNDARY_REPAIR_RULES.len() {
        let mut j = i + 1;
        while j < BOUNDARY_REPAIR_RULES.len() {
            assert!(
                BOUNDARY_REPAIR_RULES[i].kind.code() != BOUNDARY_REPAIR_RULES[j].kind.code(),
                "规则表中不得出现重复的规则标识"
            );
            j += 1;
        }
        i += 1;
    }
};

fn repair_local_boundary(
    source: &[String],
    op: &mut HashlineOp,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let HashlineOp::Swap { start, end, lines } = op else {
        return Ok(());
    };
    if !valid_range(source, *start, *end) {
        return Ok(());
    }

    let context = RepairContext {
        source,
        start: *start,
        end: *end,
        lines,
        leading: duplicate_leading_boundary(source, *start, lines),
        trailing: duplicate_trailing_boundary(source, *end, lines),
        delta: balance_delta(
            delimiter_balance(lines),
            delimiter_balance(&source[*start - 1..*end]),
        ),
    };

    // 先求值收集结果，循环外再改载荷：规则持有 `lines` 的只读借用，
    // 若在循环内可变借用会与上下文冲突。
    let mut outcome = RuleOutcome::KeepLiteral;
    for rule in BOUNDARY_REPAIR_RULES {
        match (rule.evaluate)(&context)? {
            RuleOutcome::NotApplicable => {}
            decided => {
                outcome = decided;
                break;
            }
        }
    }

    if let RuleOutcome::Repaired(repair) = outcome {
        apply_boundary_repair(lines, &repair);
        warnings.push(repair.warning);
    }
    Ok(())
}

/// 统一应用修复：先截尾再削头，与原判据链的变更顺序一致。
fn apply_boundary_repair(lines: &mut Vec<String>, repair: &BoundaryRepair) {
    if repair.drop_suffix > 0 {
        lines.truncate(lines.len() - repair.drop_suffix);
    }
    if repair.drop_prefix > 0 {
        lines.drain(..repair.drop_prefix);
    }
}

/// 双侧边界回声：载荷同时重复了范围外的前缀与后缀，且被丢弃部分的平衡可解释。
fn evaluate_both_sides_echo(context: &RepairContext<'_>) -> Result<RuleOutcome, String> {
    let (leading, trailing) = (context.leading, context.trailing);
    let lines = context.lines;
    if leading == 0 || trailing == 0 || leading + trailing >= lines.len() {
        return Ok(RuleOutcome::NotApplicable);
    }
    let dropped_balance = balance_sum(
        delimiter_balance(&lines[..leading]),
        delimiter_balance(&lines[lines.len() - trailing..]),
    );
    if !balance_is_zero(dropped_balance) && dropped_balance != context.delta {
        // 判据不足：不接管，交给后续规则。
        return Ok(RuleOutcome::NotApplicable);
    }
    Ok(RuleOutcome::Repaired(BoundaryRepair {
        drop_prefix: leading,
        drop_suffix: trailing,
        warning: format!(
            "{REPAIR_WARNING_PREFIX}已修复 SWAP {}.={} 的双侧边界回声：移除范围外已存在的 {leading} 行前缀和 {trailing} 行后缀",
            context.start, context.end
        ),
    }))
}

/// 平衡差为零时的单侧边界回声。
fn evaluate_zero_delta_one_sided_echo(context: &RepairContext<'_>) -> Result<RuleOutcome, String> {
    if !balance_is_zero(context.delta) {
        return Ok(RuleOutcome::NotApplicable);
    }
    // 本规则一旦接管，无论是否产出修复都终止链（与原判据链一致）。
    one_sided_echo_repair(context).map(|repair| match repair {
        Some(repair) => RuleOutcome::Repaired(repair),
        None => RuleOutcome::KeepLiteral,
    })
}

/// 单侧边界回声的判据与动作；`None` 表示证据不足、保持字面语义。
fn one_sided_echo_repair(context: &RepairContext<'_>) -> Result<Option<BoundaryRepair>, String> {
    let (start, end) = (context.start, context.end);
    let (leading, trailing) = (context.leading, context.trailing);
    let lines = context.lines;

    if (leading > 0) == (trailing > 0) {
        return Ok(None);
    }
    let (side, count) = if leading > 0 {
        ("前缀", leading)
    } else {
        ("后缀", trailing)
    };
    if count >= lines.len() {
        return Ok(None);
    }
    let echo = if leading > 0 {
        &lines[..count]
    } else {
        &lines[lines.len() - count..]
    };
    if !balance_is_zero(delimiter_balance(echo)) {
        return Ok(None);
    }

    let range_len = end - start + 1;
    if range_len == 1 {
        if leading > 0 || !echo.iter().all(|line| is_structural_closer(line)) {
            return Ok(None);
        }
        let payload_prefix = &lines[..lines.len() - count];
        if payload_has_opener_for_echo(payload_prefix, echo) {
            return Ok(None);
        }
    }
    if lines.len() < range_len + count {
        return Err(format!(
            "SWAP {start}.={end} 的 body 以范围外相同{side}开头或结尾，但不足以覆盖扩展后的范围；请修正范围后重试"
        ));
    }
    Ok(Some(BoundaryRepair {
        drop_prefix: if leading > 0 { count } else { 0 },
        drop_suffix: if leading > 0 { 0 } else { count },
        warning: format!(
            "{REPAIR_WARNING_PREFIX}已修复 SWAP {start}.={end} 的单侧边界回声：移除范围外已存在的 {count} 行{side}"
        ),
    }))
}

/// 后缀恰好解释平衡差：移除下方已存在的重复后缀。
fn evaluate_balances_duplicate_suffix(context: &RepairContext<'_>) -> Result<RuleOutcome, String> {
    let duplicate =
        balance_duplicate_suffix(context.source, context.end, context.lines, context.delta);
    if duplicate == 0 {
        return Ok(RuleOutcome::NotApplicable);
    }
    Ok(RuleOutcome::Repaired(BoundaryRepair {
        drop_prefix: 0,
        drop_suffix: duplicate,
        warning: format!(
            "{REPAIR_WARNING_PREFIX}已修复 SWAP {}.={} 的闭合边界：移除下方已存在且恰好解释平衡差的 {duplicate} 行后缀",
            context.start, context.end
        ),
    }))
}

/// 前缀恰好解释平衡差：移除上方已存在的重复前缀。
fn evaluate_balances_duplicate_prefix(context: &RepairContext<'_>) -> Result<RuleOutcome, String> {
    let duplicate =
        balance_duplicate_prefix(context.source, context.start, context.lines, context.delta);
    if duplicate == 0 {
        return Ok(RuleOutcome::NotApplicable);
    }
    Ok(RuleOutcome::Repaired(BoundaryRepair {
        drop_prefix: duplicate,
        drop_suffix: 0,
        warning: format!(
            "{REPAIR_WARNING_PREFIX}已修复 SWAP {}.={} 的起始边界：移除上方已存在且恰好解释平衡差的 {duplicate} 行前缀",
            context.start, context.end
        ),
    }))
}

fn valid_range(source: &[String], start: usize, end: usize) -> bool {
    start > 0 && end >= start && end <= source.len()
}

fn duplicate_leading_boundary(source: &[String], start: usize, payload: &[String]) -> usize {
    let max = payload.len().min(start.saturating_sub(1));
    (1..=max)
        .rev()
        .find(|count| {
            let candidate = &payload[..*count];
            candidate == &source[start - 1 - *count..start - 1]
                && candidate.iter().any(|line| !line.trim().is_empty())
        })
        .unwrap_or(0)
}

fn duplicate_trailing_boundary(source: &[String], end: usize, payload: &[String]) -> usize {
    let max = payload.len().min(source.len().saturating_sub(end));
    (1..=max)
        .rev()
        .find(|count| {
            let candidate = &payload[payload.len() - *count..];
            candidate == &source[end..end + *count]
                && candidate.iter().any(|line| !line.trim().is_empty())
        })
        .unwrap_or(0)
}

fn balance_duplicate_suffix(
    source: &[String],
    end: usize,
    payload: &[String],
    delta: DelimiterBalance,
) -> usize {
    let max = payload.len().min(source.len().saturating_sub(end));
    (1..=max)
        .rev()
        .find(|count| {
            payload[payload.len() - *count..] == source[end..end + *count]
                && delimiter_balance(&payload[payload.len() - *count..]) == delta
        })
        .unwrap_or(0)
}

fn balance_duplicate_prefix(
    source: &[String],
    start: usize,
    payload: &[String],
    delta: DelimiterBalance,
) -> usize {
    let max = payload.len().min(start.saturating_sub(1));
    (1..=max)
        .rev()
        .find(|count| {
            payload[..*count] == source[start - 1 - *count..start - 1]
                && delimiter_balance(&payload[..*count]) == delta
        })
        .unwrap_or(0)
}

fn repair_after_insert_landings(source: &[String], ops: &mut [HashlineOp]) -> Vec<String> {
    let targeted = ops
        .iter()
        .flat_map(HashlineOp::anchor_lines)
        .collect::<BTreeSet<_>>();
    let mut warnings = Vec::new();
    for op in ops {
        let HashlineOp::InsertAfter {
            line,
            lines,
            block_start,
        } = op
        else {
            continue;
        };
        let Some(target) = body_target_indent(lines) else {
            continue;
        };
        let authored_anchor = *line;
        if let Some((landing, crossed)) = outward_landing(source, *line, target, &targeted) {
            *line = landing;
            warnings.push(format!(
                "INS.POST {authored_anchor}: body 缩进浅于 anchor，已越过 {crossed} 行闭合符并落在第 {landing} 行之后"
            ));
            continue;
        }
        let Some(block_start) = *block_start else {
            continue;
        };
        if let Some(landing) = inward_landing(source, *line, block_start, target, &targeted) {
            *line = landing;
            warnings.push(format!(
                "INS.BLK.POST {block_start}: body 缩进深于块闭合行，已放入块内第 {landing} 行之后"
            ));
        }
    }
    warnings
}

fn outward_landing<'a>(
    source: &'a [String],
    anchor: usize,
    target: &'a str,
    targeted: &BTreeSet<usize>,
) -> Option<(usize, usize)> {
    let anchor_text = source.get(anchor - 1)?;
    if anchor_text.trim().is_empty() || !is_indent_deeper(leading_indent(anchor_text), target) {
        return None;
    }
    let mut landing = anchor;
    let mut crossed = 0;
    for line in anchor + 1..=source.len() {
        let text = &source[line - 1];
        if text.trim().is_empty() {
            continue;
        }
        if !is_delimiter_closer(text) {
            break;
        }
        let indent = leading_indent(text);
        if !indent.starts_with(target) {
            break;
        }
        if targeted.contains(&line) {
            return None;
        }
        landing = line;
        crossed += 1;
        if indent.len() == target.len() {
            break;
        }
    }
    (landing != anchor).then_some((landing, crossed))
}

fn inward_landing<'a>(
    source: &'a [String],
    anchor: usize,
    block_start: usize,
    target: &'a str,
    targeted: &BTreeSet<usize>,
) -> Option<usize> {
    let anchor_text = source.get(anchor - 1)?;
    if anchor_text.trim().is_empty() || !is_indent_deeper(target, leading_indent(anchor_text)) {
        return None;
    }
    let mut landing = anchor;
    for line in (block_start + 1..=anchor).rev() {
        let text = &source[line - 1];
        if text.trim().is_empty() {
            landing = line - 1;
            continue;
        }
        if !is_delimiter_closer(text) {
            break;
        }
        if !is_indent_deeper(target, leading_indent(text)) {
            break;
        }
        if line != anchor && targeted.contains(&line) {
            return None;
        }
        landing = line - 1;
    }
    (landing != anchor).then_some(landing)
}

fn body_target_indent(lines: &[String]) -> Option<&str> {
    let non_blank = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if non_blank.is_empty() || non_blank.iter().all(|line| is_structural_closer(line)) {
        return None;
    }
    let mut target = leading_indent(non_blank[0]);
    for line in non_blank {
        let indent = leading_indent(line);
        if indent.starts_with(target) {
            continue;
        }
        if target.starts_with(indent) {
            target = indent;
        } else {
            return None;
        }
    }
    Some(target)
}

fn leading_indent(line: &str) -> &str {
    let end = line
        .bytes()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(line.len());
    &line[..end]
}

fn is_indent_deeper(deeper: &str, shallower: &str) -> bool {
    deeper.len() > shallower.len() && deeper.starts_with(shallower)
}

fn is_delimiter_closer(line: &str) -> bool {
    let trimmed = line.trim();
    let core = trimmed
        .strip_suffix(';')
        .or_else(|| trimmed.strip_suffix(','))
        .unwrap_or(trimmed);
    !core.is_empty() && core.chars().all(|ch| matches!(ch, ')' | ']' | '}'))
}

fn is_structural_closer(line: &str) -> bool {
    is_delimiter_closer(line) || is_jsx_closer_line(line)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DelimiterBalance {
    paren: isize,
    bracket: isize,
    brace: isize,
}

fn delimiter_balance(lines: &[String]) -> DelimiterBalance {
    let mut result = DelimiterBalance::default();
    let mut in_block_comment = false;
    let mut quote = None;
    for line in lines {
        let bytes = line.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if in_block_comment {
                if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    in_block_comment = false;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }
            if let Some(active) = quote {
                if byte == b'\\' {
                    index = index.saturating_add(2);
                    continue;
                }
                if byte == active {
                    quote = None;
                }
                index += 1;
                continue;
            }
            if matches!(byte, b'\'' | b'"' | b'`') {
                quote = Some(byte);
                index += 1;
                continue;
            }
            if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
                break;
            }
            if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                in_block_comment = true;
                index += 2;
                continue;
            }
            match byte {
                b'(' => result.paren += 1,
                b')' => result.paren -= 1,
                b'[' => result.bracket += 1,
                b']' => result.bracket -= 1,
                b'{' => result.brace += 1,
                b'}' => result.brace -= 1,
                _ => {}
            }
            index += 1;
        }
        if matches!(quote, Some(b'\'' | b'"')) {
            quote = None;
        }
    }
    result
}

fn balance_delta(left: DelimiterBalance, right: DelimiterBalance) -> DelimiterBalance {
    DelimiterBalance {
        paren: left.paren - right.paren,
        bracket: left.bracket - right.bracket,
        brace: left.brace - right.brace,
    }
}

fn balance_negate(balance: DelimiterBalance) -> DelimiterBalance {
    DelimiterBalance {
        paren: -balance.paren,
        bracket: -balance.bracket,
        brace: -balance.brace,
    }
}

fn balance_sum(left: DelimiterBalance, right: DelimiterBalance) -> DelimiterBalance {
    DelimiterBalance {
        paren: left.paren + right.paren,
        bracket: left.bracket + right.bracket,
        brace: left.brace + right.brace,
    }
}

fn balance_covers(candidate: DelimiterBalance, target: DelimiterBalance) -> bool {
    component_covers(candidate.paren, target.paren)
        && component_covers(candidate.bracket, target.bracket)
        && component_covers(candidate.brace, target.brace)
}

fn component_covers(candidate: isize, target: isize) -> bool {
    target == 0
        || candidate.signum() == target.signum()
            && candidate.unsigned_abs() >= target.unsigned_abs()
}

fn balance_is_zero(balance: DelimiterBalance) -> bool {
    balance == DelimiterBalance::default()
}

#[cfg(test)]
#[path = "hashline_edit_semantics_tests.rs"]
mod tests;
