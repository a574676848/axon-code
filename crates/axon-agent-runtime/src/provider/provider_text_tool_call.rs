//! Provider 文本降级工具调用的流式识别与结构化解析。

use axon_domain::ToolCallInput;
use quick_xml::{Reader, XmlVersion, events::Event};

use crate::ProviderToolCall;

const FUNCTION_CALLS_OPEN: &str = "<function_calls>";
const FUNCTION_CALLS_CLOSE: &str = "</function_calls>";
const MARKDOWN_FENCE: &str = "```";
const TEXT_TOOL_CALL_ID_PREFIX: &str = "axon_text_tool_call_";
const TEXT_TOOL_CALL_MAX_BYTES: usize = 256 * 1024;
const TEXT_TOOL_CALL_PREFIX_MAX_BYTES: usize = 256;

#[derive(Debug, Default)]
struct CapturedBlock {
    raw: String,
    current_line: String,
}

#[derive(Debug)]
pub(crate) struct ProviderTextToolCallFilter {
    capture: Option<CapturedBlock>,
    candidate: String,
    current_line: String,
    line_candidate: bool,
    in_code_fence: bool,
    next_call_index: usize,
    tool_calls: Vec<ProviderToolCall>,
}

impl Default for ProviderTextToolCallFilter {
    fn default() -> Self {
        Self {
            capture: None,
            candidate: String::new(),
            current_line: String::new(),
            line_candidate: true,
            in_code_fence: false,
            next_call_index: 0,
            tool_calls: Vec::new(),
        }
    }
}

pub(crate) struct ProviderTextToolCallFinish {
    pub(crate) visible_text: String,
    pub(crate) tool_calls: Vec<ProviderToolCall>,
}

/// 过滤 Provider 以正文形式返回的工具调用，只投影用户可见文本。
#[derive(Debug, Default)]
pub struct ProviderTextToolCallProjection {
    filter: ProviderTextToolCallFilter,
}

impl ProviderTextToolCallProjection {
    /// 消费一个正文增量，返回当前可以安全显示的文本。
    pub fn consume(&mut self, text: &str) -> String {
        self.filter.consume(text)
    }

    /// 结束投影，将无效或未闭合的候选块按普通正文回显。
    pub fn finish(self) -> String {
        self.filter.finish().visible_text
    }
}

impl ProviderTextToolCallFilter {
    pub(crate) fn consume(&mut self, text: &str) -> String {
        let mut visible = String::new();
        for character in text.chars() {
            if self.capture.is_some() {
                self.consume_captured(character, &mut visible);
            } else {
                self.consume_visible(character, &mut visible);
            }
        }
        visible
    }

    pub(crate) fn finish(mut self) -> ProviderTextToolCallFinish {
        let mut visible_text = String::new();
        if let Some(capture) = self.capture.take() {
            self.complete_capture(capture, &mut visible_text);
        } else {
            visible_text.push_str(&self.candidate);
        }
        ProviderTextToolCallFinish {
            visible_text,
            tool_calls: self.tool_calls,
        }
    }

    fn consume_visible(&mut self, character: char, visible: &mut String) {
        self.current_line.push(character);
        if !self.line_candidate {
            visible.push(character);
            if character == '\n' {
                self.complete_visible_line();
            }
            return;
        }

        self.candidate.push(character);
        if character == '\n' {
            if !self.in_code_fence && is_opening_line(&self.candidate) {
                self.capture = Some(CapturedBlock {
                    raw: std::mem::take(&mut self.candidate),
                    current_line: String::new(),
                });
                self.current_line.clear();
            } else {
                visible.push_str(&self.candidate);
                self.candidate.clear();
                self.complete_visible_line();
            }
            return;
        }

        if self.candidate.len() > TEXT_TOOL_CALL_PREFIX_MAX_BYTES
            || !is_possible_opening_line(&self.candidate)
        {
            visible.push_str(&self.candidate);
            self.candidate.clear();
            self.line_candidate = false;
        }
    }

    fn consume_captured(&mut self, character: char, visible: &mut String) {
        let capture = self.capture.as_mut().expect("capture 已检查");
        capture.raw.push(character);
        capture.current_line.push(character);
        let completed = character == '\n' && is_closing_line(&capture.current_line);
        let overflow = capture.raw.len() > TEXT_TOOL_CALL_MAX_BYTES;
        if completed || overflow {
            let capture = self.capture.take().expect("capture 已存在");
            self.complete_capture(capture, visible);
        } else if character == '\n' {
            capture.current_line.clear();
        }
    }

    fn complete_capture(&mut self, capture: CapturedBlock, visible: &mut String) {
        if is_closing_line(&capture.current_line)
            && let Some(calls) = parse_textual_tool_calls(&capture.raw)
        {
            for call in calls {
                let id = format!("{TEXT_TOOL_CALL_ID_PREFIX}{}", self.next_call_index);
                self.next_call_index = self.next_call_index.saturating_add(1);
                self.tool_calls.push(ProviderToolCall::new(
                    id,
                    call.name,
                    ToolCallInput::json(call.input),
                ));
            }
            self.reset_line();
            return;
        }
        visible.push_str(&capture.raw);
        self.restore_visible_state(&capture.raw);
    }

    fn complete_visible_line(&mut self) {
        if self.current_line.trim_start().starts_with(MARKDOWN_FENCE) {
            self.in_code_fence = !self.in_code_fence;
        }
        self.reset_line();
    }

    fn restore_visible_state(&mut self, text: &str) {
        self.reset_line();
        for line in text.split_inclusive('\n') {
            if line.ends_with('\n') {
                if line.trim_start().starts_with(MARKDOWN_FENCE) {
                    self.in_code_fence = !self.in_code_fence;
                }
            } else {
                self.current_line.push_str(line);
                self.line_candidate = false;
            }
        }
    }

    fn reset_line(&mut self) {
        self.candidate.clear();
        self.current_line.clear();
        self.line_candidate = true;
    }
}

#[derive(Debug)]
struct TextualToolCall {
    name: String,
    input: serde_json::Value,
}

#[derive(Debug, Default)]
struct TextualToolCallBuilder {
    name: String,
    arguments: String,
    arguments_seen: bool,
}

fn is_opening_line(line: &str) -> bool {
    line.trim() == FUNCTION_CALLS_OPEN
}

fn is_closing_line(line: &str) -> bool {
    line.trim() == FUNCTION_CALLS_CLOSE
}

fn is_possible_opening_line(line: &str) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if trimmed.len() <= FUNCTION_CALLS_OPEN.len() {
        return FUNCTION_CALLS_OPEN.starts_with(trimmed);
    }
    trimmed
        .strip_prefix(FUNCTION_CALLS_OPEN)
        .is_some_and(|suffix| {
            suffix
                .chars()
                .all(|character| matches!(character, ' ' | '\t' | '\r'))
        })
}

fn parse_textual_tool_calls(raw: &str) -> Option<Vec<TextualToolCall>> {
    let mut reader = Reader::from_str(raw);
    reader.config_mut().trim_text(false);
    let mut root_open = false;
    let mut root_closed = false;
    let mut current = None::<TextualToolCallBuilder>;
    let mut in_arguments = false;
    let mut calls = Vec::new();

    loop {
        match reader.read_event().ok()? {
            Event::Start(event) if event.local_name().as_ref() == "function_calls" => {
                if root_open || root_closed || current.is_some() {
                    return None;
                }
                root_open = true;
            }
            Event::Start(event) if event.local_name().as_ref() == "function" => {
                if !root_open || root_closed || current.is_some() {
                    return None;
                }
                current = Some(TextualToolCallBuilder {
                    name: function_name(&reader, &event)?,
                    ..TextualToolCallBuilder::default()
                });
            }
            Event::Start(event) if event.local_name().as_ref() == "args_json" => {
                let call = current.as_mut()?;
                if in_arguments || call.arguments_seen {
                    return None;
                }
                call.arguments_seen = true;
                in_arguments = true;
            }
            Event::Start(_) | Event::Empty(_) => return None,
            Event::Text(text) => {
                let text = text.xml_content(XmlVersion::Implicit1_0);
                append_xml_text(&mut current, in_arguments, &text)?;
            }
            Event::CData(text) => {
                let text = text.xml_content(XmlVersion::Implicit1_0);
                append_xml_text(&mut current, in_arguments, &text)?;
            }
            Event::GeneralRef(reference) => {
                let text = decode_xml_reference(&reference)?;
                append_xml_text(&mut current, in_arguments, &text)?;
            }
            Event::End(event) if event.local_name().as_ref() == "args_json" => {
                if !in_arguments {
                    return None;
                }
                in_arguments = false;
            }
            Event::End(event) if event.local_name().as_ref() == "function" => {
                if in_arguments {
                    return None;
                }
                let call = current.take()?;
                let input = serde_json::from_str(call.arguments.trim()).ok()?;
                calls.push(TextualToolCall {
                    name: call.name,
                    input,
                });
            }
            Event::End(event) if event.local_name().as_ref() == "function_calls" => {
                if !root_open || current.is_some() || in_arguments {
                    return None;
                }
                root_open = false;
                root_closed = true;
            }
            Event::End(_) => return None,
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
            _ => return None,
        }
    }

    (root_closed && !root_open && !calls.is_empty()).then_some(calls)
}

fn function_name(
    _reader: &Reader<&[u8]>,
    event: &quick_xml::events::BytesStart<'_>,
) -> Option<String> {
    let mut name = None;
    for attribute in event.attributes() {
        let attribute = attribute.ok()?;
        if attribute.key.local_name().as_ref() != "name" || name.is_some() {
            return None;
        }
        name = Some(
            attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .ok()?
                .into_owned(),
        );
    }
    name.filter(|value| !value.trim().is_empty())
}

fn append_xml_text(
    current: &mut Option<TextualToolCallBuilder>,
    in_arguments: bool,
    text: &str,
) -> Option<()> {
    if in_arguments {
        current.as_mut()?.arguments.push_str(text);
    } else if !text.trim().is_empty() {
        return None;
    }
    Some(())
}

fn decode_xml_reference(reference: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    if let Some(character) = reference.resolve_char_ref().ok()? {
        return Some(character.to_string());
    }
    let name = reference.xml_content(XmlVersion::Implicit1_0);
    quick_xml::escape::resolve_predefined_entity(&name).map(ToOwned::to_owned)
}
