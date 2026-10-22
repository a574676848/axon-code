const STABLE_ID_FNV_OFFSET_BASIS: u64 = 14_695_981_039_346_656_037;
const STABLE_ID_FNV_PRIME: u64 = 1_099_511_628_211;
const MIN_STABLE_ID: u64 = 1;
#[cfg(test)]
const JSON_RPC_ID_MAX: u64 = i64::MAX as u64;

pub(crate) fn stable_numeric_id(value: &str) -> u64 {
    let mut hash = STABLE_ID_FNV_OFFSET_BASIS;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(STABLE_ID_FNV_PRIME);
    }
    hash.max(MIN_STABLE_ID)
}

#[cfg(test)]
pub(crate) fn stable_json_rpc_id(value: &str) -> i64 {
    let bounded = stable_numeric_id(value) % JSON_RPC_ID_MAX;
    i64::try_from(bounded.max(MIN_STABLE_ID)).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_are_non_zero_and_repeatable() {
        assert_eq!(
            stable_numeric_id("session-1"),
            stable_numeric_id("session-1")
        );
        assert_ne!(stable_numeric_id("session-1"), 0);
        assert_eq!(stable_json_rpc_id("call-1"), stable_json_rpc_id("call-1"));
        assert!(stable_json_rpc_id("call-1") > 0);
    }
}
