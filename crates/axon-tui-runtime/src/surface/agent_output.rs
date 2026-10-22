//! Agent 输出文本归一化。

pub(crate) fn normalize_agent_output(message: &str, fallback: &str) -> Vec<String> {
    let lines = message
        .trim()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return vec![fallback.to_owned()];
    }
    lines
}
