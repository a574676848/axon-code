//! Provider 工具参数生成期的稳定文件 diff 快照。

use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
};

use axon_agent_runtime::{
    ProviderStreamProtocol, ProviderToolCallPartial, ProviderToolCallStreamRuntime,
    ProviderToolCallWireKind,
};

use crate::tool_orchestrator::{ToolKind, classify_tool, tool_execution_part_id};

use super::{
    run_progress::RunToolDiffSnapshotView,
    run_progress_apply_patch_snapshot::apply_patch_diff_snapshot,
    run_progress_hashline_snapshot::hashline_diff_snapshot,
};

#[derive(Debug)]
pub(super) struct RunProgressStreamingDiffPreview {
    provider_turn_id: u64,
    /// 预览期按当前磁盘内容渲染真实 diff 时使用的会话工作区根。
    workspace_root: std::path::PathBuf,
    tool_calls: ProviderToolCallStreamRuntime,
    edit_previews: BTreeMap<String, EditPreviewCall>,
}

impl RunProgressStreamingDiffPreview {
    pub(super) fn new(workspace_root: std::path::PathBuf, provider_turn_id: u64) -> Self {
        Self {
            provider_turn_id,
            workspace_root,
            tool_calls: ProviderToolCallStreamRuntime::default(),
            edit_previews: BTreeMap::new(),
        }
    }

    pub(super) fn consume_line(
        &mut self,
        protocol: ProviderStreamProtocol,
        line: &str,
    ) -> Vec<RunToolDiffSnapshotView> {
        let Ok(partials) = self.tool_calls.consume_line(protocol, line) else {
            return Vec::new();
        };
        partials
            .into_iter()
            .filter_map(|partial| match classify_tool(&partial.name) {
                ToolKind::Edit => self.edit_preview(&partial),
                _ => None,
            })
            .collect()
    }

    fn edit_preview(
        &mut self,
        partial: &ProviderToolCallPartial,
    ) -> Option<RunToolDiffSnapshotView> {
        // 预览期读盘需要工作区根：先克隆，避免与 edit_previews 的可变借用冲突。
        let workspace_root = self.workspace_root.clone();
        let call = self.edit_previews.entry(partial.id.clone()).or_default();
        let payload = call.apply_partial(partial)?;
        let tool_part_id =
            tool_execution_part_id(self.provider_turn_id, &partial.id, partial.index);
        // PathDiff 载荷需要补齐 `*** End Patch` 才能解析；Hashline 载荷没有结束标记，
        // 直接按当前文件内容渲染真实 diff（陈旧标签、未写完的操作会被跳过）。
        let snapshot = match close_patch_preview(payload) {
            Some(closed_patch) => {
                apply_patch_diff_snapshot(&partial.id, &tool_part_id, &closed_patch)?
            }
            None => hashline_diff_snapshot(&workspace_root, &partial.id, &tool_part_id, payload)?,
        };
        let hash = content_hash(&format!("{:?}", snapshot.changes));
        if call.emitted_hash == Some(hash) {
            return None;
        }
        call.emitted_hash = Some(hash);
        Some(snapshot)
    }
}

#[derive(Debug, Default)]
struct EditPreviewCall {
    decoder: ApplyDiffArgumentsDecoder,
    emitted_hash: Option<u64>,
}

impl EditPreviewCall {
    fn apply_partial<'a>(&'a mut self, partial: &'a ProviderToolCallPartial) -> Option<&'a str> {
        if partial.input_reset {
            self.decoder = ApplyDiffArgumentsDecoder::default();
            self.emitted_hash = None;
        }
        match partial.wire_kind {
            ProviderToolCallWireKind::Custom => Some(partial.input_text.as_str()),
            ProviderToolCallWireKind::Function => {
                self.decoder.consume(&partial.input_delta);
                self.decoder.stable_input()
            }
        }
    }
}

fn close_patch_preview(patch: &str) -> Option<String> {
    if !patch.starts_with("*** Begin Patch") {
        return None;
    }
    if patch.lines().any(|line| line == "*** End Patch") {
        return Some(patch.to_owned());
    }
    let mut closed = patch.to_owned();
    if !closed.ends_with('\n') {
        closed.push('\n');
    }
    closed.push_str("*** End Patch");
    Some(closed)
}

fn content_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum JsonStringTarget {
    Key,
    Input,
    #[default]
    Ignore,
}

#[derive(Debug, Default)]
struct ApplyDiffArgumentsDecoder {
    in_string: bool,
    target: JsonStringTarget,
    escaped: bool,
    unicode_remaining: u8,
    unicode_value: u16,
    pending_high_surrogate: Option<u16>,
    expecting_key: bool,
    expecting_value: bool,
    key_buffer: String,
    current_key: String,
    input: String,
    input_complete: bool,
}

impl ApplyDiffArgumentsDecoder {
    fn consume(&mut self, delta: &str) {
        for character in delta.chars() {
            self.consume_character(character);
        }
    }

    fn stable_input(&self) -> Option<&str> {
        stable_string_prefix(&self.input, self.input_complete)
    }

    fn consume_character(&mut self, character: char) {
        if self.unicode_remaining > 0 {
            self.consume_unicode_digit(character);
            return;
        }
        if self.escaped {
            self.consume_escape(character);
            return;
        }
        if self.in_string {
            match character {
                '\\' => self.escaped = true,
                '"' => self.finish_string(),
                value => self.push_decoded(value),
            }
            return;
        }
        match character {
            '{' | ',' => self.expecting_key = true,
            ':' => self.expecting_value = true,
            '"' => self.begin_string(),
            _ => {}
        }
    }

    fn begin_string(&mut self) {
        self.in_string = true;
        self.target = if self.expecting_key {
            self.key_buffer.clear();
            JsonStringTarget::Key
        } else if self.expecting_value {
            match self.current_key.as_str() {
                "input" => JsonStringTarget::Input,
                _ => JsonStringTarget::Ignore,
            }
        } else {
            JsonStringTarget::Ignore
        };
    }

    fn finish_string(&mut self) {
        self.in_string = false;
        match self.target {
            JsonStringTarget::Key => {
                self.current_key.clone_from(&self.key_buffer);
                self.expecting_key = false;
            }
            JsonStringTarget::Input => self.input_complete = true,
            JsonStringTarget::Ignore => {}
        }
        self.expecting_value = false;
        self.target = JsonStringTarget::Ignore;
    }

    fn consume_escape(&mut self, character: char) {
        self.escaped = false;
        match character {
            '"' => self.push_decoded('"'),
            '\\' => self.push_decoded('\\'),
            '/' => self.push_decoded('/'),
            'b' => self.push_decoded('\u{0008}'),
            'f' => self.push_decoded('\u{000c}'),
            'n' => self.push_decoded('\n'),
            'r' => self.push_decoded('\r'),
            't' => self.push_decoded('\t'),
            'u' => {
                self.unicode_remaining = 4;
                self.unicode_value = 0;
            }
            _ => {}
        }
    }

    fn consume_unicode_digit(&mut self, character: char) {
        let Some(digit) = character.to_digit(16) else {
            self.unicode_remaining = 0;
            self.pending_high_surrogate = None;
            return;
        };
        let digit = u16::try_from(digit).expect("十六进制数字必须可转换为 u16");
        self.unicode_value = self.unicode_value.saturating_mul(16) + digit;
        self.unicode_remaining -= 1;
        if self.unicode_remaining == 0 {
            self.finish_unicode_escape();
        }
    }

    fn finish_unicode_escape(&mut self) {
        let value = self.unicode_value;
        if (0xD800..=0xDBFF).contains(&value) {
            self.pending_high_surrogate = Some(value);
            return;
        }
        let scalar = match (self.pending_high_surrogate.take(), value) {
            (Some(high), low @ 0xDC00..=0xDFFF) => {
                0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
            }
            (_, value) => u32::from(value),
        };
        if let Some(character) = char::from_u32(scalar) {
            self.push_decoded(character);
        }
    }

    fn push_decoded(&mut self, character: char) {
        match self.target {
            JsonStringTarget::Key => self.key_buffer.push(character),
            JsonStringTarget::Input => self.input.push(character),
            JsonStringTarget::Ignore => {}
        }
    }
}

fn stable_string_prefix(value: &str, complete: bool) -> Option<&str> {
    if complete {
        return Some(value);
    }
    let end = value.rfind('\n')?;
    Some(&value[..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_only_exposes_input_through_last_complete_line() {
        let mut decoder = ApplyDiffArgumentsDecoder::default();
        decoder.consume(r#"{"input":"first\nsec"#);

        assert_eq!(decoder.stable_input(), Some("first\n"));

        decoder.consume(r#"ond"}"#);
        assert_eq!(decoder.stable_input(), Some("first\nsecond"));
    }

    #[test]
    fn decoder_preserves_split_escapes_and_unicode() {
        let mut decoder = ApplyDiffArgumentsDecoder::default();
        decoder.consume(r#"{"input":"src/\u4e2"#);
        decoder.consume(r#"d.rs a\"b\n"}"#);

        assert_eq!(decoder.stable_input(), Some("src/中.rs a\"b\n"));
    }
}
