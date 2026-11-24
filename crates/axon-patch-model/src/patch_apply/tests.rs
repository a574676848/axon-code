//! `patch_apply` 单元测试，验证补丁计划计算。

use std::collections::HashMap;

use super::{PatchApplyError, PatchFileChange, compute_plan};
use crate::patch_parser::parse_patch;

fn snapshot(entries: &[(&str, &str)]) -> HashMap<String, String> {
    entries
        .iter()
        .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
        .collect()
}

#[test]
fn empty_patch_is_rejected() {
    let hunks = parse_patch("*** Begin Patch\n*** End Patch").unwrap();
    assert_eq!(
        compute_plan(&hunks, &HashMap::new()),
        Err(PatchApplyError::EmptyPatch)
    );
}

#[test]
fn add_file_yields_full_content() {
    let hunks =
        parse_patch("*** Begin Patch\n*** Add File: foo.txt\n+hello\n+world\n*** End Patch")
            .unwrap();
    let plan = compute_plan(&hunks, &HashMap::new()).unwrap();
    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].path, "foo.txt");
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Add {
            content: "hello\nworld\n".to_owned()
        }
    );
}

#[test]
fn delete_file_captures_previous_content() {
    let hunks = parse_patch("*** Begin Patch\n*** Delete File: gone.txt\n*** End Patch").unwrap();
    let snap = snapshot(&[("gone.txt", "old\n")]);
    let plan = compute_plan(&hunks, &snap).unwrap();
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Delete {
            previous_content: Some("old\n".to_owned())
        }
    );
}

#[test]
fn add_file_over_existing_snapshot_is_planned_as_a_whole_file_rewrite() {
    let hunks =
        parse_patch("*** Begin Patch\n*** Add File: foo.txt\n+replaced\n*** End Patch").unwrap();
    let snap = snapshot(&[("foo.txt", "old\n")]);

    let plan = compute_plan(&hunks, &snap).unwrap();

    assert_eq!(plan.operations.len(), 1);
    assert_eq!(plan.operations[0].path, "foo.txt");
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Update {
            move_path: None,
            new_content: "replaced\n".to_owned(),
        }
    );
}

#[test]
fn update_replaces_matched_lines() {
    let patch = "*** Begin Patch\n\
         *** Update File: src/lib.rs\n\
         @@\n\
         -    let a = 1;\n\
         +    let a = 2;\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let snap = snapshot(&[("src/lib.rs", "fn main() {\n    let a = 1;\n}\n")]);
    let plan = compute_plan(&hunks, &snap).unwrap();
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Update {
            move_path: None,
            new_content: "fn main() {\n    let a = 2;\n}\n".to_owned(),
        }
    );
}

#[test]
fn update_with_context_locates_chunk() {
    let patch = "*** Begin Patch\n\
         *** Update File: f.py\n\
         @@ def g():\n\
         -    return 1\n\
         +    return 2\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let original = "def f():\n    return 1\ndef g():\n    return 1\n";
    let snap = snapshot(&[("f.py", original)]);
    let plan = compute_plan(&hunks, &snap).unwrap();
    // 上下文 def g(): 应确保替换发生在第二处 return 1。
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Update {
            move_path: None,
            new_content: "def f():\n    return 1\ndef g():\n    return 2\n".to_owned(),
        }
    );
}

#[test]
fn pure_addition_appends_to_end() {
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n+new tail\n*** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let snap = snapshot(&[("a.txt", "first\nsecond\n")]);
    let plan = compute_plan(&hunks, &snap).unwrap();
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Update {
            move_path: None,
            new_content: "first\nsecond\nnew tail\n".to_owned(),
        }
    );
}

#[test]
fn update_with_move_keeps_source_path_and_records_target() {
    let patch = "*** Begin Patch\n\
         *** Update File: old.rs\n\
         *** Move to: new.rs\n\
         @@\n\
         -old\n\
         +new\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let snap = snapshot(&[("old.rs", "old\n")]);
    let plan = compute_plan(&hunks, &snap).unwrap();
    // 源路径保留在 path，移动目标放在 move_path，便于执行层删除原文件。
    assert_eq!(plan.operations[0].path, "old.rs");
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Update {
            move_path: Some("new.rs".to_owned()),
            new_content: "new\n".to_owned(),
        }
    );
}

#[test]
fn missing_target_content_is_reported() {
    let patch = "*** Begin Patch\n*** Update File: ghost.rs\n@@\n-x\n+y\n*** End Patch";
    let hunks = parse_patch(patch).unwrap();
    assert_eq!(
        compute_plan(&hunks, &HashMap::new()),
        Err(PatchApplyError::MissingFileContent {
            path: "ghost.rs".to_owned()
        })
    );
}

#[test]
fn unmatched_old_lines_are_reported() {
    let patch = "*** Begin Patch\n*** Update File: x.rs\n@@\n-not present\n+y\n*** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let snap = snapshot(&[("x.rs", "something else\n")]);
    assert_eq!(
        compute_plan(&hunks, &snap),
        Err(PatchApplyError::OldLinesNotFound {
            path: "x.rs".to_owned(),
            old_lines: "not present".to_owned(),
            attempted_levels: vec![crate::patch_apply::MatchLevel::Exact],
            similar_lines: vec!["something else".to_string()],
        })
    );
}

#[test]
fn multiple_chunks_apply_in_order() {
    let patch = "*** Begin Patch\n\
         *** Update File: multi.txt\n\
         @@\n\
         -alpha\n\
         +ALPHA\n\
         @@\n\
         -gamma\n\
         +GAMMA\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let snap = snapshot(&[("multi.txt", "alpha\nbeta\ngamma\n")]);
    let plan = compute_plan(&hunks, &snap).unwrap();
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Update {
            move_path: None,
            new_content: "ALPHA\nbeta\nGAMMA\n".to_owned(),
        }
    );
}

#[test]
fn duplicate_pathdiff_file_is_rejected_before_plan_generation() {
    let patch = "*** Begin Patch\n\
         *** Update File: docs/CONTRIBUTING.md\n\
         @@\n\
         -old\n\
         +new\n\
         *** Update File: docs/CONTRIBUTING.md\n\
         @@\n\
         -new\n\
         +newer\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let snap = snapshot(&[("docs/CONTRIBUTING.md", "old\n")]);

    assert_eq!(
        compute_plan(&hunks, &snap),
        Err(PatchApplyError::DuplicatePath {
            path: "docs/CONTRIBUTING.md".to_owned(),
        })
    );
}

#[test]
fn multiple_chunks_out_of_order_apply_successfully() {
    let patch = "*** Begin Patch\n\
         *** Update File: multi.txt\n\
         @@\n\
         -three\n\
         +THREE\n\
         @@\n\
         -one\n\
         +ONE\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    let snap = snapshot(&[("multi.txt", "one\ntwo\nthree\n")]);
    let plan = compute_plan(&hunks, &snap).unwrap();
    assert_eq!(
        plan.operations[0].change,
        PatchFileChange::Update {
            move_path: None,
            new_content: "ONE\ntwo\nTHREE\n".to_owned(),
        }
    );
}
