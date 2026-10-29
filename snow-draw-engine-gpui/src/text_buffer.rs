use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) const MAX_TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone)]
struct EditSnapshot {
    text: String,
    anchor: usize,
    caret: usize,
}

/// UTF-8 storage with explicit UTF-16 IME conversion and grapheme-safe navigation.
pub(crate) struct TextBuffer {
    pub text: String,
    pub anchor: usize,
    pub caret: usize,
    pub marked: Option<Range<usize>>,
    undo: Vec<EditSnapshot>,
    redo: Vec<EditSnapshot>,
    composition_start: Option<EditSnapshot>,
}

impl TextBuffer {
    pub fn new(text: String) -> Self {
        let caret = text.len();
        Self {
            text,
            anchor: caret,
            caret,
            marked: None,
            undo: Vec::new(),
            redo: Vec::new(),
            composition_start: None,
        }
    }
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }
    pub fn utf16(&self, range: Range<usize>) -> Range<usize> {
        self.text[..range.start].encode_utf16().count()
            ..self.text[..range.end].encode_utf16().count()
    }
    pub fn byte_range_from_utf16(&self, range: Range<usize>) -> Range<usize> {
        let start = byte_byte_range_from_utf16(&self.text, range.start, false);
        let end = byte_byte_range_from_utf16(&self.text, range.end, true);
        start.min(end)..end.max(start)
    }
    pub fn select(&mut self, byte: usize, extend: bool) {
        let boundary = self
            .text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .chain([self.text.len()])
            .rfind(|index| *index <= byte)
            .unwrap_or(0);
        self.caret = boundary;
        if !extend {
            self.anchor = boundary;
        }
    }
    pub fn move_horizontal(&mut self, forward: bool, extend: bool) {
        let selection = self.selection();
        let target = if !extend && !selection.is_empty() {
            if forward {
                selection.end
            } else {
                selection.start
            }
        } else if forward {
            self.text
                .grapheme_indices(true)
                .map(|(index, _)| index)
                .find(|index| *index > self.caret)
                .unwrap_or(self.text.len())
        } else {
            self.text
                .grapheme_indices(true)
                .map(|(index, _)| index)
                .rfind(|index| *index < self.caret)
                .unwrap_or(0)
        };
        self.select(target, extend);
    }
    pub fn move_word(&mut self, forward: bool, extend: bool) {
        let target = if forward {
            self.text
                .unicode_word_indices()
                .map(|(index, word)| index + word.len())
                .find(|index| *index > self.caret)
                .unwrap_or(self.text.len())
        } else {
            self.text
                .unicode_word_indices()
                .map(|(index, _)| index)
                .rfind(|index| *index < self.caret)
                .unwrap_or(0)
        };
        self.select(target, extend);
    }
    pub fn replace(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        composing: bool,
        selection: Option<Range<usize>>,
    ) -> bool {
        let range = range_utf16
            .map(|range| self.byte_range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        if text.contains('\0') || self.text.len() - range.len() + text.len() > MAX_TEXT_BYTES {
            return false;
        }
        if composing {
            if self.composition_start.is_none() {
                self.composition_start = Some(self.snapshot());
            }
        } else {
            let before = self
                .composition_start
                .take()
                .unwrap_or_else(|| self.snapshot());
            self.push_undo(before);
        }
        self.text.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.marked = (composing && !text.is_empty()).then_some(range.start..end);
        if let Some(selection) = selection {
            self.anchor = range.start + byte_byte_range_from_utf16(text, selection.start, false);
            self.caret = range.start + byte_byte_range_from_utf16(text, selection.end, true);
        } else {
            self.anchor = end;
            self.caret = end;
        }
        true
    }
    pub fn unmark(&mut self) {
        self.marked = None;
        if let Some(before) = self.composition_start.take() {
            self.push_undo(before);
        }
    }
    pub fn delete(&mut self, forward: bool) {
        if self.selection().is_empty() {
            self.move_horizontal(forward, true);
        }
        if !self.selection().is_empty() {
            self.replace(None, "", false, None);
        }
    }
    pub fn undo(&mut self, redo: bool) {
        self.unmark();
        let next = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(next) = next {
            let previous = self.snapshot();
            if redo {
                self.undo.push(previous);
            } else {
                self.redo.push(previous);
            }
            self.text = next.text;
            self.anchor = next.anchor;
            self.caret = next.caret;
        }
    }
    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            text: self.text.clone(),
            anchor: self.anchor,
            caret: self.caret,
        }
    }
    fn push_undo(&mut self, snapshot: EditSnapshot) {
        if self.undo.len() == 128 {
            self.undo.remove(0);
        }
        self.undo.push(snapshot);
        self.redo.clear();
    }
}

fn byte_byte_range_from_utf16(text: &str, offset: usize, round_up: bool) -> usize {
    let mut utf16 = 0;
    for (index, character) in text.char_indices() {
        if utf16 >= offset {
            return index;
        }
        if utf16 + character.len_utf16() > offset {
            return if round_up {
                index + character.len_utf8()
            } else {
                index
            };
        }
        utf16 += character.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ime_ranges_round_out_surrogates_and_composition_is_one_undo() {
        let mut buffer = TextBuffer::new("A😀Z".into());
        assert_eq!(&buffer.text[buffer.byte_range_from_utf16(2..2)], "😀");
        buffer.select(1, false);
        buffer.replace(Some(1..3), "n", true, Some(1..1));
        buffer.replace(None, "你", true, Some(1..1));
        buffer.replace(None, "你好", false, None);
        assert_eq!(buffer.text, "A你好Z");
        buffer.undo(false);
        assert_eq!(buffer.text, "A😀Z");
        buffer.undo(true);
        assert_eq!(buffer.text, "A你好Z");
    }
    #[test]
    fn deletion_never_splits_combining_or_zwj_clusters() {
        let mut buffer = TextBuffer::new("e\u{301}👩‍👩‍👦".into());
        buffer.delete(false);
        assert_eq!(buffer.text, "e\u{301}");
        buffer.delete(false);
        assert!(buffer.text.is_empty());
    }
    #[test]
    fn replacement_limit_is_atomic_and_accounts_for_removed_selection() {
        let mut buffer = TextBuffer::new("x".repeat(MAX_TEXT_BYTES));
        assert!(!buffer.replace(None, "a", false, None));
        assert!(buffer.undo.is_empty());
        buffer.anchor = 0;
        assert!(buffer.replace(None, &"y".repeat(MAX_TEXT_BYTES), false, None));
        assert_eq!(buffer.text.len(), MAX_TEXT_BYTES);
        buffer.undo(false);
        assert!(buffer.text.starts_with('x'));
        buffer.anchor = 0;
        assert!(buffer.replace(None, "你", true, None));
        let marked = buffer.marked.clone();
        assert!(!buffer.replace(None, &"z".repeat(MAX_TEXT_BYTES + 1), true, None));
        assert_eq!(buffer.text, "你");
        assert_eq!(buffer.marked, marked);
        assert!(!buffer.replace(None, "\0", false, None));
        buffer.unmark();
        buffer.undo(false);
        assert_eq!(buffer.text, "x".repeat(MAX_TEXT_BYTES));
    }
}
