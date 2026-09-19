//! Soft-wrap layout that mirrors `ratatui-textarea`'s internal wrapping.
//!
//! The widget computes word wrap privately, so to relate on-screen rows to the
//! document — the flag column, mouse clicks, the marked-block highlight, and the
//! page/line metrics on the status line — we reproduce the exact algorithm here.
//! Keep this in sync with the pinned `ratatui-textarea` version.

use ratatui_textarea::WrapMode;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// One on-screen (visual) row of a logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    /// Index of the logical line (document row) this visual row shows.
    pub line: usize,
    /// Byte range of the row's text within its logical line.
    pub start: usize,
    pub end: usize,
    /// True if this is the last visual row of its logical line (i.e. the line
    /// ends here with a hard return — a paragraph break).
    pub last: bool,
}

impl VisualRow {
    /// The row's text within `lines`.
    pub fn text<'a>(&self, lines: &'a [String]) -> &'a str {
        lines
            .get(self.line)
            .and_then(|l| l.get(self.start..self.end))
            .unwrap_or("")
    }

    /// The character column (within the logical line) where this row begins.
    pub fn start_col(&self, lines: &[String]) -> usize {
        lines
            .get(self.line)
            .and_then(|l| l.get(..self.start))
            .map_or(0, |s| s.chars().count())
    }
}

/// Compute the visual-row layout for `lines` at the given wrap `mode`/`width`.
pub fn layout(lines: &[String], mode: WrapMode, width: usize, tab: u8) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let ranges = line_ranges(line, mode, width, tab);
        let n = ranges.len();
        for (i, (start, end)) in ranges.into_iter().enumerate() {
            rows.push(VisualRow {
                line: idx,
                start,
                end,
                last: i + 1 == n,
            });
        }
    }
    rows
}

/// The character offset within a row's `text` shown at display column `x` (a
/// click on any cell of a wide character or tab lands on that character).
pub fn x_to_char(text: &str, x: usize, tab: u8) -> usize {
    let mut col = 0usize;
    for (i, c) in text.chars().enumerate() {
        let next = col + char_display_width(c, col, tab);
        if x < next {
            return i;
        }
        col = next;
    }
    text.chars().count()
}

/// The display column at which character `offset` of a row's `text` starts.
pub fn char_to_x(text: &str, offset: usize, tab: u8) -> usize {
    let mut col = 0usize;
    for c in text.chars().take(offset) {
        col += char_display_width(c, col, tab);
    }
    col
}

/// Display width of `c` when drawn at column `col` (tabs pad to the next stop).
fn char_display_width(c: char, col: usize, tab: u8) -> usize {
    display_width_from(c.encode_utf8(&mut [0; 4]), col, tab)
}

// --- The following is copied from ratatui-textarea's `wrap.rs` so our layout
// --- matches the widget exactly. ---

#[derive(Clone, Copy)]
struct Chunk {
    start: usize,
    end: usize,
}

fn line_ranges(line: &str, mode: WrapMode, width: usize, tab_len: u8) -> Vec<(usize, usize)> {
    if mode == WrapMode::None {
        return vec![(0, line.len())];
    }
    let width = width.max(1);
    let mut out = match mode {
        WrapMode::None => vec![(0, line.len())],
        WrapMode::Glyph => {
            let mut chunks = Vec::new();
            split_range_by_grapheme_width(line, 0, line.len(), width, tab_len, &mut chunks);
            chunks
        }
        WrapMode::Word => wrap_word_chunks(line, width, tab_len, false),
        WrapMode::WordOrGlyph => wrap_word_chunks(line, width, tab_len, true),
    };
    if out.is_empty() {
        out.push((0, 0));
    }
    out
}

fn wrap_word_chunks(
    line: &str,
    width: usize,
    tab_len: u8,
    fallback_to_glyph: bool,
) -> Vec<(usize, usize)> {
    let chunks: Vec<_> = UnicodeSegmentation::split_word_bound_indices(line)
        .map(|(start, text)| Chunk {
            start,
            end: start + text.len(),
        })
        .collect();

    if chunks.is_empty() {
        return vec![(0, 0)];
    }

    let mut out = Vec::new();
    let mut i = 0usize;
    let mut seg_start = chunks[0].start;
    let mut seg_end = seg_start;
    let mut seg_width = 0usize;

    while i < chunks.len() {
        let chunk = chunks[i];
        if seg_end == seg_start {
            seg_start = chunk.start;
        }

        let chunk_width = display_width_from(&line[chunk.start..chunk.end], seg_width, tab_len);
        if seg_width + chunk_width <= width {
            seg_end = chunk.end;
            seg_width += chunk_width;
            i += 1;
            continue;
        }

        if seg_end > seg_start {
            out.push((seg_start, seg_end));
            seg_start = seg_end;
            seg_width = 0;
            continue;
        }

        if fallback_to_glyph {
            split_range_by_grapheme_width(line, chunk.start, chunk.end, width, tab_len, &mut out);
        } else {
            out.push((chunk.start, chunk.end));
        }

        i += 1;
        seg_start = chunk.end;
        seg_end = chunk.end;
        seg_width = 0;
    }

    if seg_end > seg_start {
        out.push((seg_start, seg_end));
    }

    out
}

fn split_range_by_grapheme_width(
    line: &str,
    start: usize,
    end: usize,
    width: usize,
    tab_len: u8,
    out: &mut Vec<(usize, usize)>,
) {
    let mut segment_start = start;
    while segment_start < end {
        let mut segment_end = segment_start;
        let mut segment_width = 0usize;

        for (offset, grapheme) in
            UnicodeSegmentation::grapheme_indices(&line[segment_start..end], true)
        {
            let grapheme_start = segment_start + offset;
            let grapheme_end = grapheme_start + grapheme.len();
            let next_width = display_width_to(grapheme, segment_width, tab_len);
            let grapheme_width = next_width.saturating_sub(segment_width);

            if segment_end != segment_start && segment_width + grapheme_width > width {
                break;
            }

            segment_end = grapheme_end;
            segment_width = next_width;
            if segment_width > width {
                break;
            }
        }

        if segment_end == segment_start {
            if let Some(ch) = line[segment_start..end].chars().next() {
                segment_end = segment_start + ch.len_utf8();
            } else {
                break;
            }
        }

        out.push((segment_start, segment_end));
        segment_start = segment_end;
    }
}

fn display_width_from(text: &str, start_width: usize, tab_len: u8) -> usize {
    display_width_to(text, start_width, tab_len).saturating_sub(start_width)
}

fn display_width_to(text: &str, mut width: usize, tab_len: u8) -> usize {
    for c in text.chars() {
        if c == '\t' {
            if tab_len > 0 {
                let tab = tab_len as usize;
                let pad = tab - (width % tab);
                width += pad;
            }
        } else {
            width += c.width().unwrap_or(0);
        }
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_wrap_is_one_row_per_line() {
        let l = lines(&["short", "another line here"]);
        let rows = layout(&l, WrapMode::None, 10, 4);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].last && rows[1].last);
    }

    #[test]
    fn rows_carry_their_line_and_byte_range() {
        let l = lines(&["alpha beta gamma", "x"]);
        let rows = layout(&l, WrapMode::Word, 11, 4);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].line, rows[0].text(&l)), (0, "alpha beta "));
        assert_eq!((rows[1].line, rows[1].text(&l)), (0, "gamma"));
        assert_eq!(rows[1].start_col(&l), 11);
        assert_eq!((rows[2].line, rows[2].text(&l)), (1, "x"));
    }

    #[test]
    fn columns_map_both_ways_with_wide_chars_and_tabs() {
        // "a", a two-cell CJK character, then a tab to the next stop of 4.
        let t = "a中\tb";
        assert_eq!(char_to_x(t, 0, 4), 0);
        assert_eq!(char_to_x(t, 1, 4), 1);
        assert_eq!(char_to_x(t, 2, 4), 3);
        assert_eq!(char_to_x(t, 3, 4), 4);
        assert_eq!(x_to_char(t, 2, 4), 1, "second cell of the wide char");
        assert_eq!(x_to_char(t, 3, 4), 2, "the tab cell");
        assert_eq!(x_to_char(t, 4, 4), 3);
        assert_eq!(x_to_char(t, 40, 4), 4, "past the end clamps");
    }

    #[test]
    fn word_wrap_marks_only_last_row_as_paragraph_end() {
        // One logical line that wraps into multiple rows at width 10.
        let l = lines(&["alpha beta gamma delta"]);
        let rows = layout(&l, WrapMode::Word, 10, 4);
        assert!(rows.len() >= 2, "should wrap into multiple rows");
        // Only the final visual row is the paragraph end.
        let lasts: Vec<bool> = rows.iter().map(|r| r.last).collect();
        assert_eq!(*lasts.last().unwrap(), true);
        assert!(lasts[..lasts.len() - 1].iter().all(|b| !b));
    }
}
