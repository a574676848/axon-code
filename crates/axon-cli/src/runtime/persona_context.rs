//! 用户级人格上下文：稳定段、动态段、降级与遥测。
//!
//! 稳定段承载人格内核与已发布习得风格，进 session-stable 缓存分段；动态段承载本轮
//! 场景读数，进动态尾部且不参与缓存。任何异常都显式降级，不阻断回合。

use std::{fs, io::ErrorKind, path::Path, time::Instant};

use axon_agent_runtime::{
    AgentModelMessage, PromptFragment, PromptFragmentMetadata, PromptFragmentPipeline,
    SESSION_STABLE_SOUL_END_MARKER, SESSION_STABLE_SOUL_START_MARKER,
};
use axon_persona::{
    PersonaCore, PersonaState, SceneFrame, SoulDocument,
    constants::PERSONA_ASSEMBLY_BUDGET_MS,
    domain::{
        Daypart,
        expression_policy::resolve_expression,
        scene_frame::{Audience, Channel},
    },
    infrastructure::{PersonaStore, persona_store_path},
    render_dynamic, render_stable,
};
use axon_session_runtime::first_token_telemetry_enabled;
use chrono::Timelike as _;

/// 用户级人设文件名。
pub(crate) const USER_SOUL_FILE_NAME: &str = "SOUL.md";
/// 动态段起始标记。
pub(crate) const PERSONA_STATE_START_MARKER: &str = "<persona-state-context>";
/// 动态段结束标记。
pub(crate) const PERSONA_STATE_END_MARKER: &str = "</persona-state-context>";

/// 本轮场景输入。
pub(crate) struct PersonaSceneInput {
    /// 当前渠道。
    pub(crate) channel: Channel,
    /// 当前受众。
    pub(crate) audience: Audience,
    /// 是否存在 Workspace。
    pub(crate) has_workspace: bool,
}

/// 人格上下文装配结果。
pub(crate) struct PersonaContext {
    /// 稳定段消息。
    pub(crate) stable: Option<AgentModelMessage>,
    /// 动态段消息。
    pub(crate) dynamic: Option<AgentModelMessage>,
    /// 降级告警。
    pub(crate) warnings: Vec<String>,
    /// 是否发生降级。
    pub(crate) degraded: bool,
}

impl PersonaContext {
    pub(crate) fn empty() -> Self {
        Self {
            stable: None,
            dynamic: None,
            warnings: Vec::new(),
            degraded: false,
        }
    }
}

/// 读取人格上下文。
///
/// 根目录与 CLI、网关、doctor 保持同一解析口径（`runtime_axon_root_path`）。
///
/// # Errors
///
/// 用户目录无法解析或人设文件不可读时返回中文诊断；文件缺失与内容无效按降级处理，
/// 不返回错误。
pub(crate) fn load_persona_context(
    config_path: &Path,
    input: PersonaSceneInput,
) -> Result<PersonaContext, String> {
    let started = Instant::now();
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|error| format!("无法解析 Axon 运行根目录：{error:?}"))?;
    load_persona_context_from_root(&root, input, started, PERSONA_ASSEMBLY_BUDGET_MS)
}

/// 在给定装配预算内读取人格上下文。
///
/// `budget_ms` 是显式依赖而非直接读常量：真实装配耗时会随机器负载浮动，调用方
/// （尤其是测试）必须能自行决定本次允许花多久。写死常量会让「有动态段」这类结构
/// 断言退化成「只在空闲机器上成立」，并行门禁下必然翻车。
fn load_persona_context_from_root(
    root: &Path,
    input: PersonaSceneInput,
    started: Instant,
    budget_ms: u64,
) -> Result<PersonaContext, String> {
    // 合同第 11.6 条：非 trace 级别不执行分段计时，只保留熔断预算所需的一次总计时。
    let telemetry = first_token_telemetry_enabled();
    let mut soul_read_us = 0;
    let mut soul_parse_us = 0;
    let mut state_query_us = 0;
    let mut policy_us = 0;

    let read_started = telemetry.then(Instant::now);
    let raw = match fs::read_to_string(root.join(USER_SOUL_FILE_NAME)) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(PersonaContext::empty()),
        Err(error) => {
            // 权限、编码或 IO 故障只降级，不阻断回合（INV-8）。
            let mut context = PersonaContext::empty();
            context.warnings.push(format!(
                "人设未装配：读取 {USER_SOUL_FILE_NAME} 失败（{error}）"
            ));
            return Ok(context);
        }
    };
    if raw.trim().is_empty() {
        return Ok(PersonaContext::empty());
    }
    if let Some(started) = read_started {
        soul_read_us = elapsed_us(started);
    }

    let parse_started = telemetry.then(Instant::now);
    let document = match SoulDocument::parse(&raw) {
        Ok(document) => document,
        Err(error) => {
            let mut context = PersonaContext::empty();
            context
                .warnings
                .push(format!("人设未装配：{USER_SOUL_FILE_NAME} {error}"));
            return Ok(context);
        }
    };
    if let Some(started) = parse_started {
        soul_parse_us = elapsed_us(started);
    }

    let state_started = telemetry.then(Instant::now);
    let loaded = load_state_and_style(root, input.channel);
    if let Some(started) = state_started {
        state_query_us = elapsed_us(started);
    }
    let mut state_warnings = Vec::new();
    // 库不可用或无 owner：只装配稳定段，不伪造状态，也不渲染动态段。
    let (state, styles) = match loaded {
        Ok(values) => values,
        Err(reason) => {
            state_warnings.push(format!("人格状态未装配：{reason}"));
            let stable = stable_message(&document.core, &[]);
            return Ok(PersonaContext {
                stable,
                dynamic: None,
                warnings: state_warnings,
                degraded: false,
            });
        }
    };

    // 库查询已可能耗尽预算：先判一次，超预算就不再跑策略、直接降级。
    if elapsed_us(started) >= budget_ms.saturating_mul(1_000) {
        let stable = stable_message(&document.core, &styles);
        let mut context = PersonaContext {
            stable,
            dynamic: None,
            warnings: state_warnings,
            degraded: true,
        };
        context.warnings.push(format!(
            "人格装配超时（{}ms），本轮降级",
            elapsed_us(started) / 1_000
        ));
        return Ok(context);
    }

    let policy_started = telemetry.then(Instant::now);
    let now_ms = axon_time::unix_now_ms();
    let scene = scene_frame(&input, &state, now_ms);
    let directive = resolve_expression(&document.core, &state, &scene, now_ms);
    if let Some(started) = policy_started {
        policy_us = elapsed_us(started);
    }

    let stable = stable_message(&document.core, &styles);
    let stable_len = stable
        .as_ref()
        .map(|message| message.content.len())
        .unwrap_or(0);
    let mut context = PersonaContext {
        stable,
        dynamic: None,
        warnings: state_warnings,
        degraded: false,
    };
    // 体积承诺必须在装配处兑现：超软门槛预警，超硬上限显式告警（不截断不变量）。
    if stable_len > 0 && axon_persona::exceeds_hard_limit_len(stable_len) {
        context.warnings.push(format!(
            "人格稳定段超出上限（{stable_len} B），已按不变量优先原则保留"
        ));
    } else if stable_len > 0 && axon_persona::exceeds_soft_limit_len(stable_len) {
        context
            .warnings
            .push(format!("人格稳定段接近上限（{stable_len} B）"));
    }

    let stable_bytes = context
        .stable
        .as_ref()
        .map_or(0, |message| message.content.len());
    let dynamic_bytes = context
        .dynamic
        .as_ref()
        .map_or(0, |message| message.content.len());
    let total_us = elapsed_us(started);
    if total_us >= budget_ms.saturating_mul(1_000) {
        context.degraded = true;
        context
            .warnings
            .push(format!("人格装配超时（{}ms），本轮降级", total_us / 1_000));
    } else {
        context.dynamic = dynamic_message(&directive, &scene);
    }

    if first_token_telemetry_enabled() {
        axon_logger::debug(&format!(
            "persona_context_telemetry={{\"soul_read_us\":{soul_read_us},\"soul_parse_us\":{soul_parse_us},\"state_query_us\":{state_query_us},\"policy_us\":{policy_us},\"total_us\":{total_us},\"stable_bytes\":{stable_bytes},\"dynamic_bytes\":{dynamic_bytes},\"degraded\":{}}}",
            context.degraded
        ));
    }
    Ok(context)
}

fn load_state_and_style(
    root: &Path,
    channel: Channel,
) -> Result<(PersonaState, Vec<axon_persona::infrastructure::StyleEntry>), String> {
    let now_ms = axon_time::unix_now_ms();
    let path = persona_store_path(root);
    if !path.exists() {
        // 库尚未建立：状态就是基线，但这是「尚未产生」，不是故障。
        return Ok((PersonaState::baseline(now_ms), Vec::new()));
    }
    // 只读打开：不在首字关键路径上建库或获取写锁。
    let store =
        PersonaStore::open_read_only(&path).map_err(|error| format!("人格库不可用：{error}"))?;
    let owner = crate::runtime_principal_context::load_run_principal(root, channel.as_token())
        .map_err(|error| format!("本机 owner 身份不可用：{error}"))?
        .map(|principal| principal.owner_id.as_str().to_owned());
    let Some(owner) = owner else {
        return Err("本机尚无 owner 身份".to_owned());
    };
    let state = store
        .load_state(&owner)
        .map_err(|error| format!("读取人格状态失败：{error}"))?
        .unwrap_or_else(|| PersonaState::baseline(now_ms));
    let styles = store
        .published_style(&owner)
        .map_err(|error| format!("读取习得风格失败：{error}"))?
        .entries;
    Ok((state, styles))
}

fn scene_frame(input: &PersonaSceneInput, state: &PersonaState, now_ms: u64) -> SceneFrame {
    let hour = u8::try_from(chrono::Local::now().hour()).unwrap_or(0);
    let idle_gap_ms = now_ms.saturating_sub(state.updated_at_ms);
    SceneFrame::new(
        input.channel,
        input.audience,
        Daypart::from_local_hour(hour),
        input.has_workspace,
        idle_gap_ms,
    )
}

fn stable_message(
    core: &PersonaCore,
    styles: &[axon_persona::infrastructure::StyleEntry],
) -> Option<AgentModelMessage> {
    let body = render_stable(core, styles);
    let fragment = StablePersonaFragment { content: body };
    let mut pipeline = PromptFragmentPipeline::new();
    pipeline.push_fragment(Box::new(fragment));
    pipeline.into_messages().into_iter().next()
}

fn dynamic_message(
    directive: &axon_persona::ExpressionDirective,
    _scene: &SceneFrame,
) -> Option<AgentModelMessage> {
    let body = render_dynamic(directive);
    let fragment = DynamicPersonaFragment { content: body };
    let mut pipeline = PromptFragmentPipeline::new();
    pipeline.push_fragment(Box::new(fragment));
    pipeline.into_messages().into_iter().next()
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[derive(Debug)]
struct StablePersonaFragment {
    content: String,
}

impl PromptFragment for StablePersonaFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::stable_system()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            SESSION_STABLE_SOUL_START_MARKER,
            SESSION_STABLE_SOUL_END_MARKER,
        )
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content)
    }
}

#[derive(Debug)]
struct DynamicPersonaFragment {
    content: String,
}

impl PromptFragment for DynamicPersonaFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (PERSONA_STATE_START_MARKER, PERSONA_STATE_END_MARKER)
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content)
    }
}

#[cfg(test)]
#[path = "persona_context_tests.rs"]
mod tests;
