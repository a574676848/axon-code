use std::collections::{BTreeMap, BTreeSet};

/// 修复类 warning 的前缀，与语义修复层共用同一常量。
use super::super::HASHLINE_REPAIR_WARNING_PREFIX as REPAIR_WARNING_PREFIX;
use super::super::HashlineOp;
use super::{
    DelimiterBalance, balance_covers, balance_delta, balance_is_zero, balance_negate, balance_sum,
    body_target_indent, delimiter_balance, is_delimiter_closer, is_indent_deeper, leading_indent,
    valid_range,
};

#[derive(Debug, Clone, Copy)]
struct DroppedClosers {
    start: usize,
    count: usize,
    balance: DelimiterBalance,
}

struct MissingCloserCandidate<'a> {
    source: &'a [String],
    start: usize,
    end: usize,
    payload: &'a [String],
    delta: DelimiterBalance,
    remaining_delta: DelimiterBalance,
    deleted_prefix: DelimiterBalance,
}

#[derive(Debug, Default)]
struct PatchProjection {
    deleted: BTreeSet<usize>,
    inserted_by_line: BTreeMap<usize, Vec<String>>,
    inserted_before: BTreeMap<usize, Vec<String>>,
    inserted_after: BTreeMap<usize, Vec<String>>,
}

impl PatchProjection {
    fn from_ops(ops: &[HashlineOp]) -> Self {
        let mut projection = Self::default();
        for op in ops {
            match op {
                HashlineOp::Swap { start, end, lines } => {
                    projection.deleted.extend(*start..=*end);
                    projection.record_before(*start, lines);
                }
                HashlineOp::Delete { start, end } => {
                    projection.deleted.extend(*start..=*end);
                }
                HashlineOp::InsertBefore { line, lines } => {
                    projection.record_before(*line, lines);
                }
                HashlineOp::InsertAfter { line, lines, .. } => {
                    projection.record_after(*line, lines);
                }
                HashlineOp::InsertHead { .. }
                | HashlineOp::InsertTail { .. }
                | HashlineOp::SwapBlock { .. }
                | HashlineOp::DeleteBlock { .. }
                | HashlineOp::InsertAfterBlock { .. } => {}
            }
        }
        projection
    }

    fn record_before(&mut self, line: usize, lines: &[String]) {
        self.inserted_by_line
            .entry(line)
            .or_default()
            .extend_from_slice(lines);
        self.inserted_before
            .entry(line)
            .or_default()
            .extend_from_slice(lines);
    }

    fn record_after(&mut self, line: usize, lines: &[String]) {
        self.inserted_by_line
            .entry(line)
            .or_default()
            .extend_from_slice(lines);
        self.inserted_after
            .entry(line)
            .or_default()
            .extend_from_slice(lines);
    }

    fn net_deleted_prefix_balance(&self, source: &[String], start: usize) -> DelimiterBalance {
        let mut first = start;
        while first > 1 && self.deleted.contains(&(first - 1)) {
            first -= 1;
        }
        let mut deleted = Vec::new();
        let mut inserted = Vec::new();
        for line in first..start {
            deleted.push(source[line - 1].clone());
            if let Some(rows) = self.inserted_by_line.get(&line) {
                inserted.extend_from_slice(rows);
            }
        }
        balance_delta(delimiter_balance(&deleted), delimiter_balance(&inserted))
    }

    fn projected_prefix_balance(
        &self,
        source: &[String],
        start: usize,
        payload: &[String],
    ) -> DelimiterBalance {
        let mut prefix = Vec::new();
        for line in 1..start {
            if let Some(rows) = self.inserted_by_line.get(&line) {
                prefix.extend_from_slice(rows);
            }
            if !self.deleted.contains(&line) {
                prefix.push(source[line - 1].clone());
            }
        }
        if let Some(rows) = self.inserted_before.get(&start) {
            prefix.extend_from_slice(rows);
        }
        prefix.extend_from_slice(payload);
        delimiter_balance(&prefix)
    }

    fn covered_suffix_tail(&self, source: &[String], end: usize, suffix: &[String]) -> usize {
        let mut below = Vec::new();
        if !append_closers(&mut below, self.inserted_after.get(&end).map(Vec::as_slice)) {
            return 0;
        }
        for line in end + 1..=source.len() {
            if !append_closers(
                &mut below,
                self.inserted_before.get(&line).map(Vec::as_slice),
            ) {
                break;
            }
            if !self.deleted.contains(&line) {
                let text = &source[line - 1];
                if !is_delimiter_closer(text) {
                    break;
                }
                below.push(text.clone());
            }
            if !append_closers(
                &mut below,
                self.inserted_after.get(&line).map(Vec::as_slice),
            ) {
                break;
            }
        }
        matching_suffix_tail(&below, suffix)
    }
}

pub(super) fn repair_missing_closers(
    source: &[String],
    ops: Vec<HashlineOp>,
) -> Result<(Vec<HashlineOp>, Vec<String>), String> {
    let mut projection = PatchProjection::from_ops(&ops);
    let mut remaining_delta = ops.iter().fold(DelimiterBalance::default(), |total, op| {
        balance_sum(total, op_delta(source, op))
    });
    let mut repaired = Vec::with_capacity(ops.len());
    let mut warnings = Vec::new();

    for op in ops {
        let HashlineOp::Swap { start, end, lines } = op else {
            repaired.push(op);
            continue;
        };
        if !valid_range(source, start, end) {
            repaired.push(HashlineOp::Swap { start, end, lines });
            continue;
        }
        let delta = balance_delta(
            delimiter_balance(&lines),
            delimiter_balance(&source[start - 1..end]),
        );
        if balance_is_zero(delta) {
            repaired.push(HashlineOp::Swap { start, end, lines });
            continue;
        }
        let deleted_prefix = projection.net_deleted_prefix_balance(source, start);
        let Some(dropped) = find_dropped_closers(
            MissingCloserCandidate {
                source,
                start,
                end,
                payload: &lines,
                delta,
                remaining_delta,
                deleted_prefix,
            },
            &projection,
        ) else {
            repaired.push(HashlineOp::Swap { start, end, lines });
            continue;
        };

        let closer_indent = leading_indent(&source[dropped.start - 1]);
        let claims_inside = body_target_indent(&lines)
            .is_some_and(|indent| is_indent_deeper(indent, closer_indent));
        let payload_opens =
            balance_covers(delimiter_balance(&lines), balance_negate(dropped.balance));
        if !claims_inside && !payload_opens {
            return Err(format!(
                "SWAP {start}.={end} 删除了结构闭合行，但 body 无法明确落在闭合行之前或之后；请缩小范围或在 body 中明确重述闭合行"
            ));
        }

        for line in dropped.start..dropped.start + dropped.count {
            projection.deleted.remove(&line);
        }
        remaining_delta = balance_sum(remaining_delta, dropped.balance);
        warnings.push(format!(
            "{REPAIR_WARNING_PREFIX}已修复 SWAP {start}.={end} 的缺失闭合边界：保留 body 未重述的 {} 行结构闭合符",
            dropped.count
        ));
        append_spared_swap(&mut repaired, start, end, lines, dropped);
    }
    Ok((repaired, warnings))
}

fn find_dropped_closers(
    candidate: MissingCloserCandidate<'_>,
    projection: &PatchProjection,
) -> Option<DroppedClosers> {
    let MissingCloserCandidate {
        source,
        start,
        end,
        payload,
        delta,
        remaining_delta,
        deleted_prefix,
    } = candidate;
    let mut suffix_len = 0;
    while suffix_len < end - start + 1 && is_delimiter_closer(source.get(end - suffix_len - 1)?) {
        suffix_len += 1;
    }
    if suffix_len == 0 {
        return None;
    }
    let suffix_start = end - suffix_len + 1;
    let suffix = &source[suffix_start - 1..end];
    let restated_head = matching_payload_suffix_head(payload, suffix);
    let covered_tail = projection.covered_suffix_tail(source, end, suffix);
    let keep_start = restated_head;
    let keep_end = suffix_len.saturating_sub(covered_tail);
    if keep_start >= keep_end {
        return None;
    }

    let kept = &suffix[keep_start..keep_end];
    let kept_balance = delimiter_balance(kept);
    let needed_openers = balance_negate(kept_balance);
    let covered_below = delimiter_balance(&suffix[keep_end..]);
    if !balance_covers(delta, needed_openers)
        || balance_covers(deleted_prefix, needed_openers)
        || !balance_covers(remaining_delta, needed_openers)
    {
        return None;
    }
    let prefix = projection.projected_prefix_balance(source, start, payload);
    if !balance_covers(balance_sum(prefix, covered_below), needed_openers) {
        return None;
    }
    Some(DroppedClosers {
        start: suffix_start + keep_start,
        count: keep_end - keep_start,
        balance: kept_balance,
    })
}

fn matching_payload_suffix_head(payload: &[String], suffix: &[String]) -> usize {
    let max = payload.len().min(suffix.len());
    (1..=max)
        .rev()
        .find(|count| payload[payload.len() - *count..] == suffix[..*count])
        .unwrap_or(0)
}

fn matching_suffix_tail(below: &[String], suffix: &[String]) -> usize {
    let max = below.len().min(suffix.len());
    (1..=max)
        .rev()
        .find(|count| below[..*count] == suffix[suffix.len() - *count..])
        .unwrap_or(0)
}

fn append_closers(target: &mut Vec<String>, rows: Option<&[String]>) -> bool {
    let Some(rows) = rows else {
        return true;
    };
    for row in rows {
        if !is_delimiter_closer(row) {
            return false;
        }
        target.push(row.clone());
    }
    true
}

fn append_spared_swap(
    output: &mut Vec<HashlineOp>,
    start: usize,
    end: usize,
    lines: Vec<String>,
    dropped: DroppedClosers,
) {
    if start < dropped.start {
        output.push(HashlineOp::Swap {
            start,
            end: dropped.start - 1,
            lines,
        });
    } else {
        output.push(HashlineOp::InsertBefore { line: start, lines });
    }
    let tail_start = dropped.start + dropped.count;
    if tail_start <= end {
        output.push(HashlineOp::Delete {
            start: tail_start,
            end,
        });
    }
}

fn op_delta(source: &[String], op: &HashlineOp) -> DelimiterBalance {
    match op {
        HashlineOp::Swap { start, end, lines } if valid_range(source, *start, *end) => {
            balance_delta(
                delimiter_balance(lines),
                delimiter_balance(&source[*start - 1..*end]),
            )
        }
        HashlineOp::Delete { start, end } if valid_range(source, *start, *end) => {
            balance_negate(delimiter_balance(&source[*start - 1..*end]))
        }
        HashlineOp::InsertBefore { lines, .. }
        | HashlineOp::InsertAfter { lines, .. }
        | HashlineOp::InsertHead { lines }
        | HashlineOp::InsertTail { lines } => delimiter_balance(lines),
        HashlineOp::SwapBlock { .. }
        | HashlineOp::DeleteBlock { .. }
        | HashlineOp::InsertAfterBlock { .. }
        | HashlineOp::Swap { .. }
        | HashlineOp::Delete { .. } => DelimiterBalance::default(),
    }
}
