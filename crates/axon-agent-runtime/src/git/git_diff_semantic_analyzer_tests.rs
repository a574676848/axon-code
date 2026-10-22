use super::*;

#[test]
fn test_semantic_classification() {
    let analyzer = DiffAnalyzer::new();

    assert_eq!(
        analyzer.classify_semantic("src/test_foo.rs"),
        ChangeSemantic::TestChange
    );
    assert_eq!(
        analyzer.classify_semantic("README.md"),
        ChangeSemantic::DocumentationUpdate
    );
    assert_eq!(
        analyzer.classify_semantic("Cargo.toml"),
        ChangeSemantic::DependencyChange
    );
    assert_eq!(
        analyzer.classify_semantic("fix_bug.rs"),
        ChangeSemantic::BugFix
    );
    assert_eq!(
        analyzer.classify_semantic("src/main.rs"),
        ChangeSemantic::FeatureAddition
    );
}

#[test]
fn test_risk_assessment() {
    let analyzer = DiffAnalyzer::new();

    let critical =
        analyzer.assess_risk("src/core/security/auth.rs", &ChangeSemantic::BugFix, 10, 5);
    assert_eq!(critical, RiskLevel::Critical);

    let low = analyzer.assess_risk(
        "src/utils/helper.rs",
        &ChangeSemantic::FeatureAddition,
        5,
        3,
    );
    assert_eq!(low, RiskLevel::Low);

    let high_changes = analyzer.assess_risk("src/foo.rs", &ChangeSemantic::FeatureAddition, 80, 30);
    assert_eq!(high_changes, RiskLevel::High);
}
