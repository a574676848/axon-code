//! Streamable HTTP 分页与重复 cursor 测试响应。

pub(super) fn resources_paged_response(index: usize) -> (&'static str, String) {
    if index == 2 {
        return (
            "application/json",
            r#"{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"docs://guide","name":"Guide"}],"nextCursor":"page-2"}}"#
                .to_owned(),
        );
    }
    (
        "application/json",
        r#"{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"docs://api","name":"API"}]}}"#
            .to_owned(),
    )
}

pub(super) fn resource_templates_paged_response(index: usize) -> (&'static str, String) {
    if index == 2 {
        return (
            "application/json",
            r#"{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[{"uriTemplate":"docs://{name}","name":"Docs"}],"nextCursor":"templates-page-2"}}"#
                .to_owned(),
        );
    }
    (
        "application/json",
        r#"{"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[{"uriTemplate":"docs://api/{name}","name":"API"}]}}"#
            .to_owned(),
    )
}

pub(super) fn tools_paged_response(index: usize) -> (&'static str, String) {
    if index == 2 {
        return (
            "application/json",
            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query_db","description":"query db","inputSchema":{"type":"object","required":["query"]}}],"nextCursor":"tools-page-2"}}"#
                .to_owned(),
        );
    }
    (
        "application/json",
        r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"explain_db","description":"explain db","inputSchema":{"type":"object","required":["query"]}}]}}"#
            .to_owned(),
    )
}

pub(super) fn tools_duplicate_cursor_response() -> (&'static str, String) {
    (
        "application/json",
        r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query_db","description":"query db","inputSchema":{"type":"object","required":["query"]}}],"nextCursor":"loop"}}"#
            .to_owned(),
    )
}
