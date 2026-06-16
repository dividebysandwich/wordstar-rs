//! Rasterize the document to page images for the in-terminal graphical preview.
//!
//! Reuses the Markdown→blocks parser from [`crate::pdf`] and lays each block out
//! with `cosmic-text` using the system fonts, so the preview shows real
//! proportional type with actual bold/italic and scaled headings. Content is
//! paginated into A4-proportioned pages (breaking between blocks, slicing only
//! blocks taller than a page), which the app shows one at a time, zoomable and
//! scrollable. If no system fonts are available the list is empty and the caller
//! falls back to the text preview.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight,
};
use image::{Rgba, RgbaImage, imageops};

use crate::pdf::{Block, Seg, parse, strip_frontmatter};

// Layout constants, in image pixels.
const CONTENT_W: f32 = 1000.0;
const MARGIN: f32 = 56.0;
const BODY: f32 = 23.0;
const LINE: f32 = 1.4; // line-height multiplier

const PAGE_W: u32 = CONTENT_W as u32 + 2 * MARGIN as u32;
// A4 proportion (1 : 1.414).
const PAGE_H: u32 = 1573;

// Colors.
const TEXT: [u8; 3] = [20, 20, 20];
const HEADING: [u8; 3] = [0x16, 0x3a, 0x6b];
const QUOTE: [u8; 3] = [90, 90, 90];
const CODE: [u8; 3] = [40, 60, 40];
const RULE: [u8; 4] = [170, 170, 170, 255];
const PAPER: [u8; 4] = [255, 255, 255, 255];

thread_local! {
    static FONTS: RefCell<Option<FontSystem>> = const { RefCell::new(None) };
    static CACHE: RefCell<SwashCache> = RefCell::new(SwashCache::new());
}

/// An incremental rasterization job: renders one block per `step` call (within a
/// time budget) so the UI can show a progress modal for long documents.
pub struct Job {
    blocks: Vec<Block>,
    next: usize,
    strips: Vec<RgbaImage>,
}

impl Job {
    /// Start a job, or `None` if no system fonts are available (→ text preview).
    pub fn new(markdown: &str) -> Option<Job> {
        let blocks = parse(strip_frontmatter(markdown));
        let have_fonts = FONTS.with(|fonts| {
            let mut fonts = fonts.borrow_mut();
            if fonts.is_none() {
                *fonts = Some(FontSystem::new());
            }
            !fonts.as_ref().unwrap().db().is_empty()
        });
        if !have_fonts {
            return None;
        }
        let cap = blocks.len();
        Some(Job {
            blocks,
            next: 0,
            strips: Vec::with_capacity(cap),
        })
    }

    /// Fraction complete, 0.0..=1.0.
    pub fn progress(&self) -> f32 {
        if self.blocks.is_empty() {
            1.0
        } else {
            self.next as f32 / self.blocks.len() as f32
        }
    }

    pub fn is_done(&self) -> bool {
        self.next >= self.blocks.len()
    }

    /// Rasterize blocks until `budget` elapses (at least one per call).
    pub fn step(&mut self, budget: Duration) {
        let start = Instant::now();
        FONTS.with(|fonts| {
            let mut fonts = fonts.borrow_mut();
            let fs = fonts.as_mut().unwrap();
            CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                while self.next < self.blocks.len() {
                    let strip = build_strip(fs, &mut cache, &self.blocks[self.next]);
                    self.strips.push(strip);
                    self.next += 1;
                    if start.elapsed() >= budget {
                        break;
                    }
                }
            });
        });
    }

    /// Consume the finished job and paginate the strips into pages.
    pub fn finish(self) -> Vec<RgbaImage> {
        paginate(&self.strips)
    }
}

fn heading_px(level: u8) -> f32 {
    match level {
        1 => 40.0,
        2 => 33.0,
        3 => 28.0,
        4 => 25.0,
        _ => BODY,
    }
}

/// Render a single block to a full-content-width strip image.
fn build_strip(fs: &mut FontSystem, cache: &mut SwashCache, block: &Block) -> RgbaImage {
    match block {
        Block::Heading(level, segs) => {
            let bold: Vec<Seg> = segs
                .iter()
                .map(|s| Seg {
                    text: s.text.clone(),
                    bold: true,
                    italic: s.italic,
                })
                .collect();
            text_strip(
                fs,
                cache,
                &bold,
                heading_px(*level),
                Family::SansSerif,
                0.0,
                HEADING,
            )
        }
        Block::Para(segs) => text_strip(fs, cache, segs, BODY, Family::SansSerif, 0.0, TEXT),
        Block::Item {
            depth,
            marker,
            segs,
        } => {
            let indent = (*depth as f32) * 28.0;
            let mut all = vec![Seg {
                text: marker.clone(),
                bold: false,
                italic: false,
            }];
            all.extend(segs.iter().cloned());
            text_strip(fs, cache, &all, BODY, Family::SansSerif, indent, TEXT)
        }
        Block::Code(lines) => {
            let seg = Seg {
                text: lines.join("\n"),
                bold: false,
                italic: false,
            };
            text_strip(fs, cache, &[seg], BODY - 3.0, Family::Monospace, 0.0, CODE)
        }
        Block::Quote(segs) => {
            let italic: Vec<Seg> = segs
                .iter()
                .map(|s| Seg {
                    text: s.text.clone(),
                    bold: s.bold,
                    italic: true,
                })
                .collect();
            text_strip(fs, cache, &italic, BODY, Family::Serif, 28.0, QUOTE)
        }
        Block::Rule => rule_strip(),
        Block::Table { header, rows } => {
            let seg = Seg {
                text: ascii_table(header, rows),
                bold: false,
                italic: false,
            };
            text_strip(fs, cache, &[seg], BODY - 3.0, Family::Monospace, 0.0, TEXT)
        }
    }
}

/// Render `segs` into a strip of width `CONTENT_W`, the text indented by `indent`.
fn text_strip(
    fs: &mut FontSystem,
    cache: &mut SwashCache,
    segs: &[Seg],
    size: f32,
    family: Family<'static>,
    indent: f32,
    color: [u8; 3],
) -> RgbaImage {
    let buffer = text_buffer(fs, segs, size, family, CONTENT_W - indent);
    let h = buffer
        .layout_runs()
        .map(|r| r.line_top + r.line_height)
        .fold(0.0, f32::max)
        .ceil()
        .max(1.0) as u32;
    let mut strip = RgbaImage::from_pixel(PAGE_W - 2 * MARGIN as u32, h, Rgba(PAPER));
    let col = Color::rgb(color[0], color[1], color[2]);
    buffer.draw(fs, cache, col, |gx, gy, _, _, c| {
        blend(&mut strip, indent as i32 + gx, gy, c);
    });
    strip
}

fn rule_strip() -> RgbaImage {
    let h = BODY as u32;
    let w = PAGE_W - 2 * MARGIN as u32;
    let mut s = RgbaImage::from_pixel(w, h, Rgba(PAPER));
    hline(&mut s, 0, w, h / 2);
    s
}

/// Stack the strips onto A4 pages, breaking between blocks (and slicing a block
/// that is taller than a whole page).
fn paginate(strips: &[RgbaImage]) -> Vec<RgbaImage> {
    let margin = MARGIN as u32;
    let gap = (BODY * 0.55) as u32;
    let content_h = PAGE_H - 2 * margin;
    let new_page = || RgbaImage::from_pixel(PAGE_W, PAGE_H, Rgba(PAPER));

    let mut pages = Vec::new();
    let mut page = new_page();
    let mut y = margin;

    for strip in strips {
        let sh = strip.height();
        if sh <= content_h {
            if y > margin && y + sh > margin + content_h {
                pages.push(std::mem::replace(&mut page, new_page()));
                y = margin;
            }
            imageops::replace(&mut page, strip, margin as i64, y as i64);
            y += sh + gap;
        } else {
            // Block taller than a page: slice it across pages.
            let mut row = 0u32;
            while row < sh {
                if y >= margin + content_h {
                    pages.push(std::mem::replace(&mut page, new_page()));
                    y = margin;
                }
                let avail = margin + content_h - y;
                let take = avail.min(sh - row);
                let slice = imageops::crop_imm(strip, 0, row, strip.width(), take).to_image();
                imageops::replace(&mut page, &slice, margin as i64, y as i64);
                row += take;
                y += take;
            }
            y += gap;
        }
    }
    pages.push(page);
    pages
}

/// Build a shaped text buffer for `segs` at `size`, wrapping to `wrap_w`.
fn text_buffer(
    fs: &mut FontSystem,
    segs: &[Seg],
    size: f32,
    family: Family<'static>,
    wrap_w: f32,
) -> Buffer {
    let mut buffer = Buffer::new(fs, Metrics::new(size, size * LINE));
    buffer.set_size(fs, Some(wrap_w.max(50.0)), None);
    let default = Attrs::new().family(family);
    let spans: Vec<(String, Attrs)> = segs
        .iter()
        .map(|s| (s.text.clone(), seg_attrs(s, family)))
        .collect();
    buffer.set_rich_text(
        fs,
        spans.iter().map(|(t, a)| (t.as_str(), a.clone())),
        &default,
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(fs, false);
    buffer
}

fn seg_attrs(seg: &Seg, family: Family<'static>) -> Attrs<'static> {
    let mut a = Attrs::new().family(family);
    if seg.bold {
        a = a.weight(Weight::BOLD);
    }
    if seg.italic {
        a = a.style(Style::Italic);
    }
    a
}

/// Alpha-blend a coverage pixel onto the (opaque) paper image.
fn blend(img: &mut RgbaImage, x: i32, y: i32, c: Color) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let a = c.a() as f32 / 255.0;
    if a <= 0.0 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    p[0] = (c.r() as f32 * a + p[0] as f32 * (1.0 - a)) as u8;
    p[1] = (c.g() as f32 * a + p[1] as f32 * (1.0 - a)) as u8;
    p[2] = (c.b() as f32 * a + p[2] as f32 * (1.0 - a)) as u8;
    p[3] = 255;
}

fn hline(img: &mut RgbaImage, x0: u32, x1: u32, y: u32) {
    if y >= img.height() {
        return;
    }
    for x in x0..x1.min(img.width()) {
        img.put_pixel(x, y, Rgba(RULE));
    }
}

/// Render a Markdown table as a monospaced ASCII grid (so it aligns in the
/// fixed-pitch table font).
fn ascii_table(header: &[String], rows: &[Vec<String>]) -> String {
    let ncols = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if ncols == 0 {
        return String::new();
    }
    let mut w = vec![3usize; ncols];
    let fit = |row: &[String], w: &mut [usize]| {
        for (c, cell) in row.iter().enumerate() {
            if c < ncols {
                w[c] = w[c].max(cell.chars().count().min(28));
            }
        }
    };
    fit(header, &mut w);
    for r in rows {
        fit(r, &mut w);
    }
    let pad = |s: &str, width: usize| -> String {
        let n = s.chars().count();
        if n > width {
            s.chars().take(width).collect()
        } else {
            format!("{s}{}", " ".repeat(width - n))
        }
    };
    let border: String = {
        let mut s = String::from("+");
        for width in &w {
            s.push_str(&"-".repeat(width + 2));
            s.push('+');
        }
        s
    };
    let row_line = |cells: &[String]| -> String {
        let mut s = String::from("|");
        for (c, width) in w.iter().enumerate() {
            let raw = cells.get(c).map(String::as_str).unwrap_or("");
            s.push(' ');
            s.push_str(&pad(raw, *width));
            s.push_str(" |");
        }
        s
    };
    let mut out = vec![border.clone(), row_line(header), border.clone()];
    for r in rows {
        out.push(row_line(r));
    }
    out.push(border);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_table_is_aligned() {
        let t = ascii_table(&["A".into(), "Bb".into()], &[vec!["1".into(), "2".into()]]);
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with('+') && lines[0].ends_with('+'));
        let w = lines[0].chars().count();
        assert!(
            lines.iter().all(|l| l.chars().count() == w),
            "ragged table:\n{t}"
        );
    }

    #[test]
    fn paginates_long_document_into_multiple_pages() {
        // Many paragraphs should span more than one page.
        let md = (0..120)
            .map(|i| format!("Paragraph number {i}.\n\n"))
            .collect::<String>();
        let pages = match Job::new(&md) {
            Some(mut job) => {
                job.step(Duration::from_secs(60));
                job.finish()
            }
            None => return, // no system fonts in this environment
        };
        assert!(
            pages.len() >= 2,
            "expected multiple pages, got {}",
            pages.len()
        );
        for p in &pages {
            assert_eq!(p.width(), PAGE_W);
            assert_eq!(p.height(), PAGE_H);
        }
    }
}
