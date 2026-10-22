//! 飞书问答交互卡片构造器（UQ-303, UQ-305）。
//!
//! 严格遵守飞书卡片规范：
//! - 按钮 `value` 序列化字符串 <= 512 字符；
//! - 卡片正文 <= 8000 字符；
//! - 序列化载荷 <= 28 KB。
//!
//! 超出正文/载荷预算的提问按问题分片为多张卡片；单个选项的按钮值超限，或单个问题
//! 独占一张卡片仍超预算时，返回可读回执由调用方降级处理，绝不静默截断或置空。

use axon_domain::{UserAnswerItem, UserQuestionItem};
use serde_json::{Value, json};

/// 飞书卡片按钮 value 的最大长度限制。
pub const MAX_FEISHU_CARD_ACTION_VALUE_CHARS: usize = 512;
/// 飞书卡片正文的最大字符限制。
pub const MAX_FEISHU_CARD_CONTENT_CHARS: usize = 8000;
/// 飞书卡片序列化载荷的字节上限。
pub const MAX_FEISHU_CARD_PAYLOAD_BYTES: usize = 28 * 1024;
/// 单次提问允许拆分的卡片数上限。
///
/// 分片是预算降级手段而非投递策略：超过该上界说明提问本身异常，此时拒绝并要求模型
/// 精简，而不是在群里刷出几十条卡片。
pub const MAX_FEISHU_QUESTION_CARD_SHARDS: usize = 8;

/// 构造飞书待决提问卡片的分片计划。
///
/// 返回值按顺序对应将要发出的多张卡片；调用方负责按序投递并记录
/// `call_id -> message_id` 映射。
///
/// # Errors
///
/// 提问为空、单个选项按钮值超限，或单个问题独占一张卡片仍超出正文/载荷预算时，
/// 返回中文可读原因。
pub fn plan_feishu_question_cards(
    call_id: &str,
    questions: &[UserQuestionItem],
    handoff: Option<&str>,
) -> Result<Vec<Value>, String> {
    if call_id.trim().is_empty() {
        return Err("问答卡片缺少待决问题标识".to_owned());
    }
    if questions.is_empty() {
        return Err("提问列表为空，无法构造飞书问答卡片".to_owned());
    }
    for question in questions {
        validate_headless_question(call_id, question)?;
    }

    let mut shards: Vec<Vec<&UserQuestionItem>> = Vec::new();
    let mut current: Vec<&UserQuestionItem> = Vec::new();
    for question in questions {
        let mut candidate = current.clone();
        candidate.push(question);
        // 分片预算按「最终卡片」实测：正文与实际序列化载荷都必须落在飞书硬限内。
        if fits_card_budget(call_id, &candidate, handoff) {
            current = candidate;
            continue;
        }
        if current.is_empty() {
            // 分片是最后手段：单个问题独占一张卡片仍超预算时只能拒绝，绝不静默截断。
            return Err(format!(
                "问题 {} 超出单张飞书卡片预算（正文 {MAX_FEISHU_CARD_CONTENT_CHARS} 字符 / 载荷 {MAX_FEISHU_CARD_PAYLOAD_BYTES} 字节），请精简后重试",
                question.id
            ));
        }
        shards.push(std::mem::take(&mut current));
        current.push(question);
        if !fits_card_budget(call_id, &current, handoff) {
            return Err(format!(
                "问题 {} 超出单张飞书卡片预算（正文 {MAX_FEISHU_CARD_CONTENT_CHARS} 字符 / 载荷 {MAX_FEISHU_CARD_PAYLOAD_BYTES} 字节），请精简后重试",
                question.id
            ));
        }
    }
    if !current.is_empty() {
        shards.push(current);
    }
    if shards.len() > MAX_FEISHU_QUESTION_CARD_SHARDS {
        return Err(format!(
            "提问需要拆分为 {} 张飞书卡片，超过上限 {MAX_FEISHU_QUESTION_CARD_SHARDS} 张，请减少问题数量后重试",
            shards.len()
        ));
    }

    let total = shards.len();
    let numbering = (questions.len() > 1).then_some(0usize);
    let mut global_start = 0usize;
    let mut cards = Vec::with_capacity(total);
    for (index, shard) in shards.into_iter().enumerate() {
        let handoff = (index == 0).then_some(handoff).flatten();
        cards.push(build_shard_card(
            call_id,
            &shard,
            handoff,
            index,
            total,
            numbering.map(|start| start + global_start),
        ));
        global_start += shard.len();
    }
    Ok(cards)
}

/// 构造已结算的静态飞书卡片（用于 PATCH 回写）。
///
/// 回写卡片不再包含按钮：交互在结算后必须失效，重复点击只能得到 ALREADY_SETTLED。
pub fn build_settled_feishu_question_card(
    call_id: &str,
    operator_name: &str,
    answers: &[UserAnswerItem],
) -> Value {
    let mut answer_desc = Vec::new();
    for ans in answers {
        let text = ans.custom.clone().unwrap_or_else(|| {
            if ans.selected.is_empty() {
                "（已跳过）".to_string()
            } else {
                ans.selected.join("、")
            }
        });
        answer_desc.push(format!("**{}**：{}", ans.id, text));
    }
    let summary = if answer_desc.is_empty() {
        "（已作答）".to_string()
    } else {
        answer_desc.join("\n")
    };

    json!({
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": "✓ 问答已完成结算"
            },
            "template": "grey"
        },
        "elements": [
            {
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": format!("已由 **{operator_name}** 提交确认。\n\n{summary}")
                }
            },
            {
                "tag": "note",
                "elements": [
                    {
                        "tag": "plain_text",
                        "content": format!("提问标识: {call_id} · 本卡片交互已失效")
                    }
                ]
            }
        ]
    })
}

/// 构造单个分片的待决卡片。
///
/// `numbering` 为多题场景下本分片首题的全局序号起点；单题卡片不需要序号。
fn build_shard_card(
    call_id: &str,
    questions: &[&UserQuestionItem],
    handoff: Option<&str>,
    index: usize,
    total: usize,
    numbering: Option<usize>,
) -> Value {
    let mut elements: Vec<Value> = Vec::new();

    if let Some(h) = handoff
        && !h.trim().is_empty()
    {
        elements.push(json!({
            "tag": "div",
            "text": {
                "tag": "lark_md",
                "content": format!("**提示**：{h}")
            }
        }));
        elements.push(json!({ "tag": "hr" }));
    }

    for (offset, q) in questions.iter().enumerate() {
        let prefix =
            numbering.map_or_else(String::new, |start| format!("{}. ", start + offset + 1));

        let mut md_content = format!("**{prefix}{}**", q.question);
        if let Some(detail) = &q.detail
            && !detail.trim().is_empty()
        {
            md_content.push_str(&format!("\n*说明*：{detail}"));
        }

        elements.push(json!({
            "tag": "div",
            "text": {
                "tag": "lark_md",
                "content": md_content
            }
        }));

        let mut actions: Vec<Value> = Vec::new();
        for opt in &q.options {
            actions.push(json!({
                "tag": "button",
                "text": {
                    "tag": "plain_text",
                    "content": opt.label
                },
                "type": "primary",
                "value": answer_value(call_id, q, &opt.label, false)
            }));
        }
        actions.push(json!({
            "tag": "button",
            "text": {
                "tag": "plain_text",
                "content": "跳过"
            },
            "type": "default",
            "value": answer_value(call_id, q, "", true)
        }));

        elements.push(json!({
            "tag": "action",
            "actions": actions
        }));

        if offset + 1 < questions.len() {
            elements.push(json!({ "tag": "hr" }));
        }
    }

    let title = if total > 1 {
        format!("❓ 模型需要您的决策确认（{}/{total}）", index + 1)
    } else {
        "❓ 模型需要您的决策确认".to_owned()
    };
    json!({
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": title
            },
            "template": "blue"
        },
        "elements": elements
    })
}

/// 单个按钮的 value 载荷。
fn answer_value(call_id: &str, question: &UserQuestionItem, label: &str, skip: bool) -> Value {
    json!({
        "axon_action": "answer_question",
        "call_id": call_id,
        "qid": question.id,
        "opt": label,
        "skip": skip
    })
}

/// 校验与卡片正文无关的硬约束：按钮 value 长度。
fn validate_headless_question(call_id: &str, question: &UserQuestionItem) -> Result<(), String> {
    if question.id.trim().is_empty() {
        return Err("提问缺少问题 ID，无法构造飞书问答卡片".to_owned());
    }
    for label in question.options.iter().map(|option| option.label.as_str()) {
        if label.trim().is_empty() {
            return Err(format!(
                "问题 {} 存在空选项标签，无法构造飞书按钮",
                question.id
            ));
        }
        validate_value_len(
            &answer_value(call_id, question, label, false),
            question,
            label,
        )?;
    }
    validate_value_len(&answer_value(call_id, question, "", true), question, "跳过")?;
    Ok(())
}

fn validate_value_len(
    value: &Value,
    question: &UserQuestionItem,
    label: &str,
) -> Result<(), String> {
    let serialized = serde_json::to_string(value)
        .map_err(|_| format!("问题 {} 的按钮载荷序列化失败", question.id))?;
    let chars = serialized.chars().count();
    if chars > MAX_FEISHU_CARD_ACTION_VALUE_CHARS {
        return Err(format!(
            "问题 {} 的选项「{label}」按钮载荷 {chars} 字符，超过飞书 {MAX_FEISHU_CARD_ACTION_VALUE_CHARS} 字符硬限，请精简选项标签或问题 ID",
            question.id
        ));
    }
    Ok(())
}

/// 实测候选分片是否同时满足正文字符预算与载荷字节预算。
fn fits_card_budget(call_id: &str, questions: &[&UserQuestionItem], handoff: Option<&str>) -> bool {
    let card = build_shard_card(call_id, questions, handoff, 0, 1, Some(0));
    if card_content_chars(&card) > MAX_FEISHU_CARD_CONTENT_CHARS {
        return false;
    }
    serde_json::to_string(&card).is_ok_and(|json| json.len() <= MAX_FEISHU_CARD_PAYLOAD_BYTES)
}

/// 统计卡片可见正文字符数：标题、div 文本与 note 文本。
fn card_content_chars(card: &Value) -> usize {
    let mut total = 0usize;
    if let Some(title) = card["header"]["title"]["content"].as_str() {
        total += title.chars().count();
    }
    if let Some(elements) = card["elements"].as_array() {
        for element in elements {
            if let Some(content) = element["text"]["content"].as_str() {
                total += content.chars().count();
            }
            if let Some(text) = element["content"].as_str() {
                total += text.chars().count();
            }
            if let Some(actions) = element["actions"].as_array() {
                for button in actions {
                    if let Some(label) = button["text"]["content"].as_str() {
                        total += label.chars().count();
                    }
                }
            }
            if let Some(note_elements) = element["elements"].as_array() {
                for note in note_elements {
                    if let Some(content) = note["content"].as_str() {
                        total += content.chars().count();
                    }
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::UserQuestionOption;

    fn question(id: &str, text: &str, options: &[&str]) -> UserQuestionItem {
        UserQuestionItem {
            id: id.to_string(),
            question: text.to_string(),
            header: None,
            detail: None,
            options: options
                .iter()
                .map(|label| UserQuestionOption {
                    label: (*label).to_string(),
                    description: None,
                })
                .collect(),
            multi_select: false,
        }
    }

    fn button_values(card: &Value) -> Vec<Value> {
        let mut values = Vec::new();
        if let Some(elements) = card["elements"].as_array() {
            for element in elements {
                if let Some(actions) = element["actions"].as_array() {
                    for button in actions {
                        values.push(button["value"].clone());
                    }
                }
            }
        }
        values
    }

    #[test]
    fn question_card_stays_within_value_content_and_payload_budgets() {
        let questions = vec![question(
            "q_mode",
            "请选择发布环境",
            &["生产环境", "预发测试"],
        )];

        let cards = plan_feishu_question_cards("call-10086", &questions, Some("等待选择发布目标"))
            .expect("单题单卡应可构造");

        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        let json_str = serde_json::to_string(card).expect("序列化");
        assert!(json_str.len() <= MAX_FEISHU_CARD_PAYLOAD_BYTES);
        assert!(
            card["header"]["title"]["content"]
                .as_str()
                .unwrap()
                .contains("模型需要您的决策确认")
        );
        for value in button_values(card) {
            let serialized = serde_json::to_string(&value).expect("value");
            assert!(serialized.chars().count() <= MAX_FEISHU_CARD_ACTION_VALUE_CHARS);
            assert_eq!(value["axon_action"], "answer_question");
            assert_eq!(value["call_id"], "call-10086");
        }
    }

    #[test]
    fn oversized_content_is_split_into_multiple_cards() {
        // 每个问题约 1.6 KB 正文，5 个问题必然越过 8000 字符单卡正文预算。
        let long_text = "长".repeat(1_600);
        let questions: Vec<UserQuestionItem> = (0..5)
            .map(|index| question(&format!("q{index}"), &long_text, &["继续", "停止"]))
            .collect();

        let cards = plan_feishu_question_cards("call-split", &questions, None).expect("必须分片");

        assert!(cards.len() > 1, "超出正文预算必须拆分为多张卡片");
        assert!(cards.len() <= MAX_FEISHU_QUESTION_CARD_SHARDS);
        for card in &cards {
            assert!(card_content_chars(card) <= MAX_FEISHU_CARD_CONTENT_CHARS);
            assert!(
                serde_json::to_string(card).expect("序列化").len() <= MAX_FEISHU_CARD_PAYLOAD_BYTES
            );
        }
        // 每个问题都必须出现在某张卡片上，不允许静默丢弃。
        let rendered: String = cards
            .iter()
            .map(|card| serde_json::to_string(card).expect("序列化"))
            .collect();
        for question in &questions {
            assert!(rendered.contains(&question.id));
        }
    }

    #[test]
    fn oversized_button_value_is_rejected_with_readable_reason() {
        let questions = vec![question("q1", "确认？", &[&"选".repeat(600)])];

        let error = plan_feishu_question_cards("call-oversized", &questions, None)
            .expect_err("超限 value 必须被拒绝");

        assert!(error.contains("512"), "可读回执必须点明硬限：{error}");
        assert!(error.contains("q1"));
    }

    #[test]
    fn single_question_over_content_budget_is_rejected_with_readable_reason() {
        let questions = vec![question("q-huge", &"超".repeat(9_000), &["继续"])];

        let error = plan_feishu_question_cards("call-huge", &questions, None)
            .expect_err("单题超预算必须被拒绝");

        assert!(error.contains("q-huge"));
        assert!(error.contains("预算"));
    }

    #[test]
    fn empty_questions_are_rejected() {
        let error =
            plan_feishu_question_cards("call-empty", &[], None).expect_err("空提问必须被拒");
        assert!(error.contains("空"));
    }

    #[test]
    fn settled_card_renders_grey_template_and_disables_actions() {
        let answers = vec![UserAnswerItem {
            id: "q_mode".to_string(),
            selected: vec!["生产环境".to_string()],
            custom: None,
        }];
        let settled_card = build_settled_feishu_question_card("call-10086", "张三", &answers);
        assert_eq!(settled_card["header"]["template"], "grey");
        let content = settled_card["elements"][0]["text"]["content"]
            .as_str()
            .unwrap();
        assert!(content.contains("张三"));
        assert!(content.contains("生产环境"));
        // 结算卡片不得保留任何可点击按钮。
        assert!(button_values(&settled_card).is_empty());
    }
}
