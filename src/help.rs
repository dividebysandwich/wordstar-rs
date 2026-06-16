//! Static help content: a WordStar command reference for the F1 overlay.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Build the help screen as styled lines.
pub fn lines() -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    let head = |out: &mut Vec<Line<'static>>, title: &str| {
        out.push(Line::default());
        out.push(Line::from(Span::styled(
            title.to_string(),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
    };
    let row = |out: &mut Vec<Line<'static>>, keys: &str, desc: &str| {
        out.push(Line::from(vec![
            Span::styled(
                format!("  {keys:<14}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(desc.to_string(), Style::default().fg(Color::Gray)),
        ]));
    };

    out.push(Line::from(Span::styled(
        "WordStar-rs — Command Reference".to_string(),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    out.push(Line::from(Span::styled(
        "Press Esc, F1 or q to close.  Arrows / PgUp / PgDn to scroll.".to_string(),
        Style::default().fg(Color::DarkGray),
    )));

    head(&mut out, "Cursor movement (the WordStar diamond)");
    row(&mut out, "^E / ^X", "Up / Down a line");
    row(&mut out, "^S / ^D", "Left / Right a character");
    row(&mut out, "^A / ^F", "Left / Right a word");
    row(&mut out, "^R / ^C", "Page up / Page down");
    row(&mut out, "^W / ^Z", "Scroll up / down one line");
    row(&mut out, "^QS / ^QD", "Start / End of line");
    row(&mut out, "^QR / ^QC", "Start / End of document");
    row(&mut out, "Arrows", "Modern cursor movement");

    head(&mut out, "Files & program");
    row(&mut out, "^KS  / F2", "Save");
    row(&mut out, "^KD", "Save and keep editing");
    row(&mut out, "^KX", "Save and exit");
    row(&mut out, "^KP", "Export to PDF");
    row(&mut out, "^KQ  / F10", "Quit (abandon changes)");
    row(&mut out, "F3", "Open the file browser");
    row(&mut out, "F5", "Toggle the formatted preview");
    row(&mut out, "F1  / ^J", "This help screen");

    head(&mut out, "Editing");
    row(&mut out, "^V", "Toggle insert / overtype");
    row(&mut out, "^N", "Insert a line, cursor stays");
    row(&mut out, "^G", "Delete character at cursor");
    row(&mut out, "^T", "Delete word");
    row(&mut out, "^Y", "Delete line");
    row(&mut out, "^QY", "Delete to end of line");
    row(&mut out, "^Q Del", "Delete to start of line");
    row(&mut out, "^U", "Undo");

    head(&mut out, "Find & replace");
    row(&mut out, "^QF", "Find");
    row(&mut out, "^QA", "Find and replace");
    row(&mut out, "^L", "Find next");

    head(&mut out, "Blocks");
    row(&mut out, "^KB", "Mark block start");
    row(&mut out, "^KK", "Mark block end (copy to buffer)");
    row(&mut out, "^KC", "Copy block to buffer");
    row(&mut out, "^KV", "Paste block at cursor");
    row(&mut out, "^KY", "Delete block");
    row(&mut out, "^KH", "Hide block markers");

    head(&mut out, "Formatting (markdown)");
    row(&mut out, "^PB", "Bold  (**…**)");
    row(&mut out, "^PY", "Italic  (*…*)");
    row(&mut out, "^PS", "Underline  ([…]{.underline})");
    row(&mut out, "^PX", "Strikeout  (~~…~~)");

    head(&mut out, "Onscreen format (^O)");
    row(&mut out, "^OD", "Hide / show formatting markup");
    row(&mut out, "^OC", "Center the paragraph");
    row(&mut out, "^OL / ^OR", "Align left / right");
    row(&mut out, "^OJ", "Justify");

    head(&mut out, "Mouse");
    row(&mut out, "Click", "Position the cursor");
    row(&mut out, "Drag", "Mark a block (select text)");
    row(&mut out, "Double-click", "Select the word");
    row(
        &mut out,
        "Click menu",
        "Open a menu; click an item to run it",
    );
    row(&mut out, "Wheel", "Scroll the document");

    head(&mut out, "Markdown the preview renders (F5 / ^OD)");
    row(&mut out, "# …", "Headings");
    row(&mut out, "- / 1.", "Bullet / numbered lists");
    row(&mut out, "- [x]", "Task lists");
    row(&mut out, "| a | b |", "Tables (GitHub style)");
    row(&mut out, "[t](url)", "Links and ![alt](url) images");
    row(&mut out, "> …", "Block quotes");
    row(&mut out, "```", "Fenced code blocks");

    out.push(Line::default());
    out
}
