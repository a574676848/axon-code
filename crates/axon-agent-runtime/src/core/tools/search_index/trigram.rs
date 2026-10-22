use super::model::TrigramKey;

pub(crate) const TRIGRAM_WIDTH: usize = 3;

pub(crate) fn hash(bytes: &[u8; TRIGRAM_WIDTH]) -> TrigramKey {
    TrigramKey((u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]))
}

/// 提取去重且有序的 3 字节窗口 key。
///
/// 使用连续 `Vec` 加原地排序，避免 `BTreeSet` 在大文件上产生逐 key 的树节点分配。
pub(crate) fn extract(bytes: &[u8]) -> Vec<TrigramKey> {
    let mut keys = bytes
        .windows(TRIGRAM_WIDTH)
        .map(|window| hash(&[window[0], window[1], window[2]]))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::{extract, hash};
    use crate::search_index::model::TrigramKey;

    #[test]
    fn extracts_unique_three_byte_windows() {
        assert_eq!(extract(b"ababa"), vec![hash(b"aba"), hash(b"bab")]);
    }

    #[test]
    fn short_input_cannot_produce_a_key() {
        assert!(extract(b"ab").is_empty());
    }

    #[test]
    fn key_is_a_lossless_twenty_four_bit_value() {
        assert_eq!(hash(&[0x01, 0x02, 0x03]), TrigramKey(0x010203));
    }
}
