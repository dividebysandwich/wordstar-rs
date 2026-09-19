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
    row(&mut out, "^QB / ^QK", "Beginning / End of marked block");
    row(&mut out, "^QP", "Previous position (before the last jump)");
    row(&mut out, "^K0-9", "Set place marker 0-9 (again: remove)");
    row(&mut out, "^Q0-9", "Go to place marker 0-9");
    row(&mut out, "Arrows", "Modern cursor movement");

    head(&mut out, "Files & program");
    row(&mut out, "^KS  / F2", "Save");
    row(&mut out, "^KD", "Save and close the document");
    row(&mut out, "^KX", "Save and exit");
    row(&mut out, "^KT", "Save As");
    row(&mut out, "^KR", "Insert another file at the cursor");
    row(&mut out, "^KP", "Export to PDF (a .docx name: Word)");
    row(&mut out, "^KQ  / F10", "Quit (asks to save changes)");
    row(&mut out, "F3  / ^OK", "Open the file browser");
    row(&mut out, "F5  / ^OP", "Toggle preview (graphical if supported)");
    row(&mut out, "F1  / ^J", "This help screen");

    head(&mut out, "Insert & utilities");
    row(&mut out, "^KR", "Insert file at cursor");
    row(&mut out, "^K?", "Word count (document, session, goal, block)");
    row(
        &mut out,
        ".pa / .cb",
        "Page break / column break (Insert menu)",
    );
    row(&mut out, "Layout menu", "Header / footer lines (.he / .fo)");
    row(&mut out, "Insert menu", "Manuscript Setup (standard manuscript PDF)");
    row(&mut out, "^QI", "Go to page");
    row(&mut out, "^QG", "Go to heading (chapter list)");

    head(&mut out, "Editing");
    row(&mut out, "^V", "Toggle insert / overtype");
    row(&mut out, "^N", "Insert a line, cursor stays");
    row(&mut out, "^G", "Delete character at cursor");
    row(&mut out, "^T", "Delete word");
    row(&mut out, "^Y", "Delete the whole line");
    row(&mut out, "^QY", "Delete to end of line");
    row(&mut out, "^Q Del", "Delete to start of line");
    row(&mut out, "^U", "Undo");

    head(&mut out, "Find & replace");
    row(&mut out, "^QF", "Find");
    row(&mut out, "^QA", "Find and replace (asks Y/N/A per match)");
    row(&mut out, "", "Options: U ignore case, W whole words,");
    row(&mut out, "", "B backwards, G whole document, N don't ask");
    row(&mut out, "^L", "Find next / continue replacing");

    head(&mut out, "Spelling");
    row(&mut out, "^QL", "Check spelling from the cursor");
    row(&mut out, "^QN", "Check the word at the cursor");
    row(&mut out, "^QJ", "Thesaurus for the word at the cursor");
    row(&mut out, "", "1-9 suggestion · I ignore · G ignore all");
    row(&mut out, "", "A add to your dictionary · T type · Esc");

    head(&mut out, "Blocks");
    row(&mut out, "^KB", "Mark block start (then move cursor)");
    row(&mut out, "^KK", "Mark block end (block stays marked)");
    row(&mut out, "^KC", "Copy marked block to the cursor");
    row(&mut out, "^KV", "Move marked block to the cursor");
    row(&mut out, "", "(no block: paste the block buffer)");
    row(&mut out, "^KY", "Delete block (kept in the buffer)");
    row(&mut out, "^KH", "Hide / redisplay the block");
    row(&mut out, "^KW", "Write the block to a file");
    row(&mut out, "^KZ", "Sort the block's lines");
    row(&mut out, "^K\" ^K' ^K.", "Block to UPPER / lower / Sentence case");

    head(&mut out, "Formatting (markdown)");
    row(&mut out, "^PB", "Bold  (**…**)");
    row(&mut out, "^PY", "Italic  (*…*)");
    row(&mut out, "^PS", "Underline  ([…]{.underline})");
    row(&mut out, "^PX", "Strikeout  (~~…~~)");
    row(&mut out, "^P=", "Font…");

    head(&mut out, "Onscreen format (^O)");
    row(&mut out, "^OD", "Hide / show formatting markup");
    row(&mut out, "^OW", "Toggle word wrap");
    row(&mut out, "^OR", "Right margin (wrap column)");
    row(&mut out, "^OC", "Center the line in print (.oc on/off)");
    row(&mut out, "^OL / ^O]", "Align on-screen text left / right");
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
