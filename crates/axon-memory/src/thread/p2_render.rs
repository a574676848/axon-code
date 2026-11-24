//! P2 两段渲染：当前线索 + 未完成承诺（M3-4）。
//!
//! 固定模板、固定顺序、固定字段序，同一输入必须渲染出逐字节一致的文本（M7 缓存前提）。
//! 预算：线索段 ≤80 token、承诺段 ≤200 token（按 4 字节 ≈ 1 token 折算）。

use crate::store_thread::{MemoryCommitment, MemoryThread};
use crate::temporal::civil_from_days;

/// 线索段字节预算（≈80 token）。
pub const P2_THREAD_MAX_BYTES: usize = 320;
/// 承诺段字节预算（≈200 token）。
pub const P2_COMMITMENT_MAX_BYTES: usize = 800;
/// 承诺段最多条目数。
pub const P2_MAX_COMMITMENTS: usize = 5;

const MS_PER_DAY: i64 = 86_400_000;
const MS_PER_MINUTE: i64 = 60_000;
const MS_PER_HOUR: i64 = 3_600_000;

/// 渲染 P2 的线索段与承诺段；两段都为空时返回空串。
#[must_use]
pub fn render_p2_context(
    thread: Option<&MemoryThread>,
    commitments: &[MemoryCommitment],
    tz_offset_min: i32,
) -> String {
    let mut sections = Vec::new();
    if let Some(block) = render_thread_section(thread) {
        sections.push(block);
    }
    if let Some(block) = render_commitment_section(commitments, tz_offset_min) {
        sections.push(block);
    }
    sections.join("\n")
}

fn render_thread_section(thread: Option<&MemoryThread>) -> Option<String> {
    let thread = thread?;
    let label = if thread.label.trim().is_empty() {
        thread.topic.trim()
    } else {
        thread.label.trim()
    };
    if label.is_empty() {
        return None;
    }
    let hit_count = thread.hit_count.max(1);
    Some(truncate_bytes(
        format!("## 当前线索\n{label}（命中 {hit_count} 次）"),
        P2_THREAD_MAX_BYTES,
    ))
}

fn render_commitment_section(
    commitments: &[MemoryCommitment],
    tz_offset_min: i32,
) -> Option<String> {
    let mut ordered = commitments.iter().collect::<Vec<_>>();
    if ordered.is_empty() {
        return None;
    }
    ordered.sort_by(|left, right| {
        right
            .created_at_ms
            .cmp(&left.created_at_ms)
            .then_with(|| left.commitment_id.cmp(&right.commitment_id))
    });
    let mut block = String::from("## 未完成承诺");
    for commitment in ordered.into_iter().take(P2_MAX_COMMITMENTS) {
        let text = commitment.text.trim();
        if text.is_empty() {
            continue;
        }
        block.push_str(&format!(
            "\n- {text}（{}）",
            format_local_minute(commitment.created_at_ms, tz_offset_min)
        ));
    }
    if block.lines().count() <= 1 {
        return None;
    }
    Some(truncate_bytes(block, P2_COMMITMENT_MAX_BYTES))
}

/// 把毫秒时间戳格式化为本地 `YYYY-MM-DD HH:MM`。
#[must_use]
pub fn format_local_minute(ms: i64, tz_offset_min: i32) -> String {
    let local_ms = ms.saturating_add(i64::from(tz_offset_min) * MS_PER_MINUTE);
    let day = local_ms.div_euclid(MS_PER_DAY);
    let remainder = local_ms.rem_euclid(MS_PER_DAY);
    let (year, month, date) = civil_from_days(day);
    let hour = remainder / MS_PER_HOUR;
    let minute = (remainder % MS_PER_HOUR) / MS_PER_MINUTE;
    format!("{year:04}-{month:02}-{date:02} {hour:02}:{minute:02}")
}

/// 按 UTF-8 字节预算截断，且只在字符边界上切。
fn truncate_bytes(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text[..boundary].trim_end().to_owned()
}

#[cfg(test)]
#[path = "p2_render_tests.rs"]
mod tests;
