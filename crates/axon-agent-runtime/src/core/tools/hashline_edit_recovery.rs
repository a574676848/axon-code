//! Hashline 陈旧快照的 line-map 重放恢复。
//!
//! 读取后文件被外部改动时，锚点行号可能整体位移。恢复把编辑重放到**当前**内容上，
//! 但必须同时满足四条约束，否则 fail-closed 交由调用方提示重新读取：
//!
//! 1. 每个锚点都能在当前文件中定位（多候选时按邻域打分且必须显著胜出）；
//! 2. 所有锚点的位移量**完全一致**：等价于 line-map 上的一次整体平移，
//!    避免多处改动各自漂移后把内容拼进错误位置；
//! 3. 决定位移的那个锚点要通过邻域校验：锚点行值唯一时至少一侧邻居逐行一致，
//!    行值重复时两侧都必须一致；
//! 4. 后续锚点按已确定的位移直接逐行核对内容，不再二次搜索。
//!
//! 与「每段各自找位置」的旧行为相比，这里刻意收紧了恢复范围：宁可让模型重新读取，
//! 也不接受位移不一致的重放。

use std::collections::BTreeSet;

use super::HashlineOp;

/// 多候选打分时向两侧考察的邻居行数。
const RECOVERY_CONTEXT_RADIUS: usize = 3;

struct RecoveryContext<'a> {
    expected: &'a [String],
    current: &'a [String],
    duplicated_expected: BTreeSet<&'a str>,
    duplicated_current: BTreeSet<&'a str>,
}

pub(super) fn recover_ops(
    expected: &[String],
    current: &[String],
    ops: &[HashlineOp],
) -> Result<Vec<HashlineOp>, String> {
    let context = RecoveryContext {
        expected,
        current,
        duplicated_expected: collect_duplicated_values(expected),
        duplicated_current: collect_duplicated_values(current),
    };
    let mut offset: Option<isize> = None;
    let mut recovered = Vec::with_capacity(ops.len());
    for op in ops {
        recovered.push(recover_op(&context, op, &mut offset)?);
    }
    Ok(recovered)
}

fn recover_op(
    context: &RecoveryContext<'_>,
    op: &HashlineOp,
    offset: &mut Option<isize>,
) -> Result<HashlineOp, String> {
    match op {
        HashlineOp::Swap { start, end, lines } => {
            let (start, end) = map_range(context, *start, *end, offset)?;
            Ok(HashlineOp::Swap {
                start,
                end,
                lines: lines.clone(),
            })
        }
        HashlineOp::Delete { start, end } => {
            let (start, end) = map_range(context, *start, *end, offset)?;
            Ok(HashlineOp::Delete { start, end })
        }
        HashlineOp::InsertBefore { line, lines } => {
            let line = map_line(context, *line, offset)?;
            Ok(HashlineOp::InsertBefore {
                line,
                lines: lines.clone(),
            })
        }
        HashlineOp::InsertAfter {
            line,
            lines,
            block_start,
        } => {
            let line = map_line(context, *line, offset)?;
            let block_start = block_start
                .map(|start| map_line(context, start, offset))
                .transpose()?;
            Ok(HashlineOp::InsertAfter {
                line,
                lines: lines.clone(),
                block_start,
            })
        }
        HashlineOp::InsertHead { lines } => Ok(HashlineOp::InsertHead {
            lines: lines.clone(),
        }),
        HashlineOp::InsertTail { lines } => Ok(HashlineOp::InsertTail {
            lines: lines.clone(),
        }),
        HashlineOp::SwapBlock { .. }
        | HashlineOp::DeleteBlock { .. }
        | HashlineOp::InsertAfterBlock { .. } => {
            Err("Hashline BLK 操作必须先解析为具体行范围再执行 stale recovery".to_owned())
        }
    }
}

/// 把单行锚点映射到当前内容；单行是长度 1 的范围，复用同一套约束。
fn map_line(
    context: &RecoveryContext<'_>,
    line: usize,
    offset: &mut Option<isize>,
) -> Result<usize, String> {
    map_range(context, line, line, offset).map(|(mapped, _)| mapped)
}

/// 把 `expected` 中 `start..=end`（1-based，闭区间）映射到 `current` 的行号。
fn map_range(
    context: &RecoveryContext<'_>,
    start: usize,
    end: usize,
    offset: &mut Option<isize>,
) -> Result<(usize, usize), String> {
    validate_source_range(context.expected, start, end)?;
    let target = &context.expected[start - 1..end];
    let mapped_start = match *offset {
        Some(shift) => {
            let mapped = shifted_position(start, shift, context.current.len(), target.len())?;
            if context.current[mapped - 1..mapped - 1 + target.len()] != *target {
                return Err(format!(
                    "Hashline stale anchor {start}.={end} 与已确定的位移 {shift:+} 不吻合：\
                     该位置内容已改变；请重新读取文件"
                ));
            }
            mapped
        }
        None => {
            let selected = select_first_mapping(context, start, end, target)?;
            *offset = Some(selected as isize - (start as isize - 1));
            selected + 1
        }
    };
    Ok((mapped_start, mapped_start + target.len() - 1))
}

/// 首个锚点定位：唯一候选直接采用，多候选按邻域打分并要求显著胜出。
fn select_first_mapping(
    context: &RecoveryContext<'_>,
    start: usize,
    end: usize,
    target: &[String],
) -> Result<usize, String> {
    let candidates = context
        .current
        .windows(target.len())
        .enumerate()
        .filter(|(_, window)| *window == target)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(format!(
            "Hashline stale anchor {start}.={end} 已被外部修改，无法安全恢复；请重新读取文件"
        ));
    }
    let selected = match candidates.as_slice() {
        [only] => *only,
        _ => select_candidate(context, start - 1, end, &candidates)?,
    };
    validate_anchor_neighbours(context, start - 1, end, selected, target)?;
    Ok(selected)
}

fn select_candidate(
    context: &RecoveryContext<'_>,
    expected_start: usize,
    expected_end: usize,
    candidates: &[usize],
) -> Result<usize, String> {
    let mut scored = candidates
        .iter()
        .map(|candidate| {
            (
                *candidate,
                context_score(context, expected_start, expected_end, *candidate),
            )
        })
        .collect::<Vec<_>>();
    scored.sort_by_key(|(candidate, score)| (std::cmp::Reverse(*score), *candidate));
    let Some((best, score)) = scored.first().copied() else {
        return Err("Hashline stale anchor 没有恢复候选".to_owned());
    };
    if score == 0 || scored.get(1).is_some_and(|(_, second)| *second == score) {
        return Err(
            "Hashline stale anchor 在当前文件中存在多个等价位置；请重新读取目标范围".to_owned(),
        );
    }
    Ok(best)
}

/// 邻域校验：行值唯一时至少一侧邻居一致，行值重复时两侧都必须一致。
///
/// 该检查只作用于**决定位移**的那个锚点：位移一旦确定，其余锚点改为逐行核对内容，
/// 这比邻域打分更强。
fn validate_anchor_neighbours(
    context: &RecoveryContext<'_>,
    expected_start: usize,
    expected_end: usize,
    current_start: usize,
    target: &[String],
) -> Result<(), String> {
    let anchor_value = target.first().map_or("", String::as_str);
    let duplicated = context.duplicated_expected.contains(anchor_value)
        || context.duplicated_current.contains(anchor_value);
    let before_ok = expected_start
        .checked_sub(1)
        .filter(|_| current_start >= 1)
        .is_some_and(|index| context.expected[index] == context.current[current_start - 1]);
    let after_ok = context
        .expected
        .get(expected_end)
        .zip(context.current.get(current_start + target.len()))
        .is_some_and(|(left, right)| left == right);
    if expected_start == 0 && expected_end >= context.expected.len() {
        // 整文件被替换：没有可比邻域，交给内容一致性判断。
        return Ok(());
    }
    let consistent = if duplicated {
        before_ok && after_ok
    } else {
        before_ok || after_ok
    };
    if consistent {
        Ok(())
    } else {
        Err(format!(
            "Hashline stale anchor {} 的邻域与当前文件不一致，无法安全恢复；请重新读取文件",
            expected_start + 1
        ))
    }
}

fn context_score(
    context: &RecoveryContext<'_>,
    expected_start: usize,
    expected_end: usize,
    current_start: usize,
) -> usize {
    let target_len = expected_end.saturating_sub(expected_start);
    let mut score = 0usize;
    for distance in 1..=RECOVERY_CONTEXT_RADIUS {
        if let (Some(expected_index), Some(current_index)) = (
            expected_start.checked_sub(distance),
            current_start.checked_sub(distance),
        ) && context.expected.get(expected_index) == context.current.get(current_index)
        {
            score = score.saturating_add(1);
        }
        let expected_index = expected_end.saturating_add(distance - 1);
        let current_index = current_start
            .saturating_add(target_len)
            .saturating_add(distance - 1);
        if context.expected.get(expected_index).is_some()
            && context.expected.get(expected_index) == context.current.get(current_index)
        {
            score = score.saturating_add(1);
        }
    }
    score
}

fn shifted_position(
    start: usize,
    shift: isize,
    current_len: usize,
    target_len: usize,
) -> Result<usize, String> {
    let mapped = start as isize + shift;
    if mapped < 1 || (mapped as usize - 1).saturating_add(target_len) > current_len {
        return Err(format!(
            "Hashline stale anchor 位移 {shift:+} 后超出当前文件范围；请重新读取文件"
        ));
    }
    Ok(mapped as usize)
}

fn collect_duplicated_values(lines: &[String]) -> BTreeSet<&str> {
    let mut seen = BTreeSet::new();
    let mut duplicated = BTreeSet::new();
    for line in lines {
        let value = line.as_str();
        if !seen.insert(value) {
            duplicated.insert(value);
        }
    }
    duplicated
}

fn validate_source_range(lines: &[String], start: usize, end: usize) -> Result<(), String> {
    if start == 0 || end < start || end > lines.len() {
        return Err(format!(
            "Hashline snapshot anchor {start}.={end} 越界，快照共 {} 行",
            lines.len()
        ));
    }
    Ok(())
}
