//! 终态批次的载荷布局：`turn_metrics` 与终态同批落库，但不进实时投递。
//!
//! `session_metrics` 只由 `turn_metrics` 累加，header 的会话累计耗时与 token 读数全部
//! 来自它；一旦这条派生事件漏写，界面不会报错，只是整段读数消失。因此这里把布局钉住：
//! 指标必须进 add 批次，且必须落在可投递前缀之外。

use super::terminal_batch;

fn event(name: &str) -> (String, String) {
    (name.to_owned(), "{}".to_owned())
}

#[test]
fn turn_metrics_is_appended_last_and_stays_out_of_the_publishable_prefix() {
    let (payloads, publishable) =
        terminal_batch(vec![event("turn_completed")], Some(event("turn_metrics")));

    assert_eq!(payloads.len(), 2, "终态与指标必须同批");
    assert_eq!(payloads[0].0, "turn_completed");
    assert_eq!(payloads[1].0, "turn_metrics");
    assert_eq!(publishable, 1, "只有终态可投递，指标只落库");
}

#[test]
fn absent_metrics_keep_the_batch_untouched() {
    let (payloads, publishable) = terminal_batch(vec![event("turn_completed")], None);

    assert_eq!(payloads.len(), 1);
    assert_eq!(publishable, 1);
}
