//! 非 bracketed paste 的突发粘贴检测。

use std::time::{Duration, Instant};

const PASTE_BURST_MIN_CHARS: u16 = 3;
const PASTE_ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(120);
const PASTE_BURST_CHAR_INTERVAL: Duration = Duration::from_millis(8);

#[cfg(windows)]
const PASTE_BURST_ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(60);
#[cfg(not(windows))]
const PASTE_BURST_ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(8);

#[derive(Debug, Default, Clone)]
pub(super) struct PasteBurst {
    last_plain_char_time: Option<Instant>,
    consecutive_plain_char_burst: u16,
    last_unbuffered_char_time: Option<Instant>,
    burst_window_until: Option<Instant>,
    buffer: String,
    active: bool,
    pending_first_char: Option<(char, Instant)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CharDecision {
    RetainFirstChar,
    BeginBufferFromPending,
    BeginBuffer { retro_chars: u16 },
    BufferAppend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RetroGrab {
    pub(super) start_byte: usize,
    pub(super) grabbed: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FlushResult {
    None,
    Typed(char),
    Paste(String),
}

impl PasteBurst {
    pub(super) fn on_plain_char(&mut self, ch: char, now: Instant) -> CharDecision {
        self.note_plain_char(now);
        if self.active {
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            return CharDecision::BufferAppend;
        }
        if let Some((held, held_at)) = self.pending_first_char
            && now.duration_since(held_at) <= PASTE_BURST_CHAR_INTERVAL
        {
            self.active = true;
            let _ = self.pending_first_char.take();
            self.buffer.push(held);
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            return CharDecision::BeginBufferFromPending;
        }
        if self.consecutive_plain_char_burst >= PASTE_BURST_MIN_CHARS {
            return CharDecision::BeginBuffer {
                retro_chars: self.consecutive_plain_char_burst.saturating_sub(1),
            };
        }
        self.pending_first_char = Some((ch, now));
        CharDecision::RetainFirstChar
    }

    pub(super) fn append_char_to_buffer(&mut self, ch: char, now: Instant) {
        self.buffer.push(ch);
        self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
    }

    pub(super) fn append_non_ascii_char_if_buffering(&mut self, ch: char, now: Instant) -> bool {
        if !self.is_active() {
            return false;
        }
        self.note_plain_char(now);
        if !self.active {
            self.active = true;
            if let Some((held, _)) = self.pending_first_char.take() {
                self.buffer.push(held);
            }
        }
        self.append_char_to_buffer(ch, now);
        true
    }

    pub(super) fn note_unbuffered_char(&mut self, now: Instant) {
        self.last_unbuffered_char_time = Some(now);
    }

    pub(super) fn unbuffered_newline_should_insert(&self, now: Instant) -> bool {
        self.last_unbuffered_char_time
            .is_some_and(|last| now.duration_since(last) <= PASTE_BURST_CHAR_INTERVAL)
    }

    pub(super) fn append_newline_if_active(&mut self, now: Instant) -> bool {
        if !self.is_active_internal() {
            return false;
        }
        self.buffer.push('\n');
        self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
        true
    }

    pub(super) fn newline_should_insert_instead_of_submit(&self, now: Instant) -> bool {
        self.is_active()
            || self.burst_window_until.is_some_and(|until| now <= until)
            || self
                .last_plain_char_time
                .is_some_and(|last| now.duration_since(last) <= PASTE_BURST_CHAR_INTERVAL)
    }

    pub(super) fn flush_if_due(&mut self, now: Instant) -> FlushResult {
        if self
            .last_unbuffered_char_time
            .is_some_and(|last| now.duration_since(last) > PASTE_ENTER_SUPPRESS_WINDOW)
        {
            self.last_unbuffered_char_time = None;
        }
        let timeout = if self.is_active_internal() {
            PASTE_BURST_ACTIVE_IDLE_TIMEOUT
        } else {
            PASTE_BURST_CHAR_INTERVAL
        };
        let timed_out = self
            .last_plain_char_time
            .is_some_and(|last| now.duration_since(last) > timeout);
        if timed_out && self.is_active_internal() {
            self.active = false;
            return FlushResult::Paste(std::mem::take(&mut self.buffer));
        }
        if timed_out && let Some((ch, _)) = self.pending_first_char.take() {
            return FlushResult::Typed(ch);
        }
        FlushResult::None
    }

    pub(super) fn decide_begin_buffer(
        &mut self,
        now: Instant,
        before: &str,
        retro_chars: usize,
    ) -> Option<RetroGrab> {
        let start_byte = retro_start_index(before, retro_chars);
        let grabbed = before[start_byte..].to_owned();
        let looks_like_paste =
            grabbed.chars().any(char::is_whitespace) || grabbed.chars().count() >= 16;
        if !looks_like_paste {
            return None;
        }
        self.buffer.push_str(&grabbed);
        self.active = true;
        self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
        Some(RetroGrab {
            start_byte,
            grabbed,
        })
    }

    pub(super) fn flush_before_modified_input(&mut self) -> Option<String> {
        if !self.is_active() {
            self.clear_after_explicit_paste();
            return None;
        }
        self.active = false;
        let mut output = std::mem::take(&mut self.buffer);
        if let Some((ch, _)) = self.pending_first_char.take() {
            output.push(ch);
        }
        self.clear_after_explicit_paste();
        Some(output)
    }

    pub(super) fn clear_after_explicit_paste(&mut self) {
        self.last_plain_char_time = None;
        self.consecutive_plain_char_burst = 0;
        self.last_unbuffered_char_time = None;
        self.burst_window_until = None;
        self.active = false;
        self.buffer.clear();
        self.pending_first_char = None;
    }

    fn note_plain_char(&mut self, now: Instant) {
        match self.last_plain_char_time {
            Some(last) if now.duration_since(last) <= PASTE_BURST_CHAR_INTERVAL => {
                self.consecutive_plain_char_burst =
                    self.consecutive_plain_char_burst.saturating_add(1);
            }
            _ => self.consecutive_plain_char_burst = 1,
        }
        self.last_plain_char_time = Some(now);
    }

    fn is_active(&self) -> bool {
        self.is_active_internal() || self.pending_first_char.is_some()
    }

    fn is_active_internal(&self) -> bool {
        self.active || !self.buffer.is_empty()
    }
}

fn retro_start_index(before: &str, retro_chars: usize) -> usize {
    if retro_chars == 0 {
        return before.len();
    }
    before
        .char_indices()
        .rev()
        .nth(retro_chars.saturating_sub(1))
        .map_or(0, |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{CharDecision, FlushResult, PasteBurst};

    #[test]
    fn ascii_burst_buffers_and_flushes_as_paste() {
        let now = Instant::now();
        let mut burst = PasteBurst::default();

        assert_eq!(burst.on_plain_char('a', now), CharDecision::RetainFirstChar);
        assert_eq!(
            burst.on_plain_char('b', now + Duration::from_millis(1)),
            CharDecision::BeginBufferFromPending
        );
        burst.append_char_to_buffer('b', now + Duration::from_millis(1));
        assert_eq!(
            burst.flush_if_due(now + Duration::from_millis(100)),
            FlushResult::Paste("ab".to_owned())
        );
    }

    #[test]
    fn single_fast_char_flushes_as_typed_input() {
        let now = Instant::now();
        let mut burst = PasteBurst::default();

        assert_eq!(burst.on_plain_char('x', now), CharDecision::RetainFirstChar);
        assert_eq!(
            burst.flush_if_due(now + Duration::from_millis(10)),
            FlushResult::Typed('x')
        );
    }

    #[test]
    fn immediate_newline_after_single_unicode_char_is_treated_as_paste() {
        let now = Instant::now();
        let mut burst = PasteBurst::default();

        burst.note_unbuffered_char(now);

        assert!(burst.unbuffered_newline_should_insert(now + Duration::from_millis(1)));
        assert!(!burst.unbuffered_newline_should_insert(
            now + super::PASTE_BURST_CHAR_INTERVAL + Duration::from_millis(1)
        ));
    }

    #[test]
    fn mixed_ascii_unicode_burst_preserves_character_order() {
        let now = Instant::now();
        let mut burst = PasteBurst::default();

        assert_eq!(burst.on_plain_char('E', now), CharDecision::RetainFirstChar);
        assert!(burst.append_non_ascii_char_if_buffering('智', now + Duration::from_millis(1)));
        assert_eq!(
            burst.on_plain_char('A', now + Duration::from_millis(2)),
            CharDecision::BufferAppend
        );
        burst.append_char_to_buffer('A', now + Duration::from_millis(2));

        assert_eq!(
            burst.flush_if_due(now + Duration::from_millis(100)),
            FlushResult::Paste("E智A".to_owned())
        );
    }
}
