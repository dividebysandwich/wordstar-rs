//! wordstar-rs — a faithful DOS WordStar 7 clone for the terminal.
//!
//! Built on `ratatui` + `ratatui-textarea`. Edits markdown; will read original
//! WordStar binary files in a later phase. See the project plan for the roadmap.

mod app;
mod attributes;
mod browser;
mod commands;
mod gfx;
mod help;
mod keymap;
mod menu;
mod pdf;
mod preview;
mod theme;
mod ui;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

use app::App;

fn main() -> Result<()> {
    let path = std::env::args().nth(1);
    let mut terminal = ratatui::init();
    let mut app = App::new(path)?;
    // Detect terminal graphics support before enabling mouse capture, so the
    // protocol query/response isn't disturbed by mouse reports. The query blocks
    // up to ~2s waiting for a reply, so only run it on terminals that are likely
    // to answer; everywhere else we skip straight to the text preview.
    if graphics_terminal_likely()
        && let Ok(picker) = ratatui_image::picker::Picker::from_query_stdio()
    {
        app.set_picker(picker);
    }
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    );
    let result = run(&mut terminal, app);
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    result
}

/// Heuristic: does the environment look like a terminal that supports an inline
/// graphics protocol (Kitty / iTerm2 / Sixel)? Used to avoid the ~2s graphics
/// capability query on terminals that would never answer it.
fn graphics_terminal_likely() -> bool {
    use std::env::var;
    if var("KITTY_WINDOW_ID").is_ok() || var("KONSOLE_VERSION").is_ok() {
        return true;
    }
    let term = var("TERM").unwrap_or_default().to_lowercase();
    if term.contains("kitty")
        || term.contains("ghostty")
        || term.contains("sixel")
        || term.contains("wezterm")
        || term.starts_with("foot")
    {
        return true;
    }
    let prog = var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    matches!(
        prog.as_str(),
        "iterm.app" | "wezterm" | "ghostty" | "rio" | "konsole"
    )
}

fn run(terminal: &mut DefaultTerminal, mut app: App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        match event::read()? {
            Event::Key(key) => app.handle_key(key),
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            Event::Paste(text) => app.handle_paste(text),
            _ => {}
        }
    }
    Ok(())
}
