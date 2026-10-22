use ratatui::style::Style;

use super::{
    transcript_cells::display_width,
    transcript_markdown_test_support::{line_text, render_message_markdown},
};

const MERMAID_SOURCE: &str = r#"```mermaid
erDiagram
    PROVIDER_DEFINITION ||--o{ ACTION_DEFINITION : "contains actions for provider"
    PROVIDER_DEFINITION {
        string service PK
    }
```"#;

#[test]
fn mermaid_fallback_preserves_source_indentation() {
    let rendered = rendered_lines(120);

    assert!(rendered.iter().any(|line| {
        line.ends_with(
            "    PROVIDER_DEFINITION ||--o{ ACTION_DEFINITION : \"contains actions for provider\"",
        )
    }));
    assert!(
        rendered
            .iter()
            .any(|line| line.ends_with("        string service PK"))
    );
}

#[test]
fn mermaid_fallback_reflows_once_with_hanging_indent() {
    let rendered = rendered_lines(40);
    let relation_index = rendered
        .iter()
        .position(|line| line.contains("PROVIDER_DEFINITION ||--o{"))
        .expect("应渲染 Mermaid 关系行");
    let relation = &rendered[relation_index];
    let continuation = rendered
        .get(relation_index + 1)
        .expect("超宽 Mermaid 关系行应由 Markdown renderer 生成续行");

    assert!(rendered.iter().all(|line| display_width(line) <= 40));
    assert!(!continuation.contains("PROVIDER_DEFINITION {"));
    assert!(leading_width(continuation) >= leading_width(relation));
}

fn rendered_lines(width: u16) -> Vec<String> {
    render_message_markdown(MERMAID_SOURCE, Style::default(), width)
        .iter()
        .map(line_text)
        .collect()
}

fn leading_width(line: &str) -> usize {
    display_width(line.trim_end().trim_start_matches(char::is_whitespace))
        .abs_diff(display_width(line.trim_end()))
}
