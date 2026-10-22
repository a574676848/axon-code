use axon_domain::ToolInputFormat;

use crate::{ModelToolDefinition, provider_custom_tool_name, provider_tool_name};

pub(crate) fn openai_responses_tools(tools: &[ModelToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| match &tool.input_format {
            ToolInputFormat::JsonSchema => serde_json::json!({
                "type": "function",
                "name": provider_tool_name(&tool.qualified_name),
                "description": tool.description,
                "parameters": tool.input_schema,
            }),
            ToolInputFormat::Freeform { format, .. } => serde_json::json!({
                "type": "custom",
                "name": provider_custom_tool_name(&tool.qualified_name),
                "description": tool.description,
                "format": {
                    "type": format.format_type,
                    "syntax": format.syntax,
                    "definition": format.definition,
                },
            }),
        })
        .collect()
}

pub(crate) fn openai_tools(tools: &[ModelToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": provider_tool_name(&tool.qualified_name),
                    "description": tool.description,
                    "parameters": tool.input_schema,
                }
            })
        })
        .collect()
}
