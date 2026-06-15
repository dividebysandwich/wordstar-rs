//! Rendering: the WordStar screen chrome plus the editing canvas.
//!
//! Layout, top to bottom: title bar, pull-down menu bar, style bar, ruler,
//! the editing canvas (the text widget), and the status line.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Mode};
use crate::theme;
use crate::{help, menu, preview};

/// Draw the whole screen for the current app state.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // menu
            Constraint::Length(1), // style bar
            Constraint::Length(1), // ruler
            Constraint::Min(1),    // editor
            Constraint::Length(1), // status
        ])
        .split(area);

    // Record geometry for mouse hit-testing.
    app.editor_area.set(rows[4]);
    app.menu_bar_area.set(rows[1]);

    title_bar(frame, rows[0], app);
    menu_bar(frame, rows[1], app);
    style_bar(frame, rows[2], app);
    ruler(frame, rows[3]);
    if app.mode == Mode::Clean {
        clean_pane(frame, rows[4], app);
    } else {
        frame.render_widget(&app.textarea, rows[4]);
    }
    status_bar(frame, rows[5], app);

    // Overlays on top of the editor, per mode.
    match app.mode {
        Mode::Editor | Mode::Clean => {}
        Mode::Menu => menu_dropdown(frame, area, app),
        Mode::Prompt => prompt_overlay(frame, area, app),
        Mode::Browser => browser_overlay(frame, area, app),
        Mode::Preview => preview_overlay(frame, area, app),
        Mode::Help => help_overlay(frame, area, app),
    }
}

/// Render the editor region as read-only formatted text (the "hide markup"
/// view toggled with `^OD`).
fn clean_pane(frame: &mut Frame, area: Rect, app: &App) {
    let lines = preview::render(&app.textarea.lines().join("\n"));
    let para = Paragraph::new(lines)
        .style(theme::canvas())
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0));
    frame.render_widget(para, area);
}

/// A rectangle centered in `area` with the given width/height (clamped).
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

fn prompt_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let rect = centered(area, 56, 3);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Command ")
        .style(theme::status_bar());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let line = Line::from(vec![
        Span::styled(
            format!("{} ", app.prompt.label),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.prompt.input.clone()),
        Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme::status_bar()), inner);
}

fn preview_overlay(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Preview — Esc/F5/q to close ")
        .style(theme::canvas());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = preview::render(&app.textarea.lines().join("\n"));
    let para = Paragraph::new(lines)
        .style(theme::canvas())
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0));
    frame.render_widget(para, inner);
}

fn help_overlay(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .style(theme::canvas());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let para = Paragraph::new(help::lines())
        .style(theme::canvas())
        .wrap(Wrap { trim: false })
        .scroll((app.help_scroll, 0));
    frame.render_widget(para, inner);
}

fn browser_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(browser) = app.browser.as_ref() else {
        return;
    };
    frame.render_widget(Clear, area);

    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" Open File — Enter to open/descend, Esc to cancel ")
        .style(theme::canvas());
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Header: path + item count.
    let header_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    let header = Line::from(vec![
        Span::styled(" Path: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(browser.cwd.display().to_string()),
        Span::raw("    "),
        Span::styled(
            browser.free_space_hint(),
            Style::default().fg(ratatui::style::Color::LightCyan),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header).style(theme::canvas()),
        header_rows[0],
    );

    // Multi-column listing.
    let list_area = header_rows[1];
    app.browser_list_area.set(list_area);
    let rows = list_area.height.max(1) as usize;
    browser.col_height.set(rows);
    let col_width = 26u16;
    let num_cols = (list_area.width / col_width).max(1) as usize;

    let mut col_constraints = Vec::new();
    for _ in 0..num_cols {
        col_constraints.push(Constraint::Length(col_width));
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(col_constraints)
        .split(list_area);

    // Which "page" of (num_cols * rows) entries is the selection on?
    let per_page = (rows * num_cols).max(1);
    let page = browser.selected / per_page;
    let page_start = page * per_page;

    for (col_idx, col_rect) in columns.iter().enumerate() {
        let mut lines: Vec<Line> = Vec::new();
        for row_idx in 0..rows {
            let idx = page_start + col_idx * rows + row_idx;
            let Some(entry) = browser.entries.get(idx) else {
                break;
            };
            let label = if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            };
            let size = if entry.is_dir {
                String::new()
            } else {
                human_size(entry.size)
            };
            let text = format!("{label:<18}{size:>6}");
            let style = if idx == browser.selected {
                theme::selection()
            } else if entry.is_dir {
                theme::canvas().add_modifier(Modifier::BOLD)
            } else {
                theme::canvas()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
        frame.render_widget(Paragraph::new(lines).style(theme::canvas()), *col_rect);
    }
}

/// Format a byte count compactly (e.g. `2.9k`, `35k`, `1.0M`).
fn human_size(bytes: u64) -> String {
    if bytes < 1000 {
        format!("{bytes}")
    } else if bytes < 1_000_000 {
        format!("{:.1}k", bytes as f64 / 1000.0)
    } else {
        format!("{:.1}M", bytes as f64 / 1_000_000.0)
    }
}

fn title_bar(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!("WordStar    {}", app.file_name());
    let p = Paragraph::new(title)
        .alignment(Alignment::Center)
        .style(theme::title_bar());
    frame.render_widget(p, area);
}

/// The leading-space offset before the first menu title.
const MENU_LEAD: u16 = 1;
/// Spaces rendered between menu titles.
const MENU_GAP: u16 = 3;

/// X offset of each menu title on the bar (Help is right-aligned).
pub fn menu_anchors(width: u16) -> Vec<u16> {
    let mut xs = vec![0u16; menu::MENUS.len()];
    let mut x = MENU_LEAD;
    for (i, m) in menu::MENUS.iter().enumerate() {
        if i == menu::HELP_INDEX {
            xs[i] = width.saturating_sub(6); // " Help "-ish, right side
        } else {
            xs[i] = x;
            x += m.title.chars().count() as u16 + MENU_GAP;
        }
    }
    xs
}

fn menu_bar(frame: &mut Frame, area: Rect, app: &App) {
    let in_menu = app.mode == Mode::Menu;
    let selected = app.menu.menu;

    // Left group of menus, and a right-aligned "Help".
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(6)])
        .split(area);

    let mut spans: Vec<Span> = vec![Span::styled(" ", theme::menu_bar())];
    for (i, m) in menu::MENUS.iter().enumerate() {
        if i == menu::HELP_INDEX {
            continue;
        }
        if in_menu && i == selected {
            spans.push(Span::styled(m.title.to_string(), theme::menu_selected()));
        } else {
            let mut chars = m.title.chars();
            let first: String = chars.next().map(|c| c.to_string()).unwrap_or_default();
            let rest: String = chars.collect();
            spans.push(Span::styled(first, theme::menu_hotkey()));
            spans.push(Span::styled(rest, theme::menu_bar()));
        }
        spans.push(Span::styled("   ", theme::menu_bar()));
    }
    let left = Paragraph::new(Line::from(spans)).style(theme::menu_bar());
    frame.render_widget(left, cols[0]);

    let help_style = if in_menu && selected == menu::HELP_INDEX {
        theme::menu_selected()
    } else {
        theme::menu_hotkey()
    };
    let help = Paragraph::new(Line::from(vec![
        Span::styled("H", help_style),
        Span::styled(
            "elp ",
            if in_menu && selected == menu::HELP_INDEX {
                theme::menu_selected()
            } else {
                theme::menu_bar()
            },
        ),
    ]))
    .alignment(Alignment::Right)
    .style(theme::menu_bar());
    frame.render_widget(help, cols[1]);
}

fn menu_dropdown(frame: &mut Frame, area: Rect, app: &App) {
    let sel = app.menu.menu;
    let menu = &menu::MENUS[sel];

    // Panel width from the widest "label    shortcut" row.
    let mut content_w = 0usize;
    for it in menu.items {
        let w = it.label.chars().count() + it.shortcut.chars().count() + 4;
        content_w = content_w.max(w);
    }
    let width = (content_w as u16 + 2).min(area.width); // +2 for borders
    let height = (menu.items.len() as u16 + 2).min(area.height);

    let anchors = menu_anchors(area.width);
    let mut x = anchors[sel];
    if x + width > area.width {
        x = area.width.saturating_sub(width);
    }
    let y = 2u16; // just below the menu bar (title row 0, menu row 1)
    let rect = Rect::new(x, y, width, height).intersection(area);
    app.dropdown_area.set(rect);

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .style(theme::menu_panel());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let item_w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(menu.items.len());
    for (i, it) in menu.items.iter().enumerate() {
        if matches!(it.action, menu::MenuAction::Separator) {
            lines.push(Line::from(Span::styled(
                "─".repeat(item_w),
                theme::menu_panel(),
            )));
            continue;
        }
        let pad = item_w.saturating_sub(it.label.chars().count() + it.shortcut.chars().count() + 2);
        let text = format!(" {}{}{} ", it.label, " ".repeat(pad), it.shortcut);
        let style = if i == app.menu.item {
            theme::menu_panel_selected()
        } else {
            theme::menu_panel()
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    frame.render_widget(Paragraph::new(lines).style(theme::menu_panel()), inner);
}

fn style_bar(frame: &mut Frame, area: Rect, app: &App) {
    let attrs = app.attributes_at_cursor();
    let (def_font, def_size) = app.document_defaults();
    let font = attrs.font.clone().unwrap_or(def_font);
    let size = attrs.size.unwrap_or(def_size);

    // Left: paragraph style + the run's font and size.
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(26)])
        .split(area);

    let left = Paragraph::new(Line::from(vec![
        Span::styled(" Body Text", theme::style_bar()),
        Span::styled("    ", theme::style_bar()),
        Span::styled(format!("{font}  {size}pt"), theme::style_bar()),
    ]))
    .style(theme::style_bar());
    frame.render_widget(left, cols[0]);

    // Right: B I U toggles (active per the run) + alignment L C R J.
    let toggle = |label: &str, active: bool| {
        let style = if active {
            theme::style_bar_active()
        } else {
            theme::style_bar()
        };
        Span::styled(format!(" {label} "), style)
    };
    use crate::app::AlignChoice;
    let align_letter = |label: &str, active: bool| {
        let style = if active {
            theme::style_bar_active()
        } else {
            theme::style_bar()
        };
        Span::styled(label.to_string(), style)
    };
    let right = Paragraph::new(Line::from(vec![
        toggle("B", attrs.bold),
        toggle("I", attrs.italic),
        toggle("U", attrs.underline),
        Span::styled("  ", theme::style_bar()),
        align_letter("L", app.align == AlignChoice::Left),
        Span::styled(" ", theme::style_bar()),
        align_letter("C", app.align == AlignChoice::Center),
        Span::styled(" ", theme::style_bar()),
        align_letter("R", app.align == AlignChoice::Right),
        Span::styled(" ", theme::style_bar()),
        align_letter("J", app.align == AlignChoice::Justify),
        Span::styled(" ", theme::style_bar()),
    ]))
    .alignment(Alignment::Right)
    .style(theme::style_bar());
    frame.render_widget(right, cols[1]);
}

/// Default ruler geometry until layout/dot-command parsing lands.
const RIGHT_MARGIN: usize = 65;
const TAB_EVERY: usize = 5;

fn ruler(frame: &mut Frame, area: Rect) {
    let width = area.width as usize;
    let mut line = String::with_capacity(width);
    for col in 0..width {
        let ch = if col == 0 {
            'L'
        } else if col == RIGHT_MARGIN.min(width.saturating_sub(1)) {
            'R'
        } else if col % TAB_EVERY == 0 {
            '!'
        } else {
            '-'
        };
        line.push(ch);
    }
    frame.render_widget(Paragraph::new(line).style(theme::ruler()), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(width: u16, height: u16) -> String {
        let app = App::new(None).unwrap();
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render_app(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn with_sample_doc() -> App {
        let mut app = App::new(None).unwrap();
        for l in [
            "# Heading",
            "",
            "Some **bold** and *italic* words.",
            "",
            "- one",
            "- two",
        ] {
            app.textarea.insert_str(l);
            app.textarea.insert_newline();
        }
        app
    }

    #[test]
    fn chrome_renders_all_bars() {
        let screen = render(80, 24);
        assert!(screen.contains("WordStar"), "title bar missing:\n{screen}");
        assert!(screen.contains("UNTITLED"), "file name missing");
        assert!(screen.contains("File"), "menu bar missing");
        assert!(screen.contains("Help"), "help menu missing");
        assert!(screen.contains("Body Text"), "style bar missing");
        assert!(screen.contains("Insert"), "status mode missing");
        assert!(screen.contains("L1"), "status line metric missing");
    }

    #[test]
    fn find_prompt_overlay_renders() {
        let mut app = with_sample_doc();
        app.start_find();
        let screen = render_app(&app, 80, 14);
        assert!(
            screen.contains("Command"),
            "prompt frame missing:\n{screen}"
        );
        assert!(screen.contains("Find:"), "find label missing");
    }

    #[test]
    fn preview_strips_markers_and_formats() {
        let mut app = with_sample_doc();
        app.toggle_preview();
        let screen = render_app(&app, 80, 16);
        assert!(screen.contains("Preview"), "preview frame missing");
        assert!(
            screen.contains("Some bold and italic words."),
            "inline not rendered:\n{screen}"
        );
        assert!(
            !screen.contains("**bold**"),
            "raw markers leaked into preview"
        );
        assert!(screen.contains("• one"), "list bullet missing");
    }

    #[test]
    fn help_overlay_lists_commands() {
        let mut app = App::new(None).unwrap();
        app.toggle_help();
        let screen = render_app(&app, 80, 16);
        assert!(screen.contains("Command Reference"), "help title missing");
        assert!(screen.contains("^E"), "diamond keys missing");
    }

    #[test]
    fn menu_dropdown_renders_items() {
        let mut app = App::new(None).unwrap();
        app.open_menu();
        app.menu.next_menu(); // File -> Edit
        let screen = render_app(&app, 80, 16);
        assert!(screen.contains("Undo"), "menu item missing:\n{screen}");
        assert!(screen.contains("Copy Block"), "block item missing");
        assert!(screen.contains("^KC"), "shortcut hint missing");
    }

    #[test]
    fn style_bar_shows_run_font_and_size() {
        let mut app = App::new(None).unwrap();
        app.textarea
            .insert_str("abc [hi]{font=\"Courier\" size=14} def");
        app.textarea.move_cursor(ratatui_textarea::CursorMove::Head);
        for _ in 0..5 {
            app.textarea
                .move_cursor(ratatui_textarea::CursorMove::Forward);
        }
        let screen = render_app(&app, 80, 8);
        assert!(screen.contains("Courier"), "font not reflected:\n{screen}");
        assert!(screen.contains("14pt"), "size not reflected");
    }

    #[test]
    fn style_bar_falls_back_to_defaults() {
        let app = App::new(None).unwrap();
        let screen = render_app(&app, 80, 8);
        assert!(screen.contains("Default"), "default font missing");
        assert!(screen.contains("12pt"), "default size missing");
    }

    #[test]
    fn clean_view_hides_markup() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("a **bold** word");
        app.toggle_markup();
        assert_eq!(app.mode, Mode::Clean);
        let screen = render_app(&app, 80, 10);
        assert!(screen.contains("bold"), "text missing:\n{screen}");
        assert!(!screen.contains("**bold**"), "markers not hidden");
    }

    fn rendered(app: &App, w: u16, h: u16) {
        let mut term = ratatui::Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
    }

    #[test]
    fn mouse_click_positions_cursor() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("line one");
        app.textarea.insert_newline();
        app.textarea.insert_str("line two");
        rendered(&app, 80, 24); // populate geometry + viewport
        // Editor pane starts at row 4; click first line, column 3.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.textarea.cursor(), (0, 3));
    }

    #[test]
    fn mouse_click_on_menu_bar_opens_menu() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::new(None).unwrap();
        rendered(&app, 80, 24);
        // "Edit" sits at anchor 8 on an 80-wide bar.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.mode, Mode::Menu);
        assert_eq!(app.menu.menu, 1); // Edit
    }
}

fn status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let m = app.cursor_metrics();
    let mode = if app.insert_mode {
        "Insert"
    } else {
        "Overtype"
    };
    let metrics = format!(
        "{mode}    P{}  L{}  V{:.2}\"  C{}  H{:.2}\" ",
        m.page, m.line, m.vertical_inches, m.column, m.horizontal_inches,
    );

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(metrics.len() as u16)])
        .split(area);

    let message = app.status_msg.clone().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(format!(" {message}")).style(theme::status_bar()),
        cols[0],
    );
    frame.render_widget(
        Paragraph::new(metrics)
            .alignment(Alignment::Right)
            .style(theme::status_bar()),
        cols[1],
    );
}
