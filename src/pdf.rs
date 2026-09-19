//! Export the document to a formatted PDF.
//!
//! The Markdown is parsed with `pulldown-cmark` into a list of blocks, which are
//! then laid out onto pages. We use the PDF standard Courier family (no font
//! files to embed, and viewers know its metrics), which is monospaced — so line
//! wrapping, table alignment and page breaks are exact, and it suits WordStar's
//! typewriter-manuscript heritage. Bold/italic/headings come from the Courier
//! variants and larger sizes.
//!
//! WordStar's print dot commands shape the pages: `.pa` starts a new page, and
//! `.he`/`.fo` (with odd/even variants, `.op`, `.pn`) set running headers,
//! footers and page numbering. With `format: manuscript` in the frontmatter the
//! document is laid out in standard manuscript format instead (see
//! [`PageStyle::manuscript`]).
//!
//! Built-in PDF fonts use WinAnsi encoding, so text is limited to Latin-1 plus
//! the usual CP1252 punctuation; anything outside that is shown as `?`.

use printpdf::*;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::attributes::{Manuscript, PAGE_BREAK, PageSetup, Paper, RenderOptions};
pub(crate) use crate::attributes::strip_frontmatter;

/// Gap added to the font size for single-spaced lines, in points.
const LINE_GAP: f32 = 3.0;
/// Courier advance width is exactly 600/1000 em.
const ADVANCE: f32 = 0.6;
/// Size of the page number and of `.he`/`.fo` lines in the standard layout.
const MARGINALIA: f32 = 9.0;

fn char_w(size: f32) -> f32 {
    size * ADVANCE
}

/// Page size in points (1 pt = 1/72").
fn paper_size(paper: Paper) -> (f32, f32) {
    match paper {
        Paper::A4 => (595.276, 841.890),
        Paper::Letter => (612.0, 792.0),
    }
}

/// Page geometry and typography for a layout, in PDF points.
#[derive(Debug, Clone)]
struct PageStyle {
    w: f32,
    h: f32,
    margin_x: f32,
    margin_top: f32,
    /// Body text stops when its baseline would fall below this height.
    bottom_limit: f32,
    body: f32,
    double_spaced: bool,
    /// Space after each block, as a fraction of a body line.
    block_gap: f32,
    /// First-line indent of indented paragraphs, in characters.
    indent: usize,
    /// Baselines of the running header and footer, from the bottom edge.
    header_y: f32,
    footer_y: f32,
    /// Standard manuscript format rules (see [`PageStyle::manuscript`]).
    manuscript: bool,
}

impl PageStyle {
    /// The default layout: 11pt single-spaced Courier with ~20 mm margins and a
    /// centered page number.
    fn standard(paper: Paper) -> Self {
        let (w, h) = paper_size(paper);
        Self {
            w,
            h,
            margin_x: 56.7,
            margin_top: 56.7,
            bottom_limit: 56.7 + 30.0,
            body: 11.0,
            double_spaced: false,
            block_gap: 0.5,
            indent: 4,
            header_y: h - 30.0,
            footer_y: 30.0,
            manuscript: false,
        }
    }

    /// Standard manuscript format, as fiction editors expect submissions: 12pt
    /// Courier, double-spaced, 1" margins, 0.5" paragraph indents with no gaps,
    /// a contact block and word count on page one, and a `Surname / TITLE / n`
    /// header on the pages after it.
    fn manuscript(paper: Paper) -> Self {
        let (w, h) = paper_size(paper);
        Self {
            w,
            h,
            margin_x: 72.0,
            margin_top: 72.0,
            bottom_limit: 72.0,
            body: 12.0,
            double_spaced: true,
            block_gap: 0.0,
            indent: 5, // 5 × 7.2pt = 0.5"
            header_y: h - 36.0,
            footer_y: 36.0,
            manuscript: true,
        }
    }

    fn max_chars(&self, size: f32) -> usize {
        ((self.w - 2.0 * self.margin_x) / char_w(size)).floor() as usize
    }

    fn line_height(&self, size: f32) -> f32 {
        if self.double_spaced {
            size * 2.0
        } else {
            size + LINE_GAP
        }
    }

    fn heading_size(&self, level: u8) -> f32 {
        if self.manuscript {
            return self.body;
        }
        match level {
            1 => 20.0,
            2 => 16.0,
            3 => 14.0,
            4 => 12.0,
            _ => self.body,
        }
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
#[derive(Clone, Default)]
pub(crate) struct Seg {
    pub(crate) text: String,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) strike: bool,
}

impl Seg {
    /// An unstyled run of text.
    pub(crate) fn plain(text: impl Into<String>) -> Seg {
        Seg {
            text: text.into(),
            ..Seg::default()
        }
    }
}

/// A laid-out-able block of content. Shared with the graphical preview.
pub(crate) enum Block {
    Heading(u8, Vec<Seg>),
    /// A paragraph; `indent` asks for a first-line indent instead of a gap
    /// before it (prose paragraphs after the first, with `paragraphs: lines`).
    Para {
        segs: Vec<Seg>,
        indent: bool,
    },
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
    /// A `.pa` page break.
    PageBreak,
}

/// Whether `segs` are marked to be centered (a `.oc on` line), and the segments
/// without the marker.
pub(crate) fn take_centered(segs: &[Seg]) -> (bool, Vec<Seg>) {
    let mut segs = segs.to_vec();
    let centered = segs
        .first_mut()
        .and_then(|s| s.text.strip_prefix(crate::attributes::CENTER).map(str::to_owned).map(|t| s.text = t))
        .is_some();
    (centered, segs)
}

/// Render `markdown` to PDF bytes. `title` is used as the document title
/// (unless the frontmatter names one).
pub fn export(markdown: &str, title: &str) -> Vec<u8> {
    let opts = crate::attributes::render_options(markdown);
    let setup = crate::attributes::page_setup(markdown);
    let blocks = parse(strip_frontmatter(markdown), &opts);
    let lines: Vec<String> = markdown.lines().map(str::to_owned).collect();
    let words = crate::attributes::count_words(&lines).words;
    let doc_title = opts
        .manuscript
        .as_ref()
        .map(|m| m.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| title.to_string());
    let mut doc = PdfDocument::new(&doc_title);
    let pages = Layout::new(&opts, setup, words).run(&blocks);
    doc.with_pages(pages)
        .save(&PdfSaveOptions::default(), &mut Vec::new())
}

// ---------------------------------------------------------------------------
// Markdown -> blocks
// ---------------------------------------------------------------------------

/// Parse Markdown (frontmatter already stripped) into blocks.
pub(crate) fn parse(src: &str, opts: &RenderOptions) -> Vec<Block> {
    // Handle dot commands and the paragraph mode, and rewrite pandoc attribute
    // spans (underline → sentinels).
    let src = crate::attributes::prepare_render_source(src, opts);
    let mut md = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    if opts.smart {
        md |= Options::ENABLE_SMART_PUNCTUATION;
    }
    let mut b = Builder {
        prose: opts.prose_paragraphs,
        ..Builder::default()
    };
    for event in Parser::new_ext(&src, md) {
        b.handle(event);
    }
    b.blocks
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    /// `paragraphs: lines`: indent paragraphs that follow a paragraph.
    prose: bool,
    inline: Vec<Seg>,
    bold: u32,
    italic: u32,
    strike: u32,
    underline: u32,
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
                    self.push_text(&t);
                }
            }
            Event::Code(t) => self.push_seg(&t),
            Event::SoftBreak => self.push_seg(" "),
            // A hard line break (verse, letters) is kept; layout breaks there.
            Event::HardBreak => self.push_seg("\n"),
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
            Tag::Strikethrough => self.strike += 1,
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
                    let text: String = segs.iter().map(|s| s.text.as_str()).collect();
                    if text.trim() == PAGE_BREAK.to_string() {
                        self.blocks.push(Block::PageBreak);
                    } else if !segs.is_empty() {
                        let indent =
                            self.prose && matches!(self.blocks.last(), Some(Block::Para { .. }));
                        self.blocks.push(Block::Para { segs, indent });
                    }
                }
                // Inside a list item the text is flushed at Item end.
            }
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
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

    /// Push text that may contain underline sentinels, toggling underline.
    fn push_text(&mut self, text: &str) {
        use crate::attributes::{UNDERLINE_END, UNDERLINE_START};
        let mut buf = String::new();
        for ch in text.chars() {
            match ch {
                UNDERLINE_START | UNDERLINE_END => {
                    if !buf.is_empty() {
                        self.push_seg(&std::mem::take(&mut buf));
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
            self.push_seg(&buf);
        }
    }

    fn push_seg(&mut self, text: &str) {
        let seg = Seg {
            text: text.to_string(),
            bold: self.bold > 0,
            italic: self.italic > 0,
            underline: self.underline > 0,
            strike: self.strike > 0,
        };
        if let Some(last) = self.inline.last_mut()
            && last.bold == seg.bold
            && last.italic == seg.italic
            && last.underline == seg.underline
            && last.strike == seg.strike
        {
            last.text.push_str(text);
            return;
        }
        self.inline.push(seg);
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
            Event::Text(t2) | Event::Code(t2) => {
                use crate::attributes::{UNDERLINE_END, UNDERLINE_START};
                t.cell.extend(
                    t2.chars()
                        .filter(|&c| c != UNDERLINE_START && c != UNDERLINE_END),
                );
            }
            Event::SoftBreak | Event::HardBreak => t.cell.push(' '),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Blocks -> paginated PDF ops
// ---------------------------------------------------------------------------

struct Layout {
    style: PageStyle,
    setup: PageSetup,
    /// Author details for standard manuscript format.
    manuscript: Option<Manuscript>,
    /// Approximate word count for the manuscript title page.
    words: usize,
    /// `paragraphs: lines` — scene breaks become centered asterisks.
    prose: bool,
    pages: Vec<PdfPage>,
    ops: Vec<Op>,
    y: f32,
    page_no: usize,
    /// Nothing has been placed on the current page yet.
    at_top: bool,
    /// Some body block has been laid out (the first chapter heading of a
    /// manuscript goes under the byline, not on a new page).
    body_started: bool,
    /// Underline / strikethrough rules to stroke on the current page, as
    /// `(x0, x1, y)` in PDF points. Drawn after the text section (graphics
    /// operators are not allowed inside a text object).
    decorations: Vec<(f32, f32, f32)>,
}

impl Layout {
    fn new(opts: &RenderOptions, setup: PageSetup, words: usize) -> Self {
        let manuscript = opts.manuscript.clone();
        let style = match &manuscript {
            Some(_) => PageStyle::manuscript(opts.paper.unwrap_or(Paper::Letter)),
            None => PageStyle::standard(opts.paper.unwrap_or(Paper::A4)),
        };
        let mut l = Layout {
            style,
            setup,
            manuscript,
            words,
            prose: opts.prose_paragraphs,
            pages: Vec::new(),
            ops: Vec::new(),
            y: 0.0,
            page_no: 0,
            at_top: true,
            body_started: false,
            decorations: Vec::new(),
        };
        l.start_page();
        l
    }

    fn run(mut self, blocks: &[Block]) -> Vec<PdfPage> {
        if let Some(m) = self.manuscript.clone() {
            self.title_page(&m);
        }
        for (i, block) in blocks.iter().enumerate() {
            self.block(block);
            self.body_started = true;
            // Consecutive indented paragraphs follow each other without a gap.
            let next_indented = matches!(blocks.get(i + 1), Some(Block::Para { indent: true, .. }));
            if !next_indented && !matches!(block, Block::PageBreak) {
                self.blank(self.style.block_gap);
            }
        }
        if self.manuscript.is_some() {
            self.centered(&[Seg::plain("END")], self.style.body);
        }
        self.finish_page();
        self.pages
    }

    /// The printed page number of the current page.
    fn page_number(&self) -> usize {
        self.setup.first_page + self.page_no - 1
    }

    fn start_page(&mut self) {
        self.page_no += 1;
        self.ops = vec![Op::StartTextSection];
        self.y = self.style.h - self.style.margin_top - self.style.body;
        self.at_top = true;
    }

    fn finish_page(&mut self) {
        let number = self.page_number();
        let st = self.style.clone();

        // Running header: a `.he` line, or the manuscript's "Surname / TITLE / n"
        // from page two on.
        if let Some(text) = self.setup.header(number) {
            let size = if st.manuscript { st.body } else { MARGINALIA };
            self.text_at(st.margin_x, st.header_y, &text, size);
        } else if let Some(m) = self.manuscript.as_ref().filter(|_| self.page_no > 1) {
            let text = format!("{} / {} / {number}", m.surname, m.title.to_uppercase());
            let x = st.w - st.margin_x - text.chars().count() as f32 * char_w(st.body);
            self.text_at(x, st.header_y, &text, st.body);
        }

        // Footer: a `.fo` line, else a centered page number (not in manuscript
        // format, whose header carries it, nor after `.op`).
        if let Some(text) = self.setup.footer(number) {
            let size = if st.manuscript { st.body } else { MARGINALIA };
            self.text_at(st.margin_x, st.footer_y, &text, size);
        } else if !st.manuscript && !self.setup.omit_page_numbers {
            let label = format!("- {number} -");
            let x = (st.w - label.chars().count() as f32 * char_w(MARGINALIA)) / 2.0;
            self.text_at(x, st.footer_y, &label, MARGINALIA);
        }
        self.ops.push(Op::EndTextSection);

        // Stroke underline / strikethrough rules (outside the text section).
        let rules = std::mem::take(&mut self.decorations);
        if !rules.is_empty() {
            self.ops.push(Op::SetOutlineColor {
                col: Color::Rgb(Rgb {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    icc_profile: None,
                }),
            });
            self.ops.push(Op::SetOutlineThickness { pt: Pt(0.6) });
            for (x0, x1, y) in rules {
                self.ops.push(Op::DrawLine {
                    line: Line {
                        points: vec![
                            LinePoint {
                                p: Point {
                                    x: Pt(x0),
                                    y: Pt(y),
                                },
                                bezier: false,
                            },
                            LinePoint {
                                p: Point {
                                    x: Pt(x1),
                                    y: Pt(y),
                                },
                                bezier: false,
                            },
                        ],
                        is_closed: false,
                    },
                });
            }
        }

        let ops = std::mem::take(&mut self.ops);
        let mm = |pt: f32| Mm(pt * 25.4 / 72.0);
        self.pages.push(PdfPage::new(mm(st.w), mm(st.h), ops));
    }

    fn newpage(&mut self) {
        self.finish_page();
        self.start_page();
    }

    fn blank(&mut self, fraction: f32) {
        self.y -= self.style.line_height(self.style.body) * fraction;
    }

    /// Plain Courier text at an absolute position (headers, footers, the
    /// manuscript title page), outside the flowing body text.
    fn text_at(&mut self, x: f32, y: f32, text: &str, size: f32) {
        self.ops.push(Op::SetTextMatrix {
            matrix: TextMatrix::Translate(Pt(x), Pt(y)),
        });
        self.ops.push(Op::SetFont {
            font: PdfFontHandle::Builtin(BuiltinFont::Courier),
            size: Pt(size),
        });
        self.ops.push(Op::ShowText {
            items: vec![TextItem::Text(sanitize(text))],
        });
    }

    /// Emit one visual line of styled segments at `x`, in font `size`.
    fn line(&mut self, x: f32, segs: &[Seg], size: f32) {
        let lh = self.style.line_height(size);
        if self.y < self.style.bottom_limit {
            self.newpage();
        }
        self.at_top = false;
        self.ops.push(Op::SetTextMatrix {
            matrix: TextMatrix::Translate(Pt(x), Pt(self.y)),
        });
        let underline_italics = self.manuscript.as_ref().is_some_and(|m| m.underline_italics);
        let mut sx = x;
        for s in segs {
            // Classic manuscripts underline what will be set in italics.
            let (italic, underline) = if underline_italics {
                (false, s.underline || s.italic)
            } else {
                (s.italic, s.underline)
            };
            let font = courier(s.bold, italic);
            self.ops.push(Op::SetFont {
                font: PdfFontHandle::Builtin(font),
                size: Pt(size),
            });
            self.ops.push(Op::ShowText {
                items: vec![TextItem::Text(sanitize(&s.text))],
            });
            let w = s.text.chars().count() as f32 * char_w(size);
            if underline {
                self.decorations.push((sx, sx + w, self.y - size * 0.12));
            }
            if s.strike {
                self.decorations.push((sx, sx + w, self.y + size * 0.30));
            }
            sx += w;
        }
        self.y -= lh;
    }

    /// One line centered between the margins.
    fn centered(&mut self, segs: &[Seg], size: f32) {
        let len: usize = segs.iter().map(|s| s.text.chars().count()).sum();
        let x = ((self.style.w - len as f32 * char_w(size)) / 2.0).max(self.style.margin_x);
        self.line(x, segs, size);
    }

    /// Page one of a standard manuscript: the author's name and contact details
    /// top left, the approximate word count top right, and the title and byline
    /// centered halfway down, where the story then begins.
    fn title_page(&mut self, m: &Manuscript) {
        let st = self.style.clone();
        let top = st.h - st.margin_top - st.body;
        let single = st.body + LINE_GAP;
        let contact = std::iter::once(&m.author).chain(&m.contact);
        for (i, line) in contact.filter(|l| !l.is_empty()).enumerate() {
            self.text_at(st.margin_x, top - i as f32 * single, line, st.body);
        }
        let count = format!("about {} words", approximate_words(self.words));
        let x = st.w - st.margin_x - count.chars().count() as f32 * char_w(st.body);
        self.text_at(x, top, &count, st.body);

        self.y = st.h / 2.0;
        if !m.title.is_empty() {
            self.centered(&[Seg::plain(m.title.clone())], st.body);
        }
        if !m.byline.is_empty() {
            self.centered(&[Seg::plain(format!("by {}", m.byline))], st.body);
        }
        self.blank(1.0);
    }

    fn block(&mut self, block: &Block) {
        let st = self.style.clone();
        match block {
            Block::Heading(level, segs) if take_centered(segs).0 && !st.manuscript => {
                let size = st.heading_size(*level);
                let bolded: Vec<Seg> = take_centered(segs)
                    .1
                    .into_iter()
                    .map(|s| Seg { bold: true, ..s })
                    .collect();
                for line in wrap(&bolded, st.max_chars(size), 0) {
                    self.centered(&line, size);
                }
            }
            Block::Para { segs, .. } if take_centered(segs).0 => {
                for line in wrap(&take_centered(segs).1, st.max_chars(st.body), 0) {
                    self.centered(&line, st.body);
                }
            }
            Block::Heading(level, segs) if st.manuscript => {
                let segs = &take_centered(segs).1;
                // Chapters start on a new page, a third of the way down — except
                // the first, which follows the title and byline on page one.
                if *level == 1 && self.body_started && !self.at_top {
                    self.newpage();
                }
                if *level == 1 && self.at_top {
                    self.y = self.y.min(st.h * 2.0 / 3.0);
                }
                for line in wrap(segs, st.max_chars(st.body), 0) {
                    self.centered(&line, st.body);
                }
                self.blank(1.0);
            }
            Block::Heading(level, segs) => {
                let size = st.heading_size(*level);
                let bolded: Vec<Seg> = segs
                    .iter()
                    .map(|s| Seg {
                        bold: true,
                        ..s.clone()
                    })
                    .collect();
                for line in wrap(&bolded, st.max_chars(size), 0) {
                    self.line(st.margin_x, &line, size);
                }
            }
            Block::Para { segs, indent } => {
                // Manuscripts indent every paragraph; otherwise only prose
                // paragraphs that follow another paragraph.
                let indent = if st.manuscript || *indent { st.indent } else { 0 };
                for line in wrap(segs, st.max_chars(st.body), indent) {
                    self.line(st.margin_x, &line, st.body);
                }
            }
            Block::Item {
                depth,
                marker,
                segs,
            } => {
                let indent = st.margin_x + (*depth as f32) * 2.0 * char_w(st.body);
                let marker_w = marker.chars().count();
                let avail = st.max_chars(st.body).saturating_sub(depth * 2 + marker_w).max(8);
                let lines = wrap(segs, avail, 0);
                let cont_x = indent + marker_w as f32 * char_w(st.body);
                for (i, line) in lines.iter().enumerate() {
                    if i == 0 {
                        let mut first = vec![Seg::plain(marker.clone())];
                        first.extend(line.iter().cloned());
                        self.line(indent, &first, st.body);
                    } else {
                        self.line(cont_x, line, st.body);
                    }
                }
                if lines.is_empty() {
                    self.line(indent, &[Seg::plain(marker.clone())], st.body);
                }
            }
            Block::Code(lines) => {
                for raw in lines {
                    for chunk in hard_wrap(raw, st.max_chars(st.body)) {
                        self.line(st.margin_x, &[Seg::plain(chunk)], st.body);
                    }
                }
            }
            Block::Quote(segs) => {
                let avail = st.max_chars(st.body).saturating_sub(2).max(8);
                for line in wrap(segs, avail, 0) {
                    let mut row = vec![Seg::plain("> ")];
                    for s in line {
                        row.push(Seg { italic: true, ..s });
                    }
                    self.line(st.margin_x, &row, st.body);
                }
            }
            // A scene break: the manuscript convention is a centered "#".
            Block::Rule if st.manuscript => self.centered(&[Seg::plain("#")], st.body),
            Block::Rule if self.prose => self.centered(&[Seg::plain("*   *   *")], st.body),
            Block::Rule => {
                self.line(st.margin_x, &[Seg::plain("-".repeat(st.max_chars(st.body)))], st.body);
            }
            Block::Table { header, rows } => self.table(header, rows),
            Block::PageBreak => {
                if !self.at_top {
                    self.newpage();
                }
            }
        }
    }

    fn table(&mut self, header: &[String], rows: &[Vec<String>]) {
        let body = self.style.body;
        let margin = self.style.margin_x;
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
        let budget = self.style.max_chars(body);
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
            ..Seg::default()
        };

        self.line(margin, &[mono(border.clone(), false)], body);
        self.line(margin, &[mono(row_text(header), true)], body);
        self.line(margin, &[mono(border.clone(), false)], body);
        for r in rows {
            self.line(margin, &[mono(row_text(r), false)], body);
        }
        self.line(margin, &[mono(border, false)], body);
    }
}

/// A manuscript's "about N words": rounded to the nearest hundred (exact below
/// a hundred), with thousands separators.
pub(crate) fn approximate_words(words: usize) -> String {
    let n = if words < 100 {
        words
    } else {
        (words + 50) / 100 * 100
    };
    crate::attributes::group_digits(n)
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
/// The first line starts with `indent` spaces (a paragraph indent), and a `\n`
/// in the text — a hard line break — always starts a new line.
fn wrap(segs: &[Seg], max: usize, indent: usize) -> Vec<Vec<Seg>> {
    let max = max.max(1);
    let indent = indent.min(max / 2);
    let mut lines: Vec<Vec<Seg>> = Vec::new();
    let mut line: Vec<Seg> = Vec::new();
    let mut col = 0usize;
    // No word on the current line yet (so the next one needs no leading space).
    let mut fresh = true;
    if indent > 0 {
        line.push(Seg::plain(" ".repeat(indent)));
        col = indent;
    }

    for seg in segs {
        for (i, part) in seg.text.split('\n').enumerate() {
            if i > 0 {
                lines.push(std::mem::take(&mut line));
                col = 0;
                fresh = true;
            }
            for word in part.split_whitespace() {
                for piece in hard_wrap(word, max) {
                    let plen = piece.chars().count();
                    if !fresh && col + plen + 1 > max {
                        lines.push(std::mem::take(&mut line));
                        col = 0;
                        fresh = true;
                    }
                    let add_space = !fresh;
                    push_word(&mut line, &piece, seg, add_space);
                    col += if add_space { plen + 1 } else { plen };
                    fresh = false;
                }
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn push_word(line: &mut Vec<Seg>, word: &str, style: &Seg, add_space: bool) {
    let text = if add_space {
        format!(" {word}")
    } else {
        word.to_string()
    };
    if let Some(last) = line.last_mut()
        && last.bold == style.bold
        && last.italic == style.italic
        && last.underline == style.underline
        && last.strike == style.strike
    {
        last.text.push_str(&text);
        return;
    }
    line.push(Seg {
        text,
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strike: style.strike,
    });
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
    fn parse_flags_underline_and_strikethrough() {
        let blocks = parse(
            "plain [under]{.underline} and ~~struck~~ text",
            &RenderOptions::default(),
        );
        let segs = match &blocks[0] {
            Block::Para { segs, .. } => segs,
            other => panic!(
                "expected paragraph, got {:?}",
                std::mem::discriminant(other)
            ),
        };
        let find = |needle: &str| segs.iter().find(|s| s.text.contains(needle)).unwrap();
        assert!(find("under").underline, "underline flag missing");
        assert!(!find("under").strike);
        assert!(find("struck").strike, "strike flag missing");
        assert!(!find("struck").underline);
        // The sentinels and pandoc markers never reach the rendered text.
        for s in segs {
            assert!(!s.text.contains('\u{E000}') && !s.text.contains('\u{E001}'));
            assert!(!s.text.contains("underline"));
        }
    }

    #[test]
    fn export_with_decorations_is_valid_pdf() {
        // Exercises the underline/strikethrough line-drawing path.
        let md = "A [line]{.underline} and ~~gone~~ here.\n";
        let bytes = export(md, "Deco");
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 500);
    }

    #[test]
    fn frontmatter_is_stripped() {
        let md = "---\nfont: Courier\nsize: 12\n---\nHello body.\n";
        assert_eq!(strip_frontmatter(md).trim(), "Hello body.");
    }

    #[test]
    fn wrap_breaks_long_paragraph() {
        let seg = Seg::plain("word ".repeat(40).trim().to_string());
        let lines = wrap(&[seg], 20, 0);
        assert!(lines.len() > 1, "expected multiple wrapped lines");
        for l in &lines {
            let len: usize = l.iter().map(|s| s.text.chars().count()).sum();
            assert!(len <= 20, "line exceeds width: {len}");
        }
    }

    fn layout(md: &str) -> Vec<PdfPage> {
        let opts = crate::attributes::render_options(md);
        let setup = crate::attributes::page_setup(md);
        let lines: Vec<String> = md.lines().map(str::to_owned).collect();
        let words = crate::attributes::count_words(&lines).words;
        Layout::new(&opts, setup, words).run(&parse(strip_frontmatter(md), &opts))
    }

    /// All text drawn on a page, pieces joined with `|`.
    fn page_text(page: &PdfPage) -> String {
        page.ops
            .iter()
            .filter_map(|op| match op {
                Op::ShowText { items } => Some(
                    items
                        .iter()
                        .filter_map(|i| match i {
                            TextItem::Text(t) => Some(t.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn page_break_dot_command_starts_a_new_page() {
        let pages = layout("Chapter one.\n.pa\nChapter two.");
        assert_eq!(pages.len(), 2);
        assert!(page_text(&pages[0]).contains("Chapter one."));
        assert!(page_text(&pages[1]).contains("Chapter two."));
        // A break at the top of a page doesn't leave a blank page.
        assert_eq!(layout(".pa\nText").len(), 1);
    }

    #[test]
    fn headers_and_footers_print_with_page_numbers() {
        let pages = layout(".oh My Novel - page #\n.fo Draft\n.pn 3\nOne\n.pa\nTwo");
        assert!(page_text(&pages[0]).contains("My Novel - page 3"), "odd page 3");
        assert!(!page_text(&pages[1]).contains("My Novel"), "no header on even pages");
        assert!(page_text(&pages[1]).contains("Draft"));
        assert!(!page_text(&pages[1]).contains("- 4 -"), "footer replaces the number");
        // Without a footer the page number is printed, unless `.op`.
        assert!(page_text(&layout("Body")[0]).contains("- 1 -"));
        assert!(!page_text(&layout(".op\nBody")[0]).contains("- 1 -"));
    }

    #[test]
    fn smart_punctuation_curls_quotes_and_dashes() {
        let blocks = parse("\"Hello\" -- it's... fine --- ok", &RenderOptions::default());
        let Block::Para { segs, .. } = &blocks[0] else {
            panic!("expected a paragraph");
        };
        let text: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "\u{201C}Hello\u{201D} \u{2014} it\u{2019}s\u{2026} fine \u{2014} ok");
        // All of these print in the PDF's WinAnsi encoding.
        assert_eq!(sanitize(&text), text);
    }

    #[test]
    fn hard_line_breaks_are_kept() {
        let blocks = parse("Roses are red,\\\nviolets are blue", &RenderOptions::default());
        let Block::Para { segs, .. } = &blocks[0] else {
            panic!("expected a paragraph");
        };
        let lines = wrap(segs, 60, 0);
        assert_eq!(lines.len(), 2, "one line per verse line");
    }

    #[test]
    fn prose_paragraphs_indent_instead_of_code_blocks() {
        let md = "---\nparagraphs: lines\n---\nFirst paragraph.\n\tSecond paragraph.\nThird.";
        let opts = crate::attributes::render_options(md);
        let blocks = parse(strip_frontmatter(md), &opts);
        let indents: Vec<bool> = blocks
            .iter()
            .map(|b| match b {
                Block::Para { indent, .. } => *indent,
                _ => panic!("only paragraphs expected, no code block"),
            })
            .collect();
        assert_eq!(indents, [false, true, true]);
        let text = page_text(&layout(md)[0]);
        assert!(text.contains("    Second paragraph."), "indented: {text}");
    }

    #[test]
    fn manuscript_format_has_title_page_header_and_end() {
        let body = "word ".repeat(1234);
        let md = format!(
            "---\nformat: manuscript\ntitle: The Red House\nauthor: Jane Q. Writer\nbyline: J. Q. Writer\ncontact:\n  - 1 Elm St\n  - jane@example.com\n---\n# One\n\n{body}\n\n***\n\nThe end."
        );
        let pages = layout(&md);
        assert!(pages.len() >= 3);
        assert!(page_text(&pages[0]).contains("|One|"), "chapter one starts on page one");
        // US Letter by default, in points.
        assert!((pages[0].media_box.width.0 - 612.0).abs() < 0.5);
        assert!((pages[0].media_box.height.0 - 792.0).abs() < 0.5);
        let first = page_text(&pages[0]);
        for want in ["Jane Q. Writer", "1 Elm St", "jane@example.com", "about 1,200 words", "The Red House", "by J. Q. Writer"] {
            assert!(first.contains(want), "{want:?} missing from page 1: {first}");
        }
        assert!(!first.contains("- 1 -"), "no footer page number");
        let second = page_text(&pages[1]);
        assert!(second.contains("Writer / THE RED HOUSE / 2"), "header: {second}");
        let all: String = pages.iter().map(page_text).collect();
        assert!(all.contains("|#|"), "scene break as a centered #");
        assert!(all.contains("The end.|END|"), "END after the last line");
    }

    #[test]
    fn oc_lines_are_centered() {
        let pages = layout("Left.\n\n.oc on\nMiddle\n.oc off\n\nLeft again.");
        let x_of = |needle: &str| {
            let mut x = None;
            let mut last = 0.0;
            for op in &pages[0].ops {
                match op {
                    Op::SetTextMatrix { matrix: TextMatrix::Translate(px, _) } => last = px.0,
                    Op::ShowText { items } if items.iter().any(|i| matches!(i, TextItem::Text(t) if t.contains(needle))) => x = Some(last),
                    _ => {}
                }
            }
            x.unwrap()
        };
        assert!(x_of("Middle") > x_of("Left.") + 100.0, "centered");
        assert_eq!(x_of("Left again."), x_of("Left."));
    }

    #[test]
    fn approximate_word_counts_round_to_hundreds() {
        assert_eq!(approximate_words(87), "87");
        assert_eq!(approximate_words(1234), "1,200");
        assert_eq!(approximate_words(123_456), "123,500");
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
