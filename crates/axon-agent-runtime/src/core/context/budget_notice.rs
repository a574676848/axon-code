//! 上下文预算运行时提示的触发判定、去重标记与文案。
//!
//! 三档提示全部走 `AgentRuntimeNoteKind` 的低权威通道，以 cache-excluded user 消息承载：
//! 追加提示不会让稳定前缀失效，成本只有正文本身，因此单个窗口内多发一次是可接受的开销。
//!
//! 每条提示的开头都是稳定前缀。运行时靠前缀从当前窗口的历史消息里还原“已经提醒过”，
//! 进程重启或 failover 重跑时据此去重。改文案时不要改动前缀本身，否则旧窗口会重复提醒。
//!
//! 判定本身只在 turn 顶部做一次线性扫描：与既有的标记刷新合并为同一遍循环，
//! 不复制历史、不做 provider 历史规范化、不读磁盘。

use crate::builtin_notes_tools::{
    NOTES_APPEND_TO_FILE_TOOL_NAME, NOTES_APPEND_TO_FILE_TOOL_QUALIFIED_NAME,
    NOTES_WRITE_FILE_TOOL_NAME, NOTES_WRITE_FILE_TOOL_QUALIFIED_NAME,
};
use crate::token_budget_guard::TokenBudgetRuntimePolicy;
use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, AgentModelMessageKind, AgentRuntimeNoteKind,
};

/// 收尾预警的稳定前缀。
pub(crate) const WARN_NOTE_PREFIX: &str = "当前任务窗口距离自动换窗还剩 ";
/// 中段保值提示的稳定前缀。
pub(crate) const MIDWAY_NOTE_PREFIX: &str = "本窗口预算仍充足，但现在是记录成本最低的时刻：";
/// assistant 消息 role，用于统计距上一次便签写入的步数。
const ASSISTANT_ROLE: &str = "assistant";

impl super::WindowRolloverPolicyRuntime {
    /// 恢复窗口内提醒去重标记，并统计距上一次便签写入的 assistant 步数。
    ///
    /// 只扫描运行态标记：一遍循环同时还原三档去重标记与步数，避免每轮 turn 顶部
    /// 复制完整历史或做第二次遍历。
    pub(crate) fn refresh_rollover_markers(&mut self, messages: &[AgentModelMessage]) {
        let fallback_note =
            crate::runtime_note_content(AgentRuntimeNoteKind::PayloadBudgetGate, &fallback_note());
        let mut warn_emitted = false;
        let mut midway_emitted = false;
        let mut fallback_emitted = false;
        let mut assistant_steps = 0_usize;
        let mut notes_step_mark = 0_usize;
        for message in messages {
            match &message.kind {
                AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::PayloadBudgetGate) => {
                    let is_warn = message.content.contains(WARN_NOTE_PREFIX);
                    let is_fallback = message.content == fallback_note;
                    warn_emitted |= is_warn || is_fallback;
                    fallback_emitted |= is_fallback;
                }
                AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::MemoryHygiene) => {
                    midway_emitted |= message.content.contains(MIDWAY_NOTE_PREFIX);
                }
                _ => {}
            }
            if message.role != ASSISTANT_ROLE {
                continue;
            }
            assistant_steps = assistant_steps.saturating_add(1);
            if has_notes_write_call(message) {
                notes_step_mark = assistant_steps;
            }
        }
        self.warn_emitted = warn_emitted;
        // 收尾预警已经越过中段，之后不再补发保值提示。
        self.midway_emitted = midway_emitted || warn_emitted;
        self.fallback_emitted = fallback_emitted;
        self.steps_since_notes = assistant_steps.saturating_sub(notes_step_mark);
    }

    /// 观察一次 Provider 实测的上下文用量，维护步均采样。
    ///
    /// 只有 Provider 实测才产生采样：本地估算（Provider 未报 usage 时的兜底）与实测
    /// 量纲不同，混算会把中位数污染成既不代表真实步幅、也不代表估算误差的中间值。
    /// 这条规则与 `record_token_budget_snapshot` 完全一致，保证「运行时提醒」与
    /// `builtin.get_context_remaining` 对同一步给出同一个步均。
    pub(super) fn observe_provider_measurement(&mut self, provider_context_tokens: Option<u64>) {
        match provider_context_tokens.filter(|tokens| *tokens > 0) {
            Some(tokens) => {
                self.last_provider_context_tokens = Some(tokens);
                self.step_pace.observe(tokens);
            }
            None => {
                // 读数回落到估算时不能沿用上一份实测基线，否则下一步的增量会把
                // 「估算高度 - 实测高度」当成一次真实步幅。
                self.last_provider_context_tokens = None;
                self.step_pace.reset();
            }
        }
    }

    /// 按预算水位决定这一轮是否追加一条运行时提示。
    ///
    /// `current_context_tokens` 由调用方给出，必须与步均采样同源：本窗口拿到过
    /// Provider 实测就传实测，只有在完全没有实测时才退回估算。两侧口径不一致时
    /// 提醒里的剩余量会和 `builtin.get_context_remaining` 打架。
    ///
    /// 顺序固定为收尾预警优先于中段保值：两者都是每窗口一次，
    /// 一旦水位直接跳过预警线，保值提示就没有意义了。
    pub(super) fn maybe_emit_budget_notice(
        &mut self,
        messages: &mut AgentModelHistorySnapshot,
        current_context_tokens: Option<u64>,
    ) -> bool {
        let Some(observed) = current_context_tokens.filter(|tokens| *tokens > 0) else {
            return false;
        };
        let trigger = self.policy.trigger_tokens;
        if observed >= trigger {
            return false;
        }
        let tokens_per_step = self.step_pace.tokens_per_step();
        if !self.warn_emitted {
            let headroom = TokenBudgetRuntimePolicy::warn_headroom(
                trigger,
                self.policy.context_window,
                tokens_per_step,
            );
            if TokenBudgetRuntimePolicy::should_emit_warn(trigger, observed, headroom) {
                let remaining =
                    TokenBudgetRuntimePolicy::remaining_before_rollover(trigger, observed);
                let forced = self
                    .token_budget
                    .forced_rollover_tokens(trigger, self.policy.context_window);
                let forced_remaining =
                    TokenBudgetRuntimePolicy::remaining_before_rollover(forced, observed);
                // 取整与 `builtin.get_context_remaining` 对齐：同为向下取整，
                // 同一读数在两侧不会差出一步。
                let steps_of = |tokens: u64| {
                    tokens_per_step
                        .filter(|step| *step > 0)
                        .map(|step| tokens / step)
                };
                messages.push(AgentModelMessage::runtime_note(
                    AgentRuntimeNoteKind::PayloadBudgetGate,
                    warn_note(
                        remaining,
                        steps_of(remaining),
                        steps_of(forced_remaining),
                        tokens_per_step,
                        self.step_pace.sample_count(),
                    ),
                ));
                self.warn_emitted = true;
                self.midway_emitted = true;
                return true;
            }
        }
        if !self.midway_emitted
            && TokenBudgetRuntimePolicy::should_emit_midway(
                trigger,
                observed,
                self.steps_since_notes,
            )
        {
            messages.push(AgentModelMessage::runtime_note(
                AgentRuntimeNoteKind::MemoryHygiene,
                midway_note(),
            ));
            self.midway_emitted = true;
            return true;
        }
        false
    }
}

/// 判定一条 assistant 消息是否发起了便签写入。
fn has_notes_write_call(message: &AgentModelMessage) -> bool {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_ref())
        .is_some_and(|calls| {
            calls
                .iter()
                .any(|call| is_notes_write_tool_name(&call.name))
        })
}

/// 判定便签写入工具：Provider 可能下发限定名或裸名。
fn is_notes_write_tool_name(name: &str) -> bool {
    name == NOTES_WRITE_FILE_TOOL_NAME
        || name == NOTES_WRITE_FILE_TOOL_QUALIFIED_NAME
        || name == NOTES_APPEND_TO_FILE_TOOL_NAME
        || name == NOTES_APPEND_TO_FILE_TOOL_QUALIFIED_NAME
}

/// 渲染中段保值提示正文。
///
/// 这一档不催换窗，只回答“要不要现在把结论写下来”。它解决的是记忆保值，
/// 与预算水位无关，因此文案里不出现任何剩余 token 数字。
#[must_use]
pub(crate) fn midway_note() -> String {
    format!(
        "{MIDWAY_NOTE_PREFIX}如果下面任一条现在只活在这段对话里，就用 builtin.notes_append_to_file 追加进 checkpoint.md：用户明确要求、不能违反的约束；已定下并知道理由的决策；已排除并知道为什么排除的方案。没有就不写，不要把整段对话搬进便签。换窗会把整个窗口丢掉再重建对代码的理解，追加一次便签只是一次很便宜的调用。"
    )
}

/// 渲染收尾预警正文。
///
/// `steps_to_rollover` 与 `steps_to_forced` 都来自运行时的步均采样；样本不足时为
/// `None`，此时只报 token 水位，不给出不可信的步数。
#[must_use]
pub(crate) fn warn_note(
    remaining_tokens: u64,
    steps_to_rollover: Option<u64>,
    steps_to_forced: Option<u64>,
    tokens_per_step: Option<u64>,
    sample_count: usize,
) -> String {
    let pace_clause = match (tokens_per_step, steps_to_rollover, steps_to_forced) {
        (Some(step), Some(steps), Some(forced_steps)) => format!(
            "；这是收尾预警线本身，不是余量见底：预警线按最近 {sample_count} 步实测均值（约 {step} tokens/步）折算，只覆盖 {steps} 步（到强制换窗约 {forced_steps} 步）。步幅变小就能多跑几步，要精确余量就用 builtin.get_context_remaining 复核"
        ),
        _ => "（步均样本不足，暂不折算步数）".to_owned(),
    };
    format!(
        "{WARN_NOTE_PREFIX}{remaining_tokens} tokens{pace_clause}。判断：这个工作单元能在这些步内收口，就继续做完；不能就先落到可中断点——等正在进行的工具调用回执落地、检查跑完、结论确定——再用 builtin.notes_write_file 覆盖写入 checkpoint.md（上限 {max_tokens} tokens，超限会被拒绝），然后调用 builtin.new_context。便签按固定小节写：目标 / 已完成步骤（含文件路径、命令、关键 ID）/ 待办 / 已验证证据 / 下一步命令；旧窗口的原文与输出可用 history_list_windows、history_search_contents、history_read_item 回读。不要在工具调用中途换窗：窗口结束后才落地的回执没有调用可以挂靠。若要读大文件或跑长测试，按更坏的估计，一步就可能吃掉这几步的预算。",
        max_tokens = crate::session_notes::SESSION_NOTES_CHECKPOINT_MAX_TOKENS,
    )
}

/// 窗口耗尽时下发的唯一交接指令。
///
/// 预算数字从常量拼装而不是写死在字面量里：常量一调，提示语自动跟着走，
/// 不会出现「代码改成 8K、提示语还写 4K」这种漂移。正文按整段相等参与历史去重，
/// 因此不要为了措辞微调改动字面量。
#[must_use]
pub(crate) fn fallback_note() -> String {
    format!(
        "当前任务窗口已耗尽。不要在本窗口继续任务或给出最终答复，新窗口不会自动包含本次对话。现在只允许一次便签写入：调用 builtin.notes_write_file 覆盖写入 checkpoint.md，这是全会话唯一的便签，既记录工作状态也承担这次交接。必须按固定小节写，缺一节新窗口就会失去对应事实：
1. 目标：这次任务要交付什么；
2. 已完成步骤：按顺序写，每步带文件路径、执行的命令、关键 ID（会话/作业/请求标识）；
3. 待办：还没做的，按优先级列出；
4. 已验证证据：跑过的检查与结果、读过的关键文件；
5. 下一步命令：新窗口第一步该执行什么。
示例：
- 目标：修复窗口轮转失败；
- 已完成步骤：1) 改 crates/axon-session/src/.../jsonl_store.rs 增加正文外置，cargo test -p axon-session --lib 通过；2) 改 crates/axon-cli/src/.../session_prune.rs 的归属判定，未跑全量；
- 待办：跑全量门禁；
- 已验证证据：单测 9 条通过；
- 下一步命令：cargo test --workspace。
上限 {max_tokens} tokens（按 UTF-8 字节数除以 4 估算，约 {max_token_kib} KiB 文本）：超限会被拒绝并要求重写。写完后立即调用 builtin.new_context，不要使用除便签与 new_context 之外的任何工具。旧窗口逐字稿不在新上下文里：需要命令原文、工具输出或旧结论时，用 history_list_windows 找到窗口、history_search_contents 检索、history_read_item 读取，不要凭印象补全。",
        max_tokens = crate::session_notes::SESSION_NOTES_CHECKPOINT_MAX_TOKENS,
        max_token_kib = crate::session_notes::SESSION_NOTES_CHECKPOINT_MAX_TOKENS * 4 / 1024,
    )
}
