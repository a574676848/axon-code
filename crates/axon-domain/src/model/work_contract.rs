//! 开工合同的领域模型与校验（PWC-1 `K` 条款与 `T-1`）。
//!
//! 合同是调度计划本身：立约时完成单元校验、依赖分层与宽度切层，
//! 执行期只按 `levels` 推进。错误码与 `CONTRACT-RUNTIME.md` 的 `T-5` 同名。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// 单合同单元上限（`limits.yaml` 的 `agent.max_fan_out_units`）。
pub const MAX_CONTRACT_UNITS: usize = 16;
/// 单元任务的最小字符数（`limits.yaml` 的 `contract.min_unit_task_chars`）。
pub const MIN_UNIT_TASK_CHARS: usize = 40;
/// 工具调用宽度预设（`agent.max_parallel_tool_calls`）。
pub const PRESET_CALL_WIDTH: usize = 8;
/// 扇出单元宽度预设（`agent.max_fan_out_units`）。
pub const PRESET_UNIT_WIDTH: usize = 8;

/// 单元类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitKind {
    /// 只读侦察。
    Read,
    /// 写单元，必须声明写集。
    Write,
    /// 验证单元，写合同必须至少有一个。
    Verify,
}

/// 单元模型档位；同一合同必须一致（`R-12`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitModel {
    /// 继承主代理档位。
    #[default]
    Inherit,
    /// 快档。
    Fast,
}

/// 一个可执行单元。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkUnit {
    /// 合同内唯一 id。
    pub id: String,
    /// 单元类型。
    pub kind: WorkUnitKind,
    /// 自包含任务说明，子代理看不到父对话。
    pub task: String,
    /// 写集，仓库相对路径；写单元必填。
    #[serde(default)]
    pub write_scope: Vec<String>,
    /// 依赖的单元 id。
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// 模型档位。
    #[serde(default)]
    pub model: WorkUnitModel,
}

/// 立约输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkContractDraft {
    /// 目标。
    pub goal: String,
    /// 验收标准。
    pub acceptance: String,
    /// 关键路径上的单元 id，主线由主代理自己做。
    pub critical_path: Vec<String>,
    /// 收口合并策略。
    pub merge: String,
    /// 全部单元。
    pub units: Vec<WorkUnit>,
}

/// 合同状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractState {
    /// 已批准，可执行。
    Approved,
    /// 执行中。
    Executing,
    /// 已收口。
    Closed,
    /// 已中止。
    Aborted,
}

/// 校验通过后的合同。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkContract {
    /// 合同 id。
    pub contract_id: String,
    /// 修订号。
    pub revision: u32,
    /// 状态。
    pub state: ContractState,
    /// 工具调用宽度。
    pub call_width: usize,
    /// 单元宽度。
    pub unit_width: usize,
    /// 分层计划，层内可并发，层间顺序。
    pub levels: Vec<Vec<String>>,
    /// 必须串行的单元（当前为空，保留字段）。
    pub serialized: Vec<String>,
}

/// 立约错误，`code` 与 `T-5` 错误码一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractError {
    /// 必填缺失或格式错。
    Invalid(&'static str),
    /// 单元数超限。
    UnitsOverLimit,
    /// 写单元缺写集。
    WriteScopeMissing(String),
    /// 写集不是合法仓库相对路径。
    WriteScopeInvalid(String),
    /// 同层写集相交或读写同路径。
    WriteScopeOverlap(String, String),
    /// 依赖引用未声明单元。
    DependencyUnknown(String),
    /// 依赖成环。
    DependencyCycle,
    /// 无关键路径。
    CriticalPathEmpty,
    /// 写合同缺验证单元。
    VerifyMissing,
    /// 单元任务过短。
    TaskTooShort(String),
    /// 同合同模型档位不一致。
    ModelMismatch,
}

impl ContractError {
    /// 稳定错误码。
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "CONTRACT_INVALID",
            Self::UnitsOverLimit => "CONTRACT_UNITS_OVER_LIMIT",
            Self::WriteScopeMissing(_) => "CONTRACT_WRITE_SCOPE_MISSING",
            Self::WriteScopeInvalid(_) => "CONTRACT_WRITE_SCOPE_INVALID",
            Self::WriteScopeOverlap(_, _) => "CONTRACT_WRITE_SCOPE_OVERLAP",
            Self::DependencyUnknown(_) => "CONTRACT_DEPENDENCY_UNKNOWN",
            Self::DependencyCycle => "CONTRACT_DEPENDENCY_CYCLE",
            Self::CriticalPathEmpty => "CONTRACT_CRITICAL_PATH_EMPTY",
            Self::VerifyMissing => "CONTRACT_VERIFY_MISSING",
            Self::TaskTooShort(_) => "CONTRACT_TASK_TOO_SHORT",
            Self::ModelMismatch => "CONTRACT_MODEL_MISMATCH",
        }
    }

    /// 面向模型的说明。
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Invalid(reason) => format!("合同格式不合法：{reason}"),
            Self::UnitsOverLimit => {
                format!("单元数量超过上限 {MAX_CONTRACT_UNITS}，请拆分合同")
            }
            Self::WriteScopeMissing(unit) => format!("写单元 {unit} 未声明 write_scope"),
            Self::WriteScopeInvalid(unit) => {
                format!(
                    "写单元 {unit} 的 write_scope 必须是仓库相对路径，不能是绝对路径、.. 或通配符"
                )
            }
            Self::WriteScopeOverlap(left, right) => {
                format!("同层单元 {left} 与 {right} 的读写路径相交，请加依赖或拆合同")
            }
            Self::DependencyUnknown(unit) => format!("依赖引用未声明的单元：{unit}"),
            Self::DependencyCycle => "单元依赖成环，请打破环".to_owned(),
            Self::CriticalPathEmpty => "必须声明关键路径，主线由主代理自己做".to_owned(),
            Self::VerifyMissing => "写合同必须至少包含一个 verify 单元".to_owned(),
            Self::TaskTooShort(unit) => {
                format!("单元 {unit} 的 task 至少 {MIN_UNIT_TASK_CHARS} 个字符，需自包含")
            }
            Self::ModelMismatch => "同一合同的单元模型档位必须一致".to_owned(),
        }
    }
}

impl WorkContractDraft {
    /// 校验并生成计划；失败时返回稳定错误码。
    pub fn validate(&self, contract_id: &str) -> Result<WorkContract, ContractError> {
        if self.goal.trim().is_empty() {
            return Err(ContractError::Invalid("goal 不能为空"));
        }
        if self.acceptance.trim().is_empty() {
            return Err(ContractError::Invalid("acceptance 不能为空"));
        }
        if self.merge.trim().is_empty() {
            return Err(ContractError::Invalid("merge 不能为空"));
        }
        if self.units.is_empty() {
            return Err(ContractError::Invalid("units 不能为空"));
        }
        if self.units.len() > MAX_CONTRACT_UNITS {
            return Err(ContractError::UnitsOverLimit);
        }

        let mut index: BTreeMap<&str, usize> = BTreeMap::new();
        for (position, unit) in self.units.iter().enumerate() {
            if unit.id.trim().is_empty() {
                return Err(ContractError::Invalid("单元 id 不能为空"));
            }
            if index.insert(unit.id.as_str(), position).is_some() {
                return Err(ContractError::Invalid("单元 id 不能重复"));
            }
        }

        let models: BTreeSet<WorkUnitModel> = self.units.iter().map(|unit| unit.model).collect();
        if models.len() > 1 {
            return Err(ContractError::ModelMismatch);
        }

        let mut has_write = false;
        let mut has_verify = false;
        for unit in &self.units {
            if unit.task.trim().chars().count() < MIN_UNIT_TASK_CHARS {
                return Err(ContractError::TaskTooShort(unit.id.clone()));
            }
            match unit.kind {
                WorkUnitKind::Write => {
                    has_write = true;
                    if unit.write_scope.is_empty() {
                        return Err(ContractError::WriteScopeMissing(unit.id.clone()));
                    }
                    for scope in &unit.write_scope {
                        if !is_valid_scope(scope) {
                            return Err(ContractError::WriteScopeInvalid(unit.id.clone()));
                        }
                    }
                }
                WorkUnitKind::Verify => has_verify = true,
                WorkUnitKind::Read => {}
            }
        }
        if has_write && !has_verify {
            return Err(ContractError::VerifyMissing);
        }
        if self.critical_path.is_empty() {
            return Err(ContractError::CriticalPathEmpty);
        }
        for step in &self.critical_path {
            if !index.contains_key(step.as_str()) {
                return Err(ContractError::DependencyUnknown(step.clone()));
            }
        }

        let levels = compute_levels(&self.units, &index)?;
        for level in &levels {
            reject_overlaps(level, &self.units, &index)?;
        }
        Ok(WorkContract {
            contract_id: contract_id.to_owned(),
            revision: 1,
            state: ContractState::Approved,
            call_width: PRESET_CALL_WIDTH,
            unit_width: PRESET_UNIT_WIDTH,
            levels: split_wide_levels(&levels),
            serialized: Vec::new(),
        })
    }
}

fn is_valid_scope(scope: &str) -> bool {
    let trimmed = scope.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.contains("..")
        || trimmed.contains(['*', '?', '[', '{'])
    {
        return false;
    }
    // Windows 盘符形态同样视为绝对路径。
    let mut chars = trimmed.chars();
    !matches!(
        (chars.next(), chars.next()),
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
    )
}

/// 按依赖计算层，返回每层的单元 id。
fn compute_levels(
    units: &[WorkUnit],
    index: &BTreeMap<&str, usize>,
) -> Result<Vec<Vec<String>>, ContractError> {
    let mut level_of = vec![None::<usize>; units.len()];
    let mut visiting = vec![false; units.len()];

    for position in 0..units.len() {
        assign_level(position, units, index, &mut level_of, &mut visiting)?;
    }

    let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (position, unit) in units.iter().enumerate() {
        let level = level_of[position].unwrap_or(0);
        grouped.entry(level).or_default().push(unit.id.clone());
    }
    Ok(grouped.into_values().collect())
}

fn assign_level(
    position: usize,
    units: &[WorkUnit],
    index: &BTreeMap<&str, usize>,
    level_of: &mut [Option<usize>],
    visiting: &mut [bool],
) -> Result<usize, ContractError> {
    if let Some(level) = level_of[position] {
        return Ok(level);
    }
    if visiting[position] {
        return Err(ContractError::DependencyCycle);
    }
    visiting[position] = true;
    let mut level = 0usize;
    for dependency in &units[position].depends_on {
        let Some(&dependency_position) = index.get(dependency.as_str()) else {
            return Err(ContractError::DependencyUnknown(dependency.clone()));
        };
        let dependency_level = assign_level(dependency_position, units, index, level_of, visiting)?;
        level = level.max(dependency_level + 1);
    }
    visiting[position] = false;
    level_of[position] = Some(level);
    Ok(level)
}

/// 同层内写集相交视为冲突；纯读单元没有写集，不参与该判定。
fn reject_overlaps(
    level: &[String],
    units: &[WorkUnit],
    index: &BTreeMap<&str, usize>,
) -> Result<(), ContractError> {
    for (position, left_id) in level.iter().enumerate() {
        let left = &units[index[left_id.as_str()]];
        if left.write_scope.is_empty() {
            continue;
        }
        for right_id in &level[position + 1..] {
            let right = &units[index[right_id.as_str()]];
            if right.write_scope.is_empty() {
                continue;
            }
            if scopes_overlap(&left.write_scope, &right.write_scope) {
                return Err(ContractError::WriteScopeOverlap(
                    left.id.clone(),
                    right.id.clone(),
                ));
            }
        }
    }
    Ok(())
}

fn scopes_overlap(left: &[String], right: &[String]) -> bool {
    let normalized_left = left.iter().map(|scope| normalize_scope(scope));
    let normalized_right = right
        .iter()
        .map(|scope| normalize_scope(scope))
        .collect::<Vec<_>>();
    normalized_left.into_iter().any(|one| {
        normalized_right
            .iter()
            .any(|other| path_within(&one, other) || path_within(other, &one))
    })
}

fn normalize_scope(scope: &str) -> String {
    scope
        .trim()
        .trim_start_matches("./")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned()
}

fn path_within(ancestor: &str, path: &str) -> bool {
    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// 层宽超过单元宽度时按顺序切层，保持层间顺序。
fn split_wide_levels(levels: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut result = Vec::with_capacity(levels.len());
    for level in levels {
        for chunk in level.chunks(PRESET_UNIT_WIDTH) {
            result.push(chunk.to_vec());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str, kind: WorkUnitKind, scope: &[&str]) -> WorkUnit {
        WorkUnit {
            id: id.to_owned(),
            kind,
            task: "自包含任务：写清目标、涉及范围、证据要求、交付格式与验证标准；子代理看不到父对话，所有上下文必须落在本字段里。"
                .to_owned(),
            write_scope: scope.iter().map(|scope| (*scope).to_owned()).collect(),
            depends_on: Vec::new(),
            model: WorkUnitModel::Inherit,
        }
    }

    fn draft(units: Vec<WorkUnit>) -> WorkContractDraft {
        WorkContractDraft {
            goal: "完成并行调度".to_owned(),
            acceptance: "门禁全绿".to_owned(),
            critical_path: vec![units[0].id.clone()],
            merge: "按单元序合并".to_owned(),
            units,
        }
    }

    #[test]
    fn builds_levels_from_dependencies() {
        let mut write = unit("u2", WorkUnitKind::Write, &["src/lib.rs"]);
        write.depends_on = vec!["u1".to_owned()];
        let mut verify = unit("u3", WorkUnitKind::Verify, &[]);
        verify.depends_on = vec!["u2".to_owned()];
        let contract = draft(vec![unit("u1", WorkUnitKind::Read, &[]), write, verify])
            .validate("wc-1")
            .expect("合同应通过校验");
        assert_eq!(contract.levels, vec![vec!["u1"], vec!["u2"], vec!["u3"]]);
        assert_eq!(contract.state, ContractState::Approved);
        assert_eq!(contract.call_width, PRESET_CALL_WIDTH);
    }

    #[test]
    fn wide_level_is_split_by_unit_width() {
        let units = (0..PRESET_UNIT_WIDTH + 2)
            .map(|index| unit(&format!("u{index}"), WorkUnitKind::Read, &[]))
            .collect::<Vec<_>>();
        let contract = draft(units).validate("wc-2").expect("合同应通过校验");
        assert_eq!(contract.levels.len(), 2);
        assert_eq!(contract.levels[0].len(), PRESET_UNIT_WIDTH);
        assert_eq!(contract.levels[1].len(), 2);
    }

    #[test]
    fn write_unit_requires_scope_and_verify_unit() {
        let mut draft = draft(vec![unit("u1", WorkUnitKind::Write, &[])]);
        draft.critical_path = vec!["u1".to_owned()];
        assert_eq!(
            draft.validate("wc-3").unwrap_err().code(),
            "CONTRACT_WRITE_SCOPE_MISSING"
        );
        draft.units[0].write_scope = vec!["src/lib.rs".to_owned()];
        assert_eq!(
            draft.validate("wc-3").unwrap_err().code(),
            "CONTRACT_VERIFY_MISSING"
        );
    }

    #[test]
    fn scope_rejects_absolute_parent_and_wildcards() {
        for scope in ["/etc/passwd", "../out.rs", "src/*.rs", "C:/tmp/a.rs"] {
            let mut draft = draft(vec![
                unit("u1", WorkUnitKind::Write, &[scope]),
                unit("u2", WorkUnitKind::Verify, &[]),
            ]);
            draft.critical_path = vec!["u1".to_owned()];
            assert_eq!(
                draft.validate("wc-4").unwrap_err().code(),
                "CONTRACT_WRITE_SCOPE_INVALID",
                "{scope} 应被拒绝"
            );
        }
    }

    #[test]
    fn overlapping_writes_in_one_level_are_rejected() {
        let draft = draft(vec![
            unit("u1", WorkUnitKind::Write, &["src/lib.rs"]),
            unit("u2", WorkUnitKind::Write, &["src/lib.rs"]),
            unit("u3", WorkUnitKind::Verify, &[]),
        ]);
        assert_eq!(
            draft.validate("wc-5").unwrap_err().code(),
            "CONTRACT_WRITE_SCOPE_OVERLAP"
        );
    }

    #[test]
    fn read_units_do_not_participate_in_write_overlap() {
        let draft = draft(vec![
            unit("u1", WorkUnitKind::Read, &[]),
            unit("u2", WorkUnitKind::Write, &["src"]),
            unit("u3", WorkUnitKind::Verify, &[]),
        ]);
        // 纯读单元没有读集字段，同层读写交叉由运行期路径锁兜底。
        assert!(draft.validate("wc-6").is_ok());
    }

    #[test]
    fn unknown_dependency_and_cycle_are_rejected() {
        let mut unknown = unit("u1", WorkUnitKind::Read, &[]);
        unknown.depends_on = vec!["nope".to_owned()];
        assert_eq!(
            draft(vec![unknown]).validate("wc-7").unwrap_err().code(),
            "CONTRACT_DEPENDENCY_UNKNOWN"
        );

        let mut first = unit("u1", WorkUnitKind::Read, &[]);
        first.depends_on = vec!["u2".to_owned()];
        let mut second = unit("u2", WorkUnitKind::Read, &[]);
        second.depends_on = vec!["u1".to_owned()];
        assert_eq!(
            draft(vec![first, second])
                .validate("wc-8")
                .unwrap_err()
                .code(),
            "CONTRACT_DEPENDENCY_CYCLE"
        );
    }

    #[test]
    fn short_task_and_model_mismatch_are_rejected() {
        let mut short = unit("u1", WorkUnitKind::Read, &[]);
        short.task = "太短".to_owned();
        assert_eq!(
            draft(vec![short]).validate("wc-9").unwrap_err().code(),
            "CONTRACT_TASK_TOO_SHORT"
        );

        let mut fast = unit("u2", WorkUnitKind::Read, &[]);
        fast.model = WorkUnitModel::Fast;
        assert_eq!(
            draft(vec![unit("u1", WorkUnitKind::Read, &[]), fast])
                .validate("wc-10")
                .unwrap_err()
                .code(),
            "CONTRACT_MODEL_MISMATCH"
        );
    }
}
