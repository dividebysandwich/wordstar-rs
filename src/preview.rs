//! A small read-only markdown renderer: parses the buffer with `pulldown-cmark`
//! and produces styled ratatui lines for the preview overlay.
//!
//! This is intentionally lightweight — it covers the inline and block elements a
//! writer uses day to day (headings, bold/italic, inline code, lists, block
//! quotes, code blocks, rules). It is not a full CommonMark renderer.

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render markdown source into styled lines.
pub fn render(source: &str) -> Vec<Line<'static>> {
    let mut r = Renderer::default();
    for event in Parser::new(source) {
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
    /// Active inline modifiers (bold / italic depth).
    bold: u32,
    italic: u32,
    /// Heading level currently open, if any.
    heading: Option<HeadingLevel>,
    /// Inside a fenced/indented code block.
    in_code_block: bool,
    /// Nested list bullet depth.
    list_depth: usize,
    /// Pending bullet prefix to emit at the next item's first text.
    in_blockquote: bool,
}

impl Renderer {
    fn handle(&mut self, event: Event) {
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
            Tag::CodeBlock(_) => {
                self.flush_line();
                self.in_code_block = true;
            }
            Tag::List(_) => self.list_depth += 1,
            Tag::Item => {
                self.flush_line();
                let indent = "  ".repeat(self.list_depth.saturating_sub(1));
                self.current.push(Span::styled(
                    format!("{indent}  • "),
                    Style::default().fg(Color::Yellow),
                ));
            }
            Tag::BlockQuote(_) => {
                self.flush_line();
                self.in_blockquote = true;
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line();
                self.lines.push(Line::default()); // blank line between paragraphs
            }
            TagEnd::Heading(_) => {
                self.flush_line();
                self.heading = None;
                self.lines.push(Line::default());
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::CodeBlock => {
                self.flush_line();
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.list_depth = self.list_depth.saturating_sub(1);
                if self.list_depth == 0 {
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

    fn push_text(&mut self, text: &str) {
        if self.in_code_block {
            // Code blocks may carry embedded newlines.
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
        self.current
            .push(Span::styled(text.to_string(), self.inline_style()));
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
        if self.bold > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.in_blockquote {
            style = style.fg(Color::Cyan).add_modifier(Modifier::ITALIC);
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
}
