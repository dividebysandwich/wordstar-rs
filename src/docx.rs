//! Export the document as a Word file (`.docx`), the format agents, editors
//! and publishers usually ask for.
//!
//! The same parse as the PDF ([`crate::pdf::parse`]) is turned into
//! WordprocessingML: paragraphs with styles (so the file stays editable),
//! headers and footers with a live page number, page breaks, lists, quotes,
//! code and tables. With `format: manuscript` it is laid out in standard
//! manuscript format, like the PDF. The package is written by hand — a handful
//! of XML parts in an uncompressed zip — so no extra libraries are needed.

use crate::attributes::{Manuscript, PageSetup, Paper, RenderOptions};
use crate::pdf::{Block, Seg};

/// Word measures in twentieths of a point ("twips"): 1440 to the inch.
const INCH: u32 = 1440;

/// Render `markdown` to `.docx` bytes. `title` is used when the frontmatter
/// doesn't name one.
pub fn export(markdown: &str, title: &str) -> Vec<u8> {
    let opts = crate::attributes::render_options(markdown);
    let setup = crate::attributes::page_setup(markdown);
    let blocks = crate::pdf::parse(crate::attributes::strip_frontmatter(markdown), &opts);
    let lines: Vec<String> = markdown.lines().map(str::to_owned).collect();
    let words = crate::attributes::count_words(&lines).words;
    let font = crate::attributes::document_defaults(&lines).0;
    let doc = Doc::new(&opts, setup, words, font);
    let title = opts
        .manuscript
        .as_ref()
        .map(|m| m.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| title.to_string());
    let author = opts.manuscript.as_ref().map(|m| m.author.clone()).unwrap_or_default();

    let mut zip = Zip::default();
    zip.add("[Content_Types].xml", CONTENT_TYPES);
    zip.add("_rels/.rels", ROOT_RELS);
    zip.add("docProps/core.xml", &core_properties(&title, &author));
    zip.add("word/_rels/document.xml.rels", DOCUMENT_RELS);
    zip.add("word/styles.xml", &doc.styles());
    zip.add("word/settings.xml", &doc.settings());
    zip.add("word/header1.xml", &doc.header(true));
    zip.add("word/header2.xml", &doc.header(false));
    zip.add("word/footer1.xml", &doc.footer(true));
    zip.add("word/footer2.xml", &doc.footer(false));
    zip.add("word/document.xml", &doc.document(&blocks));
    zip.finish()
}

/// Layout decisions for one export.
struct Doc {
    manuscript: Option<Manuscript>,
    setup: PageSetup,
    words: usize,
    prose: bool,
    font: String,
    /// Page size and margins, in twips.
    page: (u32, u32),
    margin: u32,
}

impl Doc {
    fn new(opts: &RenderOptions, setup: PageSetup, words: usize, font: Option<String>) -> Self {
        let manuscript = opts.manuscript.clone();
        let paper = opts
            .paper
            .unwrap_or(if manuscript.is_some() { Paper::Letter } else { Paper::A4 });
        let page = match paper {
            Paper::Letter => (12240, 15840),
            Paper::A4 => (11906, 16838),
        };
        let font = match (&manuscript, font) {
            (Some(_), _) => "Courier New".to_string(),
            (None, Some(f)) => f,
            (None, None) => "Times New Roman".to_string(),
        };
        Doc {
            margin: if manuscript.is_some() { INCH } else { 1134 }, // 1" or 2 cm
            manuscript,
            setup,
            words,
            prose: opts.prose_paragraphs,
            font,
            page,
        }
    }

    /// Width of the text column, for right-aligned tab stops.
    fn text_width(&self) -> u32 {
        self.page.0 - 2 * self.margin
    }

    fn styles(&self) -> String {
        let font = xml_escape(&self.font);
        let ms = self.manuscript.is_some();
        // Manuscripts: double-spaced, half-inch first-line indents, no gaps.
        // Otherwise: comfortable single spacing with space between paragraphs.
        let normal_ppr = if ms {
            r#"<w:spacing w:before="0" w:after="0" w:line="480" w:lineRule="auto"/><w:ind w:firstLine="720"/>"#
        } else {
            r#"<w:spacing w:before="0" w:after="160" w:line="276" w:lineRule="auto"/>"#
        };
        let heading = |level: u32, size: u32| {
            let (ppr, rpr) = if ms {
                (
                    r#"<w:keepNext/><w:spacing w:before="0" w:after="480" w:line="480" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="center"/>"#.to_string(),
                    String::new(),
                )
            } else {
                (
                    r#"<w:keepNext/><w:spacing w:before="360" w:after="120"/>"#.to_string(),
                    format!(r#"<w:b/><w:sz w:val="{size}"/><w:szCs w:val="{size}"/>"#),
                )
            };
            format!(
                r#"<w:style w:type="paragraph" w:styleId="Heading{level}"><w:name w:val="heading {level}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr>{ppr}<w:outlineLvl w:val="{}"/></w:pPr><w:rPr>{rpr}</w:rPr></w:style>"#,
                level - 1
            )
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="{W}"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:eastAsia="{font}" w:cs="{font}"/><w:sz w:val="24"/><w:szCs w:val="24"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr>{normal_ppr}</w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/><w:pPr>{normal_ppr}</w:pPr></w:style>{h1}{h2}{h3}{h4}<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:ind w:left="720" w:right="720" w:firstLine="0"/></w:pPr><w:rPr><w:i/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/></w:pPr><w:rPr><w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New"/><w:sz w:val="20"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Header"><w:name w:val="header"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Footer"><w:name w:val="footer"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/></w:pPr></w:style><w:style w:type="table" w:styleId="Grid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style></w:styles>"#,
            h1 = heading(1, 36),
            h2 = heading(2, 30),
            h3 = heading(3, 26),
            h4 = heading(4, 24),
        )
    }

    fn settings(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="{W}"><w:defaultTabStop w:val="720"/>{}<w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#,
            if self.has_odd_even() { "<w:evenAndOddHeaders/>" } else { "" }
        )
    }

    /// Whether headers or footers differ between odd and even pages.
    fn has_odd_even(&self) -> bool {
        self.setup.header_odd != self.setup.header_even || self.setup.footer_odd != self.setup.footer_even
    }

    /// The running header for odd (or even) pages: a `.he` line (with `#` as
    /// the page number), or a manuscript's `Surname / TITLE / n` (not on page
    /// one).
    fn header(&self, odd: bool) -> String {
        let text = if odd { &self.setup.header_odd } else { &self.setup.header_even };
        let body = match (text, &self.manuscript) {
            (Some(text), _) => marginal_paragraph(text, "left"),
            (None, Some(m)) => marginal_paragraph(
                &format!("{} / {} / #", m.surname, m.title.to_uppercase()),
                "right",
            ),
            (None, None) => "<w:p/>".to_string(),
        };
        part("w:hdr", &body)
    }

    /// The footer: a `.fo` line, or a centered page number (none in manuscript
    /// format, whose header carries it, nor after `.op`).
    fn footer(&self, odd: bool) -> String {
        let text = if odd { &self.setup.footer_odd } else { &self.setup.footer_even };
        let body = match text {
            Some(text) => marginal_paragraph(text, "left"),
            None if self.manuscript.is_none() && !self.setup.omit_page_numbers => {
                marginal_paragraph("- # -", "center")
            }
            None => "<w:p/>".to_string(),
        };
        part("w:ftr", &body)
    }

    fn document(&self, blocks: &[Block]) -> String {
        let mut body = String::new();
        if let Some(m) = &self.manuscript {
            self.title_page(m, &mut body);
        }
        // The first chapter follows the title on page one.
        let mut first_chapter = self.manuscript.is_some();
        // A `.pa` turns into "page break before" on the next paragraph (a
        // break inside a paragraph of its own leaves a stray empty line).
        let mut pending_break = false;
        for block in blocks {
            let mut xml = String::new();
            match block {
                Block::Heading(level, segs) => {
                    let level = (*level).clamp(1, 4);
                    if self.manuscript.is_some() && level == 1 && !std::mem::take(&mut first_chapter) {
                        // A chapter: a new page, and a fixed spacer to bring the
                        // title a third of the way down (space-before is dropped
                        // at the top of a page by some programs).
                        xml.push_str(&format!(
                            r#"<w:p><w:pPr><w:pageBreakBefore/><w:spacing w:before="0" w:after="0" w:line="{}" w:lineRule="exact"/><w:ind w:firstLine="0"/></w:pPr></w:p>"#,
                            2 * INCH
                        ));
                        pending_break = false;
                    }
                    let (centered, segs) = crate::pdf::take_centered(segs);
                    let jc = if centered { r#"<w:jc w:val="center"/>"# } else { "" };
                    xml.push_str(&paragraph(&format!(r#"<w:pStyle w:val="Heading{level}"/>{jc}"#), &segs, self));
                }
                Block::Para { segs, .. } if crate::pdf::take_centered(segs).0 => {
                    let segs = crate::pdf::take_centered(segs).1;
                    xml.push_str(&paragraph(r#"<w:ind w:firstLine="0"/><w:jc w:val="center"/>"#, &segs, self));
                }
                Block::Para { segs, indent } => {
                    let ppr = if self.manuscript.is_none() && self.prose {
                        // Book-style prose: indents instead of gaps.
                        let ind = if *indent { 360 } else { 0 };
                        format!(r#"<w:spacing w:after="0"/><w:ind w:firstLine="{ind}"/>"#)
                    } else {
                        String::new()
                    };
                    xml.push_str(&paragraph(&ppr, segs, self));
                }
                Block::Item { depth, marker, segs } => {
                    let left = 360 * (*depth as u32 + 1);
                    let mut all = vec![Seg::plain(format!("{}\t", marker.trim_end()))];
                    all.extend(segs.iter().cloned());
                    let ppr = format!(
                        r#"<w:tabs><w:tab w:val="left" w:pos="{left}"/></w:tabs><w:spacing w:after="60"/><w:ind w:left="{left}" w:hanging="360"/>"#
                    );
                    xml.push_str(&paragraph(&ppr, &all, self));
                }
                Block::Code(lines) => {
                    for line in lines {
                        xml.push_str(&paragraph(r#"<w:pStyle w:val="Code"/>"#, &[Seg::plain(line.clone())], self));
                    }
                }
                Block::Quote(segs) => xml.push_str(&paragraph(r#"<w:pStyle w:val="Quote"/>"#, segs, self)),
                Block::Rule => {
                    let mark = if self.manuscript.is_some() { "#" } else { "*   *   *" };
                    xml.push_str(&centered(mark, self));
                }
                Block::Table { header, rows } => xml.push_str(&table(header, rows, self)),
                Block::PageBreak => {
                    pending_break = true;
                    continue;
                }
            }
            if std::mem::take(&mut pending_break) {
                xml = with_page_break(xml);
            }
            body.push_str(&xml);
        }
        if self.manuscript.is_some() {
            body.push_str(&centered("END", self));
        }
        let (w, h) = self.page;
        let m = self.margin;
        let title_pg = if self.manuscript.is_some() { "<w:titlePg/>" } else { "" };
        let refs = r#"<w:headerReference w:type="default" r:id="rIdHeader"/><w:headerReference w:type="even" r:id="rIdHeaderEven"/><w:footerReference w:type="default" r:id="rIdFooter"/><w:footerReference w:type="even" r:id="rIdFooterEven"/>"#;
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}<w:sectPr>{refs}<w:pgSz w:w="{w}" w:h="{h}"/><w:pgMar w:top="{m}" w:right="{m}" w:bottom="{m}" w:left="{m}" w:header="720" w:footer="720" w:gutter="0"/>{title_pg}</w:sectPr></w:body></w:document>"#
        )
    }

    /// Page one of a manuscript: name and contact details top left with the
    /// word count on the right, then the title and byline halfway down.
    fn title_page(&self, m: &Manuscript, body: &mut String) {
        let tab = self.text_width();
        let single = format!(
            r#"<w:tabs><w:tab w:val="right" w:pos="{tab}"/></w:tabs><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/>"#
        );
        let count = format!("about {} words", crate::attributes::group_digits(approximate(self.words)));
        let mut contact = std::iter::once(&m.author)
            .chain(&m.contact)
            .filter(|l| !l.is_empty());
        let first = contact.next().cloned().unwrap_or_default();
        body.push_str(&paragraph(&single, &[Seg::plain(format!("{first}\t{count}"))], self));
        for line in contact {
            body.push_str(&paragraph(&single, &[Seg::plain(line.clone())], self));
        }
        // About halfway down the page.
        let gap = (self.page.1 / 2).saturating_sub(self.margin + 1800);
        body.push_str(&format!(
            r#"<w:p><w:pPr><w:spacing w:before="{gap}" w:after="0"/><w:ind w:firstLine="0"/><w:jc w:val="center"/></w:pPr>{}</w:p>"#,
            runs(&[Seg::plain(m.title.clone())], self)
        ));
        body.push_str(&centered(&format!("by {}", m.byline), self));
        body.push_str(r#"<w:p><w:pPr><w:ind w:firstLine="0"/></w:pPr></w:p>"#);
    }
}

/// Start `xml` (one or more blocks) on a new page: its first paragraph gets
/// "page break before"; anything else (a table) is preceded by a break.
fn with_page_break(xml: String) -> String {
    let Some(rest) = xml.strip_prefix("<w:p><w:pPr>") else {
        return format!(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>{xml}"#);
    };
    // Schema order: the paragraph style first, then pageBreakBefore.
    let style_end = if rest.starts_with("<w:pStyle") {
        rest.find("/>").map_or(0, |i| i + 2)
    } else {
        0
    };
    format!(
        "<w:p><w:pPr>{}<w:pageBreakBefore/>{}",
        &rest[..style_end],
        &rest[style_end..]
    )
}

/// A manuscript's word count: the nearest hundred (exact below a hundred).
fn approximate(words: usize) -> usize {
    if words < 100 { words } else { (words + 50) / 100 * 100 }
}

/// A paragraph with properties `ppr` and styled text `segs`.
fn paragraph(ppr: &str, segs: &[Seg], doc: &Doc) -> String {
    format!("<w:p><w:pPr>{ppr}</w:pPr>{}</w:p>", runs(segs, doc))
}

fn centered(text: &str, doc: &Doc) -> String {
    paragraph(r#"<w:ind w:firstLine="0"/><w:jc w:val="center"/>"#, &[Seg::plain(text)], doc)
}

/// Styled runs; a `\n` in the text is a line break, a `\t` a tab.
fn runs(segs: &[Seg], doc: &Doc) -> String {
    let underline_italics = doc.manuscript.as_ref().is_some_and(|m| m.underline_italics);
    let mut out = String::new();
    for s in segs {
        let (italic, underline) = if underline_italics {
            (false, s.underline || s.italic)
        } else {
            (s.italic, s.underline)
        };
        let mut rpr = String::new();
        if s.bold {
            rpr.push_str("<w:b/>");
        }
        if italic {
            rpr.push_str("<w:i/>");
        }
        // Schema order: b, i, strike, …, u.
        if s.strike {
            rpr.push_str("<w:strike/>");
        }
        if underline {
            rpr.push_str(r#"<w:u w:val="single"/>"#);
        }
        out.push_str("<w:r>");
        if !rpr.is_empty() {
            out.push_str(&format!("<w:rPr>{rpr}</w:rPr>"));
        }
        for (i, line) in s.text.split('\n').enumerate() {
            if i > 0 {
                out.push_str("<w:br/>");
            }
            for (j, piece) in line.split('\t').enumerate() {
                if j > 0 {
                    out.push_str("<w:tab/>");
                }
                if !piece.is_empty() {
                    out.push_str(&format!(r#"<w:t xml:space="preserve">{}</w:t>"#, xml_escape(piece)));
                }
            }
        }
        out.push_str("</w:r>");
    }
    out
}

/// A header or footer paragraph: `text` with each `#` as a live page number.
fn marginal_paragraph(text: &str, align: &str) -> String {
    let mut runs = String::new();
    for (i, piece) in text.split('#').enumerate() {
        if i > 0 {
            runs.push_str(
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
            );
        }
        if !piece.is_empty() {
            runs.push_str(&format!(r#"<w:r><w:t xml:space="preserve">{}</w:t></w:r>"#, xml_escape(piece)));
        }
    }
    format!(r#"<w:p><w:pPr><w:pStyle w:val="Header"/><w:jc w:val="{align}"/></w:pPr>{runs}</w:p>"#)
}

/// A header (`w:hdr`) or footer (`w:ftr`) part.
fn part(tag: &str, body: &str) -> String {
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<{tag} xmlns:w="{W}" xmlns:r="{R}">{body}</{tag}>"#)
}

/// A table with a bold header row and a grid.
fn table(header: &[String], rows: &[Vec<String>], doc: &Doc) -> String {
    let ncols = header.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if ncols == 0 {
        return String::new();
    }
    let col = doc.text_width() / ncols as u32;
    let cell = |text: &str, bold: bool| {
        let seg = Seg {
            text: text.to_string(),
            bold,
            ..Seg::default()
        };
        format!(
            r#"<w:tc><w:tcPr><w:tcW w:w="{col}" w:type="dxa"/></w:tcPr>{}</w:tc>"#,
            paragraph(r#"<w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:firstLine="0"/>"#, &[seg], doc)
        )
    };
    let row = |cells: &[String], bold: bool| {
        let tcs: String = (0..ncols)
            .map(|c| cell(cells.get(c).map(String::as_str).unwrap_or(""), bold))
            .collect();
        format!("<w:tr>{tcs}</w:tr>")
    };
    let grid: String = (0..ncols).map(|_| format!(r#"<w:gridCol w:w="{col}"/>"#)).collect();
    let body: String = rows.iter().map(|r| row(r, false)).collect();
    format!(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid>{grid}</w:tblGrid>{}{body}</w:tbl><w:p/>"#,
        row(header, true)
    )
}

fn core_properties(title: &str, author: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{}</dc:title><dc:creator>{}</dc:creator></cp:coreProperties>"#,
        xml_escape(title),
        xml_escape(author)
    )
}

/// Escape text for XML, dropping characters XML can't hold.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/footer1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/word/header2.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/footer2.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#;

const DOCUMENT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdSettings" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/><Relationship Id="rIdHeader" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/><Relationship Id="rIdFooter" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/><Relationship Id="rIdHeaderEven" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header2.xml"/><Relationship Id="rIdFooterEven" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer2.xml"/></Relationships>"#;

/// A minimal zip writer: files stored uncompressed, which every Office
/// program reads.
#[derive(Default)]
struct Zip {
    data: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl Zip {
    fn add(&mut self, name: &str, content: &str) {
        let bytes = content.as_bytes();
        let crc = crc32(bytes);
        let offset = self.data.len() as u32;
        let size = bytes.len() as u32;
        // Local file header.
        self.data.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        self.data.extend_from_slice(&header_fields(crc, size, name));
        self.data.extend_from_slice(name.as_bytes());
        self.data.extend_from_slice(bytes);
        // Central directory entry.
        self.central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes()); // made by
        self.central.extend_from_slice(&header_fields(crc, size, name));
        self.central.extend_from_slice(&[0; 6]); // comment len, disk, internal attrs
        self.central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.count += 1;
    }

    fn finish(mut self) -> Vec<u8> {
        let dir_offset = self.data.len() as u32;
        let dir_size = self.central.len() as u32;
        self.data.append(&mut self.central);
        self.data.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.data.extend_from_slice(&[0; 4]); // disk numbers
        self.data.extend_from_slice(&self.count.to_le_bytes());
        self.data.extend_from_slice(&self.count.to_le_bytes());
        self.data.extend_from_slice(&dir_size.to_le_bytes());
        self.data.extend_from_slice(&dir_offset.to_le_bytes());
        self.data.extend_from_slice(&[0; 2]); // comment length
        self.data
    }
}

/// The fields shared by local and central headers: version, flags (UTF-8
/// names), method 0 (stored), a fixed 1980-01-01 timestamp, CRC and sizes,
/// and the name / extra lengths.
fn header_fields(crc: u32, size: u32, name: &str) -> Vec<u8> {
    let mut f = Vec::with_capacity(26);
    f.extend_from_slice(&20u16.to_le_bytes());
    f.extend_from_slice(&0x0800u16.to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes()); // time
    f.extend_from_slice(&0x0021u16.to_le_bytes()); // date: 1980-01-01
    f.extend_from_slice(&crc.to_le_bytes());
    f.extend_from_slice(&size.to_le_bytes());
    f.extend_from_slice(&size.to_le_bytes());
    f.extend_from_slice(&(name.len() as u16).to_le_bytes());
    f.extend_from_slice(&0u16.to_le_bytes());
    f
}

/// CRC-32 (IEEE), as zip requires.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn export_is_a_zip_with_the_word_parts() {
        let bytes = export("# Title\n\nSome **bold** & *italic* text.\n\n.pa\n\nMore.", "Test");
        assert!(bytes.starts_with(b"PK\x03\x04"));
        let text = String::from_utf8_lossy(&bytes);
        for part in ["[Content_Types].xml", "word/document.xml", "word/styles.xml", "word/header1.xml"] {
            assert!(text.contains(part), "{part} missing");
        }
        assert!(text.contains("<w:b/>") && text.contains("<w:i/>"));
        assert!(text.contains("&amp;"), "text is escaped");
        assert!(text.contains(r#"<w:pageBreakBefore/><w:spacing w:before="0" w:after="0" w:line="240""#) || text.contains("<w:pPr><w:pageBreakBefore/>"), "page break on the next paragraph");
        assert!(text.contains("Times New Roman"));
    }

    #[test]
    fn manuscript_export_uses_manuscript_layout() {
        let md = "---\nformat: manuscript\ntitle: The Red House\nauthor: Jane Q. Writer\ncontact:\n  - jane@example.com\n---\n# One\nIt was dark.\n\n***\n\nThe end.";
        let text = String::from_utf8_lossy(&export(md, "x")).into_owned();
        assert!(text.contains("Courier New"));
        assert!(text.contains(r#"w:line="480""#), "double-spaced");
        assert!(text.contains(r#"<w:ind w:firstLine="720"/>"#), "half-inch indents");
        assert!(text.contains("Jane Q. Writer<"), "contact block");
        assert!(text.contains("about 6 words"), "word count");
        assert!(text.contains("Writer / THE RED HOUSE / "), "running header");
        assert!(text.contains("<w:titlePg/>"), "no header on page one");
        assert!(text.contains(r#"w:w="12240""#), "US Letter");
        assert!(text.contains(">END<"));
    }
}
