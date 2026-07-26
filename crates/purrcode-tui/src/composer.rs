//! Unicode-safe multiline chat composer.

use std::collections::VecDeque;
use unicode_segmentation::UnicodeSegmentation;

const HISTORY_LIMIT: usize = 50;
const UNDO_LIMIT: usize = 100;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerViewport {
    pub first_line: usize,
    pub first_column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    buffer: String,
    cursor: usize,
}

pub struct Composer {
    pub buffer: String,
    /// Cursor position measured in grapheme clusters, never bytes.
    pub cursor: usize,
    pub selection_anchor: Option<usize>,
    pub viewport: ComposerViewport,
    pub history: VecDeque<String>,
    pub history_pos: Option<usize>,
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    preferred_column: Option<usize>,
    pasted_since_submit: bool,
}

impl Composer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            selection_anchor: None,
            viewport: ComposerViewport::default(),
            history: VecDeque::new(),
            history_pos: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            preferred_column: None,
            pasted_since_submit: false,
        }
    }

    pub fn grapheme_count(&self) -> usize {
        self.buffer.graphemes(true).count()
    }

    pub fn line_count(&self) -> usize {
        self.buffer.matches('\n').count() + 1
    }

    pub fn cursor_line_column(&self) -> (usize, usize) {
        let mut line = 0;
        let mut column = 0;
        for grapheme in self.buffer.graphemes(true).take(self.cursor) {
            if grapheme == "\n" {
                line += 1;
                column = 0;
            } else {
                column += 1;
            }
        }
        (line, column)
    }

    pub fn insert_char(&mut self, ch: char) {
        self.replace_selection_or_insert(&ch.to_string(), true);
    }

    pub fn insert_newline(&mut self) {
        self.replace_selection_or_insert("\n", true);
    }

    pub fn insert_tab(&mut self) {
        self.replace_selection_or_insert("    ", true);
    }

    /// Inserts a complete terminal paste as one undoable edit.
    pub fn insert_paste(&mut self, content: &str) {
        let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
        self.replace_selection_or_insert(&normalized, true);
        self.pasted_since_submit = true;
    }

    pub fn pasted_since_submit(&self) -> bool {
        self.pasted_since_submit
    }

    pub fn delete_before(&mut self) {
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        self.record_edit();
        let start = self.cursor - 1;
        self.replace_grapheme_range(start, self.cursor, "");
        self.cursor = start;
        self.preferred_column = None;
    }

    pub fn delete_after(&mut self) {
        if self.delete_selection() || self.cursor >= self.grapheme_count() {
            return;
        }
        self.record_edit();
        self.replace_grapheme_range(self.cursor, self.cursor + 1, "");
        self.preferred_column = None;
    }

    pub fn move_left(&mut self) {
        self.clear_selection();
        self.cursor = self.cursor.saturating_sub(1);
        self.preferred_column = None;
    }

    pub fn move_right(&mut self) {
        self.clear_selection();
        self.cursor = (self.cursor + 1).min(self.grapheme_count());
        self.preferred_column = None;
    }

    pub fn move_word_left(&mut self) {
        self.clear_selection();
        let graphemes: Vec<&str> = self.buffer.graphemes(true).collect();
        while self.cursor > 0 && graphemes[self.cursor - 1].chars().all(char::is_whitespace) {
            self.cursor -= 1;
        }
        while self.cursor > 0 && !graphemes[self.cursor - 1].chars().all(char::is_whitespace) {
            self.cursor -= 1;
        }
    }

    pub fn move_word_right(&mut self) {
        self.clear_selection();
        let graphemes: Vec<&str> = self.buffer.graphemes(true).collect();
        while self.cursor < graphemes.len()
            && !graphemes[self.cursor].chars().all(char::is_whitespace)
        {
            self.cursor += 1;
        }
        while self.cursor < graphemes.len()
            && graphemes[self.cursor].chars().all(char::is_whitespace)
        {
            self.cursor += 1;
        }
    }

    pub fn delete_word_before(&mut self) {
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        let end = self.cursor;
        self.move_word_left();
        let start = self.cursor;
        self.record_edit();
        self.replace_grapheme_range(start, end, "");
    }

    pub fn delete_word_after(&mut self) {
        if self.delete_selection() || self.cursor >= self.grapheme_count() {
            return;
        }
        let start = self.cursor;
        self.move_word_right();
        let end = self.cursor;
        self.record_edit();
        self.replace_grapheme_range(start, end, "");
        self.cursor = start;
    }

    pub fn select_all(&mut self) {
        self.selection_anchor = Some(0);
        self.cursor = self.grapheme_count();
    }

    pub fn outdent_current_line(&mut self) {
        let (line, column) = self.cursor_line_column();
        let start = self.position_for_line_column(line, 0);
        let removable = self
            .buffer
            .graphemes(true)
            .skip(start)
            .take(4)
            .take_while(|grapheme| *grapheme == " ")
            .count();
        if removable == 0 {
            return;
        }
        self.record_edit();
        self.replace_grapheme_range(start, start + removable, "");
        self.cursor = self.cursor.saturating_sub(removable.min(column));
    }

    pub fn move_home(&mut self) {
        let (line, _) = self.cursor_line_column();
        self.cursor = self.position_for_line_column(line, 0);
        self.clear_selection();
        self.preferred_column = None;
    }

    pub fn move_end(&mut self) {
        let (line, _) = self.cursor_line_column();
        self.cursor = self.position_for_line_column(line, usize::MAX);
        self.clear_selection();
        self.preferred_column = None;
    }

    pub fn move_document_start(&mut self) {
        self.cursor = 0;
        self.clear_selection();
    }

    pub fn move_document_end(&mut self) {
        self.cursor = self.grapheme_count();
        self.clear_selection();
    }

    pub fn move_up(&mut self) {
        self.move_vertical(-1);
    }

    pub fn move_down(&mut self) {
        self.move_vertical(1);
    }

    pub fn undo(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.restore(snapshot);
    }

    pub fn redo(&mut self) {
        let Some(snapshot) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        self.restore(snapshot);
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = self.history_pos.unwrap_or(self.history.len());
        if pos > 0 {
            let new_pos = pos - 1;
            self.buffer = self.history[new_pos].clone();
            self.cursor = self.grapheme_count();
            self.history_pos = Some(new_pos);
        }
    }

    pub fn history_down(&mut self) {
        if let Some(pos) = self.history_pos {
            if pos + 1 < self.history.len() {
                self.history_pos = Some(pos + 1);
                self.buffer = self.history[pos + 1].clone();
                self.cursor = self.grapheme_count();
            } else {
                self.clear();
            }
        }
    }

    pub fn submit(&mut self) -> String {
        // Keep leading indentation and internal/trailing newlines. Remove only spaces/tabs
        // after the final non-whitespace character, so code formatting is not destroyed.
        let terminal = self.buffer.trim_end_matches([' ', '\t']).to_owned();
        let is_empty = terminal.chars().all(char::is_whitespace);
        let message = if is_empty { String::new() } else { terminal };
        if !message.is_empty() && self.history.back() != Some(&message) {
            self.history.push_back(message.clone());
            if self.history.len() > HISTORY_LIMIT {
                self.history.pop_front();
            }
        }
        self.clear();
        message
    }

    pub fn is_command(&self) -> bool {
        !self.pasted_since_submit && !self.buffer.contains('\n') && self.buffer.starts_with('/')
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.selection_anchor = None;
        self.viewport = ComposerViewport::default();
        self.history_pos = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.pasted_since_submit = false;
    }

    fn move_vertical(&mut self, delta: isize) {
        let (line, column) = self.cursor_line_column();
        let preferred = self.preferred_column.unwrap_or(column);
        let target = line.saturating_add_signed(delta).min(self.line_count() - 1);
        self.cursor = self.position_for_line_column(target, preferred);
        self.preferred_column = Some(preferred);
        self.clear_selection();
    }

    fn position_for_line_column(&self, target_line: usize, target_column: usize) -> usize {
        let mut position = 0;
        let mut line = 0;
        let mut column = 0;
        for grapheme in self.buffer.graphemes(true) {
            if line == target_line && (column == target_column || grapheme == "\n") {
                break;
            }
            if grapheme == "\n" {
                line += 1;
                column = 0;
            } else {
                column += 1;
            }
            position += 1;
        }
        position
    }

    fn replace_selection_or_insert(&mut self, text: &str, record: bool) {
        if record {
            self.record_edit();
        }
        let (start, end) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        self.replace_grapheme_range(start, end, text);
        self.cursor = start + text.graphemes(true).count();
        self.selection_anchor = None;
        self.preferred_column = None;
        self.history_pos = None;
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection_range() else {
            return false;
        };
        self.record_edit();
        self.replace_grapheme_range(start, end, "");
        self.cursor = start;
        self.selection_anchor = None;
        true
    }

    fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;
        (anchor != self.cursor).then(|| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    fn replace_grapheme_range(&mut self, start: usize, end: usize, replacement: &str) {
        let start_byte = byte_offset(&self.buffer, start);
        let end_byte = byte_offset(&self.buffer, end);
        self.buffer.replace_range(start_byte..end_byte, replacement);
    }

    fn record_edit(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            buffer: self.buffer.clone(),
            cursor: self.cursor,
        }
    }
    fn restore(&mut self, snapshot: Snapshot) {
        self.buffer = snapshot.buffer;
        self.cursor = snapshot.cursor;
        self.selection_anchor = None;
        self.preferred_column = None;
    }
}

fn byte_offset(text: &str, grapheme_index: usize) -> usize {
    text.grapheme_indices(true)
        .nth(grapheme_index)
        .map_or(text.len(), |(offset, _)| offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_cursor_and_cross_line_deletion_are_safe() {
        let mut composer = Composer::new();
        composer.insert_paste("a👩‍💻\n界");
        assert_eq!(composer.grapheme_count(), 4);
        composer.move_left();
        composer.move_left();
        composer.delete_before();
        assert_eq!(composer.buffer, "a\n界");
        composer.delete_after();
        assert_eq!(composer.buffer, "a界");
    }

    #[test]
    fn multiline_paste_normalizes_crlf_and_preserves_indentation() {
        let mut composer = Composer::new();
        composer.insert_paste("def f():\r\n    return 1\r\n");
        assert_eq!(composer.submit(), "def f():\n    return 1\n");
    }

    #[test]
    fn paste_is_one_undo_operation_and_slash_is_not_a_command() {
        let mut composer = Composer::new();
        composer.insert_paste("/connect\nnext");
        assert!(!composer.is_command());
        composer.undo();
        assert!(composer.buffer.is_empty());
        composer.redo();
        assert_eq!(composer.buffer, "/connect\nnext");
    }

    #[test]
    fn vertical_movement_keeps_preferred_column() {
        let mut composer = Composer::new();
        composer.insert_paste("abcd\nx\nwxyz");
        composer.move_document_start();
        for _ in 0..3 {
            composer.move_right();
        }
        composer.move_down();
        assert_eq!(composer.cursor_line_column(), (1, 1));
        composer.move_down();
        assert_eq!(composer.cursor_line_column(), (2, 3));
    }

    #[test]
    fn history_keeps_multiline_entries() {
        let mut composer = Composer::new();
        composer.insert_paste("one\n  two");
        assert_eq!(composer.submit(), "one\n  two");
        composer.history_up();
        assert_eq!(composer.buffer, "one\n  two");
    }

    #[test]
    fn large_paste_is_preserved() {
        let source = "    value = 1\n".repeat(100);
        let mut composer = Composer::new();
        composer.insert_paste(&source);
        assert_eq!(composer.submit(), source);
        let large = "界".repeat(20_000);
        composer.insert_paste(&large);
        assert_eq!(composer.grapheme_count(), 20_000);
    }

    #[test]
    fn word_editing_selection_and_outdent_work() {
        let mut composer = Composer::new();
        composer.insert_paste("    hello world");
        composer.delete_word_before();
        assert_eq!(composer.buffer, "    hello ");
        composer.move_home();
        composer.outdent_current_line();
        assert_eq!(composer.buffer, "hello ");
        composer.select_all();
        composer.insert_char('x');
        assert_eq!(composer.buffer, "x");
    }
}
