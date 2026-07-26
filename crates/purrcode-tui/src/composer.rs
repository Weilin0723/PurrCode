//! Chat input field with /command detection and history.

use std::collections::VecDeque;

pub struct Composer {
    pub buffer: String,
    pub cursor: usize,
    pub history: VecDeque<String>,
    pub history_pos: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_supports_editing_submission_and_history() {
        let mut composer = Composer::new();
        for character in "hello".chars() {
            composer.insert_char(character);
        }
        composer.move_left();
        composer.delete_before();
        composer.insert_char('!');
        assert_eq!(composer.submit(), "hel!o");
        composer.history_up();
        assert_eq!(composer.buffer, "hel!o");
    }

    #[test]
    fn slash_commands_are_detected_without_execution() {
        let mut composer = Composer::new();
        for character in "/connect".chars() {
            composer.insert_char(character);
        }
        assert!(composer.is_command());
        assert_eq!(composer.submit(), "/connect");
    }
}

impl Composer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            history: VecDeque::new(),
            history_pos: None,
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += 1;
    }

    pub fn delete_before(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
        }
    }

    pub fn delete_after(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = self.history_pos.unwrap_or(self.history.len());
        if pos > 0 {
            let new_pos = pos - 1;
            self.buffer = self.history[new_pos].clone();
            self.cursor = self.buffer.len();
            self.history_pos = Some(new_pos);
        }
    }

    pub fn history_down(&mut self) {
        if let Some(pos) = self.history_pos {
            if pos + 1 < self.history.len() {
                let new_pos = pos + 1;
                self.buffer = self.history[new_pos].clone();
                self.cursor = self.buffer.len();
                self.history_pos = Some(new_pos);
            } else {
                self.buffer.clear();
                self.cursor = 0;
                self.history_pos = None;
            }
        }
    }

    pub fn submit(&mut self) -> String {
        let msg = self.buffer.trim().to_string();
        if !msg.is_empty() && (self.history.is_empty() || self.history.back() != Some(&msg)) {
            self.history.push_back(msg.clone());
            if self.history.len() > 50 {
                self.history.pop_front();
            }
        }
        self.buffer.clear();
        self.cursor = 0;
        self.history_pos = None;
        msg
    }

    pub fn is_command(&self) -> bool {
        self.buffer.starts_with('/')
    }
}
