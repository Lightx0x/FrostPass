//! Colours and styles. One accent, FrostPass red `#D42A20`, for focus, selection and anything
//! that needs attention; everything else stays neutral so the accent keeps its meaning.

use ratatui::style::{Color, Modifier, Style};

use crate::model::EventStatus;

pub const ACCENT: Color = Color::Rgb(0xD4, 0x2A, 0x20);
/// The accent darkened, for selected rows in unfocused panes.
pub const ACCENT_DIM: Color = Color::Rgb(0x5C, 0x1D, 0x19);
pub const TEXT: Color = Color::Rgb(0xE8, 0xE6, 0xE3);
pub const MUTED: Color = Color::Rgb(0x8A, 0x86, 0x82);
pub const BORDER: Color = Color::Rgb(0x4A, 0x46, 0x44);
pub const OK: Color = Color::Rgb(0x5F, 0xB8, 0x78);
pub const WARN: Color = Color::Rgb(0xE0, 0xA9, 0x3B);
pub const ON_ACCENT: Color = Color::Rgb(0xFF, 0xFF, 0xFF);

pub fn text() -> Style {
    Style::new().fg(TEXT)
}

pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn strong() -> Style {
    Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
}

pub fn ok() -> Style {
    Style::new().fg(OK)
}

pub fn warn() -> Style {
    Style::new().fg(WARN)
}

/// Pane borders: the accent when the pane has focus.
pub fn border(focused: bool) -> Style {
    Style::new().fg(if focused { ACCENT } else { BORDER })
}

pub fn title(focused: bool) -> Style {
    let style = Style::new().add_modifier(Modifier::BOLD);
    if focused {
        style.fg(ACCENT)
    } else {
        style.fg(TEXT)
    }
}

/// The selected row: solid accent in the focused pane, a dim band elsewhere.
pub fn selected(focused: bool) -> Style {
    if focused {
        Style::new()
            .bg(ACCENT)
            .fg(ON_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(ACCENT_DIM).fg(TEXT)
    }
}

/// Key hints: the key in the accent, its action muted.
pub fn key() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn status(status: EventStatus) -> Style {
    match status {
        EventStatus::OnSale => ok(),
        EventStatus::SalesClosed => warn(),
        EventStatus::Ended => muted(),
        EventStatus::Cancelled => accent(),
    }
}
