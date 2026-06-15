//! WordStar 7 color palette and shared styles.
//!
//! Classic WordStar for DOS: a blue editing canvas with light text, framed by
//! gray status/menu bars. We approximate that here with named terminal colors so
//! it renders well on both 16-color and truecolor terminals.

use ratatui::style::{Color, Modifier, Style};

/// Editing canvas background (the iconic WordStar blue).
pub const CANVAS_BG: Color = Color::Blue;
/// Editing canvas foreground (body text).
pub const CANVAS_FG: Color = Color::Gray;

/// Background for the gray chrome bars (title, ruler, status).
pub const BAR_BG: Color = Color::Gray;
/// Foreground for the gray chrome bars.
pub const BAR_FG: Color = Color::Black;

/// The editor text area style.
pub fn canvas() -> Style {
    Style::default().bg(CANVAS_BG).fg(CANVAS_FG)
}

/// The title bar style (top line).
pub fn title_bar() -> Style {
    Style::default().bg(BAR_BG).fg(BAR_FG)
}

/// The bottom status line style.
pub fn status_bar() -> Style {
    Style::default().bg(BAR_BG).fg(BAR_FG)
}

/// The pull-down menu bar style (File / Edit / View ...).
pub fn menu_bar() -> Style {
    Style::default().bg(CANVAS_BG).fg(Color::White)
}

/// The highlighted accelerator letter inside a menu title.
pub fn menu_hotkey() -> Style {
    Style::default()
        .bg(CANVAS_BG)
        .fg(Color::LightRed)
        .add_modifier(Modifier::BOLD)
}

/// A menu title that is currently selected on the bar.
pub fn menu_selected() -> Style {
    Style::default()
        .bg(Color::White)
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD)
}

/// The drop-down panel background.
pub fn menu_panel() -> Style {
    Style::default().bg(BAR_BG).fg(BAR_FG)
}

/// The highlighted item inside an open drop-down.
pub fn menu_panel_selected() -> Style {
    Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

/// The style bar (Body Text / Default font / B I U ...).
pub fn style_bar() -> Style {
    Style::default().bg(BAR_BG).fg(BAR_FG)
}

/// An *active* style-bar toggle (e.g. B when bold is on under the cursor).
#[allow(dead_code)] // wired up in Phase 3 (style detection under cursor)
pub fn style_bar_active() -> Style {
    Style::default()
        .bg(BAR_BG)
        .fg(Color::Blue)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
}

/// The ruler line (margins + tab stops).
pub fn ruler() -> Style {
    Style::default().bg(BAR_BG).fg(BAR_FG)
}

/// Text selection / marked block highlight inside the editor.
pub fn selection() -> Style {
    Style::default().bg(Color::Cyan).fg(Color::Black)
}

/// Search-match highlight.
pub fn search() -> Style {
    Style::default().bg(Color::Yellow).fg(Color::Black)
}
