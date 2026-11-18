//! 会话身份与标识生成。
//!
//! 会话标识是服务端生成的 ULID：与渠道、投递目标、受众和工作区都无关，生成一次、此后
//! 不变。渠道属于轮次事实（`origin_channel` 与会话级分布投影），不参与会话身份，也不
//! 能由投递目标反解出来。
//!
//! 生成点集中在本文一处：任何模块都不得自行拼接或用哈希派生会话标识。

use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};

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
/// Crockford Base32 字符集：去掉 `I`、`L`、`O`、`U`。
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 上一次生成使用的毫秒时间戳。
static LAST_TIMESTAMP_MS: AtomicU64 = AtomicU64::new(0);
/// 随机部分高 16 位；同毫秒内进位时递增。
static RANDOM_HIGH: AtomicU16 = AtomicU16::new(0);
/// 随机部分低 64 位；同毫秒内每次生成递增，保证整体字典序单调。
static RANDOM_LOW: AtomicU64 = AtomicU64::new(0);

/// 生成一个新的 ULID 文本。
///
/// 前 48 位是毫秒时间戳，后 80 位是随机量；同一毫秒内的重复生成会递增随机部分，因此
/// 返回值按字典序单调递增。全过程只用原子操作，不抢全局锁，也不在循环里反复读时钟。
#[must_use]
pub fn new_ulid() -> String {
    let now_ms = axon_time::unix_now_ms();
    claim_random(now_ms);
    let random = (u128::from(RANDOM_HIGH.load(Ordering::Acquire)) << 64)
        | u128::from(RANDOM_LOW.load(Ordering::Acquire));
    encode(now_ms, random & ((1_u128 << RANDOM_BITS) - 1))
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

/// 认领一次生成：新毫秒重新随机，同一毫秒递增随机部分。
fn claim_random(now_ms: u64) {
    loop {
        let last = LAST_TIMESTAMP_MS.load(Ordering::Acquire);
        if now_ms > last {
            if LAST_TIMESTAMP_MS
                .compare_exchange(last, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let (high, low) = random_80_bits();
                RANDOM_HIGH.store(high, Ordering::Release);
                RANDOM_LOW.store(low, Ordering::Release);
                return;
            }
            continue;
        }
        let previous = RANDOM_LOW.fetch_add(1, Ordering::AcqRel);
        if previous == u64::MAX {
            RANDOM_HIGH.fetch_add(1, Ordering::AcqRel);
        }
        return;
    }
}

/// 取 80 位随机量：没有独立随机源时用 UUIDv4 的随机位，来源同一熵池。
fn random_80_bits() -> (u16, u64) {
    let raw = uuid::Uuid::new_v4().as_u128();
    let high = (raw >> 64) as u16;
    let low = raw as u64;
    (high, low)
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
