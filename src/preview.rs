//! A read-only markdown renderer: parses the buffer with `pulldown-cmark` and
//! produces styled ratatui lines for the preview overlay and the `^OD` view.
//!
//! WordStar never understood tables, task lists, links, fenced code, and the
//! like — but a writer can type that Markdown by hand and see it rendered here.
//! Supported: headings, bold/italic/strikethrough, inline + fenced code,
//! bullet/ordered/task lists, block quotes, rules, links, images, and GFM
//! tables.

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render markdown source into styled lines.
///
/// The source is first run through [`crate::attributes::prepare_render_source`],
/// which drops WordStar dot commands and rewrites pandoc attribute spans (so
/// `[text]{.underline}` renders underlined rather than showing its markers).
pub fn render(source: &str) -> Vec<Line<'static>> {
    let prepared = crate::attributes::prepare_render_source(source);
    let mut r = Renderer::default();
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(&prepared, options) {
        r.handle(event);
    }
    r.flush_line();
    if r.lines.is_empty() {
        r.lines.push(Line::default());
    }
    r.lines
}

#[derive(Default)]
struct Renderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    /// Active inline modifiers (depth counters so nesting works).
    bold: u32,
    italic: u32,
    strike: u32,
    underline: u32,
    link: u32,
    /// Heading level currently open, if any.
    heading: Option<HeadingLevel>,
    /// Inside a fenced/indented code block.
    in_code_block: bool,
    /// Stack of open lists: `Some(next_number)` for ordered, `None` for bullets.
    list_stack: Vec<Option<u64>>,
    /// Inside a block quote.
    in_blockquote: bool,
    /// URLs of open links, to print after the link text.
    link_urls: Vec<String>,
    /// Table being assembled, if inside one.
    table: Option<TableBuilder>,
}

impl Renderer {
    fn handle(&mut self, event: Event) {
        // Table cells are buffered as plain text and laid out on table end.
        if self.table.is_some() {
            self.handle_table_event(event);
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => {
                let style = Style::default().fg(Color::LightGreen);
                self.current.push(Span::styled(code.to_string(), style));
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.flush_line();
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(40),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Event::TaskListMarker(checked) => self.push_task_marker(checked),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.flush_line(),
            Tag::Heading { level, .. } => {
                self.flush_line();
                self.heading = Some(level);
            }
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.link += 1;
                self.link_urls.push(dest_url.to_string());
            }
            Tag::Image { dest_url, .. } => {
                self.current
                    .push(Span::styled("🖼 ", Style::default().fg(Color::Magenta)));
                self.link += 1; // style the alt text like a link
                self.link_urls.push(dest_url.to_string());
            }
            Tag::CodeBlock(_) => {
                self.flush_line();
                self.in_code_block = true;
            }
            Tag::List(first) => self.list_stack.push(first),
            Tag::Item => {
                self.flush_line();
                let indent = "  ".repeat(self.list_stack.len().saturating_sub(1));
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "• ".to_string(),
                };
                self.current.push(Span::styled(
                    format!("{indent}  {marker}"),
                    Style::default().fg(Color::Yellow),
                ));
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.in_blockquote = true;
            }
            Tag::Table(aligns) => {
                self.flush_line();
                self.table = Some(TableBuilder::new(aligns));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line();
                self.lines.push(Line::default());
            }
            TagEnd::Heading(_) => {
                self.flush_line();
                self.heading = None;
                self.lines.push(Line::default());
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.link = self.link.saturating_sub(1);
                if let Some(url) = self.link_urls.pop()
                    && !url.is_empty()
                {
                    self.current.push(Span::styled(
                        format!(" ({url})"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
            TagEnd::CodeBlock => {
                self.flush_line();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.lines.push(Line::default());
                }
            }
            TagEnd::Item => self.flush_line(),
            TagEnd::BlockQuote(_) => {
                self.in_blockquote = false;
                self.lines.push(Line::default());
            }
            _ => {}
        }
    }

    /// Handle one event while assembling a table.
    fn handle_table_event(&mut self, event: Event) {
        let Some(tb) = self.table.as_mut() else {
            return;
        };
        match event {
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => tb.row.clear(),
            Event::Start(Tag::TableCell) => tb.cell.clear(),
            Event::End(TagEnd::TableCell) => {
                let cell = std::mem::take(&mut tb.cell);
                tb.row.push(cell);
            }
            Event::End(TagEnd::TableHead) => {
                tb.header = std::mem::take(&mut tb.row);
            }
            Event::End(TagEnd::TableRow) => {
                let row = std::mem::take(&mut tb.row);
                tb.body.push(row);
            }
            Event::End(TagEnd::Table) => {
                let tb = self.table.take().unwrap();
                self.lines.extend(tb.render());
                self.lines.push(Line::default());
            }
            Event::Text(t) | Event::Code(t) => tb.cell.push_str(&t),
            Event::SoftBreak | Event::HardBreak => tb.cell.push(' '),
            _ => {}
        }
    }

    fn push_task_marker(&mut self, checked: bool) {
        let (mark, color) = if checked {
            ("[x] ", Color::LightGreen)
        } else {
            ("[ ] ", Color::Yellow)
        };
        // Replace the plain bullet the list item already emitted, if present.
        if let Some(last) = self.current.last_mut()
            && last.content.ends_with("• ")
        {
            let indent = last.content.trim_end_matches("• ").to_string();
            *last = Span::styled(format!("{indent}{mark}"), Style::default().fg(color));
            return;
        }
        self.current
            .push(Span::styled(mark, Style::default().fg(color)));
    }

    fn push_text(&mut self, text: &str) {
        if self.in_code_block {
            for (i, part) in text.split('\n').enumerate() {
                if i > 0 {
                    self.flush_line();
                }
                if !part.is_empty() {
                    self.current.push(Span::styled(
                        part.to_string(),
                        Style::default().fg(Color::LightGreen),
                    ));
                }
            }
            return;
        }
        // Split on the underline sentinels, toggling underline between runs.
        use crate::attributes::{UNDERLINE_END, UNDERLINE_START};
        let mut buf = String::new();
        for ch in text.chars() {
            match ch {
                UNDERLINE_START | UNDERLINE_END => {
                    if !buf.is_empty() {
                        self.current
                            .push(Span::styled(std::mem::take(&mut buf), self.inline_style()));
                    }
                    if ch == UNDERLINE_START {
                        self.underline += 1;
                    } else {
                        self.underline = self.underline.saturating_sub(1);
                    }
                }
                _ => buf.push(ch),
            }
        }
        if !buf.is_empty() {
            self.current.push(Span::styled(buf, self.inline_style()));
        }
    }

    fn inline_style(&self) -> Style {
        let mut style = Style::default().fg(Color::Gray);
        if let Some(level) = self.heading {
            style = Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD);
            if matches!(level, HeadingLevel::H1) {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
        }
        if self.in_blockquote {
            style = style.fg(Color::Cyan).add_modifier(Modifier::ITALIC);
        }
        if self.link > 0 {
            style = style
                .fg(Color::LightBlue)
                .add_modifier(Modifier::UNDERLINED);
        }
        if self.bold > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.underline > 0 {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        style
    }

    fn flush_line(&mut self) {
        if self.current.is_empty() {
            return;
        }
        let mut spans = std::mem::take(&mut self.current);
        if self.in_blockquote {
            spans.insert(0, Span::styled("│ ", Style::default().fg(Color::DarkGray)));
        }
        self.lines.push(Line::from(spans));
    }
}

/// Accumulates a GFM table so it can be laid out once column widths are known.
struct TableBuilder {
    aligns: Vec<Alignment>,
    header: Vec<String>,
    body: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
}

impl TableBuilder {
    fn new(aligns: Vec<Alignment>) -> Self {
        Self {
            aligns,
            header: Vec::new(),
            body: Vec::new(),
            row: Vec::new(),
            cell: String::new(),
        }
    }

    fn render(&self) -> Vec<Line<'static>> {
        let ncols = self
            .aligns
            .len()
            .max(self.header.len())
            .max(self.body.iter().map(Vec::len).max().unwrap_or(0));
        if ncols == 0 {
            return Vec::new();
        }

        let mut widths = vec![3usize; ncols];
        for (c, h) in self.header.iter().enumerate() {
            widths[c] = widths[c].max(h.chars().count());
        }
        for row in &self.body {
            for (c, cell) in row.iter().enumerate() {
                if c < ncols {
                    widths[c] = widths[c].max(cell.chars().count());
                }
            }
        }

        let border = Style::default().fg(Color::DarkGray);
        let make_border = |left: &str, mid: &str, right: &str| {
            let mut s = String::from(left);
            for (i, w) in widths.iter().enumerate() {
                s.push_str(&"─".repeat(w + 2));
                s.push_str(if i + 1 < ncols { mid } else { right });
            }
            Line::from(Span::styled(s, border))
        };

        let mut out = Vec::new();
        out.push(make_border("┌", "┬", "┐"));
        out.push(self.content_row(&self.header, &widths, true));
        out.push(make_border("├", "┼", "┤"));
        for row in &self.body {
            out.push(self.content_row(row, &widths, false));
        }
        out.push(make_border("└", "┴", "┘"));
        out
    }

    fn content_row(&self, cells: &[String], widths: &[usize], header: bool) -> Line<'static> {
        let border = Style::default().fg(Color::DarkGray);
        let cell_style = if header {
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let mut spans = Vec::new();
        for (c, &w) in widths.iter().enumerate() {
            spans.push(Span::styled("│ ", border));
            let text = cells.get(c).map(String::as_str).unwrap_or("");
            let align = self.aligns.get(c).copied().unwrap_or(Alignment::None);
            spans.push(Span::styled(pad(text, w, align), cell_style));
            spans.push(Span::styled(" ", border));
        }
        spans.push(Span::styled("│", border));
        Line::from(spans)
    }
}

/// Pad `s` to width `w` per the column alignment.
fn pad(s: &str, w: usize, align: Alignment) -> String {
    let len = s.chars().count();
    if len >= w {
        return s.chars().take(w).collect();
    }
    let extra = w - len;
    match align {
        Alignment::Right => format!("{}{}", " ".repeat(extra), s),
        Alignment::Center => {
            let l = extra / 2;
            let r = extra - l;
            format!("{}{}{}", " ".repeat(l), s, " ".repeat(r))
        }
        _ => format!("{}{}", s, " ".repeat(extra)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_headings_and_inline() {
        let md = "# Title\n\nSome **bold** and *italic* text.";
        let out = flat(&render(md));
        assert!(out.contains("Title"));
        assert!(out.contains("bold"));
        assert!(out.contains("italic"));
    }

    #[test]
    fn renders_list_bullets() {
        let out = flat(&render("- one\n- two"));
        assert!(out.contains("• one"), "got: {out}");
        assert!(out.contains("• two"));
    }

    #[test]
    fn renders_ordered_list() {
        let out = flat(&render("1. one\n2. two\n3. three"));
        assert!(out.contains("1. one"), "got: {out}");
        assert!(out.contains("2. two"));
        assert!(out.contains("3. three"));
    }

    #[test]
    fn renders_task_list() {
        let out = flat(&render("- [x] done\n- [ ] todo"));
        assert!(out.contains("[x] done"), "got: {out}");
        assert!(out.contains("[ ] todo"));
    }

    #[test]
    fn renders_table_with_borders() {
        let md = "| Name | Qty |\n| --- | ---: |\n| Apples | 3 |\n| Pears | 12 |";
        let out = flat(&render(md));
        assert!(out.contains("Name"), "header missing:\n{out}");
        assert!(out.contains("Apples"), "body missing");
        assert!(
            out.contains("┌") && out.contains("│"),
            "no table borders:\n{out}"
        );
    }

    #[test]
    fn renders_link_with_url() {
        let out = flat(&render("See [the site](https://example.com)."));
        assert!(out.contains("the site"), "link text missing:\n{out}");
        assert!(out.contains("https://example.com"), "url missing");
    }

    #[test]
    fn renders_underline_and_strikethrough() {
        // Underline span and strikethrough should carry the matching modifiers.
        let lines = render("a [hi]{.underline} ~~bye~~ c");
        let mut saw_underline = false;
        let mut saw_strike = false;
        for line in &lines {
            for span in &line.spans {
                if span.content.contains("hi")
                    && span.style.add_modifier.contains(Modifier::UNDERLINED)
                {
                    saw_underline = true;
                }
                if span.content.contains("bye")
                    && span.style.add_modifier.contains(Modifier::CROSSED_OUT)
                {
                    saw_strike = true;
                }
            }
        }
        assert!(saw_underline, "underline modifier missing");
        assert!(saw_strike, "strikethrough modifier missing");
        // The pandoc markers must not appear as literal text.
        let out = flat(&lines);
        assert!(!out.contains("{.underline}"), "markers leaked:\n{out}");
        assert!(out.contains("hi") && out.contains("bye"));
    }

    #[test]
    fn dot_commands_are_dropped() {
        let out = flat(&render(".he My Header\n.pa\n\nReal body text"));
        assert!(out.contains("Real body text"), "body missing:\n{out}");
        assert!(!out.contains("My Header"), "dot command leaked:\n{out}");
        assert!(!out.contains(".pa"), "dot command leaked:\n{out}");
    }
}
