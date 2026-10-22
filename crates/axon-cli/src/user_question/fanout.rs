//! 问答事件扇出：同一次状态变化要同时到达运行态通道与渠道卡片。
//!
//! 两个消费者的职责不重叠：运行态通道负责逐字稿与界面刷新，渠道 sink 负责把提问
//! 变成用户能看到的卡片或消息。提问、延长、挂起、结算四类事件都必须两边都到，
//! 因此扇出按注册顺序逐个投递，任一 sink 自身的降级逻辑不影响其它 sink。

use std::sync::Arc;

use axon_domain::{UserAnswerItem, UserQuestionItem};

use super::UserQuestionEventSink;

/// 把一条问答事件依次投给多个 sink。
pub struct UserQuestionFanoutSink {
    sinks: Vec<Arc<dyn UserQuestionEventSink>>,
}

impl UserQuestionFanoutSink {
    /// 以固定顺序构造扇出。
    #[must_use]
    pub fn new(sinks: Vec<Arc<dyn UserQuestionEventSink>>) -> Self {
        Self { sinks }
    }

    /// 已注册的 sink 数量；装配自检用。
    #[must_use]
    pub fn sink_count(&self) -> usize {
        self.sinks.len()
    }
}

impl UserQuestionEventSink for UserQuestionFanoutSink {
    fn emit_question_event(
        &self,
        session_id: &str,
        part_id: &str,
        call_id: &str,
        status: &str,
        questions: &[UserQuestionItem],
        answers: &[UserAnswerItem],
        handoff: Option<&str>,
        deadline_at_ms: i64,
    ) {
        for sink in &self.sinks {
            sink.emit_question_event(
                session_id,
                part_id,
                call_id,
                status,
                questions,
                answers,
                handoff,
                deadline_at_ms,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 记录收到的事件，用于断言扇出确实两份都到。
    #[derive(Default)]
    struct RecordingSink {
        seen: Mutex<Vec<String>>,
    }

    impl UserQuestionEventSink for RecordingSink {
        fn emit_question_event(
            &self,
            _session_id: &str,
            _part_id: &str,
            call_id: &str,
            status: &str,
            _questions: &[UserQuestionItem],
            _answers: &[UserAnswerItem],
            _handoff: Option<&str>,
            _deadline_at_ms: i64,
        ) {
            self.seen
                .lock()
                .expect("记录锁")
                .push(format!("{call_id}:{status}"));
        }
    }

    #[test]
    fn fanout_delivers_every_event_to_every_sink_in_order() {
        let first = Arc::new(RecordingSink::default());
        let second = Arc::new(RecordingSink::default());
        let sink = UserQuestionFanoutSink::new(vec![first.clone(), second.clone()]);

        sink.emit_question_event(
            "lane-1",
            "part:question:call-1",
            "call-1",
            "pending",
            &[],
            &[],
            Some("等我"),
            1_000,
        );
        sink.emit_question_event(
            "lane-1",
            "part:question:call-1",
            "call-1",
            "settled",
            &[],
            &[],
            None,
            1_000,
        );

        let expected = vec!["call-1:pending".to_owned(), "call-1:settled".to_owned()];
        assert_eq!(*first.seen.lock().expect("记录锁"), expected);
        assert_eq!(*second.seen.lock().expect("记录锁"), expected);
        assert_eq!(sink.sink_count(), 2);
    }
}
