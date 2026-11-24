//! Hashline BLK 对统一语言注册表的公开入口回归测试。

use axon_agent_runtime::{
    HashlineApplyFile, HashlineApplyRequest, WorkspaceFileSession, apply_hashline_edit,
};

#[test]
fn block_edit_resolves_nested_typescript_statement() {
    let content = "function f() {\n  if (a) {\n    g();\n  }\n}\n";
    let file = observed_file("src/f.ts", content);
    let tag = file.snapshot.tag();

    let report = apply_hashline_edit(HashlineApplyRequest {
        input: format!("[src/f.ts#{tag}]\nSWAP.BLK 2:\n+  if (b) {{\n+    h();\n+  }}\n"),
        files: vec![file],
    })
    .expect("TypeScript BLK 应解析嵌套语句");

    assert_eq!(
        report.files[0].next_content,
        "function f() {\n  if (b) {\n    h();\n  }\n}\n"
    );
    assert_eq!(report.files[0].block_resolutions[0].start, 2);
    assert_eq!(report.files[0].block_resolutions[0].end, 4);
}

#[test]
fn block_edit_resolves_extensionless_shell_profile() {
    let content = "ZSH_COMPDUMP=x\nif [[ -f \"$ZSH_COMPDUMP\" ]]; then\n  compinit -C\nelse\n  compinit\nfi\n";
    let file = observed_file(".zshrc", content);
    let tag = file.snapshot.tag();

    let report = apply_hashline_edit(HashlineApplyRequest {
        input: format!("[.zshrc#{tag}]\nDEL.BLK 2\n"),
        files: vec![file],
    })
    .expect("无扩展名 shell profile 应使用 Bash grammar");

    assert_eq!(report.files[0].next_content, "ZSH_COMPDUMP=x\n");
}

#[test]
fn block_edit_resolves_swift_declaration_after_blank_line() {
    let content = "import Foundation\n\nfunc greet() {\n    print(\"hi\")\n}\n";
    let file = observed_file("src/g.swift", content);
    let tag = file.snapshot.tag();

    let report = apply_hashline_edit(HashlineApplyRequest {
        input: format!(
            "[src/g.swift#{tag}]\nSWAP.BLK 3:\n+func greet() {{\n+    print(\"hello\")\n+}}\n"
        ),
        files: vec![file],
    })
    .expect("Swift 空行后的 declaration 应按一列范围定位");

    assert_eq!(
        report.files[0].next_content,
        "import Foundation\n\nfunc greet() {\n    print(\"hello\")\n}\n"
    );
}

fn observed_file(path: &str, content: &str) -> HashlineApplyFile {
    let session = WorkspaceFileSession::default();
    let snapshot = session
        .record_read(path, content, 1..=content.lines().count())
        .expect("测试快照应成功记录");
    HashlineApplyFile {
        path: path.to_owned(),
        current_content: content.to_owned(),
        snapshot,
    }
}
