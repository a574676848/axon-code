use serde_json::json;

use crate::mcp_transport_prompts::{parse_prompt_get_response, parse_prompts_list_response};

#[test]
fn parses_mcp_prompts_list_projection() {
    let value = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "result": {
            "prompts": [
                {
                    "name": "code_review",
                    "title": "Code Review",
                    "description": "review prompt",
                    "arguments": [
                        {
                            "name": "target",
                            "title": "Target",
                            "description": "target path",
                            "required": true
                        }
                    ]
                }
            ]
        }
    });

    let prompts = parse_prompts_list_response(&value).expect("prompts/list 应可解析");

    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "code_review");
    assert_eq!(prompts[0].arguments[0].name, "target");
    assert!(prompts[0].arguments[0].required);
}

#[test]
fn parses_mcp_prompt_get_text_message() {
    let value = json!({
        "jsonrpc": "2.0",
        "id": 8,
        "result": {
            "description": "review prompt",
            "messages": [
                {
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": "review src/lib.rs"
                    }
                }
            ]
        }
    });

    let messages = parse_prompt_get_response(&value).expect("prompts/get 应可解析");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content_type, "text");
    assert_eq!(messages[0].text.as_deref(), Some("review src/lib.rs"));
}
