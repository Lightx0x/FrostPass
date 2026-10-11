//! The terminal UI: one tab per actor (organizer, minter, buyer, scanner) over the same wallet.

mod app;
mod form;
mod theme;
mod view;

use std::{io::stdout, time::Duration};

use anyhow::Result;
use ratatui::crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
};
use solana_keypair::Keypair;

pub use app::{App, Tab};
pub use view::draw;

use crate::chain::Chain;

/// How often the screen redraws without input, for countdowns (door passes, sales end).
const TICK: Duration = Duration::from_millis(250);

pub fn run(chain: impl Chain, wallet: Keypair, cluster: String) -> Result<()> {
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableBracketedPaste)?;
    let result = run_app(&mut terminal, App::new(chain, wallet, cluster));
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn run_app(terminal: &mut ratatui::DefaultTerminal, mut app: App<impl Chain>) -> Result<()> {
    while !app.quit {
        terminal.draw(|frame| view::draw(frame, &app))?;
        if let Some(text) = app.clipboard.take() {
            // OSC 52: most modern terminals put this on the system clipboard
            print!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
            std::io::Write::flush(&mut stdout())?;
        }
        if app.has_pending() {
            // Network calls block; the busy status is already on screen
            app.run_pending();
            continue;
        }
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
        }
    }
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, &b| (n << 8) | b as u32) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"FROSTPASS:1:"), "RlJPU1RQQVNTOjE6");
    }
}
