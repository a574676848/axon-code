//! 「用其他应用打开」：按文件在系统里的关联枚举候选，并回放用户选中的那一条。
//!
//! 候选来自**系统自己的关联登记**（[`axon_shell_assoc`]），不是一份精选目录：
//! 本机装了什么、某个扩展名默认由谁打开，都由操作系统回答，因此换一台机器、
//! 装一个新应用，菜单会自己跟着变。界面只交出标识，应用由本模块回找系统登记的
//! 处理程序，界面无法借这个入口启动任意程序。
//!
//! 路径边界与文件浏览共用一套判断：[`crate::workspace_files::resolve_within`]
//! 把目标钳在工作区根目录内，任何越界路径在这里就被拒绝，不会走到系统调用。

use axon_shell_assoc::FileApplication;

/// 解析工作区内的目标文件，并列出系统为它登记的打开方式。
///
/// # Errors
///
/// 目标越出工作区、不存在，或系统关联枚举整体失败时返回错误描述（中文，可直接展示）。
/// 单条候选取不到图标不会变成错误，只会让那条候选没有图标。
pub fn applications(root: &str, relative: &str) -> Result<Vec<FileApplication>, String> {
    let path = crate::workspace_files::resolve_within(root, relative)?;
    axon_shell_assoc::applications_for(&path)
}

/// 用系统登记的一条打开方式打开工作区内的文件。
///
/// `application` 必须来自 [`applications`] 返回的标识；它不是任意程序路径。
///
/// # Errors
///
/// 目标越出工作区，或该标识不属于这个文件的关联列表时返回错误描述。
pub fn open_with(root: &str, relative: &str, application: &str) -> Result<(), String> {
    let path = crate::workspace_files::resolve_within(root, relative)?;
    axon_shell_assoc::open_with(&path, application)
}

/// Tauri IPC 命令：列出某个文件在系统里的候选打开方式。
///
/// 枚举要走平台的 Shell / 桌面条目接口，在 Windows 上还要进 COM 套间并泵一小段消息循环，
/// 因此挪到阻塞线程上执行，不占用异步运行时的工作线程。
#[tauri::command]
pub async fn workspace_applications(
    root: String,
    relative_path: String,
) -> Result<Vec<FileApplication>, String> {
    tauri::async_runtime::spawn_blocking(move || applications(&root, &relative_path))
        .await
        .map_err(|error| format!("枚举打开方式失败：{error}"))?
}

/// Tauri IPC 命令：用系统登记的某个应用打开工作区内的文件。
#[tauri::command]
pub async fn workspace_open_with(
    root: String,
    relative_path: String,
    application: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || open_with(&root, &relative_path, &application))
        .await
        .map_err(|error| format!("打开失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用例用的临时工作区；由 `TestTempDir` 的守卫负责清理。
    ///
    /// 用仓内共享的唯一目录而不是自拼时间戳：并行用例撞上同一个时间戳时，
    /// 两个用例会共用同一棵树而互相删掉对方的工作区。
    fn scratch() -> axon_test_support::TestTempDir {
        let root = axon_test_support::test_temp_dir();
        std::fs::write(root.path().join("sample.txt"), "内容").expect("写入测试文件");
        root
    }

    #[test]
    fn escaping_the_workspace_root_is_rejected_before_any_system_call() {
        let root = scratch();
        let root = root.path().to_path_buf();
        let outside = root.to_string_lossy().to_string();
        for escape in ["../outside.txt", "C:/Windows/win.ini", ".."] {
            assert!(
                applications(&outside, escape).is_err(),
                "越界路径 {escape} 必须在触碰系统之前被拒绝"
            );
            assert!(
                open_with(&outside, escape, "anything").is_err(),
                "越界路径 {escape} 不得进入打开流程"
            );
        }
    }

    #[test]
    fn an_identifier_the_system_never_registered_is_rejected() {
        let root = scratch();
        let root = root.path().to_path_buf();
        let displayed = root.to_string_lossy().to_string();
        // 关联列表里绝不会出现的标识：必须如实报错，绝不退回「随便找一个应用」。
        let outcome = open_with(&displayed, "sample.txt", "axon-no-such-application");
        assert!(
            outcome.is_err(),
            "未登记的标识必须以错误收场，而不是静默打开别的东西"
        );
    }
}
