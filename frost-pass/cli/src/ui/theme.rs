//! Colours and styles. One accent, frost blue `#7DD3FC`, for focus, selection and the logo;
//! red is kept for danger only (refused passes, failures, cancellations, irreversible actions),
//! and everything else stays neutral so both keep their meaning.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::model::EventStatus;

pub const ACCENT: Color = Color::Rgb(0x7D, 0xD3, 0xFC);
/// The accent darkened, for selected rows in unfocused panes.
pub const ACCENT_DIM: Color = Color::Rgb(0x17, 0x35, 0x48);
/// Dark text on the accent (white on pale blue is unreadable).
pub const ON_ACCENT: Color = Color::Rgb(0x06, 0x20, 0x2E);
/// Refused, failed, cancelled, irreversible.
pub const DANGER: Color = Color::Rgb(0xE5, 0x53, 0x4B);
pub const TEXT: Color = Color::Rgb(0xE6, 0xEC, 0xF0);
pub const MUTED: Color = Color::Rgb(0x84, 0x8E, 0x96);
pub const BORDER: Color = Color::Rgb(0x3A, 0x46, 0x50);
pub const OK: Color = Color::Rgb(0x5F, 0xB8, 0x78);
pub const WARN: Color = Color::Rgb(0xE0, 0xA9, 0x3B);

/// The logo's frost: deep ice at the faded edges, near-white where the shimmer catches.
const ICE_DEEP: (u8, u8, u8) = (0x12, 0x3C, 0x55);
const ICE: (u8, u8, u8) = (0x7D, 0xD3, 0xFC);
const FROST: (u8, u8, u8) = (0xF2, 0xFB, 0xFF);

pub fn text() -> Style {
    Style::new().fg(TEXT)
}

pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn danger() -> Style {
    Style::new().fg(DANGER)
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
        EventStatus::Cancelled => danger(),
    }
}

// ---------------------------------------------------------------------------
// The logo
// ---------------------------------------------------------------------------

const WORD: &str = " FROSTPASS ";
/// Frost thinning out on each side of the badge.
const EDGE_LEFT: [&str; 3] = ["░", "▒", "▓"];
const EDGE_RIGHT: [&str; 3] = ["▓", "▒", "░"];
/// The glow breathes over this period.
const PULSE_MS: u128 = 2_400;
/// A shimmer sweeps across the word once per cycle, then the frost rests.
const SHIMMER_MS: u128 = 3_200;
const SWEEP_MS: u128 = 1_400;

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let channel = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::Rgb(channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

/// The FROSTPASS badge at `ms` milliseconds into the animation: frosted edges that glow and
/// fade, and a highlight of frost light passing over the letters. `ms = None` draws it still.
pub fn logo(ms: Option<u128>) -> Line<'static> {
    let ms = ms.unwrap_or(0);
    let still = ms == 0;
    // 0..1, breathing
    let pulse = if still {
        0.5
    } else {
        let phase = (ms % PULSE_MS) as f32 / PULSE_MS as f32;
        0.5 - 0.5 * (phase * std::f32::consts::TAU).cos()
    };
    // Where the shimmer is, in letters (off the word between sweeps)
    let width = WORD.chars().count() as f32;
    let shimmer = match ms % SHIMMER_MS {
        t if !still && t < SWEEP_MS => -3.0 + (width + 6.0) * t as f32 / SWEEP_MS as f32,
        _ => -100.0,
    };

    let mut spans = Vec::with_capacity(WORD.len() + 6);
    // Outer edge dimmest; the glow brightens the whole edge together
    for (i, block) in EDGE_LEFT.iter().enumerate() {
        let depth = (i + 1) as f32 / EDGE_LEFT.len() as f32;
        spans.push(Span::styled(
            *block,
            Style::new().fg(mix(ICE_DEEP, ICE, depth * (0.45 + 0.55 * pulse))),
        ));
    }
    for (i, letter) in WORD.chars().enumerate() {
        let near = (1.0 - (i as f32 - shimmer).abs() / 2.5).max(0.0);
        let glow = 0.12 * pulse + 0.88 * near;
        spans.push(Span::styled(
            letter.to_string(),
            Style::new()
                .bg(mix(ICE, FROST, glow))
                .fg(ON_ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    }
    for (i, block) in EDGE_RIGHT.iter().enumerate() {
        let depth = (EDGE_RIGHT.len() - i) as f32 / EDGE_RIGHT.len() as f32;
        spans.push(Span::styled(
            *block,
            Style::new().fg(mix(ICE_DEEP, ICE, depth * (0.45 + 0.55 * pulse))),
        ));
    }
    Line::from(spans)
}

/// The cluster's "live" dot: frost blue, fading out and back like a beacon, about once a
/// second. Solid when the animation is off.
pub fn live_dot(ms: Option<u128>) -> Span<'static> {
    const BLINK_MS: u128 = 1_200;
    let glow = match ms {
        None => 1.0,
        Some(ms) => {
            let phase = (ms % BLINK_MS) as f32 / BLINK_MS as f32;
            0.5 + 0.5 * (phase * std::f32::consts::TAU).cos()
        }
    };
    Span::styled(
        "●",
        Style::new()
            .fg(mix(ICE_DEEP, ICE, 0.15 + 0.85 * glow))
            .add_modifier(Modifier::BOLD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_reads_frostpass_and_shimmers() {
        let text = |ms| logo(ms).to_string();
        assert_eq!(text(None), "░▒▓ FROSTPASS ▓▒░");
        // Mid-sweep, some letter is lit brighter than the rest; between sweeps none is
        let backgrounds = |ms| -> Vec<Option<Color>> {
            logo(Some(ms)).spans[3..3 + WORD.len()]
                .iter()
                .map(|span| span.style.bg)
                .collect()
        };
        let mid = backgrounds(SWEEP_MS / 2);
        assert!(mid.iter().any(|bg| *bg != mid[0]), "the shimmer shows");
        let rest = backgrounds(SWEEP_MS + 600);
        assert!(rest.iter().all(|bg| *bg == rest[0]), "then the frost rests");
    }
}
