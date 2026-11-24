//! SQLite 写锁争用时的有界重试。
//!
//! 运行态库是单写者：并发写者在 WAL 下按 SQLite 自己的写锁排队，等待上限由连接的
//! `busy_timeout` 决定。突发写入（子代理逐帧追加、主轮收口、supervisor 逐秒 tick、
//! WAL 回写）叠在一起时，这一拍仍可能超时并返回 `database is locked`。
//!
//! 这类失败是**瞬时**的：锁的持有者仍在写，几毫秒到几百毫秒后就会放掉。把它直接当成
//! 终态上报，代价是丢一条 durable 事件（子代理逐字稿缺帧）或让整个 tick 失败。因此
//! 对「忙」这一类错误做有界重试，其余错误照旧立刻上抛。
//!
//! 重试只允许用在事务边界：失败的事务整体回滚，重来一次不会留下半成品。调用方因此
//! 必须把「一个完整事务」交给这里，不能把已经提交过的一半再包进来，否则水位之类的
//! 单调计数会被推进两次。

use super::RuntimeStoreError;
use std::time::Duration;

/// 单次写入的总尝试次数（含首次）。
const BUSY_RETRY_ATTEMPTS: u32 = 3;
/// 线性退避的基准间隔：第 n 次重试前等 `n * 基准`。
const BUSY_RETRY_BASE_DELAY: Duration = Duration::from_millis(40);

/// 是否为「库正忙」这一类可重试错误。
///
/// 只认 SQLite 自己的三种忙文案，不用宽泛的 `busy` 子串：网关侧的 `gateway_busy`
/// 也含这个词，把它当存储忙来重试只会白等。
fn is_busy(error: &RuntimeStoreError) -> bool {
    let RuntimeStoreError::Storage(detail) = error else {
        return false;
    };
    let lowered = detail.to_ascii_lowercase();
    lowered.contains("database is locked")
        || lowered.contains("database table is locked")
        || lowered.contains("database is busy")
}

/// 在「库正忙」时按有界退避重试一次事务。
pub(crate) fn with_busy_retry<T>(
    mut operation: impl FnMut() -> Result<T, RuntimeStoreError>,
) -> Result<T, RuntimeStoreError> {
    let mut attempt: u32 = 0;
    loop {
        match operation() {
            Ok(value) => {
                if attempt > 0 {
                    axon_logger::warning(&format!("运行态写入在第 {attempt} 次忙重试后成功"));
                }
                return Ok(value);
            }
            Err(error) if is_busy(&error) && attempt + 1 < BUSY_RETRY_ATTEMPTS => {
                attempt = attempt.saturating_add(1);
                let delay = BUSY_RETRY_BASE_DELAY.saturating_mul(attempt);
                axon_logger::warning(&format!(
                    "运行态写入遇到库忙，{}ms 后重试（第 {attempt} 次）: {error}",
                    delay.as_millis()
                ));
                std::thread::sleep(delay);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy() -> RuntimeStoreError {
        RuntimeStoreError::Storage("database is locked".to_owned())
    }

    #[test]
    fn a_transient_busy_is_absorbed() {
        let mut calls = 0_u32;
        let outcome = with_busy_retry(|| {
            calls = calls.saturating_add(1);
            if calls < 2 { Err(busy()) } else { Ok(7) }
        });
        assert_eq!(outcome.expect("一次忙必须被重试掉"), 7);
        assert_eq!(calls, 2);
    }

    #[test]
    fn persistent_busy_gives_up_after_the_attempt_budget() {
        let mut calls = 0_u32;
        let outcome: Result<u32, RuntimeStoreError> = with_busy_retry(|| {
            calls = calls.saturating_add(1);
            Err(busy())
        });
        assert!(outcome.is_err(), "预算用完必须如实上报");
        assert_eq!(calls, BUSY_RETRY_ATTEMPTS, "不得无限重试");
    }

    /// 别的错误不是「忙」：一次都不许重试，否则真实故障会被拖成静默。
    #[test]
    fn non_busy_errors_are_not_retried() {
        let mut calls = 0_u32;
        let outcome: Result<u32, RuntimeStoreError> = with_busy_retry(|| {
            calls = calls.saturating_add(1);
            Err(RuntimeStoreError::Fenced)
        });
        assert!(outcome.is_err());
        assert_eq!(calls, 1);
    }
}
