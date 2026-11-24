//! Agent TUI 输入区的稳定提示与宽度契约。

use crate::surface::display_width::DisplayWidthText;

pub(crate) const AGENT_TUI_PROMPT_INDICATOR: &str = "›";
pub(crate) const TUI_HINT_IDLE: &str = "Enter 发送 · / 命令 · Alt+Enter 换行";
pub(crate) const TUI_HINT_WORKING: &str =
    "Agent 正在处理 · Enter 加入待发送 · Esc 有队列立即发送/无队列中断 · Alt+Enter 换行";
pub(crate) const TUI_HINT_DONE: &str = "任务已完成 · 可继续输入下一轮任务";
pub(crate) const TUI_HINT_COMMAND: &str = "Tab 补全命令 · /help 查看能力 · 粘贴路径自动识别附件";

pub(crate) fn agent_tui_prompt_width() -> u16 {
    u16::try_from(DisplayWidthText::width(AGENT_TUI_PROMPT_INDICATOR) + 1).unwrap_or(u16::MAX)
}
