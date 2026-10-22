//! `RunRequest` 的会话身份与恢复输入。

use std::path::Path;

use axon_config::AxonConfig;

use super::{
    RunCommand,
    run_request::{PreparedRunInput, prepare_run_input},
    run_thread_store,
};

pub(super) struct RunRequestSession<'a> {
    previous: Option<&'a run_thread_store::PreparedRunSession>,
    session_id: String,
}

impl<'a> RunRequestSession<'a> {
    pub(super) fn with_id(
        previous: Option<&'a run_thread_store::PreparedRunSession>,
        session_id: String,
    ) -> Self {
        Self {
            previous,
            session_id,
        }
    }

    pub(super) fn explicit(session_id: String) -> Self {
        Self {
            previous: None,
            session_id,
        }
    }

    pub(super) fn prepare(
        self,
        command: &RunCommand,
        config: &AxonConfig,
        cwd: Option<&Path>,
    ) -> Result<
        (
            PreparedRunInput,
            Option<&'a run_thread_store::PreparedRunSession>,
        ),
        String,
    > {
        let previous = self.previous;
        let prepared = prepare_run_input(command, previous, self.session_id, config, cwd)?;
        Ok((prepared, previous))
    }
}
