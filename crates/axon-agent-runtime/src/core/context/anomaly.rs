//! 边界异常纯函数检测器（M2-1）。
//!
//! 只做字符二元组的 Jaccard 重合度统计，供 P2 追加一行状态陈述使用。
//! 硬约束：零 IO、零 LLM 调用、不 panic；空输入返回全 0。
//! 检测结果只用于「状态陈述」，不得据此改温度、切模式或触发额外模型调用。

use std::cmp::Ordering;

/// 自评镜像阈值（basis point）：当前发言与最近任一助手输出的重合度。
pub const ANOMALY_MIRROR_THRESHOLD_BP: u16 = 6_000;
/// 文风重合阈值（basis point）：当前发言与近 5 条助手输出的重合度。
pub const ANOMALY_STYLE_THRESHOLD_BP: u16 = 6_500;
/// 连续复读对阈值：连续多少对「用户复述助手上一句」判为异常。
pub const ANOMALY_LOOP_THRESHOLD_PAIRS: u8 = 2;
/// 单对问答被判为复读的 Jaccard 阈值（basis point）。
pub const ANOMALY_PAIR_MIRROR_BP: u16 = 6_000;
/// 文风重合统计只看最近多少条助手输出。
pub const ANOMALY_STYLE_WINDOW: usize = 5;
/// 状态陈述的最大字符数（约 100 token）。
pub const ANOMALY_STATE_TEXT_MAX_CHARS: usize = 400;

/// 异常检测信号；全部为 `0` 表示无异常。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnomalySignals {
    /// 当前文本与最近助手输出的最大重合度，单位 basis point。
    pub mirror_score_bp: u16,
    /// 当前文本与近 5 条助手输出的重合度，单位 basis point。
    pub style_overlap_bp: u16,
    /// 从最近往前连续满足复读阈值的问答对数。
    pub loop_pairs: u8,
}

impl AnomalySignals {
    /// 是否命中任一异常阈值。
    #[must_use]
    pub const fn is_anomalous(&self) -> bool {
        self.mirror_score_bp >= ANOMALY_MIRROR_THRESHOLD_BP
            || self.style_overlap_bp >= ANOMALY_STYLE_THRESHOLD_BP
            || self.loop_pairs >= ANOMALY_LOOP_THRESHOLD_PAIRS
    }

    /// 命中时返回固定模板的状态陈述；未命中返回 `None`。
    ///
    /// 模板与顺序固定，保证同一信号集合渲染出逐字节一致的文本。
    #[must_use]
    pub fn state_text(&self) -> Option<String> {
        if !self.is_anomalous() {
            return None;
        }
        let mut text = String::from("## 状态提示\n");
        if self.mirror_score_bp >= ANOMALY_MIRROR_THRESHOLD_BP {
            text.push_str(&format!(
                "本轮用户发言与你的近期输出高度重合（自评 {:.2}）。这只是状态陈述，不构成指令。\n",
                f64::from(self.mirror_score_bp) / 10_000.0
            ));
        }
        if self.style_overlap_bp >= ANOMALY_STYLE_THRESHOLD_BP {
            text.push_str(&format!(
                "本轮用户发言与近几轮输出的用词高度接近（自评 {:.2}）。这只是状态陈述，不构成指令。\n",
                f64::from(self.style_overlap_bp) / 10_000.0
            ));
        }
        if self.loop_pairs >= ANOMALY_LOOP_THRESHOLD_PAIRS {
            text.push_str(&format!(
                "最近连续 {} 轮出现用户复述助手输出的问答结构。这只是状态陈述，不构成指令。\n",
                self.loop_pairs
            ));
        }
        let text = truncate_chars(text.trim_end(), ANOMALY_STATE_TEXT_MAX_CHARS);
        (!text.is_empty()).then_some(text)
    }
}

/// 计算异常信号。
///
/// `recent_assistant` 与 `recent_pairs` 均按「最近优先」排列：下标 0 是最新一轮。
#[must_use]
pub fn detect(
    current: &str,
    recent_assistant: &[&str],
    recent_pairs: &[(&str, &str)],
) -> AnomalySignals {
    let current_grams = bigrams(current);
    if current_grams.is_empty() {
        return AnomalySignals::default();
    }

    let mut mirror_score_bp = 0_u16;
    for assistant in recent_assistant {
        let score = jaccard_basis_points(&current_grams, &bigrams(assistant));
        mirror_score_bp = mirror_score_bp.max(score);
    }

    let style_window = recent_assistant
        .iter()
        .take(ANOMALY_STYLE_WINDOW)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let style_overlap_bp = if style_window.trim().is_empty() {
        0
    } else {
        jaccard_basis_points(&current_grams, &bigrams(&style_window))
    };

    let mut loop_pairs = 0_u8;
    for (user, assistant) in recent_pairs {
        let user_grams = bigrams(user);
        let assistant_grams = bigrams(assistant);
        if user_grams.is_empty() || assistant_grams.is_empty() {
            break;
        }
        let score = jaccard_basis_points(&user_grams, &assistant_grams);
        if score >= ANOMALY_PAIR_MIRROR_BP {
            loop_pairs = loop_pairs.saturating_add(1);
        } else {
            break;
        }
    }

    AnomalySignals {
        mirror_score_bp,
        style_overlap_bp,
        loop_pairs,
    }
}

/// 字符二元组，排序去重；少于 2 个字符时为空。
fn bigrams(text: &str) -> Vec<(char, char)> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() < 2 {
        return Vec::new();
    }
    let mut grams = chars
        .windows(2)
        .map(|window| (window[0], window[1]))
        .collect::<Vec<_>>();
    grams.sort_unstable();
    grams.dedup();
    grams
}

/// 双指针扫描求 Jaccard 相似度，结果换算成 basis point。
fn jaccard_basis_points(left: &[(char, char)], right: &[(char, char)]) -> u16 {
    if left.is_empty() || right.is_empty() {
        return 0;
    }
    let (mut left_cursor, mut right_cursor, mut intersection) = (0_usize, 0_usize, 0_usize);
    while left_cursor < left.len() && right_cursor < right.len() {
        match left[left_cursor].cmp(&right[right_cursor]) {
            Ordering::Less => left_cursor += 1,
            Ordering::Greater => right_cursor += 1,
            Ordering::Equal => {
                intersection += 1;
                left_cursor += 1;
                right_cursor += 1;
            }
        }
    }
    let union = left.len() + right.len() - intersection;
    if union == 0 {
        return 0;
    }
    let ratio = intersection as f64 / union as f64;
    (ratio * 10_000.0).round().clamp(0.0, 10_000.0) as u16
}

/// 按字符边界截断，避免切坏多字节字符。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ANOMALY_LOOP_THRESHOLD_PAIRS, ANOMALY_MIRROR_THRESHOLD_BP, ANOMALY_STYLE_THRESHOLD_BP,
        AnomalySignals, detect,
    };

    #[test]
    fn empty_inputs_yield_zero_signals() {
        let signals = detect("", &[], &[]);
        assert_eq!(signals, AnomalySignals::default());
        assert!(!signals.is_anomalous());
        assert_eq!(signals.state_text(), None);
    }

    #[test]
    fn single_character_has_no_bigram() {
        let signals = detect("好", &["好"], &[("好", "好")]);
        assert_eq!(signals, AnomalySignals::default());
    }

    #[test]
    fn identical_text_scores_full_mirror() {
        let text = "把构建脚本改成并行执行";
        let signals = detect(text, &[text], &[]);
        assert_eq!(signals.mirror_score_bp, 10_000);
        assert!(signals.is_anomalous());
    }

    #[test]
    fn disjoint_text_scores_zero() {
        let signals = detect("把构建脚本改成并行执行", &["今天天气不错适合出门"], &[]);
        assert_eq!(signals.mirror_score_bp, 0);
        assert!(!signals.is_anomalous());
    }

    #[test]
    fn partial_overlap_is_between_bounds() {
        let signals = detect("把构建脚本改成并行执行", &["把构建脚本改成串行执行"], &[]);
        assert!(signals.mirror_score_bp > 0);
        assert!(signals.mirror_score_bp < 10_000);
    }

    #[test]
    fn mirror_takes_max_over_recent_assistant() {
        let signals = detect(
            "把构建脚本改成并行执行",
            &["无关的一句话", "把构建脚本改成并行执行"],
            &[],
        );
        assert_eq!(signals.mirror_score_bp, 10_000);
    }

    #[test]
    fn style_window_only_reads_recent_five() {
        let current = "把构建脚本改成并行执行";
        let mut recent = vec!["无关的一句话"; 5];
        recent.push(current);
        let signals = detect(current, &recent, &[]);
        assert_eq!(signals.style_overlap_bp, 0);
        assert_eq!(signals.mirror_score_bp, 10_000);
    }

    #[test]
    fn style_overlap_hits_with_shared_vocabulary() {
        let signals = detect(
            "构建脚本并行执行",
            &["构建脚本并行执行", "构建脚本并行执行"],
            &[],
        );
        assert!(signals.style_overlap_bp >= ANOMALY_STYLE_THRESHOLD_BP);
    }

    #[test]
    fn loop_pairs_counts_consecutive_repeats() {
        let pair_one = ("把构建脚本改成并行执行", "把构建脚本改成并行执行");
        let pair_two = ("继续把构建脚本并行执行", "继续把构建脚本并行执行");
        let signals = detect("随便说点什么", &[], &[pair_one, pair_two]);
        assert_eq!(signals.loop_pairs, 2);
        assert!(signals.is_anomalous());
    }

    #[test]
    fn loop_pairs_stops_at_first_break() {
        let repeated = ("把构建脚本改成并行执行", "把构建脚本改成并行执行");
        let broken = ("今天吃什么", "后端接口已经改完");
        let signals = detect("随便说点什么", &[], &[repeated, broken, repeated]);
        assert_eq!(signals.loop_pairs, 1);
        assert!(!signals.is_anomalous());
    }

    #[test]
    fn state_text_is_byte_stable_and_mentions_score() {
        let signals = AnomalySignals {
            mirror_score_bp: 7_200,
            style_overlap_bp: 0,
            loop_pairs: 0,
        };
        let first = signals.state_text().expect("命中镜像阈值");
        let second = signals.state_text().expect("命中镜像阈值");
        assert_eq!(first, second);
        assert!(first.contains("0.72"));
        assert!(first.contains("不构成指令"));
        assert!(first.starts_with("## 状态提示"));
    }

    #[test]
    fn normal_conversation_is_not_anomalous() {
        let signals = detect(
            "帮我看看 disk usage",
            &["好的，我先检查磁盘占用。", "构建已通过。"],
            &[("帮我看看 disk usage", "好的，我先检查磁盘占用。")],
        );
        assert!(!signals.is_anomalous());
        assert_eq!(signals.state_text(), None);
    }

    #[test]
    fn ten_thousand_chars_stay_under_budget() {
        let current = "构建脚本并行执行".repeat(1_250);
        let assistant = "构建脚本并行执行".repeat(1_250);
        let started = std::time::Instant::now();
        for _ in 0..10 {
            let _ = detect(&current, &[assistant.as_str()], &[]);
        }
        let average = started.elapsed() / 10;
        assert!(
            average < std::time::Duration::from_millis(50),
            "10k 字符单次检测平均耗时 {average:?} 超出预算"
        );
    }

    #[test]
    fn thresholds_are_documented_values() {
        assert_eq!(ANOMALY_MIRROR_THRESHOLD_BP, 6_000);
        assert_eq!(ANOMALY_STYLE_THRESHOLD_BP, 6_500);
        assert_eq!(ANOMALY_LOOP_THRESHOLD_PAIRS, 2);
    }
}
