//! 按交互定向读取：一页历史只需要「每条交互自己的那几帧」。
//!
//! 与 `read.rs` 的分工：整会话分页、尾部窗口与快照仍在那里，这里只放「按键取帧」这一族。
//! 它们共用同一个前提：帧索引里的交互指纹可以把解压范围收窄到某条交互自己的帧，
//! 命中后仍逐条比对归属，指纹只当粗筛。没有索引时退化为单趟顺序解压，不按轮数重复。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::error::EventLogError;
use crate::format::{FrameSpan, HEADER_FRAME_INDEX, scan_file_frames};
use crate::index;
use crate::read::{frame_records, log_len};
use crate::record::RuntimeEventRecord;

/// 每个交互在一次批量读取里最多解压的帧数。
///
/// 历史索引页要为十几轮补读数，不能因为索引缺失或指纹不可用退化成一整条会话的
/// 反复解压。触顶时返回已经取到的部分：缺读数的轮次由界面降级成「只有用时」，
/// 绝不因此挡住历史分页本身。
const MAX_READING_FRAMES: usize = 4_096;

/// 每条缺失交互预留的帧预算：一页历史补十几轮时，逐轮都要有自己的额度。
///
/// 过去的预算是全局共享的固定值，缺失轮次一多，排在后面的轮次就只能拿到「按用时」
/// 这档降级结果。这里改成按缺失条数放大，并仍用 [`MAX_READING_FRAMES_CEILING`] 封顶。
const PER_INTERACTION_FRAMES: usize = 256;

/// 一次批量补帧最多解压多少帧：放大后仍要有硬上界，避免一页历史退化成整条日志冷读。
const MAX_READING_FRAMES_CEILING: usize = 16_384;

/// 按交互取到的一条事件。
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionRecord {
    /// 该事件归属的交互标识。
    pub interaction_id: String,
    /// 命中的事件记录；同一交互只取最近一条。
    pub record: RuntimeEventRecord,
}

/// 批量读取一组交互**各自最近一条**指定类型的事件。
///
/// 用途固定：历史索引页要给每条轮次一行读数（用时、token、首字、速率），权威事实是
/// 每轮收口时写下的那条 `turn_metrics`。逐轮整段回放会把一页十几轮的正文全部解压一遍，
/// 因此这里按索引里的交互指纹从尾部倒推：每条交互通常只需解压它自己的最后几帧。
///
/// 命中后仍逐条比对 `interaction_id`，指纹只是粗筛；指纹碰撞只会多解压一帧。
/// 没有索引时退化为单趟顺序解压，仍然是「一个会话一遍」，不按轮数重复。
pub fn read_latest_per_interaction(
    path: &Path,
    interaction_ids: &[&str],
    event_type: &str,
) -> Result<Vec<InteractionRecord>, EventLogError> {
    let wanted: Vec<&str> = interaction_ids
        .iter()
        .copied()
        .filter(|id| !id.is_empty())
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let distinct: HashSet<&str> = wanted.iter().copied().collect();
    let mut remaining = distinct.len();
    let mut found: HashMap<&str, RuntimeEventRecord> = HashMap::new();
    let frame_budget = MAX_READING_FRAMES
        .max(wanted.len().saturating_mul(PER_INTERACTION_FRAMES))
        .min(MAX_READING_FRAMES_CEILING);
    let tags: Vec<u32> = wanted
        .iter()
        .map(|id| index::interaction_tag(Some(id)))
        .collect();
    if let Some(index) = index::load(path, len) {
        let mut scanned = 0_usize;
        for entry in index.entries().iter().rev() {
            if remaining == 0 || scanned >= frame_budget {
                break;
            }
            if !tags.iter().any(|tag| entry.may_hold(*tag)) {
                continue;
            }
            let (Ok(start), Ok(end)) =
                (usize::try_from(entry.offset), usize::try_from(entry.end()))
            else {
                break;
            };
            scanned += 1;
            for record in frame_records(&mut file, &FrameSpan { start, end })? {
                remaining = collect_latest(&wanted, event_type, record, &mut found, remaining);
            }
        }
    } else {
        let scan = scan_file_frames(&mut file)?;
        let spans: Vec<FrameSpan> = scan
            .frames
            .iter()
            .skip(HEADER_FRAME_INDEX + 1)
            .rev()
            .take(frame_budget)
            .cloned()
            .collect();
        for span in spans {
            if remaining == 0 {
                break;
            }
            for record in frame_records(&mut file, &span)? {
                remaining = collect_latest(&wanted, event_type, record, &mut found, remaining);
            }
        }
    }
    let mut collected: Vec<InteractionRecord> = Vec::with_capacity(found.len());
    for id in wanted {
        if let Some(record) = found.remove(id) {
            collected.push(InteractionRecord {
                interaction_id: id.to_owned(),
                record,
            });
        }
    }
    Ok(collected)
}

/// 收下一条命中记录；返回还差几条交互没有取到。
fn collect_latest<'a>(
    wanted: &[&'a str],
    event_type: &str,
    record: RuntimeEventRecord,
    found: &mut HashMap<&'a str, RuntimeEventRecord>,
    remaining: usize,
) -> usize {
    if record.event_type.as_str() != event_type {
        return remaining;
    }
    let Some(id) = record.interaction_id.as_deref() else {
        return remaining;
    };
    let Some(key) = wanted.iter().copied().find(|candidate| **candidate == *id) else {
        return remaining;
    };
    if found.contains_key(key) {
        return remaining;
    }
    found.insert(key, record);
    remaining.saturating_sub(1)
}
