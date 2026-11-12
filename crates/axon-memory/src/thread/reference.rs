//! 指代与一次性输入判定（M3-2 纯函数部分）。
//!
//! 只做词表匹配，不调用模型、不做 IO：
//! * 命中指代词 → `AnaphoraRecent`（沿用当前前台线索，不新建）；
//! * 命中时间/版本/文档等精确锚点 + 指代词 → `PreciseCallback(锚点词)`；
//! * 一次性信息需求（天气/汇率/几点/寒暄）→ `is_one_off_leaf = true`，不建线索。

/// 指代最近上下文的标记词。
const ANAPHORA_MARKERS: [&str; 10] = [
    "这个", "那个", "它", "继续", "接着", "上面", "刚才", "然后呢", "再来", "同上",
];

/// 精确回调锚点词。
const ANCHOR_MARKERS: [&str; 10] = [
    "上周", "昨天", "前天", "大前天", "本月", "上月", "第", "版", "方案", "文档",
];

/// 一次性信息需求的标记词。
const ONE_OFF_MARKERS: [&str; 12] = [
    "天气",
    "气温",
    "汇率",
    "几点",
    "当前时间",
    "你好",
    "您好",
    "谢谢",
    "在吗",
    "早安",
    "晚安",
    "再见",
];

/// 一次性判定的最大文本长度；更长的句子即使含标记词也按真实话题处理。
const ONE_OFF_MAX_CHARS: usize = 20;

/// 用户发言对上下文的指代类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// 泛指最近上下文（这个 / 它 / 继续）。
    AnaphoraRecent,
    /// 带精确锚点的回调（上周三那个方案），携带命中的锚点词。
    PreciseCallback(Vec<String>),
    /// 无指代（新话题）。
    None,
}

/// 判定指代类型。
#[must_use]
pub fn classify_reference(text: &str) -> Reference {
    let text = text.trim();
    if text.is_empty() {
        return Reference::None;
    }
    let has_anaphora = ANAPHORA_MARKERS
        .iter()
        .any(|marker| text.contains(marker));
    let anchors = ANCHOR_MARKERS
        .iter()
        .filter(|marker| text.contains(**marker))
        .map(|marker| (*marker).to_owned())
        .collect::<Vec<_>>();
    if has_anaphora && !anchors.is_empty() {
        return Reference::PreciseCallback(anchors);
    }
    if has_anaphora {
        return Reference::AnaphoraRecent;
    }
    Reference::None
}

/// 是否为一次性信息需求（天气 / 汇率 / 几点 / 寒暄）。
///
/// 只在短句上生效：长句里的「天气」更可能是真实工程话题（例如接天气接口）。
#[must_use]
pub fn is_one_off_leaf(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > ONE_OFF_MAX_CHARS {
        return false;
    }
    ONE_OFF_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::{Reference, classify_reference, is_one_off_leaf};

    #[test]
    fn bare_anaphora_is_recent_reference() {
        assert_eq!(classify_reference("这个"), Reference::AnaphoraRecent);
        assert_eq!(classify_reference("继续"), Reference::AnaphoraRecent);
        assert_eq!(classify_reference("它呢"), Reference::AnaphoraRecent);
    }

    #[test]
    fn anchored_reference_is_precise_callback() {
        match classify_reference("上周三那个方案") {
            Reference::PreciseCallback(anchors) => {
                assert!(anchors.iter().any(|anchor| anchor == "上周"));
                assert!(anchors.iter().any(|anchor| anchor == "方案"));
            }
            other => panic!("应命中精确回调，实际 {other:?}"),
        }
    }

    #[test]
    fn plain_new_topic_has_no_reference() {
        assert_eq!(classify_reference("帮我看看构建脚本"), Reference::None);
        assert_eq!(classify_reference(""), Reference::None);
    }

    #[test]
    fn anchor_without_anaphora_is_not_a_reference() {
        assert_eq!(classify_reference("上周的部署日志在哪"), Reference::None);
    }

    #[test]
    fn weather_and_greetings_are_one_off() {
        for text in ["今天天气怎么样", "现在几点了", "你好", "谢谢"] {
            assert!(is_one_off_leaf(text), "{text} 应按一次性处理");
        }
    }

    #[test]
    fn long_engineering_sentence_with_keyword_is_not_one_off() {
        assert!(!is_one_off_leaf(
            "帮我把天气接口的鉴权改成从配置读取并补上重试"
        ));
    }

    #[test]
    fn empty_text_is_not_one_off() {
        assert!(!is_one_off_leaf(""));
        assert!(!is_one_off_leaf("   "));
    }
}
