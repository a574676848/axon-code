//! 会话级窗口身份与目标锚点（Session Notes & Goal Anchor）。
//!
//! 跨上下文窗口的模型自述事实不再保存在本文件中，而是由 notes 命名空间的
//! 私有便签文件承载（见 `session_notes_store.rs`）。本文件只保留：
//! 1. 用户交付的最初核心目标与不可妥协约束（Goal Anchor）
//! 2. 代际窗口身份链（first/previous/current window id）
//! 3. 换窗前是否已成功写入便签的 fail-closed 标记

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult};
use serde::{Deserialize, Serialize};

/// 会话战术笔记持久化文件名。
pub const SESSION_NOTES_FILE_NAME: &str = "session_notes.json";
/// 当前目标态 SessionNotes schema 版本。
pub const SESSION_NOTES_SCHEMA_VERSION: u32 = 4;
/// 单次窗口交接 checkpoint 的模型可见字节上限。
pub const SESSION_NOTES_CHECKPOINT_MAX_BYTES: usize = 4 * 1024;

const CHECKPOINT_TASK_MAX_BYTES: usize = 768;
const CHECKPOINT_CONSTRAINTS_MAX_BYTES: usize = 512;
const CHECKPOINT_VERIFIED_MAX_BYTES: usize = 1_536;
const CHECKPOINT_OPEN_ISSUES_MAX_BYTES: usize = 512;
const CHECKPOINT_TRUNCATED_SUFFIX: &str = "...（完整内容可从会话记录中查找）";
const CHECKPOINT_TRUNCATED_HINT: &str = "- 较早内容已归档；已知 window ID 与 item ID 时优先调用 builtin.history_read_item，否则用 builtin.history_list_items 或 builtin.history_search_contents 定位，便签内容用 builtin.notes_read_file 回读。";

/// 用户核心目标与非妥协约束锚点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GoalAnchor {
    /// 用户交付的最初核心目标。
    pub user_objective: String,
    /// 必须绝对遵守的非妥协约束与偏好。
    pub non_negotiable_constraints: Vec<String>,
}

impl GoalAnchor {
    /// 创建新目标锚点。
    #[must_use]
    pub fn new(user_objective: impl Into<String>, constraints: Vec<String>) -> Self {
        Self {
            user_objective: user_objective.into(),
            non_negotiable_constraints: constraints,
        }
    }
}

/// 会话专属战术便签。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionNotes {
    /// notes schema 版本。
    pub schema_version: u32,
    /// 用户目标锚点。
    pub goal_anchor: GoalAnchor,
    /// 当前会话代际窗口序号（从 1 开始）。
    pub window_number: u64,
    /// 首个窗口 ID，用于跨代际恢复链路。
    pub first_window_id: Option<String>,
    /// 上一窗口 ID。
    pub previous_window_id: Option<String>,
    /// 当前窗口 ID。
    pub current_window_id: Option<String>,
    /// 最近一次已提交轮转生命周期，用于重放幂等。
    pub last_rollover_lifecycle_id: Option<String>,
    /// 最近归档窗口在归档前指向的上一窗口 ID。
    pub last_archived_previous_window_id: Option<String>,
    /// 最近一次轮转提交后的 notes revision，用于回滚 CAS。
    pub last_rollover_revision: Option<u64>,
    /// 最近一次成功保存任务状态所属的窗口序号。
    #[serde(default)]
    pub last_saved_state_window_number: Option<u64>,
    /// 战术便签版本修订号。
    pub revision: u64,
    /// 最后更新时间戳（Unix 毫秒）。
    pub updated_at_ms: i64,
}

impl SessionNotes {
    /// 基于目标锚点创建空便签。
    #[must_use]
    pub fn new(goal_anchor: GoalAnchor, now_ms: i64) -> Self {
        let initial_window_id = uuid::Uuid::new_v4().to_string();
        Self {
            schema_version: SESSION_NOTES_SCHEMA_VERSION,
            goal_anchor,
            window_number: 1,
            first_window_id: Some(initial_window_id.clone()),
            previous_window_id: None,
            current_window_id: Some(initial_window_id),
            last_rollover_lifecycle_id: None,
            last_archived_previous_window_id: None,
            last_rollover_revision: None,
            last_saved_state_window_number: None,
            revision: 1,
            updated_at_ms: now_ms,
        }
    }

    /// 在固定注入预算内渲染 canonical checkpoint；超限时 fail-closed。
    ///
    /// 事实来源是模型自己写入的私有便签文件（对齐 Codex 的 notes → 换窗回读
    /// 交接方式），因此 checkpoint 只做有界投影，细节由模型按需回读。
    pub fn try_render_canonical_summary(
        &self,
        session_dir: &Path,
        history: &[crate::agent_model_message::AgentModelMessage],
    ) -> AxonResult<String> {
        let task = if self.goal_anchor.user_objective.trim().is_empty() {
            "执行用户既定任务".to_owned()
        } else {
            bounded_checkpoint_text(&self.goal_anchor.user_objective, CHECKPOINT_TASK_MAX_BYTES)
        };

        let constraints = render_bounded_checkpoint_list(
            self.goal_anchor.non_negotiable_constraints.iter().cloned(),
            CHECKPOINT_CONSTRAINTS_MAX_BYTES,
        );

        let verified = crate::session_notes_store::NotesStore::render_checkpoint(
            session_dir,
            CHECKPOINT_VERIFIED_MAX_BYTES,
        )?;

        let open = crate::session_notes_store::NotesStore::render_read_back_hint(
            session_dir,
            CHECKPOINT_OPEN_ISSUES_MAX_BYTES,
        )?;

        let tools = render_active_tool_calls(history);
        let summary = format!(
            "P0:\n\
            TASK:\n\
            {task}\n\n\
            CONSTRAINTS:\n\
            {constraints}\n\n\
            VERIFIED:\n\
            {verified}\n\n\
            OPEN:\n\
            {open}\n\n\
            TOOLS:\n\
            {tools}"
        );
        if summary.len() > SESSION_NOTES_CHECKPOINT_MAX_BYTES {
            return Err(AxonError::runtime_failure(format!(
                "窗口交接 checkpoint 超出固定预算: {} > {} bytes",
                summary.len(),
                SESSION_NOTES_CHECKPOINT_MAX_BYTES
            )));
        }
        Ok(summary)
    }

    /// 将笔记原子持久化到 session 目录。
    pub fn save_to_session(&self, session_dir: &Path) -> AxonResult<PathBuf> {
        fs::create_dir_all(session_dir)
            .map_err(|e| AxonError::runtime_failure(format!("创建会话笔记目录失败: {e:?}")))?;
        let file_path = session_dir.join(SESSION_NOTES_FILE_NAME);
        let data = serde_json::to_vec_pretty(self)
            .map_err(|e| AxonError::runtime_failure(format!("序列化会话笔记失败: {e:?}")))?;
        write_file_replacing(&file_path, &data, "会话笔记")?;
        Ok(file_path)
    }

    /// 从 session 目录加载笔记。
    pub fn load_from_session(session_dir: &Path) -> AxonResult<Option<Self>> {
        let file_path = session_dir.join(SESSION_NOTES_FILE_NAME);
        let data = match fs::read(&file_path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(AxonError::runtime_failure(format!(
                    "读取会话笔记失败: {e:?}"
                )));
            }
        };
        if data.is_empty() {
            return Ok(None);
        }
        let value = serde_json::from_slice::<serde_json::Value>(&data)
            .map_err(|e| AxonError::runtime_failure(format!("解析会话笔记失败: {e:?}")))?;
        let schema_version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .ok_or_else(|| AxonError::runtime_failure("会话笔记缺少有效 schema_version"))?;
        if schema_version != SESSION_NOTES_SCHEMA_VERSION {
            return Err(AxonError::runtime_failure(format!(
                "会话笔记 schema 版本不支持: {schema_version}"
            )));
        }
        let notes = serde_json::from_value(value)
            .map_err(|e| AxonError::runtime_failure(format!("解析会话笔记失败: {e:?}")))?;
        Ok(Some(notes))
    }
}

pub(crate) fn write_file_replacing(
    target_path: &Path,
    content: &[u8],
    description: &str,
) -> AxonResult<()> {
    let parent = target_path
        .parent()
        .ok_or_else(|| AxonError::runtime_failure(format!("{description}目标路径缺少父目录")))?;
    fs::create_dir_all(parent).map_err(|error| {
        AxonError::runtime_failure(format!("创建{description}目录失败: {error:?}"))
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        AxonError::runtime_failure(format!("创建{description}临时文件失败: {error:?}"))
    })?;
    temporary.write_all(content).map_err(|error| {
        AxonError::runtime_failure(format!("写入{description}临时文件失败: {error:?}"))
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        AxonError::runtime_failure(format!("同步{description}临时文件失败: {error:?}"))
    })?;
    temporary.persist(target_path).map_err(|error| {
        AxonError::runtime_failure(format!("替换{description}文件失败: {:?}", error.error))
    })?;
    Ok(())
}

fn render_active_tool_calls(history: &[crate::agent_model_message::AgentModelMessage]) -> String {
    let mut call_ids = std::collections::BTreeSet::new();
    let mut result_ids = std::collections::BTreeSet::new();
    for message in history {
        if let Some(metadata) = message.provider_metadata.as_ref() {
            for call in metadata.tool_calls.as_deref().unwrap_or_default() {
                let call_id = call.id.trim();
                if !call_id.is_empty() {
                    call_ids.insert(call_id.to_owned());
                }
            }
            if let Some(result) = metadata.tool_result.as_ref() {
                let call_id = result.call_id.trim();
                if !call_id.is_empty() {
                    result_ids.insert(call_id.to_owned());
                }
            }
        }
    }
    let active_calls = call_ids
        .difference(&result_ids)
        .cloned()
        .collect::<Vec<_>>();
    if active_calls.is_empty() {
        "- none".to_owned()
    } else {
        active_calls
            .iter()
            .map(|id| format!("- ID: {id}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn render_bounded_checkpoint_list(
    values: impl IntoIterator<Item = String>,
    max_bytes: usize,
) -> String {
    let lines = values
        .into_iter()
        .map(|value| format!("- {}", normalize_checkpoint_text(&value)))
        .filter(|line| line != "- ")
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return "- none".to_owned();
    }
    let full = lines.join("\n");
    if full.len() <= max_bytes {
        return full;
    }

    let content_budget = max_bytes.saturating_sub(CHECKPOINT_TRUNCATED_HINT.len() + 1);
    let mut retained = String::new();
    for line in lines {
        let separator_bytes = usize::from(!retained.is_empty());
        if retained
            .len()
            .saturating_add(separator_bytes)
            .saturating_add(line.len())
            > content_budget
        {
            break;
        }
        if !retained.is_empty() {
            retained.push('\n');
        }
        retained.push_str(&line);
    }
    if retained.is_empty() {
        retained = bounded_checkpoint_text(&full, content_budget);
    }
    if !retained.is_empty() {
        retained.push('\n');
    }
    retained.push_str(CHECKPOINT_TRUNCATED_HINT);
    retained
}

fn bounded_checkpoint_text(value: &str, max_bytes: usize) -> String {
    let normalized = normalize_checkpoint_text(value);
    if normalized.len() <= max_bytes {
        return normalized;
    }
    let content_budget = max_bytes.saturating_sub(CHECKPOINT_TRUNCATED_SUFFIX.len());
    let boundary = normalized
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= content_budget)
        .last()
        .unwrap_or_default();
    format!(
        "{}{}",
        normalized[..boundary].trim_end(),
        CHECKPOINT_TRUNCATED_SUFFIX
    )
}

fn normalize_checkpoint_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
