use super::*;

#[test]
fn invalid_commit_messages_emit_expected_issue_codes() {
    let lint = CommitMessageLint::new();
    let long_desc = "a".repeat(100);
    let cases = [
        (
            "foobar: some changes".to_owned(),
            LintCode::MissingOrInvalidType,
        ),
        (
            "feat: add new feature".to_owned(),
            LintCode::DescriptionStartsLowercase,
        ),
        (
            "feat!: add breaking change".to_owned(),
            LintCode::BreakingChangeInconsistency,
        ),
        (format!("feat: {long_desc}"), LintCode::DescriptionTooLong),
        ("feat(): empty scope".to_owned(), LintCode::InvalidScope),
    ];

    for (message, code) in cases {
        let result = lint.lint(&message);
        assert!(
            result.issues.iter().any(|issue| issue.code == code),
            "{message} 应触发 {code:?}"
        );
    }

    assert!(!lint.lint("feat:").is_valid);
    assert!(!lint.lint("").is_valid);
}

#[test]
fn lowercase_description_auto_fix_capitalizes_first_word() {
    let result = CommitMessageLint::new().lint("feat: add new feature");

    assert!(
        result
            .fixed_message
            .as_deref()
            .is_some_and(|fixed| fixed.contains("Add"))
    );
}
