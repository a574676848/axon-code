//! pre-final 守卫的行为测试（合同 §5.1.6）。
//!
//! 判据三条：无未终态作业时**不得**注入（反例，防止每轮都注入）；有未终态作业时
//! 注入一次；注入后模型仍给出最终答复则放行：至多延迟一轮，不得无限阻塞。

use super::*;

/// 守卫判据由它给出的 steer 源；除 pending 数外与测试源等价。
struct GuardTestSteerSource {
    inner: TestSteerSource,
    pending_jobs: std::cell::Cell<usize>,
}

impl GuardTestSteerSource {
    fn new(pending_jobs: usize) -> Self {
        Self {
            inner: TestSteerSource {
                messages: std::cell::RefCell::new(Vec::new()),
                reopen_count: std::cell::Cell::new(0),
            },
            pending_jobs: std::cell::Cell::new(pending_jobs),
        }
    }
}

impl AgentTurnSteerSource for GuardTestSteerSource {
    fn drain(&self) -> Vec<AgentTurnSteerInput> {
        self.inner.drain()
    }

    fn close_and_drain(&self) -> Vec<AgentTurnSteerInput> {
        self.inner.close_and_drain()
    }

    fn reopen(&self) {
        self.inner.reopen();
    }

    fn pending_background_jobs(&self) -> usize {
        self.pending_jobs.get()
    }
}

/// 跑一次完整 loop，返回（provider 请求体, turn 报告数）。
fn run_loop_with_pending_jobs(source: &dyn AgentTurnSteerSource) -> (Vec<String>, usize) {
    let input = test_transport_input();
    let request_bodies = std::cell::RefCell::new(Vec::new());

    let report = AgentTurnRuntime
        .run(
            &input,
            |transport_request| {
                request_bodies
                    .borrow_mut()
                    .push(transport_request.request.body_text.clone());
                crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
            },
            |_transport_report, _provider_history, _turn_id| {
                Ok::<AgentTurnToolDecision<String>, axon_domain::AxonError>(
                    AgentTurnToolDecision::AwaitExternal {
                        receipts: Vec::new(),
                    },
                )
            },
            Some(source),
            None,
        )
        .unwrap();

    (request_bodies.into_inner(), report.turn_reports.len())
}

/// 反例：没有未终态作业时不得注入守卫提醒，否则每轮都会白烧一次 provider 请求。
#[test]
fn pre_final_guard_does_not_inject_without_pending_jobs() {
    let source = GuardTestSteerSource::new(0);

    let (bodies, turns) = run_loop_with_pending_jobs(&source);

    assert_eq!(turns, 1, "无未终态作业时必须直接给出最终答复");
    assert_eq!(bodies.len(), 1);
    assert!(
        !bodies[0].contains("pre-final-guard"),
        "无未终态作业时不得注入守卫提醒"
    );
}

/// 有未终态作业时注入一次，并因此多请求一轮。
#[test]
fn pre_final_guard_injects_once_when_pending_jobs_exist() {
    let source = GuardTestSteerSource::new(2);

    let (bodies, turns) = run_loop_with_pending_jobs(&source);

    assert_eq!(turns, 2, "守卫必须延迟一轮并重新请求 provider");
    assert_eq!(bodies.len(), 2);
    let second: serde_json::Value = serde_json::from_str(&bodies[1]).unwrap();
    let messages = second["messages"].as_array().unwrap();
    assert!(
        messages.iter().any(|message| {
            message["role"] == "user"
                && message["content"].as_str().is_some_and(|content| {
                    content.contains("<runtime-note kind=\"pre-final-guard\">")
                        && content.contains("2 个后台作业")
                })
        }),
        "第二轮必须带上守卫提醒正文"
    );
}

/// 至多延迟一轮：注入后模型仍答复则放行，不得再拦第二次。
#[test]
fn pre_final_guard_delays_at_most_one_round() {
    let source = GuardTestSteerSource::new(5);

    let (bodies, turns) = run_loop_with_pending_jobs(&source);

    assert_eq!(turns, 2, "守卫至多延迟一轮，第二轮必须放行");
    assert_eq!(bodies.len(), 2, "不得为同一个守卫判据反复请求 provider");
    assert_eq!(
        bodies
            .iter()
            .filter(|body| body.contains("pre-final-guard"))
            .count(),
        1,
        "守卫提醒只允许注入一次"
    );
}
