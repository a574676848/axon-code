//! TUI surface 有界行缓存。

use std::collections::VecDeque;

/// 只保留尾部窗口的行级 ring buffer。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineRingBuffer {
    capacity: usize,
    lines: VecDeque<String>,
    pushed: usize,
    dropped: usize,
}

impl LineRingBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            lines: VecDeque::new(),
            pushed: 0,
            dropped: 0,
        }
    }

    pub(crate) fn push(&mut self, line: impl Into<String>) {
        self.pushed = self.pushed.saturating_add(1);
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.lines.push_back(line.into());
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn dropped(&self) -> usize {
        self.dropped
    }

    pub(crate) fn retained_lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::LineRingBuffer;

    #[test]
    fn keeps_tail_window_and_counts_dropped_lines() {
        let mut buffer = LineRingBuffer::new(3);

        for index in 1..=5 {
            buffer.push(format!("line {index}"));
        }

        assert_eq!(buffer.retained_lines(), ["line 3", "line 4", "line 5"]);
        assert_eq!(buffer.len() + buffer.dropped(), 5);
        assert_eq!(buffer.dropped(), 2);
    }
}
