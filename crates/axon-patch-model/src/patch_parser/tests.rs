//! `patch_parser` 单元测试，对齐 Codex 解析器行为。

use super::{PatchHunk, PatchParseError, UpdateFileChunk, parse_patch, parse_patch_strict};

fn lines(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn rejects_missing_begin_marker() {
    assert_eq!(
        parse_patch("bad"),
        Err(PatchParseError::InvalidPatch(
            "The first line of the patch must be '*** Begin Patch'".to_owned()
        ))
    );
}

#[test]
fn rejects_missing_end_marker() {
    assert_eq!(
        parse_patch("*** Begin Patch\nbad"),
        Err(PatchParseError::InvalidPatch(
            "The last line of the patch must be '*** End Patch'".to_owned()
        ))
    );
}

#[test]
fn parses_add_file() {
    let hunks = parse_patch("*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch").unwrap();
    assert_eq!(
        hunks,
        vec![PatchHunk::AddFile {
            path: "foo".to_owned(),
            contents: "hi\n".to_owned(),
        }]
    );
}

#[test]
fn empty_update_hunk_is_rejected() {
    assert_eq!(
        parse_patch("*** Begin Patch\n*** Update File: test.py\n*** End Patch"),
        Err(PatchParseError::InvalidHunk {
            message: "Update file hunk for path 'test.py' is empty".to_owned(),
            line_number: 2,
        })
    );
}

#[test]
fn empty_patch_has_no_hunks() {
    assert_eq!(
        parse_patch("*** Begin Patch\n*** End Patch").unwrap(),
        vec![]
    );
}

#[test]
fn parses_multiple_operations() {
    let patch = "*** Begin Patch\n\
         *** Add File: path/add.py\n\
         +abc\n\
         +def\n\
         *** Delete File: path/delete.py\n\
         *** Update File: path/update.py\n\
         *** Move to: path/update2.py\n\
         @@ def f():\n\
         -    pass\n\
         +    return 123\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    assert_eq!(
        hunks,
        vec![
            PatchHunk::AddFile {
                path: "path/add.py".to_owned(),
                contents: "abc\ndef\n".to_owned(),
            },
            PatchHunk::DeleteFile {
                path: "path/delete.py".to_owned(),
            },
            PatchHunk::UpdateFile {
                path: "path/update.py".to_owned(),
                move_path: Some("path/update2.py".to_owned()),
                chunks: vec![UpdateFileChunk {
                    change_context: Some("def f():".to_owned()),
                    old_lines: lines(&["    pass"]),
                    new_lines: lines(&["    return 123"]),
                    is_end_of_file: false,
                }],
            },
        ]
    );
}

#[test]
fn update_hunk_followed_by_add() {
    let patch = "*** Begin Patch\n\
         *** Update File: file.py\n\
         @@\n\
         +line\n\
         *** Add File: other.py\n\
         +content\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    assert_eq!(
        hunks,
        vec![
            PatchHunk::UpdateFile {
                path: "file.py".to_owned(),
                move_path: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec![],
                    new_lines: lines(&["line"]),
                    is_end_of_file: false,
                }],
            },
            PatchHunk::AddFile {
                path: "other.py".to_owned(),
                contents: "content\n".to_owned(),
            },
        ]
    );
}

#[test]
fn update_hunk_without_explicit_context_marker() {
    let patch = "*** Begin Patch\n*** Update File: file2.py\n import foo\n+bar\n*** End Patch";
    let hunks = parse_patch(patch).unwrap();
    assert_eq!(
        hunks,
        vec![PatchHunk::UpdateFile {
            path: "file2.py".to_owned(),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: lines(&["import foo"]),
                new_lines: lines(&["import foo", "bar"]),
                is_end_of_file: false,
            }],
        }]
    );
}

#[test]
fn lenient_strips_heredoc_wrapper() {
    let inner = "*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch";
    for wrapper in ["<<EOF", "<<'EOF'", "<<\"EOF\""] {
        let wrapped = format!("{wrapper}\n{inner}\nEOF\n");
        let hunks = parse_patch(&wrapped).unwrap();
        assert_eq!(
            hunks,
            vec![PatchHunk::AddFile {
                path: "foo".to_owned(),
                contents: "hi\n".to_owned(),
            }]
        );
    }
}

#[test]
fn lenient_extracts_unique_patch_envelope_from_model_wrapper() {
    let wrapped = "下面是补丁：\n```patch\n\u{feff}*** Begin Patch\n\n*** Add File: 中文 文件.txt\n+内容\n\n*** End Patch\n```\n已完成";

    assert_eq!(
        parse_patch(wrapped).unwrap(),
        vec![PatchHunk::AddFile {
            path: "中文 文件.txt".to_owned(),
            contents: "内容\n".to_owned(),
        }]
    );
}

#[test]
fn lenient_allows_blank_lines_between_hunks() {
    let patch = "*** Begin Patch\n\n*** Add File: first.txt\n+first\n\n*** Delete File: second.txt\n\n*** End Patch";

    assert_eq!(
        parse_patch(patch).unwrap(),
        vec![
            PatchHunk::AddFile {
                path: "first.txt".to_owned(),
                contents: "first\n".to_owned(),
            },
            PatchHunk::DeleteFile {
                path: "second.txt".to_owned(),
            },
        ]
    );
}

#[test]
fn lenient_rejects_multiple_patch_envelopes() {
    let patch = "*** Begin Patch\n*** Add File: first.txt\n+first\n*** End Patch\n*** Begin Patch\n*** Add File: second.txt\n+second\n*** End Patch";

    assert!(matches!(
        parse_patch(patch),
        Err(PatchParseError::InvalidPatch(message))
            if message.contains("exactly one complete")
    ));
}

#[test]
fn strict_mode_rejects_heredoc_wrapper() {
    let inner = "*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch";
    let wrapped = format!("<<EOF\n{inner}\nEOF\n");
    assert_eq!(
        parse_patch_strict(&wrapped),
        Err(PatchParseError::InvalidPatch(
            "The first line of the patch must be '*** Begin Patch'".to_owned()
        ))
    );
    // 严格模式下不带包裹的补丁仍可解析。
    assert_eq!(
        parse_patch_strict(inner).unwrap(),
        vec![PatchHunk::AddFile {
            path: "foo".to_owned(),
            contents: "hi\n".to_owned(),
        }]
    );
}

#[test]
fn environment_id_preamble_is_consumed() {
    let patch = "*** Begin Patch\n\
         *** Environment ID: remote\n\
         *** Add File: hello.txt\n\
         +hello\n\
         *** End Patch";
    let hunks = parse_patch(patch).unwrap();
    assert_eq!(
        hunks,
        vec![PatchHunk::AddFile {
            path: "hello.txt".to_owned(),
            contents: "hello\n".to_owned(),
        }]
    );
}

#[test]
fn empty_environment_id_is_rejected() {
    let patch = "*** Begin Patch\n*** Environment ID:   \n*** Add File: x\n+y\n*** End Patch";
    assert_eq!(
        parse_patch(patch),
        Err(PatchParseError::InvalidPatch(
            "apply_patch environment_id cannot be empty".to_owned()
        ))
    );
}

#[test]
fn invalid_hunk_header_reports_line_number() {
    let patch = "*** Begin Patch\n*** Frobnicate File: foo\n*** End Patch";
    match parse_patch(patch) {
        Err(PatchParseError::InvalidHunk { line_number, .. }) => assert_eq!(line_number, 2),
        other => panic!("expected InvalidHunk, got {other:?}"),
    }
}

#[test]
fn rejects_hashline_header_inside_pathdiff_update_hunk() {
    let patch = "*** Begin Patch\n\
         *** Update File: tools/update_docs_and_svgs.py\n\
         @@\n\
         [tools/update_docs_and_svgs.py#CE1523]\n\
         +new content\n\
         *** End Patch";

    match parse_patch(patch) {
        Err(PatchParseError::InvalidHunk {
            message,
            line_number,
        }) => {
            assert_eq!(line_number, 4);
            assert!(message.contains("Unexpected line found in update hunk"));
            assert!(message.contains("Every line should start with"));
            assert!(message.contains("改用 builtin.shell_command"));
        }
        other => panic!("expected mixed Hashline/PathDiff input to be rejected, got {other:?}"),
    }
}

#[test]
fn eof_marker_sets_end_of_file_flag() {
    let patch = "*** Begin Patch\n*** Update File: f\n@@\n+line\n*** End of File\n*** End Patch";
    let hunks = parse_patch(patch).unwrap();
    assert_eq!(
        hunks,
        vec![PatchHunk::UpdateFile {
            path: "f".to_owned(),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: vec![],
                new_lines: lines(&["line"]),
                is_end_of_file: true,
            }],
        }]
    );
}
