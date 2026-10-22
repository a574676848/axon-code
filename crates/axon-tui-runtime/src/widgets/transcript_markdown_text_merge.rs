//! Markdown text 事件合并器。

use std::{iter::Peekable, ops::Range};

use pulldown_cmark::Event;

pub(super) struct DecodedTextMerge<I: Iterator> {
    iter: Peekable<I>,
}

impl<I: Iterator> DecodedTextMerge<I> {
    pub(super) fn new(iter: I) -> Self {
        Self {
            iter: iter.peekable(),
        }
    }
}

impl<'a, I> Iterator for DecodedTextMerge<I>
where
    I: Iterator<Item = (Event<'a>, Range<usize>)>,
{
    type Item = (Event<'a>, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        let (event, mut range) = self.iter.next()?;
        let Event::Text(text) = event else {
            return Some((event, range));
        };
        if !matches!(self.iter.peek(), Some((Event::Text(_), _))) {
            return Some((Event::Text(text), range));
        }

        let mut merged = text.into_string();
        while matches!(self.iter.peek(), Some((Event::Text(_), _))) {
            let Some((Event::Text(text), next_range)) = self.iter.next() else {
                break;
            };
            merged.push_str(&text);
            range.end = next_range.end;
        }
        Some((Event::Text(merged.into()), range))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulldown_cmark::CowStr;

    #[test]
    fn merges_adjacent_decoded_text_events_and_preserves_combined_range() {
        let events = vec![
            (Event::Text(CowStr::from("alpha")), 0..5),
            (Event::Text(CowStr::from(" beta")), 5..10),
            (Event::SoftBreak, 10..11),
            (Event::Text(CowStr::from("gamma")), 11..16),
        ];

        let merged = DecodedTextMerge::new(events.into_iter()).collect::<Vec<_>>();

        assert_eq!(merged.len(), 3);
        assert!(
            matches!(&merged[0], (Event::Text(text), range) if text.as_ref() == "alpha beta" && *range == (0..10))
        );
        assert!(matches!(&merged[1], (Event::SoftBreak, range) if *range == (10..11)));
        assert!(
            matches!(&merged[2], (Event::Text(text), range) if text.as_ref() == "gamma" && *range == (11..16))
        );
    }
}
