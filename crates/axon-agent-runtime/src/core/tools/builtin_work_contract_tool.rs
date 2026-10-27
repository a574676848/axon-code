//! 内置 `work_contract` 工具：开工合同的立约、修订、收口与中止。
//!
//! 工具只负责合同状态机与计划生成；执行由 `fan_out` 按 `levels` 推进。
//! 错误码与 `CONTRACT-RUNTIME.md` 的 `T-5` 同名，文案可改、码不可改。

use axon_domain::{
    AxonResult, ContractError, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind, WorkContract, WorkContractDraft, WorkUnit,
    WorkUnitKind, WorkUnitModel,
};
use serde_json::{Value, json};

/// 工具短名。
pub const WORK_CONTRACT_TOOL_NAME: &str = "work_contract";
/// 工具完整名。
pub const WORK_CONTRACT_TOOL_QUALIFIED_NAME: &str = "builtin.work_contract";
/// schema 版本。
pub const WORK_CONTRACT_TOOL_SCHEMA_VERSION: u32 = 1;
/// 工具 ID。
pub const WORK_CONTRACT_TOOL_ID: ToolId = ToolId(40);

/// 工具定义。
pub fn work_contract_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        WORK_CONTRACT_TOOL_SCHEMA_VERSION,
        ["action"],
        vec![ToolSchemaField::new(
            "action",
            ToolSchemaFieldKind::NonBlankString,
            "open 立约；amend 修订；close 收口；abort 中止。",
        )],
    );
    let properties = &mut schema.input_schema["properties"];
    properties["contractId"] = json!({"type": "string", "description": "amend/close/abort 必填。"});
    properties["revision"] = json!({"type": "integer", "minimum": 1, "description": "amend/close/abort 必填，用于拒绝陈旧修订。"});
    properties["goal"] = json!({"type": "string"});
    properties["acceptance"] = json!({"type": "string"});
    properties["criticalPath"] = json!({"type": "array", "items": {"type": "string"}});
    properties["merge"] = json!({"type": "string"});
    properties["units"] = json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": ["id", "kind", "task"],
            "properties": {
                "id": {"type": "string"},
                "kind": {"type": "string", "enum": ["read", "write", "verify"]},
                "task": {"type": "string"},
                "writeScope": {"type": "array", "items": {"type": "string"}},
                "dependsOn": {"type": "array", "items": {"type": "string"}},
                "model": {"type": "string", "enum": ["inherit", "fast"]}
            },
            "additionalProperties": false
        }
    });
    properties["outcome"] = json!({"type": "string", "enum": ["completed", "partial", "aborted"]});
    properties["notes"] = json!({"type": "string"});
    properties["reason"] = json!({"type": "string"});

    ToolDefinition {
        id: WORK_CONTRACT_TOOL_ID,
        namespace: "builtin".to_owned(),
        name: WORK_CONTRACT_TOOL_NAME.to_owned(),
        description: "任何写操作或 fan_out 之前先立约：声明目标、验收、关键路径、合并策略与单元。写单元必须声明 writeScope；写合同必须含 verify 单元。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

/// 工具错误（`T-5` 码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkContractToolError {
    /// 入参不合法。
    Invalid(String),
    /// 已有未关闭合同。
    AlreadyOpen,
    /// 合同已关闭或已中止。
    AlreadyClosed,
    /// 有在途单元。
    UnitsInFlight,
    /// 修订号陈旧。
    StaleRevision,
    /// 找不到合同。
    NotFound,
    /// 立约校验失败。
    Contract(ContractError),
    /// 修订次数超限。
    AmendLimit,
}

impl WorkContractToolError {
    /// 稳定错误码。
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "CONTRACT_INVALID",
            Self::AlreadyOpen => "CONTRACT_ALREADY_OPEN",
            Self::AlreadyClosed => "CONTRACT_ALREADY_CLOSED",
            Self::UnitsInFlight => "CONTRACT_UNITS_IN_FLIGHT",
            Self::StaleRevision => "CONTRACT_STALE_REVISION",
            Self::NotFound => "CONTRACT_NOT_FOUND",
            Self::AmendLimit => "CONTRACT_AMEND_LIMIT",
            Self::Contract(error) => error.code(),
        }
    }

    /// 面向模型的说明。
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Invalid(reason) => format!("work_contract 入参不合法：{reason}"),
            Self::AlreadyOpen => "已有未关闭的合同，先 close 或 abort".to_owned(),
            Self::AlreadyClosed => "合同已关闭或已中止，请重新立约".to_owned(),
            Self::UnitsInFlight => "仍有在途单元，等待收敛或先 abort".to_owned(),
            Self::StaleRevision => "修订号陈旧，请重新读取合同投影".to_owned(),
            Self::NotFound => "合同不存在，请重新立约".to_owned(),
            Self::AmendLimit => "修订次数超限，请新开合同".to_owned(),
            Self::Contract(error) => error.message(),
        }
    }
}

/// 合同状态机。
#[derive(Debug, Default)]
pub struct WorkContractRuntime {
    active: Option<ActiveContract>,
    finished: Vec<WorkContract>,
    next_id: u64,
}

#[derive(Debug)]
struct ActiveContract {
    contract: WorkContract,
    amendments: u32,
    in_flight: usize,
}

/// 一次工具调用的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkContractOutcome {
    /// 返回给模型的结构化字段。
    pub value: Value,
}

impl WorkContractRuntime {
    /// 是否有已批准/执行中的合同。
    #[must_use]
    pub fn active_contract(&self) -> Option<&WorkContract> {
        self.active.as_ref().map(|active| &active.contract)
    }

    /// 标记合同进入执行态。
    pub fn mark_executing(&mut self) -> Result<(), WorkContractToolError> {
        let active = self
            .active
            .as_mut()
            .ok_or(WorkContractToolError::NotFound)?;
        active.contract.state = axon_domain::ContractState::Executing;
        Ok(())
    }

    /// 登记一个已启动单元；`close`/`amend` 会因此拒绝。
    pub fn unit_started(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.in_flight += 1;
        }
    }

    /// 登记一个单元结束。
    pub fn unit_finished(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.in_flight = active.in_flight.saturating_sub(1);
        }
    }

    /// 应用一次工具调用。
    pub fn apply(&mut self, input: &Value) -> Result<WorkContractOutcome, WorkContractToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkContractToolError::Invalid("缺少 action".to_owned()))?;
        match action {
            "open" => self.open(input),
            "amend" => self.amend(input),
            "close" => self.close(input),
            "abort" => self.abort(input),
            other => Err(WorkContractToolError::Invalid(format!(
                "未知 action：{other}"
            ))),
        }
    }

    fn open(&mut self, input: &Value) -> Result<WorkContractOutcome, WorkContractToolError> {
        if self.active.is_some() {
            return Err(WorkContractToolError::AlreadyOpen);
        }
        let draft = parse_draft(input)?;
        self.next_id += 1;
        let contract = draft
            .validate(&format!("wc-{}", self.next_id))
            .map_err(WorkContractToolError::Contract)?;
        let value = projection(&contract);
        self.active = Some(ActiveContract {
            contract,
            amendments: 0,
            in_flight: 0,
        });
        Ok(WorkContractOutcome { value })
    }

    fn amend(&mut self, input: &Value) -> Result<WorkContractOutcome, WorkContractToolError> {
        let (contract_id, revision) = identity(input)?;
        let active = self
            .active
            .as_ref()
            .ok_or(WorkContractToolError::NotFound)?;
        check_identity(active, &contract_id, revision)?;
        let draft = parse_draft(input)?;
        let contract = draft
            .validate(&contract_id)
            .map_err(WorkContractToolError::Contract)?;
        let active = self
            .active
            .as_mut()
            .ok_or(WorkContractToolError::NotFound)?;
        if active.in_flight > 0 {
            return Err(WorkContractToolError::UnitsInFlight);
        }
        if active.amendments >= 3 {
            return Err(WorkContractToolError::AmendLimit);
        }
        active.amendments += 1;
        active.contract = WorkContract {
            revision: revision + 1,
            ..contract
        };
        Ok(WorkContractOutcome {
            value: projection(&active.contract),
        })
    }

    fn close(&mut self, input: &Value) -> Result<WorkContractOutcome, WorkContractToolError> {
        let (contract_id, revision) = identity(input)?;
        let active = self
            .active
            .as_ref()
            .ok_or(WorkContractToolError::NotFound)?;
        check_identity(active, &contract_id, revision)?;
        if active.in_flight > 0 {
            return Err(WorkContractToolError::UnitsInFlight);
        }
        let outcome = input
            .get("outcome")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkContractToolError::Invalid("缺少 outcome".to_owned()))?;
        if !matches!(outcome, "completed" | "partial" | "aborted") {
            return Err(WorkContractToolError::Invalid(
                "outcome 取值非法".to_owned(),
            ));
        }
        let mut closed = self.active.take().ok_or(WorkContractToolError::NotFound)?;
        closed.contract.state = axon_domain::ContractState::Closed;
        let value = json!({
            "contract_id": closed.contract.contract_id,
            "revision": closed.contract.revision,
            "state": "closed",
            "outcome": outcome,
            "amendments": closed.amendments,
        });
        self.finished.push(closed.contract);
        Ok(WorkContractOutcome { value })
    }

    fn abort(&mut self, input: &Value) -> Result<WorkContractOutcome, WorkContractToolError> {
        let (contract_id, revision) = identity(input)?;
        let active = self
            .active
            .as_ref()
            .ok_or(WorkContractToolError::NotFound)?;
        check_identity(active, &contract_id, revision)?;
        let reason = input
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("未提供原因");
        let mut aborted = self.active.take().ok_or(WorkContractToolError::NotFound)?;
        aborted.contract.state = axon_domain::ContractState::Aborted;
        let value = json!({
            "contract_id": aborted.contract.contract_id,
            "revision": aborted.contract.revision,
            "state": "aborted",
            "reason": reason,
            "released_units": aborted.in_flight,
        });
        self.finished.push(aborted.contract);
        Ok(WorkContractOutcome { value })
    }
}

fn identity(input: &Value) -> Result<(String, u32), WorkContractToolError> {
    let contract_id = input
        .get("contractId")
        .and_then(Value::as_str)
        .ok_or_else(|| WorkContractToolError::Invalid("缺少 contractId".to_owned()))?
        .to_owned();
    let revision = input
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| WorkContractToolError::Invalid("缺少 revision".to_owned()))?;
    let revision = u32::try_from(revision)
        .map_err(|_| WorkContractToolError::Invalid("revision 超出范围".to_owned()))?;
    Ok((contract_id, revision))
}

fn check_identity(
    active: &ActiveContract,
    contract_id: &str,
    revision: u32,
) -> Result<(), WorkContractToolError> {
    if active.contract.contract_id != contract_id {
        return Err(WorkContractToolError::NotFound);
    }
    if active.contract.revision != revision {
        return Err(WorkContractToolError::StaleRevision);
    }
    if matches!(
        active.contract.state,
        axon_domain::ContractState::Closed | axon_domain::ContractState::Aborted
    ) {
        return Err(WorkContractToolError::AlreadyClosed);
    }
    Ok(())
}

fn parse_draft(input: &Value) -> Result<WorkContractDraft, WorkContractToolError> {
    let goal = text(input, "goal")?;
    let acceptance = text(input, "acceptance")?;
    let merge = text(input, "merge")?;
    let critical_path = string_array(input, "criticalPath")?;
    let units_value = input
        .get("units")
        .and_then(Value::as_array)
        .ok_or_else(|| WorkContractToolError::Invalid("缺少 units".to_owned()))?;
    let units = units_value
        .iter()
        .map(parse_unit)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(WorkContractDraft {
        goal,
        acceptance,
        critical_path,
        merge,
        units,
    })
}

fn parse_unit(value: &Value) -> Result<WorkUnit, WorkContractToolError> {
    let id = text(value, "id")?;
    let kind = match text(value, "kind")?.as_str() {
        "read" => WorkUnitKind::Read,
        "write" => WorkUnitKind::Write,
        "verify" => WorkUnitKind::Verify,
        other => {
            return Err(WorkContractToolError::Invalid(format!(
                "单元 {id} 的 kind 非法：{other}"
            )));
        }
    };
    let model = match value.get("model").and_then(Value::as_str) {
        None | Some("inherit") => WorkUnitModel::Inherit,
        Some("fast") => WorkUnitModel::Fast,
        Some(other) => {
            return Err(WorkContractToolError::Invalid(format!(
                "单元 {id} 的 model 非法：{other}"
            )));
        }
    };
    Ok(WorkUnit {
        id,
        kind,
        task: text(value, "task")?,
        write_scope: optional_string_array(value, "writeScope")?,
        depends_on: optional_string_array(value, "dependsOn")?,
        model,
    })
}

fn text(value: &Value, field: &str) -> Result<String, WorkContractToolError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| WorkContractToolError::Invalid(format!("缺少 {field}")))
}

fn string_array(value: &Value, field: &str) -> Result<Vec<String>, WorkContractToolError> {
    let items = value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| WorkContractToolError::Invalid(format!("缺少 {field}")))?;
    array_to_strings(items, field)
}

fn optional_string_array(value: &Value, field: &str) -> Result<Vec<String>, WorkContractToolError> {
    let Some(items) = value.get(field).and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    array_to_strings(items, field)
}

fn array_to_strings(items: &[Value], field: &str) -> Result<Vec<String>, WorkContractToolError> {
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| WorkContractToolError::Invalid(format!("{field} 只能包含字符串")))
        })
        .collect()
}

fn projection(contract: &WorkContract) -> Value {
    json!({
        "contract_id": contract.contract_id,
        "revision": contract.revision,
        "state": "approved",
        "call_width": contract.call_width,
        "unit_width": contract.unit_width,
        "levels": contract.levels,
        "ready_units": contract.levels.first().cloned().unwrap_or_default(),
    })
}

/// 校验一次 `work_contract` 调用并返回投影；`AxonResult` 版本供执行器复用。
pub fn apply_work_contract(runtime: &mut WorkContractRuntime, input: &Value) -> AxonResult<Value> {
    runtime
        .apply(input)
        .map(|outcome| outcome.value)
        .map_err(|error| {
            axon_domain::AxonError::tool_execution_failed(format!(
                "{}: {}",
                error.code(),
                error.message()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str, kind: &str, scope: &[&str]) -> Value {
        json!({
            "id": id,
            "kind": kind,
            "task": "自包含任务：写清目标、涉及范围、证据要求、交付格式与验证标准；子代理看不到父对话。",
            "writeScope": scope,
        })
    }

    fn open_input(units: Vec<Value>) -> Value {
        json!({
            "action": "open",
            "goal": "完成并行调度",
            "acceptance": "门禁全绿",
            "criticalPath": ["u1"],
            "merge": "按单元序合并",
            "units": units,
        })
    }

    #[test]
    fn open_builds_plan_and_blocks_second_open() {
        let mut runtime = WorkContractRuntime::default();
        let value = runtime
            .apply(&open_input(vec![
                unit("u1", "read", &[]),
                unit("u2", "write", &["src/lib.rs"]),
                unit("u3", "verify", &[]),
            ]))
            .expect("立约应成功");
        assert_eq!(value.value["state"], "approved");
        assert_eq!(value.value["levels"].as_array().expect("levels").len(), 1);
        assert_eq!(value.value["ready_units"], json!(["u1", "u2", "u3"]));

        let error = runtime
            .apply(&open_input(vec![unit("u1", "read", &[])]))
            .unwrap_err();
        assert_eq!(error.code(), "CONTRACT_ALREADY_OPEN");
    }

    #[test]
    fn amend_increments_revision_and_rejects_stale() {
        let mut runtime = WorkContractRuntime::default();
        runtime
            .apply(&open_input(vec![unit("u1", "read", &[])]))
            .expect("立约应成功");
        let mut amend = open_input(vec![unit("u1", "read", &[])]);
        amend["action"] = json!("amend");
        amend["contractId"] = json!("wc-1");
        amend["revision"] = json!(2);
        let error = runtime.apply(&amend).unwrap_err();
        assert_eq!(error.code(), "CONTRACT_STALE_REVISION");

        amend["revision"] = json!(1);
        let value = runtime.apply(&amend).expect("修订应成功");
        assert_eq!(value.value["revision"], 2);
    }

    #[test]
    fn close_requires_outcome_and_releases_contract() {
        let mut runtime = WorkContractRuntime::default();
        runtime
            .apply(&open_input(vec![unit("u1", "read", &[])]))
            .expect("立约应成功");
        let mut close = json!({"action": "close", "contractId": "wc-1", "revision": 1});
        assert_eq!(
            runtime.apply(&close).unwrap_err().code(),
            "CONTRACT_INVALID"
        );
        close["outcome"] = json!("completed");
        let value = runtime.apply(&close).expect("收口应成功");
        assert_eq!(value.value["state"], "closed");
        assert!(runtime.active_contract().is_none());
    }

    #[test]
    fn abort_releases_in_flight_units() {
        let mut runtime = WorkContractRuntime::default();
        runtime
            .apply(&open_input(vec![unit("u1", "read", &[])]))
            .expect("立约应成功");
        runtime.unit_started();
        let value = runtime
            .apply(&json!({
                "action": "abort",
                "contractId": "wc-1",
                "revision": 1,
                "reason": "改方案"
            }))
            .expect("中止应成功");
        assert_eq!(value.value["released_units"], 1);
        assert!(runtime.active_contract().is_none());
    }

    #[test]
    fn write_contract_without_verify_unit_reports_domain_code() {
        let mut runtime = WorkContractRuntime::default();
        let error = runtime
            .apply(&open_input(vec![unit("u1", "write", &["src/lib.rs"])]))
            .unwrap_err();
        assert_eq!(error.code(), "CONTRACT_VERIFY_MISSING");
    }
}
