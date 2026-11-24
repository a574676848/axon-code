//! 会话身份与标识生成。
//!
//! 会话标识是服务端生成的 ULID：与渠道、投递目标、受众和工作区都无关，生成一次、此后
//! 不变。渠道属于轮次事实（`origin_channel` 与会话级分布投影），不参与会话身份，也不
//! 能由投递目标反解出来。
//!
//! 生成点集中在本文一处：任何模块都不得自行拼接或用哈希派生会话标识。

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord,
};

use crate::domain::runtime::{SessionKind, SessionTrustScope};

/// ULID 文本长度。
pub const ULID_LENGTH: usize = 26;
/// 毫秒时间戳占用的字符数。
const TIME_CHARS: usize = 10;
/// 随机部分占用的字符数。
const RANDOM_CHARS: usize = 16;
/// 随机部分的总位数。
const RANDOM_BITS: u32 = 80;
/// 随机部分的有效位掩码。
const RANDOM_MASK: u128 = (1_u128 << RANDOM_BITS) - 1;
/// Crockford Base32 字符集：去掉 `I`、`L`、`O`、`U`。
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 上一次生成使用的毫秒时间戳；同一毫秒内沿用，时钟回拨也不后退。
static LAST_TIMESTAMP_MS: AtomicU64 = AtomicU64::new(0);
/// 进程级随机起点：高 16 位取一次随机，低 64 位作为序号基数。
static RANDOM_EPOCH: OnceLock<(u16, u64)> = OnceLock::new();
/// 进程内单调递增的序号；与随机基数相加得到随机部分低 64 位。
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 生成一个新的 ULID 文本。
///
/// 前 48 位是毫秒时间戳，后 80 位是随机量。单调性由两条互不干扰的事实保证：
///
/// - 时间戳只前进：同一毫秒内沿用上一次的毫秒，时钟回拨时也不后退；
/// - 随机部分低 64 位取「进程级随机基数 + 只增序号」，序号由一次原子 `fetch_add`
///   独占取得，因此同一毫秒内严格递增，且不存在「先读高位、再读低位」被别的线程
///   插进来造成的组合错乱（这正是把 80 位拆成两个原子分别读写的旧实现的问题）。
///
/// 跨毫秒时时间戳变大，随机部分即使被重置也不影响字典序。全过程只用原子操作与一次
/// 惰性初始化，不抢全局锁。
#[must_use]
pub fn new_ulid() -> String {
    let timestamp_ms = monotonic_timestamp_ms(axon_time::unix_now_ms());
    let random = (u128::from(random_epoch().0) << 64) | u128::from(next_random_low());
    encode(timestamp_ms, random & RANDOM_MASK)
}

/// 时间戳只前进：同一毫秒内沿用上一次的毫秒，时钟回拨也停在原地。
fn monotonic_timestamp_ms(now_ms: u64) -> u64 {
    let mut last = LAST_TIMESTAMP_MS.load(Ordering::Acquire);
    loop {
        if now_ms <= last {
            return last;
        }
        match LAST_TIMESTAMP_MS.compare_exchange_weak(
            last,
            now_ms,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return now_ms,
            Err(observed) => last = observed,
        }
    }
}

/// 本次使用的随机部分低 64 位：随机基数加只增序号。
///
/// 序号在 2^64 次生成后回绕，那之后与随机基数相加可能重复；单进程不可能达到这个量级，
/// 因此按「不回绕」处理，不做额外的饱和判断。
fn next_random_low() -> u64 {
    random_epoch()
        .1
        .wrapping_add(SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

/// 生成新的会话标识。
///
/// # Panics
///
/// ULID 恒定非空且长度固定，构造失败只可能是标识类型约束被改坏。
#[must_use]
pub fn new_session_id() -> DaemonSessionId {
    DaemonSessionId::new(new_ulid()).expect("ULID 必为合法会话标识")
}

/// 生成新的交互记录标识。
///
/// # Panics
///
/// ULID 恒定非空且长度固定，构造失败只可能是标识类型约束被改坏。
#[must_use]
pub fn new_interaction_id() -> InteractionId {
    InteractionId::new(new_ulid()).expect("ULID 必为合法交互标识")
}

/// 判断文本是否是形态合法的 ULID。
#[must_use]
pub fn is_ulid(value: &str) -> bool {
    value.len() == ULID_LENGTH
        && value
            .bytes()
            .all(|byte| CROCKFORD.contains(&byte.to_ascii_uppercase()))
}

/// 由 Interaction 读取会话标识。
///
/// 会话标识是入站事实的一列，随 Interaction 一起持久化；本函数只做读取，不再派生。
#[must_use]
pub fn interaction_session_id(interaction: &InteractionRecord) -> String {
    interaction.session_id.as_str().to_owned()
}

/// 按受众与披露边界判定会话的隔离维度。
#[must_use]
pub const fn trust_scope_of(audience: &AudienceContext) -> SessionTrustScope {
    match (audience.kind(), audience.disclosure()) {
        (AudienceKind::Private, DisclosurePolicy::PrivateOnly) => SessionTrustScope::Private,
        (AudienceKind::Group, DisclosurePolicy::Workspace) => SessionTrustScope::GroupWorkspace,
        _ => SessionTrustScope::Restricted,
    }
}

/// 主 turn 由入站事实直接使用，子代理与执行会话由各自的创建路径显式声明类型。
#[must_use]
pub const fn default_kind_for_interaction() -> SessionKind {
    SessionKind::Primary
}

/// 进程级随机起点：高 16 位取一次随机，低 64 位作为序号基数。
///
/// 只在第一次生成时取一次：随机部分因此不再逐毫秒重置，跨毫秒的单调性交给时间戳，
/// 同毫秒的单调性交给序号，两者互不使用对方的原子字，也就不存在组合错乱。
fn random_epoch() -> (u16, u64) {
    *RANDOM_EPOCH.get_or_init(|| {
        let raw = uuid::Uuid::new_v4().as_u128();
        ((raw >> 64) as u16, raw as u64)
    })
}

/// 把时间戳与随机部分编码成 26 字符 Crockford Base32。
fn encode(timestamp_ms: u64, random: u128) -> String {
    let value = (u128::from(timestamp_ms) & ((1_u128 << 48) - 1)) << RANDOM_BITS | random;
    let mut buffer = String::with_capacity(ULID_LENGTH);
    for index in 0..ULID_LENGTH {
        let shift = 5 * (ULID_LENGTH - 1 - index);
        let digit = (value >> shift) & 0x1F;
        buffer.push(char::from(CROCKFORD[digit as usize]));
    }
    debug_assert_eq!(buffer.chars().count(), ULID_LENGTH);
    debug_assert_eq!(
        TIME_CHARS + RANDOM_CHARS,
        ULID_LENGTH,
        "时间戳与随机部分的字符数必须等于总长度"
    );
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{InteractionSource, ReplyRoute, ScopeRef};

    #[test]
    fn ulid_uses_crockford_alphabet_and_fixed_length() {
        for _ in 0..64 {
            let value = new_ulid();
            assert_eq!(value.len(), ULID_LENGTH);
            assert!(is_ulid(&value), "字符集必须在 Crockford Base32 内：{value}");
            assert!(!value.contains(['I', 'L', 'O', 'U']));
        }
    }

    #[test]
    fn ulid_is_monotonic_even_within_the_same_millisecond() {
        let mut previous = String::new();
        for _ in 0..256 {
            let current = new_ulid();
            assert!(
                current > previous,
                "同毫秒内生成的标识必须严格递增：{previous} -> {current}"
            );
            previous = current;
        }
    }

    /// 并发下也成立：每个线程内部严格递增，跨线程不重复。
    ///
    /// 回归背景：旧实现把 80 位随机量拆成 `RANDOM_HIGH` 与 `RANDOM_LOW` 两个原子，
    /// 「认领序号」与「读取最终值」是两次独立读，别的线程一旦推进就会读到错位的组合，
    /// 同一毫秒内出现相等或逆序，全量并行跑必现。
    #[test]
    fn ulid_stays_unique_and_monotonic_under_concurrency() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};

        let threads = 8;
        let per_thread = 500;
        let barrier = Arc::new(Barrier::new(threads));
        let handles = (0..threads)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    (0..per_thread).map(|_| new_ulid()).collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();

        let mut seen = HashSet::new();
        for handle in handles {
            let generated = handle.join().expect("生成线程不应 panic");
            assert!(
                generated.windows(2).all(|pair| pair[0] < pair[1]),
                "同一线程内必须严格递增"
            );
            for value in generated {
                assert!(seen.insert(value.clone()), "标识必须唯一：{value}");
            }
        }
        assert_eq!(seen.len(), threads * per_thread);
    }

    #[test]
    fn ulid_encodes_the_current_millisecond_in_the_first_ten_chars() {
        let value = new_ulid();
        let decoded = value.bytes().take(TIME_CHARS).fold(0_u64, |acc, byte| {
            let digit = CROCKFORD
                .iter()
                .position(|candidate| *candidate == byte)
                .expect("字符必须在字符集内");
            (acc << 5) | digit as u64
        });
        // 前 10 个字符承载 50 位，其中低 48 位是毫秒时间戳；高位补零不参与比较。
        let timestamp = decoded & ((1_u64 << 48) - 1);
        let now = axon_time::unix_now_ms();
        assert!(
            now.saturating_sub(timestamp) < 60_000,
            "时间戳部分必须落在当前毫秒附近：{timestamp} 对 {now}"
        );
    }

    #[test]
    fn session_identity_reads_the_column_instead_of_deriving_from_route() {
        let session = new_session_id();
        let interaction = InteractionRecord::new(
            session.clone(),
            InteractionId::new("interaction-1").expect("合法 ID"),
            InteractionSource::new("webhook", "chat-a:message-1").expect("合法来源"),
            ScopeRef::workspace("workspace-a").expect("合法作用域"),
            AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace),
            ReplyRoute::new("webhook", "chat-a").expect("合法路由"),
            "检查构建",
            42,
        )
        .expect("合法 Interaction");

        assert_eq!(interaction_session_id(&interaction), session.as_str());
        assert_eq!(
            trust_scope_of(&interaction.audience),
            SessionTrustScope::GroupWorkspace
        );
    }

    #[test]
    fn trust_scope_maps_audience_boundaries() {
        assert_eq!(
            trust_scope_of(&AudienceContext::new(
                AudienceKind::Private,
                DisclosurePolicy::PrivateOnly
            )),
            SessionTrustScope::Private
        );
        assert_eq!(
            trust_scope_of(&AudienceContext::new(
                AudienceKind::Group,
                DisclosurePolicy::Workspace
            )),
            SessionTrustScope::GroupWorkspace
        );
        assert_eq!(
            trust_scope_of(&AudienceContext::new(
                AudienceKind::Private,
                DisclosurePolicy::Workspace
            )),
            SessionTrustScope::Restricted
        );
    }

    #[test]
    fn session_kind_round_trips_through_its_column_value() {
        for kind in [
            SessionKind::Primary,
            SessionKind::Subagent,
            SessionKind::Cron,
            SessionKind::Goal,
        ] {
            assert_eq!(SessionKind::parse(kind.as_str()), Some(kind));
        }
        for scope in [
            SessionTrustScope::Private,
            SessionTrustScope::GroupWorkspace,
            SessionTrustScope::Restricted,
        ] {
            assert_eq!(SessionTrustScope::parse(scope.as_str()), Some(scope));
        }
        assert!(SessionKind::parse("unknown").is_none());
        assert!(SessionTrustScope::parse("unknown").is_none());
        assert_eq!(default_kind_for_interaction(), SessionKind::Primary);
    }
}
