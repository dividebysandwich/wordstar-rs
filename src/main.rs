//! wordstar-rs — a faithful DOS WordStar 7 clone for the terminal.
//!
//! Built on `ratatui` + `ratatui-textarea`. Edits markdown; will read original
//! WordStar binary files in a later phase. See the project plan for the roadmap.

mod app;
mod attributes;
mod browser;
mod commands;
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
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    );
    let result = run(&mut terminal, App::new(path)?);
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    result
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
