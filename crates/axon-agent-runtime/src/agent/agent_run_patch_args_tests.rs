//! Patch 与 edit 工具参数归一化：pathdiff 信封、惰性编辑块补行与 hashline 头剥离。

use super::*;

#[test]
fn normalizes_pathdiff_envelope_from_text_and_json_inputs() {
    let pathdiff = "*** Begin PathDiff\n*** Add File: created.txt\n+created\n*** End PathDiff";
    for input in [
        ToolCallInput::text(pathdiff),
        ToolCallInput::json(serde_json::json!({ "input": pathdiff })),
    ] {
        let edit = agent_run_edit_tool_args_from_provider_input(&input).expect("edit args");
        assert_eq!(
            edit.input,
            "*** Begin Patch\n*** Add File: created.txt\n+created\n*** End Patch"
        );
        assert!(agent_run_edit_input_is_apply_patch(&edit.input));
    }
}

#[test]
fn normalizes_common_new_file_pathdiff_spelling_and_body() {
    let input = ToolCallInput::text(
        "*** Begin PathDiff\n*** Create File: created.txt\ncreated\n*** End PathDiff",
    );

    let edit = agent_run_edit_tool_args_from_provider_input(&input).expect("edit args");

    assert_eq!(
        edit.input,
        "*** Begin Patch\n*** Add File: created.txt\n+created\n*** End Patch"
    );
}

#[test]
fn normalizes_unwrapped_and_fenced_pathdiff() {
    // 未带 *** Begin Patch / *** End Patch
    let raw_unwrapped = "*** Update File: scan_cfg.py\n@@\n-old\n+new";
    let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(raw_unwrapped))
        .expect("edit args");
    assert!(agent_run_edit_input_is_apply_patch(&edit.input));
    assert!(edit.input.starts_with("*** Begin Patch"));
    assert!(edit.input.ends_with("*** End Patch"));

    // 带 markdown ```diff 围栏
    let fenced = "```diff\n*** Add File: scan_cfg.py\n+import os\n```";
    let edit_fenced = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(fenced))
        .expect("edit args");
    assert!(agent_run_edit_input_is_apply_patch(&edit_fenced.input));
    assert!(edit_fenced.input.starts_with("*** Begin Patch"));
    assert!(edit_fenced.input.ends_with("*** End Patch"));
}

#[test]
fn normalizes_lazy_edit_markers_as_pathdiff_chunk_boundaries() {
    let input = ToolCallInput::text(
        "*** Begin Patch\n\
         *** Update File: src/lib.rs\n\
         // ... existing code ...\n\
         -pub fn old() {}\n\
         +pub fn new() {}\n\
         # ... existing code ...\n\
         -fn second_old() {}\n\
         +fn second_new() {}\n\
         *** End Patch",
    );

    let edit = agent_run_edit_tool_args_from_provider_input(&input).expect("edit args");
    assert_eq!(
        edit.input,
        "*** Begin Patch\n\
         *** Update File: src/lib.rs\n\
         @@\n\
         -pub fn old() {}\n\
         +pub fn new() {}\n\
         @@\n\
         -fn second_old() {}\n\
         +fn second_new() {}\n\
         *** End Patch"
    );

    let hunks = axon_patch_model::parse_patch(&edit.input).expect("lazy PathDiff");
    assert_eq!(hunks.len(), 1);
    let axon_patch_model::PatchHunk::UpdateFile { chunks, .. } = &hunks[0] else {
        panic!("expected update hunk");
    };
    assert_eq!(chunks.len(), 2);
}

#[test]
fn preserves_prefixed_lazy_edit_text_as_real_patch_lines() {
    let input = ToolCallInput::text(
        "*** Begin Patch\n\
         *** Update File: docs/lazy.md\n\
         @@\n\
         -old\n\
         + // ... existing code ...\n\
         *** End Patch",
    );

    let edit = agent_run_edit_tool_args_from_provider_input(&input).expect("edit args");
    assert!(edit.input.contains("+ // ... existing code ..."));
    assert!(!edit.input.contains("\n@@\n+ //"));
}

#[test]
fn pads_unprefixed_lazy_update_body_lines_as_context_lines() {
    let input = ToolCallInput::text(
        "*** Begin Patch\n*** Update File: src/lib.rs\n// ... existing code ...\nfn old() {}\n-old_call();\n+new_call();\nfn tail() {}\n*** End Patch",
    );

    let edit = agent_run_edit_tool_args_from_provider_input(&input).expect("edit args");
    assert_eq!(
        edit.input,
        "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n fn old() {}\n-old_call();\n+new_call();\n fn tail() {}\n*** End Patch"
    );

    let hunks =
        axon_patch_model::parse_patch(&edit.input).expect("补正后的 Lazy PathDiff 应可解析");
    assert_eq!(hunks.len(), 1);
    let axon_patch_model::PatchHunk::UpdateFile { chunks, .. } = &hunks[0] else {
        panic!("expected update hunk");
    };
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0].old_lines,
        vec!["fn old() {}", "old_call();", "fn tail() {}"]
    );
    assert_eq!(
        chunks[0].new_lines,
        vec!["fn old() {}", "new_call();", "fn tail() {}"]
    );
}

#[test]
fn leaves_unprefixed_update_body_untouched_without_lazy_boundary() {
    let input = "*** Begin Patch\n*** Update File: src/lib.rs\n@@\nfn old() {}\n*** End Patch";
    let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input))
        .expect("edit args");

    assert_eq!(edit.input, input);
    assert!(axon_patch_model::parse_patch(&edit.input).is_err());
}

#[test]
fn keeps_shell_command_lines_in_lazy_blocks_unpadded() {
    let input = "*** Begin Patch\n*** Update File: scripts/build.py\n// ... existing code ...\npython -m pytest tests\n*** End Patch";
    let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input))
        .expect("edit args");
    assert_eq!(
        edit.input,
        "*** Begin Patch\n*** Update File: scripts/build.py\n@@\npython -m pytest tests\n*** End Patch"
    );
    let error = axon_patch_model::parse_patch(&edit.input).expect_err("命令原文必须继续硬失败");
    assert!(error.to_string().contains("builtin.shell_command"));

    let prompted = "*** Begin Patch\n*** Update File: scripts/build.py\n// ... existing code ...\n$ cargo test\n*** End Patch";
    let prompted_edit =
        agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(prompted))
            .expect("edit args");
    assert!(prompted_edit.input.contains("\n$ cargo test\n"));
}

#[test]
fn keeps_structural_markers_in_lazy_blocks_unpadded() {
    let input = "*** Begin Patch\n*** Update File: src/app.css\n// ... existing code ...\n```\n[a.css#A1B2C3]\n@@ hooks\nbody { color: red; }\n*** End Patch";

    let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input))
        .expect("edit args");
    assert_eq!(
        edit.input,
        "*** Begin Patch\n*** Update File: src/app.css\n@@\n```\n[a.css#A1B2C3]\n@@ hooks\n body { color: red; }\n*** End Patch"
    );
}

#[test]
fn pads_only_the_lazy_update_block_in_a_multi_block_patch() {
    let input = "*** Begin Patch\n*** Update File: a.rs\n@@\n-let a = 1;\n+let a = 2;\n*** Update File: b.rs\n// ... existing code ...\nfn b() {}\n*** End Patch";

    let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input))
        .expect("edit args");
    assert_eq!(
        edit.input,
        "*** Begin Patch\n*** Update File: a.rs\n@@\n-let a = 1;\n+let a = 2;\n*** Update File: b.rs\n@@\n fn b() {}\n*** End Patch"
    );

    let hunks = axon_patch_model::parse_patch(&edit.input).expect("多块 PathDiff 应可解析");
    assert_eq!(hunks.len(), 2);
}

#[test]
fn pads_lazy_block_lines_with_crlf_endings() {
    let input = "*** Begin Patch\r\n*** Update File: a.rs\r\n// ... existing code ...\r\nfn a() {}\r\n*** End Patch";

    let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input))
        .expect("edit args");
    assert_eq!(
        edit.input,
        "*** Begin Patch\r\n*** Update File: a.rs\r\n@@\r\n fn a() {}\r\n*** End Patch"
    );

    let hunks = axon_patch_model::parse_patch(&edit.input).expect("CRLF Lazy PathDiff 应可解析");
    assert_eq!(hunks.len(), 1);
}

#[test]
fn extracts_structured_write_paths_in_stable_order() {
    let patch = ToolCallInput::json(serde_json::json!({
        "input": "*** Begin Patch\n*** Update File: old.rs\n*** Move to: new.rs\n@@\n-old\n+new\n*** End Patch"
    }));
    let edit = ToolCallInput::text("[src/b.rs#ABCDEF]\nDEL 1.=1\n[src/a.rs#EF0123]\nMV src/c.rs");

    assert_eq!(
        agent_run_edit_write_paths_from_provider_input(&patch).unwrap(),
        vec!["new.rs", "old.rs"]
    );
    assert_eq!(
        agent_run_edit_write_paths_from_provider_input(&edit).unwrap(),
        vec!["src/a.rs", "src/b.rs", "src/c.rs"]
    );
    let patch = ToolCallInput::text(
        "*** Begin Patch\n*** Update File: old.rs\n*** Move to: new.rs\n@@\n-old\n+new\n*** End Patch",
    );
    assert_eq!(
        agent_run_edit_write_paths_from_provider_input(&patch).unwrap(),
        vec!["new.rs", "old.rs"]
    );
}

#[test]
fn strips_common_apply_patch_spellings_from_hashline_headers() {
    for (input, expected) in [
        (
            "[*** Update File:src/a.ts#1A2B3C]\nDEL 1.=1",
            "[src/a.ts#1A2B3C]\nDEL 1.=1",
        ),
        (
            "[Update:src/a.ts#1A2B3C]\nDEL 1.=1",
            "[src/a.ts#1A2B3C]\nDEL 1.=1",
        ),
        (
            "[UpdateFile:src/a.ts#1A2B3C]\nDEL 1.=1",
            "[src/a.ts#1A2B3C]\nDEL 1.=1",
        ),
        (
            "[Update/File:src/a.ts#1A2B3C]\nDEL 1.=1",
            "[src/a.ts#1A2B3C]\nDEL 1.=1",
        ),
        (
            "[Update-file:src/a.ts#1A2B3C]\nDEL 1.=1",
            "[src/a.ts#1A2B3C]\nDEL 1.=1",
        ),
        (
            "[Update(File):src/a.ts#1A2B3C]\nDEL 1.=1",
            "[src/a.ts#1A2B3C]\nDEL 1.=1",
        ),
        (
            "[Update<File:src/a.ts#1A2B3C]\nDEL 1.=1",
            "[src/a.ts#1A2B3C]\nDEL 1.=1",
        ),
        (
            "[Add File:src/b.ts#4D5E6F]\nDEL 1.=1",
            "[src/b.ts#4D5E6F]\nDEL 1.=1",
        ),
        (
            "[Delete File:src/c.ts#708192]\nDEL 1.=1",
            "[src/c.ts#708192]\nDEL 1.=1",
        ),
        (
            "[Move to:src/d.ts#A1B2C3]\nDEL 1.=1",
            "[src/d.ts#A1B2C3]\nDEL 1.=1",
        ),
        (
            "[***src/e.ts#B2C3D4]\nDEL 1.=1",
            "[src/e.ts#B2C3D4]\nDEL 1.=1",
        ),
    ] {
        let edit = agent_run_edit_tool_args_from_provider_input(&ToolCallInput::text(input))
            .expect("edit args");
        assert_eq!(edit.input, expected, "输入 {input}");
        let headers = crate::hashline_headers(&edit.input).expect("修复后的头必须可解析");
        assert_eq!(headers.len(), 1, "输入 {input}");
    }
}
