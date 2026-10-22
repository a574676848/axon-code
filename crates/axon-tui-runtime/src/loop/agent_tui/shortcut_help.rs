//! 快捷键总览 Dock 状态与文案。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShortcutEntry {
    pub(crate) key: &'static str,
    pub(crate) action: &'static str,
    pub(crate) context: &'static str,
}

const SHORTCUTS: [ShortcutEntry; 16] = [
    entry(
        "Esc",
        "关闭当前 Dock；有待发送消息时立即发送，否则中断运行",
        "全局",
    ),
    entry("?", "打开或关闭快捷键总览", "空输入"),
    entry("/", "打开命令建议面板", "输入区"),
    entry("Enter", "发送任务；运行中加入待发送队列", "输入区"),
    entry("Shift+Enter", "插入换行", "输入区"),
    entry("Ctrl+V", "粘贴文本", "输入区"),
    entry("Command+V", "粘贴剪贴板图片或文件", "输入区"),
    entry("Alt+V", "粘贴剪贴板图片", "输入区"),
    entry("Ctrl+X", "打开外部编辑器", "输入区"),
    entry("Ctrl+R", "搜索历史输入", "输入区"),
    entry("Ctrl+O", "打开最新工具完整输出", "全局"),
    entry("拖拽选择", "复制可见文本", "复制"),
    entry("PgUp/PgDn", "滚动主渲染区历史", "浏览"),
    entry("Ctrl+Home/End", "跳到主渲染区历史顶部或底部", "浏览"),
    entry("Alt+E", "把未发送的排队输入恢复到输入区继续编辑", "运行中"),
    entry("Y / N", "批准或拒绝待审批工具", "审批"),
];

const fn entry(key: &'static str, action: &'static str, context: &'static str) -> ShortcutEntry {
    ShortcutEntry {
        key,
        action,
        context,
    }
}

pub(crate) fn shortcut_entries() -> &'static [ShortcutEntry] {
    &SHORTCUTS
}

#[cfg(test)]
mod tests {
    use super::shortcut_entries;

    #[test]
    fn shortcut_entries_describe_drag_selection_copy() {
        let copy = shortcut_entries()
            .iter()
            .find(|entry| entry.key == "拖拽选择")
            .expect("快捷键总览必须说明拖拽选择复制");

        assert_eq!(copy.context, "复制");
        assert!(copy.action.contains("复制"));
    }

    #[test]
    fn shortcut_entries_describe_history_as_render_viewport() {
        let paging = shortcut_entries()
            .iter()
            .find(|entry| entry.key == "PgUp/PgDn")
            .expect("快捷键总览必须说明滚动边界");
        let jump = shortcut_entries()
            .iter()
            .find(|entry| entry.key == "Ctrl+Home/End")
            .expect("快捷键总览必须说明跳转边界");

        assert!(paging.action.contains("主渲染区历史"));
        assert!(jump.action.contains("主渲染区历史"));
    }
}
