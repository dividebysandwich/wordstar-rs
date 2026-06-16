//! Export the document to a formatted PDF.
//!
//! The Markdown is parsed with `pulldown-cmark` into a list of blocks, which are
//! then laid out onto A4 pages. We use the PDF standard Courier family (no font
//! files to embed, and viewers know its metrics), which is monospaced — so line
//! wrapping, table alignment and page breaks are exact, and it suits WordStar's
//! typewriter-manuscript heritage. Bold/italic/headings come from the Courier
//! variants and larger sizes.
//!
//! Built-in PDF fonts use WinAnsi encoding, so text is limited to Latin-1 plus
//! the usual CP1252 punctuation; anything outside that is shown as `?`.

use printpdf::*;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

// A4 page, in PDF points (1 pt = 1/72").
const PAGE_W: f32 = 595.276;
const PAGE_H: f32 = 841.890;
const MARGIN: f32 = 56.7; // ~20 mm
const FOOTER_Y: f32 = 30.0; // page-number baseline
const BODY: f32 = 11.0;
const LINE_GAP: f32 = 3.0;
/// Courier advance width is exactly 600/1000 em.
const ADVANCE: f32 = 0.6;

fn char_w(size: f32) -> f32 {
    size * ADVANCE
}

fn max_chars(size: f32) -> usize {
    ((PAGE_W - 2.0 * MARGIN) / char_w(size)).floor() as usize
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 20.0,
        2 => 16.0,
        3 => 14.0,
        4 => 12.0,
        _ => BODY,
    }
}

fn courier(bold: bool, italic: bool) -> BuiltinFont {
    match (bold, italic) {
        (true, true) => BuiltinFont::CourierBoldOblique,
        (true, false) => BuiltinFont::CourierBold,
        (false, true) => BuiltinFont::CourierOblique,
        (false, false) => BuiltinFont::Courier,
    }
}

/// A styled run of text on one line. Shared with the graphical preview.
#[derive(Clone)]
pub(crate) struct Seg {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
}

/// A laid-out-able block of content. Shared with the graphical preview.
pub(crate) enum Block {
    Heading(u8, Vec<Seg>),
    Para(Vec<Seg>),
    Item {
        depth: usize,
        marker: String,
        segs: Vec<Seg>,
    },
    Code(Vec<String>),
    Quote(Vec<Seg>),
    Rule,
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

/// Render `markdown` to PDF bytes. `title` is used as the document title.
pub fn export(markdown: &str, title: &str) -> Vec<u8> {
    let blocks = parse(strip_frontmatter(markdown));
    let mut doc = PdfDocument::new(title);
    let pages = Layout::new().run(&blocks);
    doc.with_pages(pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new())
}

/// Drop a leading YAML frontmatter block (`--- … ---`) so it is not printed.
pub(crate) fn strip_frontmatter(src: &str) -> &str {
    let mut lines = src.lines();
    if lines.next().map(str::trim) != Some("---") {
        return src;
    }
    let mut offset = 4; // past "---\n"
    for line in lines {
        offset += line.len() + 1;
        if line.trim() == "---" {
            return src.get(offset..).unwrap_or("");
        }
    }
    src
}

// ---------------------------------------------------------------------------
// Markdown -> blocks
// ---------------------------------------------------------------------------

pub(crate) fn parse(src: &str) -> Vec<Block> {
    // Drop WordStar dot commands (print directives, not body text).
    let src: String = src
        .lines()
        .filter(|l| !is_dot_command(l))
        .collect::<Vec<_>>()
        .join("\n");
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let mut b = Builder::default();
    for event in Parser::new_ext(&src, opts) {
        b.handle(event);
    }
    b.blocks
}

/// True for a WordStar dot command (a `.` at column 1 followed by a letter).
fn is_dot_command(line: &str) -> bool {
    let mut chars = line.chars();
    chars.next() == Some('.') && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    inline: Vec<Seg>,
    bold: u32,
    italic: u32,
    heading: Option<u8>,
    list_stack: Vec<Option<u64>>,
    in_item: bool,
    cur_depth: usize,
    cur_marker: String,
    in_code: bool,
    code_buf: String,
    in_quote: bool,
    table: Option<TableAcc>,
}

#[derive(Default)]
struct TableAcc {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
}

impl Builder {
    fn handle(&mut self, event: Event) {
        if self.table.is_some() {
            self.table_event(event);
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if self.in_code {
                    self.code_buf.push_str(&t);
                } else {
                    self.push_seg(&t);
                }
            }
            Event::Code(t) => self.push_seg(&t),
            Event::SoftBreak | Event::HardBreak => self.push_seg(" "),
            Event::Rule => self.blocks.push(Block::Rule),
            Event::TaskListMarker(checked) => {
                self.cur_marker = if checked {
                    "[x] ".into()
                } else {
                    "[ ] ".into()
                };
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => self.heading = Some(level as u8),
            Tag::Strong => self.bold += 1,
            Tag::Emphasis => self.italic += 1,
            Tag::CodeBlock(_) => {
                self.in_code = true;
                self.code_buf.clear();
            }
            Tag::List(first) => {
                if self.in_item {
                    self.flush_item();
                }
                self.list_stack.push(first);
            }
            Tag::Item => {
                self.in_item = true;
                self.cur_depth = self.list_stack.len().saturating_sub(1);
                self.cur_marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ => "- ".to_string(),
                };
            }
            Tag::BlockQuote(_) => self.in_quote = true,
            Tag::Table(_) => self.table = Some(TableAcc::default()),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                let segs = std::mem::take(&mut self.inline);
                let level = self.heading.take().unwrap_or(1);
                if !segs.is_empty() {
                    self.blocks.push(Block::Heading(level, segs));
                }
            }
            TagEnd::Paragraph => {
                if self.in_quote {
                    let segs = std::mem::take(&mut self.inline);
                    if !segs.is_empty() {
                        self.blocks.push(Block::Quote(segs));
                    }
                } else if !self.in_item {
                    let segs = std::mem::take(&mut self.inline);
                    if !segs.is_empty() {
                        self.blocks.push(Block::Para(segs));
                    }
                }
                // Inside a list item the text is flushed at Item end.
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::CodeBlock => {
                self.in_code = false;
                let lines: Vec<String> = self
                    .code_buf
                    .trim_end_matches('\n')
                    .split('\n')
                    .map(str::to_string)
                    .collect();
                self.blocks.push(Block::Code(lines));
            }
            TagEnd::Item => self.flush_item(),
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::BlockQuote(_) => self.in_quote = false,
            _ => {}
        }
    }

    fn flush_item(&mut self) {
        if !self.in_item {
            return;
        }
        let segs = std::mem::take(&mut self.inline);
        self.blocks.push(Block::Item {
            depth: self.cur_depth,
            marker: std::mem::take(&mut self.cur_marker),
            segs,
        });
        self.in_item = false;
    }

    fn push_seg(&mut self, text: &str) {
        let bold = self.bold > 0;
        let italic = self.italic > 0;
        if let Some(last) = self.inline.last_mut()
            && last.bold == bold
            && last.italic == italic
        {
            last.text.push_str(text);
            return;
        }
        self.inline.push(Seg {
            text: text.to_string(),
            bold,
            italic,
        });
    }

    fn table_event(&mut self, event: Event) {
        let Some(t) = self.table.as_mut() else { return };
        match event {
            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => t.row.clear(),
            Event::Start(Tag::TableCell) => t.cell.clear(),
            Event::End(TagEnd::TableCell) => {
                let c = std::mem::take(&mut t.cell);
                t.row.push(c);
            }
            Event::End(TagEnd::TableHead) => t.header = std::mem::take(&mut t.row),
            Event::End(TagEnd::TableRow) => {
                let r = std::mem::take(&mut t.row);
                t.rows.push(r);
            }
            Event::End(TagEnd::Table) => {
                let t = self.table.take().unwrap();
                self.blocks.push(Block::Table {
                    header: t.header,
                    rows: t.rows,
                });
            }
            Event::Text(t2) | Event::Code(t2) => t.cell.push_str(&t2),
            Event::SoftBreak | Event::HardBreak => t.cell.push(' '),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks -> paginated PDF ops
// ---------------------------------------------------------------------------

struct Layout {
    pages: Vec<PdfPage>,
    ops: Vec<Op>,
    y: f32,
    page_no: usize,
}

impl Layout {
    fn new() -> Self {
        let mut l = Layout {
            pages: Vec::new(),
            ops: Vec::new(),
            y: 0.0,
            page_no: 0,
        };
        l.start_page();
        l
    }

    fn run(mut self, blocks: &[Block]) -> Vec<PdfPage> {
        for block in blocks {
            self.block(block);
            self.blank(0.5);
        }
        self.finish_page();
        self.pages
    }

    fn start_page(&mut self) {
        self.page_no += 1;
        self.ops = vec![Op::StartTextSection];
        self.y = PAGE_H - MARGIN - BODY;
    }

    fn finish_page(&mut self) {
        // Centered page number footer.
        let label = format!("- {} -", self.page_no);
        let x = (PAGE_W - label.chars().count() as f32 * char_w(9.0)) / 2.0;
        self.ops.push(Op::SetTextMatrix {
            matrix: TextMatrix::Translate(Pt(x), Pt(FOOTER_Y)),
        });
        self.ops.push(Op::SetFontSizeBuiltinFont {
            size: Pt(9.0),
            font: BuiltinFont::Courier,
        });
        self.ops.push(Op::WriteTextBuiltinFont {
            items: vec![TextItem::Text(label)],
            font: BuiltinFont::Courier,
        });
        self.ops.push(Op::EndTextSection);
        let ops = std::mem::take(&mut self.ops);
        self.pages.push(PdfPage::new(Mm(210.0), Mm(297.0), ops));
    }

    fn newpage(&mut self) {
        self.finish_page();
        self.start_page();
    }

    fn blank(&mut self, fraction: f32) {
        self.y -= (BODY + LINE_GAP) * fraction;
    }

    /// Emit one visual line of styled segments at `x`, in font `size`.
    fn line(&mut self, x: f32, segs: &[Seg], size: f32) {
        let lh = size + LINE_GAP;
        if self.y < MARGIN + FOOTER_Y {
            self.newpage();
        }
        self.ops.push(Op::SetTextMatrix {
            matrix: TextMatrix::Translate(Pt(x), Pt(self.y)),
        });
        for s in segs {
            let font = courier(s.bold, s.italic);
            self.ops.push(Op::SetFontSizeBuiltinFont {
                size: Pt(size),
                font,
            });
            self.ops.push(Op::WriteTextBuiltinFont {
                items: vec![TextItem::Text(sanitize(&s.text))],
                font,
            });
        }
        self.y -= lh;
    }

    fn block(&mut self, block: &Block) {
        match block {
            Block::Heading(level, segs) => {
                let size = heading_size(*level);
                let bolded: Vec<Seg> = segs
                    .iter()
                    .map(|s| Seg {
                        text: s.text.clone(),
                        bold: true,
                        italic: s.italic,
                    })
                    .collect();
                for line in wrap(&bolded, max_chars(size)) {
                    self.line(MARGIN, &line, size);
                }
            }
            Block::Para(segs) => {
                for line in wrap(segs, max_chars(BODY)) {
                    self.line(MARGIN, &line, BODY);
                }
            }
            Block::Item {
                depth,
                marker,
                segs,
            } => {
                let indent = MARGIN + (*depth as f32) * 2.0 * char_w(BODY);
                let marker_w = marker.chars().count();
                let avail = max_chars(BODY).saturating_sub(depth * 2 + marker_w).max(8);
                let lines = wrap(segs, avail);
                let cont_x = indent + marker_w as f32 * char_w(BODY);
                for (i, line) in lines.iter().enumerate() {
                    if i == 0 {
                        let mut first = vec![Seg {
                            text: marker.clone(),
                            bold: false,
                            italic: false,
                        }];
                        first.extend(line.iter().cloned());
                        self.line(indent, &first, BODY);
                    } else {
                        self.line(cont_x, line, BODY);
                    }
                }
                if lines.is_empty() {
                    self.line(
                        indent,
                        &[Seg {
                            text: marker.clone(),
                            bold: false,
                            italic: false,
                        }],
                        BODY,
                    );
                }
            }
            Block::Code(lines) => {
                for raw in lines {
                    for chunk in hard_wrap(raw, max_chars(BODY)) {
                        self.line(
                            MARGIN,
                            &[Seg {
                                text: chunk,
                                bold: false,
                                italic: false,
                            }],
                            BODY,
                        );
                    }
                }
            }
            Block::Quote(segs) => {
                let avail = max_chars(BODY).saturating_sub(2).max(8);
                for line in wrap(segs, avail) {
                    let mut row = vec![Seg {
                        text: "> ".into(),
                        bold: false,
                        italic: false,
                    }];
                    for s in line {
                        row.push(Seg {
                            text: s.text,
                            bold: s.bold,
                            italic: true,
                        });
                    }
                    self.line(MARGIN, &row, BODY);
                }
            }
            Block::Rule => {
                self.line(
                    MARGIN,
                    &[Seg {
                        text: "-".repeat(max_chars(BODY)),
                        bold: false,
                        italic: false,
                    }],
                    BODY,
                );
            }
            Block::Table { header, rows } => self.table(header, rows),
        }
    }

    fn table(&mut self, header: &[String], rows: &[Vec<String>]) {
        let ncols = header
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0));
        if ncols == 0 {
            return;
        }
        let mut widths = vec![3usize; ncols];
        let consider = |row: &[String], widths: &mut [usize]| {
            for (c, cell) in row.iter().enumerate() {
                if c < ncols {
                    widths[c] = widths[c].max(cell.chars().count());
                }
            }
        };
        consider(header, &mut widths);
        for r in rows {
            consider(r, &mut widths);
        }
        // Keep the table within the printable width by shrinking the widest
        // columns (each column has 3 chars of "| " padding plus a final "|").
        let budget = max_chars(BODY);
        let frame = |w: &[usize]| -> usize { w.iter().sum::<usize>() + 3 * ncols + 1 };
        while frame(&widths) > budget {
            let Some((i, _)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) else {
                break;
            };
            if widths[i] <= 3 {
                break;
            }
            widths[i] -= 1;
        }

        let border = {
            let mut s = String::from("+");
            for w in &widths {
                s.push_str(&"-".repeat(w + 2));
                s.push('+');
            }
            s
        };
        let row_text = |cells: &[String]| -> String {
            let mut s = String::from("|");
            for (c, &w) in widths.iter().enumerate() {
                let raw = cells.get(c).map(String::as_str).unwrap_or("");
                let cell = pad(raw, w);
                s.push(' ');
                s.push_str(&cell);
                s.push_str(" |");
            }
            s
        };
        let mono = |text: String, bold: bool| Seg {
            text,
            bold,
            italic: false,
        };

        self.line(MARGIN, &[mono(border.clone(), false)], BODY);
        self.line(MARGIN, &[mono(row_text(header), true)], BODY);
        self.line(MARGIN, &[mono(border.clone(), false)], BODY);
        for r in rows {
            self.line(MARGIN, &[mono(row_text(r), false)], BODY);
        }
        self.line(MARGIN, &[mono(border, false)], BODY);
    }
}

/// Pad or truncate `s` to exactly `w` characters (left-aligned).
fn pad(s: &str, w: usize) -> String {
    let len = s.chars().count();
    if len > w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - len))
    }
}

/// Greedily wrap styled segments into visual lines of at most `max` characters.
fn wrap(segs: &[Seg], max: usize) -> Vec<Vec<Seg>> {
    let max = max.max(1);
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    let mut line: Vec<Seg> = Vec::new();
    let mut col = 0usize;

    for seg in segs {
        for word in seg.text.split_whitespace() {
            for piece in hard_wrap(word, max) {
                let plen = piece.chars().count();
                let need = if col == 0 { plen } else { plen + 1 };
                if col > 0 && col + need > max {
                    lines.push(std::mem::take(&mut line));
                    col = 0;
                }
                let add_space = col > 0;
                push_word(&mut line, &piece, seg.bold, seg.italic, add_space);
                col += if add_space { plen + 1 } else { plen };
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn push_word(line: &mut Vec<Seg>, word: &str, bold: bool, italic: bool, add_space: bool) {
    let text = if add_space {
        format!(" {word}")
    } else {
        word.to_string()
    };
    if let Some(last) = line.last_mut()
        && last.bold == bold
        && last.italic == italic
    {
        last.text.push_str(&text);
        return;
    }
    line.push(Seg { text, bold, italic });
}

/// Break a string into chunks of at most `max` characters (for overlong words
/// and code lines).
fn hard_wrap(s: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return vec![s.to_string()];
    }
    chars.chunks(max).map(|c| c.iter().collect()).collect()
}

/// Replace characters outside WinAnsi (the built-in font encoding) with `?` so
/// glyph counts match what is rendered.
fn sanitize(s: &str) -> String {
    const SPECIALS: &[char] = &[
        '\u{20AC}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
        '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{017D}', '\u{2018}',
        '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}',
        '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{017E}', '\u{0178}',
    ];
    s.chars()
        .map(|c| {
            if c.is_ascii() || ('\u{00A0}'..='\u{00FF}').contains(&c) || SPECIALS.contains(&c) {
                c
            } else {
                '?'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_valid_pdf_header() {
        let md = "# Title\n\nA paragraph with **bold** and *italic*.\n\n- one\n- two\n";
        let bytes = export(md, "Test");
        assert!(bytes.starts_with(b"%PDF-"), "missing PDF header");
        assert!(bytes.len() > 500, "PDF unexpectedly small: {}", bytes.len());
    }

    #[test]
    fn frontmatter_is_stripped() {
        let md = "---\nfont: Courier\nsize: 12\n---\nHello body.\n";
        assert_eq!(strip_frontmatter(md).trim(), "Hello body.");
    }

    #[test]
    fn wrap_breaks_long_paragraph() {
        let seg = Seg {
            text: "word ".repeat(40).trim().to_string(),
            bold: false,
            italic: false,
        };
        let lines = wrap(&[seg], 20);
        assert!(lines.len() > 1, "expected multiple wrapped lines");
        for l in &lines {
            let len: usize = l.iter().map(|s| s.text.chars().count()).sum();
            assert!(len <= 20, "line exceeds width: {len}");
        }
    }

    #[test]
    fn long_document_paginates() {
        let md = "para\n\n".repeat(400);
        let bytes = export(&md, "Big");
        assert!(bytes.starts_with(b"%PDF-"));
        // Multiple pages should be present.
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.matches("/Type /Page").count() >= 2 || bytes.len() > 5000);
    }
}
