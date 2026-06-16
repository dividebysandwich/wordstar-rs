//! Native terminal entry point for wordstar-rs.
//!
//! The editor itself lives in the library crate (`lib.rs`); this binary only
//! wires it to a real terminal via `ratatui` + `crossterm`. The browser build
//! uses `wasm::start` instead, so on `wasm32` this file collapses to an empty
//! `main` and pulls in none of the terminal stack.

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
use anyhow::Result;
#[cfg(not(target_arch = "wasm32"))]
use ratatui::DefaultTerminal;
#[cfg(not(target_arch = "wasm32"))]
use ratatui::crossterm::event::{self, Event};
#[cfg(not(target_arch = "wasm32"))]
use wordstar_rs::app::App;
#[cfg(not(target_arch = "wasm32"))]
use wordstar_rs::ui;

#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn run(terminal: &mut DefaultTerminal, mut app: App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        if app.preview_loading() {
            // Render the next slice of the graphical preview, redraw the progress
            // modal, and stay responsive to a cancel key without blocking.
            app.step_preview_job();
            if event::poll(std::time::Duration::ZERO)? {
                dispatch(&mut app, event::read()?);
            }
        } else {
            dispatch(&mut app, event::read()?);
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch(app: &mut App, event: Event) {
    match event {
        Event::Key(key) => app.handle_key(key),
        Event::Mouse(mouse) => app.handle_mouse(mouse),
        Event::Paste(text) => app.handle_paste(text),
        _ => {}
    }
}
