//! 会话级窗口身份与目标锚点（Session Notes & Goal Anchor）。
//!
//! 跨上下文窗口的模型自述事实不再保存在本文件中，而是由 notes 命名空间的
//! 私有便签文件承载（见 `session_notes_store.rs`）。本文件只保留：
//! 1. 用户交付的最初核心目标与不可妥协约束（Goal Anchor）
//! 2. 代际窗口身份链（first/previous/current window id）
//! 3. 换窗前是否已成功写入便签的 fail-closed 标记
//!
//! 交接摘要的语义权归模型：模型把摘要写入保留便签 `handoff/summary.md` 时，
//! 运行时在预算内原样采用，内容格式不做任何限制；缺失或超预算时才回退到
//! 目标锚点与便签的投影。
//! 投影对短便签内联正文、对长便签只给路径与行数，运行时只做总量预算校验，
//! 两条路径都不按栏目截断模型内容。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult};
use serde::{Deserialize, Serialize};

use crate::token_estimator::estimated_text_tokens;

/// 会话战术笔记持久化文件名。
pub const SESSION_NOTES_FILE_NAME: &str = "session_notes.json";
/// 当前目标态 SessionNotes schema 版本。
pub const SESSION_NOTES_SCHEMA_VERSION: u32 = 5;
/// 仍可直接读取并就地迁移的最低 schema 版本；低于它的文件显式拒绝。
pub const SESSION_NOTES_MIN_SUPPORTED_SCHEMA_VERSION: u32 = 4;
/// 单次窗口交接 checkpoint 的模型可见 token 上限（常量默认值，不做配置项）。
///
/// 取 4 096 是固定成本下限：每次换窗都要把这部分摘要顶到上下文顶部，
/// 总量越小，长会话的可用预算越大。与 `NOTES_INLINE_MAX_BYTES`（4 KiB）配套。
pub const SESSION_NOTES_CHECKPOINT_MAX_TOKENS: u64 = 4_096;
/// 单次窗口交接 checkpoint 的字节硬上限；仅作估算偏差的溢出保护。
pub const SESSION_NOTES_CHECKPOINT_MAX_BYTES: usize = 32 * 1024;
/// 模型提交窗口交接摘要的保留便签路径。
///
/// 与自由格式工作便签（如 `checkpoint.md`）分开：模型可以继续用任意 markdown 结构
/// 记录工作便签，需要单独控制摘要语义时才写这个文件。路径本身就是提交通道的识别
/// 依据，内容不限格式、只受总预算约束。
pub const SESSION_NOTES_SUBMITTED_SUMMARY_PATH: &str = "handoff/summary.md";

/// 换窗交接摘要的来源，用于复盘摘要由谁生成、是否被冒充。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointSource {
    /// 模型通过 `handoff/summary.md` 原样提交的摘要。
    Model,
    /// 运行时按目标锚点与便签投影生成的摘要。
    Projection,
}

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
    /// 最近一次换窗交接摘要的来源；v4 及更早文件缺省为 `None`。
    #[serde(default)]
    pub checkpoint_source: Option<CheckpointSource>,
    /// 最近一次换窗交接摘要对应的窗口序号，用于识别过期摘要。
    #[serde(default)]
    pub checkpoint_window_number: Option<u64>,
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
            checkpoint_source: None,
            checkpoint_window_number: None,
            revision: 1,
            updated_at_ms: now_ms,
        }
    }

    /// 渲染窗口交接摘要：模型提交优先，缺失时回退投影，只做总预算校验。
    ///
    /// 模型把摘要写进保留便签 `handoff/summary.md` 时，该文本被原样采用（内容不限格式）；
    /// 缺失或超预算时按目标锚点与全部便签投影出等价交接。
    /// 两条路径都不截断内容：超出总预算直接 fail-closed，由模型压缩后重写。
    pub fn try_render_canonical_summary(
        &self,
        session_dir: &Path,
        history: &[crate::agent_model_message::AgentModelMessage],
    ) -> AxonResult<String> {
        self.render_canonical_summary_with_source(session_dir, history)
            .map(|(summary, _source)| summary)
    }

    /// 渲染交接摘要并返回它的来源，供换窗落库与复盘使用。
    ///
    /// 来源是「谁写的这段摘要」的权威记录：`Model` 表示逐字节采用模型提交，
    /// `Projection` 表示运行时按目标锚点与便签投影。调用方负责把它写回
    /// `checkpoint_source` / `checkpoint_window_number`，后续窗口才能判断摘要是否为模型原话。
    ///
    /// 上一份摘要若已落后两个窗口以上，说明中间有换窗没写回来源：此时**丢弃模型提交的摘要、
    /// 强制走投影**。投影按当前便签实时生成，天然不带旧代际的事实；而换窗不允许因此失败：
    /// 唯一能修好「来源缺失」的动作就是换窗，在这里 fail-closed 会让这个会话再也换不了窗。
    pub fn render_canonical_summary_with_source(
        &self,
        session_dir: &Path,
        history: &[crate::agent_model_message::AgentModelMessage],
    ) -> AxonResult<(String, CheckpointSource)> {
        // 过期只降级来源，不阻断换窗：提交摘要可能是两个窗口前的旧文件，投影永远对应当前便签。
        let submitted = if self.checkpoint_is_stale() {
            None
        } else {
            read_submitted_summary(session_dir)?
        };
        let (summary, source) = match submitted {
            Some(submitted) if submitted_fits_budget(&submitted) => {
                (submitted, CheckpointSource::Model)
            }
            _ => (
                self.render_projected_summary(session_dir, history)?,
                CheckpointSource::Projection,
            ),
        };
        validate_checkpoint_budget(&summary)?;
        Ok((summary, source))
    }

    /// 上一次交接摘要是否已落后于当前窗口代际。
    ///
    /// 约定：摘要由窗口 N 写出，服务 N → N+1 的交接。当前窗口推进到 N+2 或更远时，
    /// 说明中间有换窗没有写回来源；旧摘要描述的是更早的状态，不能当作现状使用。
    /// `None`（从未写过）不算过期，`Some(N)` 且当前窗口 ≤ N+1 也不算。
    ///
    /// 判定结果只决定「用提交摘要还是用投影」，不决定「能否换窗」。
    #[must_use]
    pub fn checkpoint_is_stale(&self) -> bool {
        self.checkpoint_window_number
            .is_some_and(|written_for| self.window_number > written_for.saturating_add(1))
    }

    /// 按目标锚点与全部便签投影 canonical 六段摘要（兼容路径，不截断）。
    fn render_projected_summary(
        &self,
        session_dir: &Path,
        history: &[crate::agent_model_message::AgentModelMessage],
    ) -> AxonResult<String> {
        let task = if self.goal_anchor.user_objective.trim().is_empty() {
            "执行用户既定任务".to_owned()
        } else {
            normalize_checkpoint_text(&self.goal_anchor.user_objective)
        };
        let constraints =
            render_checkpoint_list(self.goal_anchor.non_negotiable_constraints.iter().cloned());
        let verified = crate::session_notes_store::NotesStore::render_checkpoint(session_dir)?;
        let open = crate::session_notes_store::NotesStore::render_read_back_hint(session_dir)?;
        let tools = render_active_tool_calls(history);
        Ok(format!(
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
        ))
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
        if !(SESSION_NOTES_MIN_SUPPORTED_SCHEMA_VERSION..=SESSION_NOTES_SCHEMA_VERSION)
            .contains(&schema_version)
        {
            return Err(AxonError::runtime_failure(format!(
                "会话笔记 schema 版本不支持: {schema_version}；当前支持 {} 到 {}",
                SESSION_NOTES_MIN_SUPPORTED_SCHEMA_VERSION, SESSION_NOTES_SCHEMA_VERSION
            )));
        }
        let mut notes: SessionNotes = serde_json::from_value(value)
            .map_err(|e| AxonError::runtime_failure(format!("解析会话笔记失败: {e:?}")))?;
        // 旧版本只在内存里补齐新字段并升到当前版本，文件本身等下一次 save 时才落成新格式，
        // 因此降级回旧二进制仍读得到同一份未破坏的旧文件。
        if notes.schema_version < SESSION_NOTES_SCHEMA_VERSION {
            notes.schema_version = SESSION_NOTES_SCHEMA_VERSION;
        }
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
        return "- none".to_owned();
    }
    active_calls
        .iter()
        .map(|id| format!("- ID: {id}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_checkpoint_list(values: impl IntoIterator<Item = String>) -> String {
    let lines = values
        .into_iter()
        .map(|value| format!("- {}", normalize_checkpoint_text(&value)))
        .filter(|line| line != "- ")
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return "- none".to_owned();
    }
    lines.join("\n")
}

/// 读取模型提交的交接摘要；未提交或内容为空时返回 `None`。
///
/// 判定只看保留路径：内容不限格式，写什么就用什么。
fn read_submitted_summary(session_dir: &Path) -> AxonResult<Option<String>> {
    let target = crate::session_notes_store::NotesStore::notes_root(session_dir)
        .join(SESSION_NOTES_SUBMITTED_SUMMARY_PATH);
    let text = match fs::read_to_string(&target) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AxonError::runtime_failure(format!(
                "读取模型提交的交接摘要失败: {error:?}"
            )));
        }
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(text.to_owned()))
}

/// 提交摘要能否直接带进新窗口：只受预算约束，内容不限。
///
/// 超预算时回落到投影而不是拒绝换窗：投影按当前便签实时生成、天然在预算内，
/// 而换窗是解除上下文耗尽的唯一动作，在这里 fail-closed 会让会话再也换不了窗。
fn submitted_fits_budget(submitted: &str) -> bool {
    !submitted.trim().is_empty() && validate_checkpoint_budget(submitted).is_ok()
}

/// 校验交接摘要的总预算；超限 fail-closed，不截断内容。
fn validate_checkpoint_budget(summary: &str) -> AxonResult<()> {
    let summary_tokens = estimated_text_tokens(summary);
    if summary_tokens <= SESSION_NOTES_CHECKPOINT_MAX_TOKENS
        && summary.len() <= SESSION_NOTES_CHECKPOINT_MAX_BYTES
    {
        return Ok(());
    }
    Err(AxonError::runtime_failure(format!(
        "窗口交接摘要超出预算：估算 {} tokens > {} tokens，或 {} bytes > {} bytes；请压缩便签后重写摘要，运行时不会截断内容",
        summary_tokens,
        SESSION_NOTES_CHECKPOINT_MAX_TOKENS,
        summary.len(),
        SESSION_NOTES_CHECKPOINT_MAX_BYTES
    )))
}

fn normalize_checkpoint_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
