//! Text composer — a UI-agnostic text buffer with cursor tracking.
//!
//! Extracted from `chat_core` so the input-line state machine can be tested
//! independently of network, storage and rendering.

// ── Composer ─────────────────────────────────────────────────────────────────

/// A text buffer with cursor tracking, suitable for a message composer / input line.
#[derive(Clone, Debug, Default)]
pub struct Composer {
    text: String,
    cursor: usize,
}

impl From<&str> for Composer {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
        }
    }
}

impl Composer {
    /// The current text content.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Byte offset of the cursor.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Visual column (character count up to cursor) for rendering.
    pub fn cursor_column(&self) -> u16 {
        self.text[..self.cursor].chars().count() as u16
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    /// Insert a string at the cursor position.
    pub fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert_char(ch);
        }
    }

    /// Move cursor one character left.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = prev_char_boundary(&self.text, self.cursor);
        }
    }

    /// Move cursor one character right.
    pub fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = next_char_boundary(&self.text, self.cursor);
        }
    }

    /// Move cursor to the start.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end.
    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let start = prev_char_boundary(&self.text, self.cursor);
            self.text.drain(start..self.cursor);
            self.cursor = start;
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            let end = next_char_boundary(&self.text, self.cursor);
            self.text.drain(self.cursor..end);
        }
    }

    /// Take the buffer contents and reset.
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        text
    }
}

fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(idx, _)| cursor + idx)
        .unwrap_or(text.len())
}
