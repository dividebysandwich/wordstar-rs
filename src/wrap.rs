//! Soft-wrap layout that mirrors `ratatui-textarea`'s internal wrapping.
//!
//! The widget computes word wrap privately, so to align the right-border flag
//! column (paragraph vs. wrapped-line indicators) with the on-screen rows we
//! reproduce the exact algorithm here. Keep this in sync with the pinned
//! `ratatui-textarea` version.

use ratatui_textarea::WrapMode;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// One on-screen (visual) row of a logical line.
pub struct VisualRow {
    /// Index of the logical line (buffer line) this row belongs to.
    pub logical: usize,
    /// One past the last character column of this row.
    pub end_col: usize,
    /// True if this is the last visual row of its logical line (i.e. the line
    /// ends here with a hard return — a paragraph break).
    pub last: bool,
}

/// Compute the visual-row layout for `lines` at the given wrap `mode`/`width`.
pub fn layout(lines: &[String], mode: WrapMode, width: usize, tab: u8) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    for (logical, line) in lines.iter().enumerate() {
        let ranges = line_ranges(line, mode, width, tab);
        let n = ranges.len();
        let mut start_col = 0usize;
        for (i, (sb, eb)) in ranges.iter().copied().enumerate() {
            let end_col = start_col + line[sb..eb].chars().count();
            rows.push(VisualRow {
                logical,
                end_col,
                last: i + 1 == n,
            });
            start_col = end_col;
        }
    }
    rows
}

/// Index of the visual row containing char column `col` of logical line `logical`.
pub fn visual_index(rows: &[VisualRow], logical: usize, col: usize) -> usize {
    let mut last_match = 0usize;
    let mut found = false;
    for (i, r) in rows.iter().enumerate() {
        if r.logical == logical {
            last_match = i;
            found = true;
            if col < r.end_col {
                return i;
            }
        } else if found {
            break;
        }
    }
    last_match
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

    #[test]
    fn visual_index_finds_cursor_row() {
        let l = lines(&["alpha beta gamma delta"]);
        let rows = layout(&l, WrapMode::Word, 10, 4);
        // Column 0 is on the first visual row.
        assert_eq!(visual_index(&rows, 0, 0), 0);
        // A column near the end lands on a later visual row.
        let end = rows.last().unwrap().end_col;
        assert_eq!(visual_index(&rows, 0, end), rows.len() - 1);
    }
}
