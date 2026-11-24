//! Atomic commit planner support types.

/// 文件变更状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeStatus {
    /// 新增文件
    Added,
    /// 修改文件
    Modified,
    /// 删除文件
    Deleted,
    /// 重命名文件
    Renamed,
}

/// 计算两组文件的交集。
pub fn intersection(a: &[&str], b: &[&str]) -> Vec<String> {
    let set: std::collections::HashSet<_> = b.iter().cloned().collect();
    a.iter()
        .filter(|f| set.contains(*f))
        .map(|f| f.to_string())
        .collect()
}

/// 计算两组文件的差集。
pub fn difference(a: &[&str], b: &[&str]) -> Vec<String> {
    let set: std::collections::HashSet<_> = b.iter().cloned().collect();
    a.iter()
        .filter(|f| !set.contains(*f))
        .map(|f| f.to_string())
        .collect()
}
