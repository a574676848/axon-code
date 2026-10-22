use crate::{AgentRunToolResultReceipt, AgentRunToolResultReceiptFacts, ProviderToolCall};

pub(super) fn tool_call(id: &str) -> ProviderToolCall {
    ProviderToolCall::new(id, "builtin.search", "{}")
}

pub(super) fn receipt(id: &str) -> AgentRunToolResultReceipt {
    AgentRunToolResultReceipt {
        provider_call_id: id.to_owned(),
        tool_name: "builtin.search".to_owned(),
        status: "completed".to_owned(),
        summary: "ok".to_owned(),
        metadata: None,
        output_ref: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    }
}

#[derive(Debug)]
pub(super) struct FixtureToolReceipt {
    pub(super) provider_call_id: String,
    pub(super) tool_name: String,
    pub(super) status: String,
    pub(super) summary: String,
    pub(super) metadata: Option<serde_json::Value>,
    pub(super) output_ref: Option<axon_domain::OutputRef>,
    pub(super) error_type: Option<String>,
    pub(super) remote_trigger_allowed: bool,
    pub(super) filesystem_write_performed: bool,
}

impl AgentRunToolResultReceiptFacts for FixtureToolReceipt {
    fn provider_call_id(&self) -> &str {
        &self.provider_call_id
    }

    fn tool_name(&self) -> &str {
        &self.tool_name
    }

    fn status(&self) -> &str {
        &self.status
    }

    fn summary(&self) -> &str {
        &self.summary
    }

    fn metadata(&self) -> Option<&serde_json::Value> {
        self.metadata.as_ref()
    }

    fn output_ref(&self) -> Option<&axon_domain::OutputRef> {
        self.output_ref.as_ref()
    }

    fn error_type(&self) -> Option<&str> {
        self.error_type.as_deref()
    }

    fn remote_trigger_allowed(&self) -> bool {
        self.remote_trigger_allowed
    }

    fn filesystem_write_performed(&self) -> bool {
        self.filesystem_write_performed
    }
}
