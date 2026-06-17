//! Rendering: the WordStar screen chrome plus the editing canvas.
//!
//! Layout, top to bottom: title bar, pull-down menu bar, style bar, ruler,
//! the editing canvas (the text widget), and the status line.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

#[cfg(not(target_arch = "wasm32"))]
use ratatui_image::{FilterType, Resize, StatefulImage};

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

    app.menu_bar_area.set(rows[1]);

    title_bar(frame, rows[0], app);
    menu_bar(frame, rows[1], app);
    style_bar(frame, rows[2], app);
    ruler(frame, rows[3], app);
    if app.mode == Mode::Clean {
        app.editor_area.set(rows[4]);
        clean_pane(frame, rows[4], app);
    } else {
        editor_pane(frame, rows[4], app);
    }
    status_bar(frame, rows[5], app);

    // Overlays on top of the editor, per mode.
    match app.mode {
        Mode::Editor | Mode::Clean => {}
        Mode::Menu => menu_dropdown(frame, area, app),
        Mode::Prompt => prompt_overlay(frame, area, app),
        Mode::Confirm => confirm_overlay(frame, area, app),
        // The in-app browser exists on native only; the browser build reaches
        // Open through the host file picker and never enters Browser mode.
        #[cfg(not(target_arch = "wasm32"))]
        Mode::Browser => browser_overlay(frame, area, app),
        #[cfg(target_arch = "wasm32")]
        Mode::Browser => {}
        Mode::Preview => preview_overlay(frame, area, app),
        Mode::Help => help_overlay(frame, area, app),
        Mode::Info => info_overlay(frame, area, app),
        Mode::Header => header_overlay(frame, area, app),
        Mode::Calculator => calc_overlay(frame, area, app),
    }
}

/// Render the editing canvas plus the WordStar right-border columns: a flag
/// column (`<` = paragraph break, blank = soft word-wrap continuation) and a
/// vertical scrollbar, both on a black background.
fn editor_pane(frame: &mut Frame, area: Rect, app: &App) {
    // The text widget only styles cells it draws into, so paint the whole canvas
    // WordStar-blue first; otherwise empty space shows the terminal's default bg.
    frame.render_widget(Block::default().style(theme::canvas()), area);

    // Reserve two right-hand columns: the flag column and the scrollbar.
    let reserve: u16 = if area.width >= 3 { 2 } else { 0 };
    let text_area = Rect {
        width: area.width - reserve,
        ..area
    };
    app.editor_area.set(text_area);
    frame.render_widget(&app.textarea, text_area);

    if reserve != 2 {
        return;
    }

    let height = text_area.height as usize;
    let (rows, top) = wrap_view(app, text_area);

    // Flag column (black background).
    let flag_style = Style::default()
        .bg(ratatui::style::Color::Black)
        .fg(ratatui::style::Color::LightCyan);
    let flag_lines: Vec<Line> = (0..height)
        .map(|y| {
            let ch = match rows.get(top + y) {
                Some(r) if r.last => '<', // hard return — paragraph break
                _ => ' ',                 // soft wrap continuation or past EOF
            };
            Line::from(Span::styled(ch.to_string(), flag_style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(flag_lines).style(flag_style),
        Rect::new(area.x + text_area.width, area.y, 1, area.height),
    );

    // Scrollbar column (black background).
    frame.render_widget(
        Paragraph::new(scrollbar_lines(rows.len(), top, height)),
        Rect::new(area.x + text_area.width + 1, area.y, 1, area.height),
    );
}

/// The wrapped visual-row layout and the index of the first visible row.
///
/// `screen_cursor().row` is the cursor's *absolute* visual-row index (across the
/// whole document), not viewport-relative, so it can't tell us the scroll offset
/// on its own. The widget derives its viewport top with a stateful "keep the
/// cursor in view" rule; we replicate it here, tracking the previous top in
/// `app.scroll_top`, so the scrollbar thumb mirrors the textarea's real scroll.
fn wrap_view(app: &App, text_area: Rect) -> (Vec<crate::wrap::VisualRow>, usize) {
    let rows = crate::wrap::layout(
        app.textarea.lines(),
        app.textarea.wrap_mode(),
        text_area.width as usize,
        app.textarea.tab_length(),
    );
    let cursor_row = app.textarea.screen_cursor().row;
    let height = text_area.height as usize;
    let top = next_scroll_top(app.scroll_top.get(), cursor_row, height);
    app.scroll_top.set(top);
    (rows, top)
}

/// Mirror of `ratatui-textarea`'s internal viewport scroll rule: keep `cursor`
/// within `[top, top + height)`, moving `top` only as far as needed.
fn next_scroll_top(prev_top: usize, cursor: usize, height: usize) -> usize {
    if cursor < prev_top {
        cursor
    } else if height > 0 && prev_top + height <= cursor {
        cursor + 1 - height
    } else {
        prev_top
    }
}

/// Build the scrollbar column: ↑ arrow, stippled track with a thumb, ↓ arrow.
fn scrollbar_lines(total: usize, top: usize, height: usize) -> Vec<Line<'static>> {
    use ratatui::style::Color;
    let black = Style::default().bg(Color::Black);
    let arrow = black.fg(Color::Gray);
    let track = black.fg(Color::DarkGray);
    let thumb = black.fg(Color::Gray);

    if height == 0 {
        return Vec::new();
    }
    if height <= 2 {
        return (0..height)
            .map(|_| Line::from(Span::styled("↕", arrow)))
            .collect();
    }

    let track_h = height - 2;
    let total = total.max(1);
    let view = height.min(total);
    let thumb_len = (((track_h * view) as f32 / total as f32).round() as usize).clamp(1, track_h);
    let max_top = total.saturating_sub(view);
    let thumb_pos = if max_top == 0 {
        0
    } else {
        (((track_h - thumb_len) as f32) * (top.min(max_top) as f32 / max_top as f32)).round()
            as usize
    };

    let mut lines = Vec::with_capacity(height);
    lines.push(Line::from(Span::styled("↑", arrow)));
    for t in 0..track_h {
        if t >= thumb_pos && t < thumb_pos + thumb_len {
            lines.push(Line::from(Span::styled("█", thumb)));
        } else {
            lines.push(Line::from(Span::styled("▒", track)));
        }
    }
    lines.push(Line::from(Span::styled("↓", arrow)));
    lines
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

fn confirm_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(c) = app.confirm.as_ref() else {
        return;
    };

    // Choices rendered as ASCII buttons, e.g. `[ Yes ]  [ No ]  [ Cancel ]`,
    // with the accelerator key highlighted.
    let buttons: &[(&str, &str)] = match c.action {
        crate::app::ConfirmAction::SaveBeforeQuit => &[("Y", "es"), ("N", "o"), ("C", "ancel")],
        _ => &[("Y", "es"), ("N", "o")],
    };
    let button_line = Line::from(ascii_buttons(buttons));

    let content_w = c.message.chars().count().max(button_line.width()).max(24);
    let width = (content_w + 4).min(area.width as usize) as u16;
    let rect = centered(area, width, 5);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm ")
        .style(theme::status_bar());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let body = vec![
        Line::from(Span::raw(c.message.clone())),
        Line::default(),
        button_line,
    ];
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Center)
            .style(theme::status_bar()),
        inner,
    );
}

/// A centered, dismissable information modal (e.g. Word Count).
fn info_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(info) = app.info.as_ref() else {
        return;
    };
    let content_w = info
        .lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(info.title.chars().count())
        .max(20);
    let width = (content_w + 4).min(area.width as usize) as u16;
    let height = (info.lines.len() + 4).min(area.height as usize) as u16;
    let rect = centered(area, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", info.title))
        .style(theme::status_bar());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut body: Vec<Line> = info.lines.iter().map(|l| Line::from(l.clone())).collect();
    body.push(Line::default());
    body.push(Line::from(Span::styled(
        "Press any key to close",
        Style::default().fg(ratatui::style::Color::DarkGray),
    )));
    frame.render_widget(Paragraph::new(body).style(theme::status_bar()), inner);
}

/// The header / footer dialog (text line + odd/even/both selection).
fn header_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use crate::app::{HeaderKind, HeaderPages};
    let Some(h) = app.header_dialog.as_ref() else {
        return;
    };
    let title = match h.kind {
        HeaderKind::Header => " Header ",
        HeaderKind::Footer => " Footer ",
    };
    let rect = centered(area, 54, 9);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(theme::status_bar());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let radio = |label: &str, on: bool| {
        let mark = if on { "(•)" } else { "( )" };
        Span::styled(
            format!("{mark} {label}   "),
            if on { bold } else { Style::default() },
        )
    };
    let body = vec![
        Line::from(vec![
            Span::styled("Line: ", bold),
            Span::raw(h.text.clone()),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ]),
        Line::default(),
        Line::from(vec![Span::styled("For pages:  ", bold)]),
        Line::from(vec![
            radio("Both", h.pages == HeaderPages::Both),
            radio("Odd", h.pages == HeaderPages::Odd),
            radio("Even", h.pages == HeaderPages::Even),
        ]),
        Line::default(),
        Line::from(Span::styled(
            "↑/↓ pages · Enter = OK · Esc = Cancel",
            Style::default().fg(ratatui::style::Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(body).style(theme::status_bar()), inner);
}

/// The calculator dialog: expression input, last result, and the valid-symbol
/// reference grid (mirrors WordStar 7's calculator).
fn calc_overlay(frame: &mut Frame, area: Rect, app: &App) {
    use ratatui::style::Color;
    let Some(calc) = app.calc.as_ref() else {
        return;
    };

    let rect = centered(area, 72, 18);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Calculator ")
        .style(theme::status_bar());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let sym = Style::default().fg(Color::Cyan);
    let dim = Style::default().fg(Color::DarkGray);

    // The expression input, shown on a black field with a blinking cursor.
    let field_w = inner.width.saturating_sub(2) as usize;
    let mut field = format!("{}_", calc.input);
    let chars = field.chars().count();
    if chars > field_w {
        field = field.chars().skip(chars - field_w).collect(); // keep the tail visible
    }
    let field_style = Style::default().bg(Color::Black).fg(Color::White);

    // One reference cell: a coloured symbol followed by its name.
    let cell = |s: &str, name: &str, name_w: usize| {
        vec![
            Span::styled(format!("{s:<3} "), sym),
            Span::raw(format!("{name:<name_w$}")),
        ]
    };
    let row = |c1: (&str, &str), c2: (&str, &str), c3: (&str, &str), c4: (&str, &str)| {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(cell(c1.0, c1.1, 11));
        spans.extend(cell(c2.0, c2.1, 14));
        spans.extend(cell(c3.0, c3.1, 13));
        spans.extend(cell(c4.0, c4.1, 8));
        Line::from(spans)
    };

    let body = vec![
        Line::from(Span::styled(
            "Enter Mathematical Expression to be Calculated:",
            bold,
        )),
        Line::from(Span::styled(format!(" {field:<field_w$} "), field_style)),
        Line::default(),
        Line::from(Span::styled("Result of Last Calculation:", bold)),
        Line::from(format!("  {}", calc.result)),
        Line::default(),
        Line::from(Span::styled("Valid Symbols:", bold)),
        row(
            ("+", "Add"),
            ("%", "Percent"),
            ("int", "Integer"),
            ("sin", "Sine"),
        ),
        row(
            ("-", "Subtract"),
            ("sqr", "Square Root"),
            ("log", "Base 10 Log"),
            ("cos", "Cosine"),
        ),
        row(
            ("*", "Multiply"),
            ("^", "Exponentiate"),
            ("ln", "Base e Log"),
            ("tan", "Tangent"),
        ),
        row(
            ("/", "Divide"),
            ("", ""),
            ("exp", "e^x"),
            ("atn", "Arc Tan"),
        ),
        Line::default(),
        Line::from(Span::styled(
            "Enter = OK (calculate) · Esc = Cancel",
            dim,
        )),
    ];
    frame.render_widget(Paragraph::new(body).style(theme::status_bar()), inner);
}

/// A centered modal showing graphical-preview rasterization progress.
fn loading_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let progress = app.preview_progress().clamp(0.0, 1.0);
    let pct = (progress * 100.0).round() as u32;

    let bar_w: usize = 30;
    let filled = (progress * bar_w as f32).round() as usize;
    let bar = format!(
        "[{}{}]",
        "#".repeat(filled),
        "-".repeat(bar_w.saturating_sub(filled))
    );

    let rect = centered(area, (bar_w as u16) + 10, 6);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Preview ")
        .style(theme::status_bar());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let body = vec![
        Line::from(Span::raw("Generating graphical preview…")),
        Line::default(),
        Line::from(Span::raw(format!("{bar} {pct}%"))),
        Line::from(Span::styled(
            "Esc to cancel",
            Style::default().fg(ratatui::style::Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Center)
            .style(theme::status_bar()),
        inner,
    );
}

/// Build a row of ASCII buttons like `[ Yes ]  [ No ]`, each `(hotkey, rest)`
/// with the hotkey letter emphasized.
fn ascii_buttons(buttons: &[(&str, &str)]) -> Vec<Span<'static>> {
    let key = Style::default()
        .fg(ratatui::style::Color::Blue)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    for (i, (hot, rest)) in buttons.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw("[ "));
        spans.push(Span::styled((*hot).to_string(), key));
        spans.push(Span::raw(format!("{rest} ]")));
    }
    spans
}

fn preview_overlay(frame: &mut Frame, area: Rect, app: &App) {
    if app.preview_loading() {
        loading_overlay(frame, area, app);
        return;
    }
    frame.render_widget(Clear, area);

    let graphical = !app.preview_pages.is_empty();
    let title = if graphical {
        format!(
            " Preview  page {}/{}  {:.0}%  —  PgUp/PgDn pages · +/- zoom · arrows · Esc ",
            app.preview_page + 1,
            app.preview_pages.len(),
            app.preview_zoom * 100.0,
        )
    } else {
        " Preview — Esc/F5/q to close ".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(theme::canvas());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.preview_area.set(inner);

    if graphical {
        // Native: build/encode only what this view needs (cached per page; the
        // zoom crop is re-encoded only when the view changes). `Scale` (not
        // `Fit`) so the page/crop fills the pane. `Lanczos3` downsamples the
        // high-resolution page with antialiasing, keeping glyph edges smooth.
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.ensure_preview(inner);
            let image =
                StatefulImage::default().resize(Resize::Scale(Some(FilterType::Lanczos3)));
            if app.preview_zoom <= 1.001 {
                let mut cache = app.preview_page_protocols.borrow_mut();
                if let Some(Some(state)) = cache.get_mut(app.preview_page) {
                    frame.render_stateful_widget(image, inner, state);
                }
            } else if let Some(state) = app.preview_zoom_protocol.borrow_mut().as_mut() {
                frame.render_stateful_widget(image, inner, state);
            }
        }
        // Browser: the page is painted by the canvas overlay (see
        // `wasm::canvas`), driven from the render loop and stacked above this
        // pane, so there is nothing to draw into the ratatui buffer here.
        #[cfg(target_arch = "wasm32")]
        let _ = inner;
    } else {
        // Text preview fallback.
        let lines = preview::render(&app.textarea.lines().join("\n"));
        let para = Paragraph::new(lines)
            .style(theme::canvas())
            .wrap(Wrap { trim: false })
            .scroll((app.preview_scroll, 0));
        frame.render_widget(para, inner);
    }
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

#[cfg(not(target_arch = "wasm32"))]
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

/// Format a byte count compactly (e.g. `2.9k`, `35k`, `1.0M`). Used by the
/// native file browser only.
#[cfg(not(target_arch = "wasm32"))]
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

    let anchors = menu_anchors(area.width);
    let mut x = anchors[sel];
    let width = panel_width(menu.items, area.width);
    if x + width > area.width {
        x = area.width.saturating_sub(width);
    }
    let y = 2u16; // just below the menu bar (title row 0, menu row 1)
    let rect = Rect::new(x, y, width, area.height).intersection(area);
    let rect = render_menu_panel(frame, rect, menu.items, app.menu.item, area);
    app.dropdown_area.set(rect);

    // The open submenu, anchored to the right of its parent item's row.
    if app.menu.sub_open
        && let Some(items) = app.menu.submenu_items()
    {
        let sub_w = panel_width(items, area.width);
        let mut sx = rect.x + rect.width;
        if sx + sub_w > area.width {
            sx = rect.x.saturating_sub(sub_w); // flip to the left edge
        }
        let sy = (rect.y + 1 + app.menu.item as u16).min(area.height.saturating_sub(1));
        let srect = Rect::new(sx, sy, sub_w, area.height.saturating_sub(sy)).intersection(area);
        let srect = render_menu_panel(frame, srect, items, app.menu.sub_item, area);
        app.sub_dropdown_area.set(srect);
    } else {
        app.sub_dropdown_area.set(Rect::ZERO);
    }
}

/// Width of a drop-down panel from the widest "label  shortcut" row (a submenu
/// item reserves space for the `▶` marker).
fn panel_width(items: &[menu::MenuItem], max: u16) -> u16 {
    let mut content_w = 0usize;
    for it in items {
        let extra = if matches!(it.action, menu::MenuAction::Submenu(_)) {
            2
        } else {
            0
        };
        let w = it.label.chars().count() + it.shortcut.chars().count() + 4 + extra;
        content_w = content_w.max(w);
    }
    (content_w as u16 + 2).min(max)
}

/// Render one drop-down panel and return its (clamped) rectangle.
fn render_menu_panel(
    frame: &mut Frame,
    rect: Rect,
    items: &[menu::MenuItem],
    selected: usize,
    area: Rect,
) -> Rect {
    let height = (items.len() as u16 + 2).min(area.height);
    let rect = Rect { height, ..rect }.intersection(area);

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .style(theme::menu_panel());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let item_w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        if matches!(it.action, menu::MenuAction::Separator) {
            lines.push(Line::from(Span::styled(
                "─".repeat(item_w),
                theme::menu_panel(),
            )));
            continue;
        }
        // Submenu items show a right-pointing marker instead of a shortcut.
        let right = if matches!(it.action, menu::MenuAction::Submenu(_)) {
            "▶".to_string()
        } else {
            it.shortcut.to_string()
        };
        let pad = item_w.saturating_sub(it.label.chars().count() + right.chars().count() + 2);
        let text = format!(" {}{}{} ", it.label, " ".repeat(pad), right);
        let style = if i == selected {
            theme::menu_panel_selected()
        } else {
            theme::menu_panel()
        };
        lines.push(Line::from(Span::styled(text, style)));
    }
    frame.render_widget(Paragraph::new(lines).style(theme::menu_panel()), inner);
    rect
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

fn ruler_char(col: usize, width: usize) -> char {
    if col == 0 {
        'L'
    } else if col == RIGHT_MARGIN.min(width.saturating_sub(1)) {
        'R'
    } else if col.is_multiple_of(TAB_EVERY) {
        '!'
    } else {
        '-'
    }
}

fn ruler(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let base: Vec<char> = (0..width).map(|c| ruler_char(c, width)).collect();

    // Cursor position on the ruler, counted in printed columns (markers ignored).
    let cursor = app.textarea.cursor();
    let line = app
        .textarea
        .lines()
        .get(cursor.0)
        .map(String::as_str)
        .unwrap_or("");
    let indicator = crate::attributes::visible_column(line, cursor.1).min(width - 1);

    let style = theme::ruler();
    let mark = Style::default()
        .bg(ratatui::style::Color::Blue)
        .fg(ratatui::style::Color::White)
        .add_modifier(Modifier::BOLD);

    let before: String = base[..indicator].iter().collect();
    let at: String = std::iter::once('▮').collect();
    let after: String = base[(indicator + 1).min(width)..].iter().collect();

    let row = Line::from(vec![
        Span::styled(before, style),
        Span::styled(at, mark),
        Span::styled(after, style),
    ]);
    frame.render_widget(Paragraph::new(row).style(style), area);
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
        assert!(
            screen.contains("Mark Block Beginning"),
            "block item missing"
        );
        assert!(screen.contains("^KC"), "shortcut hint missing");
    }

    #[test]
    fn submenu_renders_marker_and_panel() {
        let mut app = App::new(None).unwrap();
        app.open_menu();
        assert!(app.menu.jump_to_title('l')); // Layout
        let screen = render_app(&app, 80, 20);
        assert!(
            screen.contains("Headers/Footers"),
            "submenu parent missing:\n{screen}"
        );
        assert!(screen.contains("▶"), "submenu marker missing:\n{screen}");

        // Open the submenu and confirm the second panel shows its leaves.
        while app.menu.submenu_items().is_none() {
            app.menu.next_item();
        }
        app.menu.move_right();
        let screen = render_app(&app, 80, 20);
        assert!(
            screen.contains("Header..."),
            "submenu leaf missing:\n{screen}"
        );
        assert!(
            screen.contains("Footer..."),
            "submenu leaf missing:\n{screen}"
        );
    }

    #[test]
    fn header_dialog_overlay_renders() {
        let mut app = App::new(None).unwrap();
        app.start_header(crate::app::HeaderKind::Header);
        let screen = render_app(&app, 80, 20);
        assert!(screen.contains("Header"), "title missing:\n{screen}");
        assert!(screen.contains("For pages"), "pages row missing:\n{screen}");
        assert!(screen.contains("Both"), "radio missing:\n{screen}");
    }

    #[test]
    fn calculator_overlay_renders_and_computes() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(None).unwrap();
        app.start_calculator();
        let screen = render_app(&app, 80, 24);
        assert!(screen.contains("Calculator"), "title missing:\n{screen}");
        assert!(screen.contains("Valid Symbols"), "symbols missing:\n{screen}");
        assert!(screen.contains("Square Root"), "Square Root missing:\n{screen}");
        assert!(screen.contains("Arc Tan"), "Arc Tan missing:\n{screen}");
        assert!(
            screen.contains("Result of Last Calculation"),
            "result label missing:\n{screen}"
        );

        // Type "2^10" and press Enter → result becomes 1024.
        for c in "2^10".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.calc.as_ref().unwrap().result, "1024");

        // Esc closes the dialog.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, crate::app::Mode::Editor);
    }

    #[test]
    fn word_count_modal_renders() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("hello world");
        app.show_word_count();
        let screen = render_app(&app, 80, 20);
        assert!(screen.contains("Word Count"), "title missing:\n{screen}");
        assert!(screen.contains("Words:"), "stat missing:\n{screen}");
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

    #[test]
    fn confirm_modal_renders_message() {
        use crate::app::{ConfirmAction, ConfirmState};
        let mut app = App::new(None).unwrap();
        app.confirm = Some(ConfirmState {
            message: "out.pdf already exists. Overwrite?".into(),
            action: ConfirmAction::OverwritePdf(std::path::PathBuf::from("out.pdf")),
        });
        app.mode = Mode::Confirm;
        let screen = render_app(&app, 80, 16);
        assert!(screen.contains("Confirm"), "modal frame missing:\n{screen}");
        assert!(screen.contains("Overwrite?"), "message missing");
        assert!(
            screen.contains("[ Yes ]") && screen.contains("[ No ]"),
            "ASCII buttons missing:\n{screen}"
        );
    }

    #[test]
    fn quit_modal_has_three_ascii_buttons() {
        use crate::app::{ConfirmAction, ConfirmState};
        let mut app = App::new(None).unwrap();
        app.confirm = Some(ConfirmState {
            message: "Save changes before quitting?".into(),
            action: ConfirmAction::SaveBeforeQuit,
        });
        app.mode = Mode::Confirm;
        let screen = render_app(&app, 80, 16);
        assert!(screen.contains("[ Yes ]"), "Yes button missing:\n{screen}");
        assert!(screen.contains("[ No ]"), "No button missing");
        assert!(screen.contains("[ Cancel ]"), "Cancel button missing");
    }

    #[test]
    fn loading_modal_shows_progress() {
        let mut app = App::new(None).unwrap();
        let Some(job) = crate::gfx::Job::new(&"para\n\n".repeat(60)) else {
            return; // no system fonts in this environment
        };
        app.preview_job = Some(job);
        app.mode = Mode::Preview;
        let screen = render_app(&app, 80, 20);
        assert!(
            screen.contains("Generating graphical preview"),
            "loading title missing:\n{screen}"
        );
        assert!(screen.contains('%'), "percent missing");
        assert!(
            screen.contains('[') && screen.contains(']'),
            "progress bar missing"
        );
    }

    fn ruler_indicator_col(app: &App, w: u16) -> usize {
        let backend = TestBackend::new(w, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..w)
            .position(|x| buf[(x, 3)].symbol() == "▮")
            .expect("ruler indicator present")
    }

    #[test]
    fn ruler_indicator_ignores_markers() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("**bold** text");
        app.textarea.move_cursor(ratatui_textarea::CursorMove::Head);
        for _ in 0..6 {
            app.textarea
                .move_cursor(ratatui_textarea::CursorMove::Forward);
        }
        // Raw column 6 is inside `**bold**`; printed column is 4.
        assert_eq!(ruler_indicator_col(&app, 40), 4);

        app.textarea.move_cursor(ratatui_textarea::CursorMove::End);
        // "bold text" is 9 printed characters.
        assert_eq!(ruler_indicator_col(&app, 40), 9);
    }

    #[test]
    fn flag_column_marks_paragraphs_vs_wraps() {
        let mut app = App::new(None).unwrap();
        app.textarea
            .insert_str("This is a fairly long line that will wrap across rows");
        app.textarea.insert_newline();
        app.textarea.insert_str("Short");
        app.textarea.move_cursor(ratatui_textarea::CursorMove::Top);

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        // Columns (width 40): text 0..38, flag at 38, scrollbar at 39.
        // The editor starts at y = 4.
        let flag = |y: u16| buf[(38, y)].symbol().chars().next().unwrap();
        assert_eq!(flag(4), ' ', "wrapped continuation row has no flag");
        assert_eq!(flag(5), '<', "paragraph end (hard return) is flagged");
        assert_eq!(flag(6), '<', "second paragraph end is flagged");
        assert_eq!(flag(7), ' ', "past end of document is blank");
    }

    #[test]
    fn scrollbar_has_arrows_and_thumb() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("a line");
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        // Scrollbar is the rightmost column (x = 39); editor rows are y = 4..=8.
        let sym = |y: u16| buf[(39, y)].symbol().to_string();
        assert_eq!(sym(4), "↑", "up arrow at top");
        assert_eq!(sym(8), "↓", "down arrow at bottom");
        // The whole short document fits, so the thumb fills the track.
        assert_eq!(
            sym(5),
            "█",
            "thumb fills the track when nothing is scrolled"
        );
        // Black background behind the flag/scrollbar columns.
        assert_eq!(buf[(39, 4)].bg, ratatui::style::Color::Black);
        assert_eq!(buf[(38, 4)].bg, ratatui::style::Color::Black);
    }

    #[test]
    fn scrollbar_thumb_tracks_scroll_position() {
        use ratatui_textarea::CursorMove;
        let mut app = App::new(None).unwrap();
        for i in 0..60 {
            app.textarea.insert_str(format!("line {i}"));
            app.textarea.insert_newline();
        }
        // Editor rows are y = 4..=8; the track (between the arrows) is y = 5..=7.
        let thumb_y = |app: &App| -> Option<u16> {
            let backend = TestBackend::new(40, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (5..=7).find(|&y| buf[(39, y)].symbol() == "█")
        };

        // Cursor at the very top: thumb sits at the top of the track.
        app.textarea.move_cursor(CursorMove::Top);
        assert_eq!(
            thumb_y(&app),
            Some(5),
            "thumb at top when scrolled to start"
        );

        // Cursor at the very bottom: thumb moves to the bottom of the track.
        app.textarea.move_cursor(CursorMove::Bottom);
        app.textarea.move_cursor(CursorMove::End);
        assert_eq!(
            thumb_y(&app),
            Some(7),
            "thumb at bottom when scrolled to end"
        );
    }

    #[test]
    fn scrollbar_thumb_follows_page_down() {
        use crate::commands::{Command, execute};
        use ratatui_textarea::CursorMove;
        let mut app = App::new(None).unwrap();
        for i in 0..60 {
            app.textarea.insert_str(format!("line {i}"));
            app.textarea.insert_newline();
        }
        app.textarea.move_cursor(CursorMove::Top);

        let thumb_y = |app: &App| -> Option<u16> {
            let backend = TestBackend::new(40, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (5..=7).find(|&y| buf[(39, y)].symbol() == "█")
        };

        // First render establishes the viewport height; thumb starts at the top.
        assert_eq!(thumb_y(&app), Some(5), "thumb at top before paging");

        // Paging down repeatedly must walk the thumb toward the bottom — the bug
        // was that explicit scrolls left it pinned in place.
        for _ in 0..30 {
            execute(&mut app, Command::PageDown);
        }
        assert_eq!(
            thumb_y(&app),
            Some(7),
            "thumb reaches the bottom after paging down"
        );

        // And paging back up returns it to the top.
        for _ in 0..30 {
            execute(&mut app, Command::PageUp);
        }
        assert_eq!(
            thumb_y(&app),
            Some(5),
            "thumb returns to top after paging up"
        );
    }

    #[test]
    fn scrollbar_thumb_follows_pagedown_key() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(None).unwrap();
        for i in 0..60 {
            app.textarea.insert_str(format!("line {i}"));
            app.textarea.insert_newline();
        }
        app.textarea.move_cursor(ratatui_textarea::CursorMove::Top);

        let thumb_y = |app: &App| -> Option<u16> {
            let backend = TestBackend::new(40, 10);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (5..=7).find(|&y| buf[(39, y)].symbol() == "█")
        };

        // Establish the viewport height; thumb starts at the top.
        assert_eq!(thumb_y(&app), Some(5), "thumb at top before paging");

        // The PageDown *key* scrolls via `TextArea::input`, a different path than
        // the ^C command — it must still drive the thumb. This was the bug.
        for _ in 0..30 {
            app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        }
        assert_eq!(
            thumb_y(&app),
            Some(7),
            "thumb reaches the bottom after PageDown key"
        );

        for _ in 0..30 {
            app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        }
        assert_eq!(
            thumb_y(&app),
            Some(5),
            "thumb returns to top after PageUp key"
        );
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
