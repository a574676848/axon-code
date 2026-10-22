//! Provider 请求发送；窗口门禁由自动换窗策略统一负责。

use super::*;
pub(super) struct WindowRequest<'a> {
    pub input: &'a ProviderStreamingTransportInput,
}

impl WindowRequest<'_> {
    pub(super) fn send<F>(&self, transport: F) -> AxonResult<ProviderStreamingTransportResult>
    where
        F: FnMut(&ProviderStreamingTransportRequest) -> ProviderStreamingTransportOutcome,
    {
        ProviderStreamingTransportRuntime.run_with_transport(self.input, transport)
    }
}

#[cfg(test)]
#[path = "request_window_tests.rs"]
mod tests;
