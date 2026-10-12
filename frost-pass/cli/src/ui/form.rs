//! A small modal form: labelled text fields, one focused at a time, each with its own cursor.

use anchor_lang::prelude::Pubkey;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a submitted form does.
#[derive(Clone, Debug)]
pub enum FormKind {
    NewEvent,
    Scanners { event: Pubkey },
    Mint { event: Pubkey },
    List { asset: Pubkey },
    ScanPass,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub hint: String,
    /// Where typing goes, in characters from the start (0..=len).
    pub cursor: usize,
}

impl Field {
    pub fn new(label: &'static str, value: impl Into<String>, hint: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            label,
            cursor: value.chars().count(),
            value,
            hint: hint.into(),
        }
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    /// The byte offset of character `index`.
    fn byte(&self, index: usize) -> usize {
        self.value
            .char_indices()
            .nth(index)
            .map_or(self.value.len(), |(byte, _)| byte)
    }

    fn insert(&mut self, text: &str) {
        let at = self.byte(self.cursor);
        self.value.insert_str(at, text);
        self.cursor += text.chars().count();
    }

    /// Deletes characters `from..to` and leaves the cursor at `from`.
    fn delete(&mut self, from: usize, to: usize) {
        let (start, end) = (self.byte(from), self.byte(to));
        self.value.replace_range(start..end, "");
        self.cursor = from;
    }

    /// Where the word before the cursor starts: back over spaces and separators, then over the
    /// word (so `"a, bc|"` → `"a, |"`).
    fn word_start(&self) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    /// Where the word after the cursor ends.
    fn word_end(&self) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && !chars[i].is_alphanumeric() {
            i += 1;
        }
        while i < chars.len() && chars[i].is_alphanumeric() {
            i += 1;
        }
        i
    }

    /// Edits the field; false if the key isn't an editing key.
    fn edit(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let word = ctrl || alt;
        match key.code {
            KeyCode::Left if word => self.cursor = self.word_start(),
            KeyCode::Right if word => self.cursor = self.word_end(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len(),
            // Ctrl+Backspace (most terminals send it as Ctrl+H), Alt+Backspace, Ctrl+W: the word
            // before the cursor
            KeyCode::Backspace if word => self.delete(self.word_start(), self.cursor),
            KeyCode::Char('h' | 'w') if ctrl => self.delete(self.word_start(), self.cursor),
            KeyCode::Delete if word => self.delete(self.cursor, self.word_end()),
            KeyCode::Backspace if self.cursor > 0 => self.delete(self.cursor - 1, self.cursor),
            KeyCode::Delete if self.cursor < self.len() => {
                self.delete(self.cursor, self.cursor + 1)
            }
            KeyCode::Char('u') if ctrl => {
                self.value.clear();
                self.cursor = 0;
            }
            // From the cursor to the end
            KeyCode::Char('k') if ctrl => self.delete(self.cursor, self.len()),
            // Other Ctrl/Alt combinations aren't text
            KeyCode::Char(_) if word => return false,
            KeyCode::Char(c) => self.insert(c.encode_utf8(&mut [0; 4])),
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct Form {
    pub kind: FormKind,
    pub title: String,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// Why the last submit was refused, shown under the fields.
    pub error: Option<String>,
}

pub enum FormInput {
    Editing,
    Submit,
    Cancel,
}

impl Form {
    pub fn new(kind: FormKind, title: impl Into<String>, fields: Vec<Field>) -> Self {
        Self {
            kind,
            title: title.into(),
            fields,
            focus: 0,
            error: None,
        }
    }

    pub fn value(&self, index: usize) -> &str {
        self.fields[index].value.trim()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> FormInput {
        let last = self.fields.len() - 1;
        match key.code {
            KeyCode::Esc => return FormInput::Cancel,
            KeyCode::Enter if self.focus == last => return FormInput::Submit,
            KeyCode::Enter | KeyCode::Tab | KeyCode::Down => {
                self.focus = (self.focus + 1).min(last);
            }
            KeyCode::BackTab | KeyCode::Up => self.focus = self.focus.saturating_sub(1),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return FormInput::Submit;
            }
            _ => {
                if !self.fields[self.focus].edit(key) {
                    return FormInput::Editing;
                }
            }
        }
        self.error = None;
        FormInput::Editing
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.trim_end_matches(['\n', '\r']);
        self.fields[self.focus].insert(text);
        self.error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(field: &mut Field, code: KeyCode, modifiers: KeyModifiers) {
        field.edit(KeyEvent::new(code, modifiers));
    }

    #[test]
    fn fields_edit_at_the_cursor() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let mut field = Field::new("URI", "https//example.com", "");
        assert_eq!(field.cursor, 18);
        // Fix the missing colon in the middle, without retyping
        for _ in 0..13 {
            press(&mut field, KeyCode::Left, none);
        }
        press(&mut field, KeyCode::Char(':'), none);
        assert_eq!(field.value, "https://example.com");
        press(&mut field, KeyCode::Delete, none);
        press(&mut field, KeyCode::Char('/'), none);
        assert_eq!(field.value, "https://example.com");

        // Ctrl+Backspace (sent as Ctrl+H) deletes a word, and never types an "h"
        press(&mut field, KeyCode::End, none);
        press(&mut field, KeyCode::Char('h'), ctrl);
        assert_eq!(field.value, "https://example.");
        press(&mut field, KeyCode::Backspace, ctrl);
        assert_eq!(field.value, "https://");
        press(&mut field, KeyCode::Char('x'), KeyModifiers::ALT);
        assert_eq!(field.value, "https://");

        press(&mut field, KeyCode::Home, none);
        press(&mut field, KeyCode::Char('k'), ctrl);
        assert_eq!((field.value.as_str(), field.cursor), ("", 0));

        // Multi-byte text keeps the cursor on character boundaries
        let mut name = Field::new("Name", "Café", "");
        press(&mut name, KeyCode::Left, none);
        press(&mut name, KeyCode::Backspace, none);
        assert_eq!(name.value, "Caé");
    }
}
