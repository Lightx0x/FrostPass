//! The terminal UI: one tab per actor (organizer, minter, buyer, scanner) over the same wallet,
//! plus the wallet's transaction log.

mod app;
mod form;
mod theme;
mod view;

use std::{
    io::{stdout, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Result;
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyEventKind,
    },
    execute,
};
use solana_keypair::Keypair;

pub use app::{App, Hit, HitTarget, Pane, Tab};
pub use view::draw;

use crate::chain::Chain;

/// How often the screen redraws without input: countdowns (door passes, sales end) and the
/// logo's glow. Only changed cells are sent to the terminal, so this stays cheap.
const TICK: Duration = Duration::from_millis(80);

/// Runs the app; `data_dir` keeps the transaction log and event order between runs.
pub fn run(
    chain: impl Chain,
    wallet: Keypair,
    cluster: String,
    data_dir: Option<&Path>,
    animate: bool,
) -> Result<()> {
    let mut app = App::new(chain, wallet, cluster);
    if !animate {
        app = app.without_animation();
    }
    if let Some(dir) = data_dir {
        app = app.with_store(dir);
    }
    let mut terminal = ratatui::init();
    // The mouse clicks and scrolls panes; hold Shift to select text with it instead
    execute!(stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let result = run_app(&mut terminal, app);
    let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    result
}

fn run_app(terminal: &mut ratatui::DefaultTerminal, mut app: App<impl Chain>) -> Result<()> {
    while !app.quit {
        terminal.draw(|frame| view::draw(frame, &app))?;
        if let Some((what, text)) = app.clipboard.take() {
            // OSC 52: most terminals (and tmux) take this; some, like GNOME Terminal, ignore it,
            // so the desktop's clipboard command gets it too
            print!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
            stdout().flush()?;
            app.copied(what, system_clipboard(&text));
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
                Event::Mouse(mouse) => app.on_mouse(mouse),
                _ => {}
            }
        } else if app.has_idle_work() {
            app.run_idle();
        }
    }
    Ok(())
}

/// Copies with the desktop's clipboard command, if there is one: `wl-copy` on Wayland, `xclip`
/// or `xsel` on X11, `pbcopy` on macOS. Returns the command that took it.
fn system_clipboard(text: &str) -> Option<&'static str> {
    let mut commands: Vec<(&'static str, &[&str])> = Vec::new();
    if cfg!(target_os = "macos") {
        commands.push(("pbcopy", &[]));
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        commands.push(("wl-copy", &[]));
    }
    if std::env::var_os("DISPLAY").is_some() {
        commands.push(("xclip", &["-selection", "clipboard"]));
        commands.push(("xsel", &["--clipboard", "--input"]));
    }
    commands
        .into_iter()
        .find(|(command, args)| pipe_to(command, args, text))
        .map(|(command, _)| command)
}

/// Runs `command` with `text` on its stdin; true if it took it. Never blocks the UI for long:
/// xclip and wl-copy fork a background process that keeps serving the clipboard.
fn pipe_to(command: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return written && status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            // Still running: it has the text and is serving it
            Ok(None) => return written,
            Err(_) => return false,
        }
    }
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
