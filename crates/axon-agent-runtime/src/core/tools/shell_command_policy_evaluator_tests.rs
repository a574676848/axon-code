//! `builtin.shell_command` 权限判定器的回归用例。
//!
//! 从 `shell_command_policy_evaluator.rs` 拆出（INV-7 单文件 700 行铁律）；
//! 通过 `#[path]` 挂回原模块，`use super::*` 的可见性与内联时一致。

use super::*;

fn evaluate(command: &str) -> ShellCommandPolicyEvaluation {
    ShellCommandPolicyEvaluator::evaluate(
        &ToolCallInput::json(serde_json::json!({ "command": command })),
        Path::new("."),
    )
}

#[test]
fn shell_uses_its_command_and_workdir_for_policy() {
    let evaluation = ShellCommandPolicyEvaluator::evaluate(
        &ToolCallInput::json(serde_json::json!({
            "command": "printf terminal-ok > output.txt",
            "workdir": "nested"
        })),
        Path::new("workspace"),
    );

    assert_eq!(
        evaluation.execution_permission,
        ToolExecutionPermission::Allowed
    );
    assert_eq!(
        evaluation.affected_paths,
        vec![PathBuf::from("workspace/nested/output.txt")]
    );
}

#[test]
fn write_scope_is_complete_only_for_output_only_programs() {
    for command in [
        "echo x > a.log",
        "printf '%s' value > a.log",
        "echo x > a.log && echo y > b.log",
    ] {
        assert!(
            evaluate(command).write_paths_are_complete,
            "{command} 只由纯输出程序构成，重定向目标即完整写入集"
        );
    }

    // 构建、包管理、脚本解释器与 tee 都会写重定向目标之外的位置。
    for command in [
        "cargo test > out.log",
        "cargo build 2> err.txt",
        "npm install > install.log",
        "cargo test | tee out.log",
        "echo x > $TARGET; cargo test > out.log",
        "python build.py > out.log",
    ] {
        assert!(
            !evaluate(command).write_paths_are_complete,
            "{command} 的写入集不可证完整，不得收窄写作用域"
        );
    }
}

/// R-24 回归：白名单程序 + 动态目标 / 动态执行标记，必须与「非白名单程序」同等否决。
///
/// 这些用例的**段首词全部在白名单内**，因此只靠程序名判据会全部误判为「写入集完整」
///：它们正是独立审计构造出 fail-open 残留时所用的反例。
///
/// 注意上一用例中的 `echo x > $TARGET; cargo test > out.log` **不构成本缺陷的守护**：
/// 它之所以被否决，靠的是同一脚本里的 `cargo` 段，而不是 `$TARGET` 被识别。
#[test]
fn write_scope_is_complete_rejects_dynamic_targets_within_whitelisted_programs() {
    for command in [
        // 动态重定向目标被路径提取拒绝，但 shell 仍会真实写入它。
        "echo x > a.log; echo y > $TARGET",
        "Write-Output x > a.log; Write-Output y > $env:TEMP\\out.log",
        // 命令替换：`$(...)` 内的命令可写任意路径，且完全不经过目标提取。
        "Write-Output $(npm install) > a.log",
        "echo $(cargo build) > a.log",
        // 反引号同义形式。
        "echo `cargo build` > a.log",
        // 花括号展开：单个 token，展开后是多个真实目标。
        "echo x > {a,b}.log",
        // 通配目标。
        "echo x > *.log",
        // 重定向符后缺失目标：无法证明它不写任何路径。
        "echo x >",
    ] {
        assert!(
            !evaluate(command).write_paths_are_complete,
            "{command} 的段首词虽在白名单内，但写入集不可证完整，不得收窄写作用域"
        );
    }
}

/// R2 审计对抗样本回归（N-A-01 / N-A-02 / N-A-03，对应审计报告对抗推演样本 4/5/6/8）。
///
/// 这些用例的共同点是**段首词全部在白名单或只读名单内**，修复前的判据全部误放行：
/// - 样本 6：`<(...)` 进程替换不在 marker 列表，会被误判为只读；
/// - 样本 4/5：PowerShell `(...)` 子表达式与 `[Type]::` 静态调用不可证写入集；
/// - 样本 8：`&` 未被路径提取拒绝 → 假路径 `a.log&echo` 顶替真目标 `a.log`。
#[test]
fn adversarial_substitution_forms_are_rejected() {
    // 进程替换：不得误判为只读（is_provably_read_only 否决）。
    let evaluation = evaluate("sort <(npm install) | head");
    assert_eq!(
        evaluation.execution_permission,
        ToolExecutionPermission::Allowed,
        "含 <(...) 进程替换的命令不得标记为只读"
    );
    assert!(!evaluation.is_read_only);

    // 写作用域：PowerShell 子表达式与静态调用的求值内容不可证完整。
    let evaluation = evaluate("Write-Output (iex 'npm install') > a.log");
    assert!(
        !evaluation.write_paths_are_complete,
        "PowerShell 子表达式 (...) 的写入集不可证完整"
    );
    assert_eq!(
        evaluation.affected_paths,
        vec![PathBuf::from("./a.log")],
        "重定向目标仍如实上报，只是不得用于收窄"
    );
    assert!(
        !evaluate("Write-Output [IO.File]::WriteAllText('poc.txt','x') > a.log")
            .write_paths_are_complete,
        "[Type]:: 静态调用的写入集不可证完整"
    );

    // `&` 后台操作符粘连的假路径必须触发丢弃信号。
    let evaluation = evaluate("echo x >a.log&echo y>b.log");
    assert!(
        !evaluation.write_paths_are_complete,
        "shell_words 不按 & 分词，提取到的 a.log&echo 是假路径，写入集不可证完整"
    );
}

/// R2 §5.1 回归：PowerShell 文件类 cmdlet 的写入目标必须进入 `affected_paths`。
///
/// 缺了这一步，会话不会为这些路径建立前后快照 → 回执 `filesystem_write_performed`
/// 漏报（实测：文件确实被创建却报 false）、`writeIntent` 与 post-run LSP 一并缺失。
#[test]
fn powershell_file_cmdlets_report_their_write_targets() {
    for (command, expected) in [
        ("Set-Content -Path a.txt -Value x", "a.txt"),
        ("Set-Content b.txt -Value x", "b.txt"),
        ("Out-File -FilePath c.txt -InputObject x", "c.txt"),
        ("Remove-Item d.txt", "d.txt"),
        ("New-Item -ItemType Directory -Force nd", "nd"),
        ("Add-Content -Path e.txt -Value y", "e.txt"),
    ] {
        let evaluation = evaluate(command);
        assert!(
            evaluation
                .affected_paths
                .iter()
                .any(|path| path.ends_with(expected)),
            "{command} 的写入目标 {expected} 必须被登记，实际 {:?}",
            evaluation.affected_paths
        );
    }

    // 源与目标都参与变化。
    let copy = evaluate("Copy-Item src.txt dst.txt");
    for expected in ["src.txt", "dst.txt"] {
        assert!(
            copy.affected_paths
                .iter()
                .any(|path| path.ends_with(expected)),
            "Copy-Item 必须登记 {expected}，实际 {:?}",
            copy.affected_paths
        );
    }

    // 参数取值不得被误登记为写入目标。
    let value_not_path = evaluate("Set-Content -Path f.txt -Value payload.txt");
    assert!(
        value_not_path
            .affected_paths
            .iter()
            .any(|path| path.ends_with("f.txt"))
            && !value_not_path
                .affected_paths
                .iter()
                .any(|path| path.ends_with("payload.txt")),
        "-Value 的取值不得被当作路径：{:?}",
        value_not_path.affected_paths
    );

    // 动态目标仍走同一 fail-closed 口径。
    assert!(
        !evaluate("Set-Content -Path $env:TEMP\\x.txt -Value y").write_paths_are_complete,
        "cmdlet 的动态目标必须触发「写入集不可证完整」"
    );

    // cmdlet 名出现在**参数**位置时不得产生幻影路径：幻影路径会进入
    // `writeIntent.affectedPaths`，让模型以为命令碰过它根本没写的文件。
    let phantom = evaluate("echo Set-Content -Path ghost.txt > real.log");
    assert!(
        !phantom
            .affected_paths
            .iter()
            .any(|path| path.ends_with("ghost.txt")),
        "参数位置的 cmdlet 名不得被当成命令：{:?}",
        phantom.affected_paths
    );

    // 管道后段仍是段首词，必须继续被识别。
    let piped = evaluate("Get-Content src.txt | Set-Content dst.txt");
    assert!(
        piped
            .affected_paths
            .iter()
            .any(|path| path.ends_with("dst.txt")),
        "管道后段的 cmdlet 必须被识别：{:?}",
        piped.affected_paths
    );
}

#[test]
fn affected_paths_stay_reported_even_when_scope_is_not_narrowable() {
    let evaluation = evaluate("cargo test > out.log");

    // 路径仍如实上报（供变更检测使用），只是不再用于收窄并发写作用域。
    assert_eq!(evaluation.affected_paths, vec![PathBuf::from("./out.log")]);
    assert!(!evaluation.write_paths_are_complete);
}

#[test]
fn write_scope_allowlist_excludes_programs_that_write_files() {
    // 这些程序看似纯输出，实则可写重定向目标之外的文件；纳入白名单即重新引入 fail-open。
    for program in ["env", "sort", "sed", "awk", "tee", "tee-object", "logger"] {
        assert!(
            !WRITE_SCOPE_COMPLETE_COMMANDS.contains(&program),
            "{program} 可写重定向目标之外的文件，不得进入收窄白名单"
        );
    }
}

#[test]
fn known_read_only_commands_are_allowed_and_marked_read_only() {
    for command in [
        "Get-Location; Get-ChildItem",
        "Get-Content Cargo.toml | Select-String workspace",
        "pwd && ls",
        "rg --files",
        "git status",
    ] {
        let evaluation = evaluate(command);
        assert_eq!(
            evaluation.execution_permission,
            ToolExecutionPermission::Allowed,
            "command={command}"
        );
        assert!(evaluation.is_read_only, "command={command}");
    }
}

#[test]
fn writes_builds_and_unknown_commands_are_allowed_but_not_read_only() {
    for command in [
        "Set-Content out.txt value",
        "Get-Content input.txt > out.txt",
        "cargo check --workspace",
        "custom-tool inspect",
        "git checkout main",
        "@'\n*** Begin Patch\n*** End Patch\n'@ | apply_patch",
        "python - <<'PY'\nprint('ok')\nPY",
        "Get-Process | Where-Object { Remove-Item out.txt }",
    ] {
        let evaluation = evaluate(command);
        assert_eq!(
            evaluation.execution_permission,
            ToolExecutionPermission::Allowed,
            "command={command}"
        );
        assert!(!evaluation.is_read_only, "command={command}");
    }
}

/// 对齐 DSH 的核心契约：命令内容**永不**决定执行权限。
///
/// 修复前这些脚本会被灾难性命令词表或 here-string 判定拦下：`rm -rf /` 直接 Forbidden，
/// 以 `@'` 开头的多行脚本在 Windows 上又与 Bash heredoc 禁例互相锁死。现在一律放行，
/// 由 shell 自身在运行期裁决。
#[test]
fn command_content_never_decides_execution_permission() {
    for command in [
        "rm -rf /",
        "rm --recursive --force /",
        "Remove-Item -Recurse -Force C:\\",
        "mkfs.ext4 /dev/sda",
        "Format-Volume -DriveLetter C",
        "dd if=/dev/zero of=/dev/sda",
        "python - <<'PY'\nprint('ok')\nPY",
        "@'\nprint('ok')\n'@ | python -",
        // 脚本内自带的 shell wrapper（dispatching 到内层 shell）同样只是内容。
        "Write-Host a; pwsh -Command Get-Location",
    ] {
        assert_eq!(
            evaluate(command).execution_permission,
            ToolExecutionPermission::Allowed,
            "命令内容不得参与执行权限判定，command={command}"
        );
    }
}

#[test]
fn invalid_arguments_are_still_forbidden() {
    let evaluation = ShellCommandPolicyEvaluator::evaluate(
        &ToolCallInput::json(serde_json::json!({ "command": 42 })),
        Path::new("."),
    );

    let ToolExecutionPermission::Forbidden { reason } = evaluation.execution_permission else {
        panic!("非字符串 command 必须被参数校验拒绝");
    };
    assert!(reason.starts_with(FORBIDDEN_PREFIX), "reason={reason}");
    assert!(reason.contains(INVALID_ARGUMENTS_REASON), "reason={reason}");
    assert!(!evaluation.is_read_only);
    assert!(evaluation.affected_paths.is_empty());
    assert!(!evaluation.write_paths_are_complete);
}

#[test]
fn write_paths_are_reported_by_the_shared_evaluator() {
    let evaluation = evaluate("Get-Content input.txt | Tee-Object -FilePath out.txt");

    assert_eq!(
        evaluation.execution_permission,
        ToolExecutionPermission::Allowed
    );
    assert_eq!(
        evaluation.affected_paths,
        vec![Path::new(".").join(".").join("out.txt")]
    );
}

#[test]
fn attached_append_redirect_reports_the_complete_path() {
    let evaluation = evaluate("Get-Content input.txt >>out.txt");

    assert_eq!(
        evaluation.affected_paths,
        vec![Path::new(".").join(".").join("out.txt")]
    );
}
