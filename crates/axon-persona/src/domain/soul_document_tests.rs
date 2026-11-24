//! `SOUL.md` 解析与校验的边界用例。

use super::*;
use crate::domain::voice::HumorLevel;

const VALID: &str = "---\nschema: axon-soul/v1\nidentity:\n  name: Axon\n  address: 长官\n  role: 战术副官\nbackstory: |\n  来自工程一线。\ninvariants:\n  - 依据事实作答\ntraits: [沉稳, 克制]\nvoice:\n  warmth: 40\n  formality: 60\n  humor: subtle\n  verbosity: concise\nrules:\n  - when: { mode: incident }\n    voice: { humor: off, verbosity: minimal }\n---\n正文补充。\n";

fn assert_invalid(result: Result<SoulDocument, SoulError>, expected_field: &str) {
    match result {
        Err(SoulError::Invalid { field, .. }) => assert_eq!(field, expected_field),
        other => panic!("期望字段 {expected_field} 校验失败，实际为 {other:?}"),
    }
}

#[test]
fn valid_document_parses_every_section() {
    let document = SoulDocument::parse(VALID).expect("合法文档应解析成功");

    assert_eq!(document.schema, SOUL_SCHEMA_V1);
    assert_eq!(document.core.identity.name, "Axon");
    assert_eq!(document.core.identity.address, "长官");
    assert_eq!(document.core.backstory, "来自工程一线。");
    assert_eq!(document.core.invariants, vec!["依据事实作答"]);
    assert_eq!(document.core.traits, vec!["沉稳", "克制"]);
    assert_eq!(document.core.voice.warmth, 40);
    assert_eq!(document.core.voice.formality, 60);
    assert_eq!(document.core.voice.humor, HumorLevel::Subtle);
    assert_eq!(document.core.rules.len(), 1);
    assert_eq!(document.core.rules[0].voice.humor, Some(HumorLevel::Off));
    assert_eq!(document.body.as_deref(), Some("正文补充。"));
}

#[test]
fn empty_body_is_none_not_empty_string() {
    let raw = VALID.replace("正文补充。\n", "\n   \n");
    let document = SoulDocument::parse(&raw).expect("正文为空也应解析成功");

    assert_eq!(document.body, None);
    assert_eq!(document.core.notes, None);
}

#[test]
fn crlf_and_bom_are_normalized() {
    let raw = format!("\u{feff}{}", VALID.replace('\n', "\r\n"));
    let document = SoulDocument::parse(&raw).expect("CRLF 与 BOM 应被容忍");

    assert_eq!(document.core.identity.name, "Axon");
}

#[test]
fn missing_frontmatter_is_rejected() {
    assert_eq!(
        SoulDocument::parse("[SOUL_DEFINITION]\n- 角色名称：Axon\n"),
        Err(SoulError::MissingFrontmatter)
    );
}

#[test]
fn unterminated_frontmatter_is_rejected() {
    assert_eq!(
        SoulDocument::parse("---\nschema: axon-soul/v1\n"),
        Err(SoulError::UnterminatedFrontmatter)
    );
}

#[test]
fn schema_version_mismatch_is_rejected_with_field_path() {
    let raw = VALID.replace("axon-soul/v1", "axon-soul/v0");

    assert_invalid(SoulDocument::parse(&raw), "schema");
}

#[test]
fn unknown_root_field_is_rejected_instead_of_ignored() {
    let raw = VALID.replace("schema: axon-soul/v1", "schema: axon-soul/v1\ntemper: 暴躁");

    assert_invalid(SoulDocument::parse(&raw), "根节点.temper");
}

#[test]
fn unknown_nested_field_is_rejected_with_full_path() {
    let raw = VALID.replace("  role: 战术副官", "  role: 战术副官\n  age: 3");

    assert_invalid(SoulDocument::parse(&raw), "identity.age");
}

#[test]
fn enum_out_of_range_reports_the_leaf_field() {
    let raw = VALID.replace("humor: subtle", "humor: loud");

    assert_invalid(SoulDocument::parse(&raw), "voice.humor");
}

#[test]
fn numeric_out_of_range_is_rejected() {
    let raw = VALID.replace("warmth: 40", "warmth: 140");
    assert_invalid(SoulDocument::parse(&raw), "voice.warmth");

    let negative = VALID.replace("warmth: 40", "warmth: -1");
    assert_invalid(SoulDocument::parse(&negative), "voice.warmth");
}

#[test]
fn empty_invariants_and_traits_are_rejected() {
    let no_invariants = VALID.replace("invariants:\n  - 依据事实作答\n", "invariants: []\n");
    assert_invalid(SoulDocument::parse(&no_invariants), "invariants");

    let no_traits = VALID.replace("traits: [沉稳, 克制]", "traits: []");
    assert_invalid(SoulDocument::parse(&no_traits), "traits");
}

#[test]
fn blank_identity_field_is_rejected() {
    let raw = VALID.replace("  name: Axon", "  name: \"   \"");

    assert_invalid(SoulDocument::parse(&raw), "identity.name");
}

#[test]
fn rule_without_condition_is_rejected() {
    let raw = VALID.replace("  - when: { mode: incident }", "  - when: {}");

    assert_invalid(SoulDocument::parse(&raw), "rules[0].when");
}

#[test]
fn rule_without_patch_is_rejected() {
    let raw = VALID.replace(
        "    voice: { humor: off, verbosity: minimal }",
        "    voice: {}",
    );

    assert_invalid(SoulDocument::parse(&raw), "rules[0].voice");
}

#[test]
fn rule_with_unknown_condition_key_is_rejected() {
    let raw = VALID.replace("mode: incident", "mode: incident, cwd: /tmp");

    assert_invalid(SoulDocument::parse(&raw), "rules[0].when.cwd");
}

#[test]
fn oversized_document_is_rejected_before_parsing() {
    let filler = "x".repeat(SOUL_DOCUMENT_MAX_BYTES + 1);
    let raw = format!("---\nschema: axon-soul/v1\n---\n{filler}");

    assert!(matches!(
        SoulDocument::parse(&raw),
        Err(SoulError::TooLarge { .. })
    ));
}

#[test]
fn syntax_error_reports_one_based_line_number() {
    let raw = "---\nschema: axon-soul/v1\nidentity: [unclosed\n---\n";
    let error = SoulDocument::parse(raw).expect_err("语法错误应被拒绝");

    match error {
        SoulError::Syntax { line, .. } => assert!(line >= 2, "行号应为一基且覆盖 frontmatter"),
        other => panic!("期望语法错误，实际为 {other:?}"),
    }
}

#[test]
fn error_messages_are_chinese_and_actionable() {
    let error = SoulDocument::parse("no frontmatter").expect_err("应报错");

    assert!(error.to_string().contains("frontmatter"));
    assert!(error.to_string().contains("axon-soul/v1"));
}
#[test]
fn shipped_default_template_parses() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defaults/SOUL.md");
    /*
     * 仓库资产在极端情况下会短暂读不到：并发执行的打包、备份或索引进程，以及同仓库的
     * git 操作都可能让文件在毫秒级窗口里不可见。重试两次再判定缺失，缺失时给出确切
     * 路径与「文件不存在」的结论，而不是让断言报一句与原因无关的话。
     */
    let mut attempts = 0;
    let mut raw = String::new();
    let mut last_error = None;
    while attempts < 3 {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                raw = text;
                break;
            }
            Err(error) => {
                last_error = Some(error);
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    assert!(
        !raw.is_empty(),
        "默认模板读不到：{}（{last_error:?}）",
        path.display()
    );

    let document = SoulDocument::parse(&raw).expect("默认模板必须符合 axon-soul/v1");

    assert_eq!(document.core.identity.address, "长官");
    assert!(!document.core.invariants.is_empty());
    assert_eq!(document.core.rules.len(), 2);
}
