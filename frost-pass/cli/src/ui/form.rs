//! A small modal form: labelled text fields, one focused at a time.

use anchor_lang::prelude::Pubkey;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a submitted form does.
#[derive(Clone, Debug)]
pub enum FormKind {
    NewEvent,
    Scanners { event: Pubkey },
    Mint { event: Pubkey },
    List { asset: Pubkey },
    Buy { asset: Pubkey },
    ScanPass,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub hint: String,
}

impl Field {
    pub fn new(label: &'static str, value: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            hint: hint.into(),
        }
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
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.fields[self.focus].value.clear();
            }
            KeyCode::Backspace => {
                self.fields[self.focus].value.pop();
            }
            KeyCode::Char(c) => self.fields[self.focus].value.push(c),
            _ => {}
        }
        self.error = None;
        FormInput::Editing
    }

    pub fn paste(&mut self, text: &str) {
        let text = text.trim_end_matches(['\n', '\r']);
        self.fields[self.focus].value.push_str(text);
        self.error = None;
    }
}
