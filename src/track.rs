//! Keep document positions attached to their text across an edit.
//!
//! Place markers, the "previous position" and the ends of a marked block are
//! stored as `(row, col)` positions. The text widget doesn't report its edits,
//! so after each keystroke we compare the text before and after, find the one
//! span that changed, and move each position the way the text around it moved:
//! positions before the change stay put, positions after it shift with the
//! text, and positions inside replaced text collapse to where the change began.

/// The difference between two versions of the document, as one changed span.
pub struct TextChange<'a> {
    new: &'a [String],
    /// Unchanged lines at the top and bottom.
    top: usize,
    old_bottom_start: usize,
    new_bottom_start: usize,
    /// Characters of the changed lines (joined with `\n`), before and after.
    old_len: usize,
    new_len: usize,
    old_lines: &'a [String],
    /// Characters unchanged at the start and end of the changed lines.
    prefix: usize,
    suffix: usize,
}

impl<'a> TextChange<'a> {
    /// The change from `old` to `new`, or `None` if the text is the same.
    /// `cursor` (in `new`, after the edit) breaks ties where the text alone is
    /// ambiguous — deleting "two " from "one two three" looks the same as
    /// deleting "wo t" — since edits happen at the cursor.
    pub fn new(old: &'a [String], new: &'a [String], cursor: (usize, usize)) -> Option<Self> {
        if old == new {
            return None;
        }
        let top = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let room = old.len().min(new.len()) - top;
        let bottom = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(room)
            .take_while(|(a, b)| a == b)
            .count();
        let old_chars: Vec<char> = old[top..old.len() - bottom].join("\n").chars().collect();
        let new_chars: Vec<char> = new[top..new.len() - bottom].join("\n").chars().collect();
        let common_suffix = |from: usize| {
            old_chars[from..]
                .iter()
                .rev()
                .zip(new_chars[from..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count()
        };
        let mut prefix = old_chars.iter().zip(&new_chars).take_while(|(a, b)| a == b).count();
        let mut suffix = common_suffix(prefix);
        let new_bottom_start = new.len() - bottom;
        if (top..new_bottom_start).contains(&cursor.0) {
            let at: usize = new[top..cursor.0]
                .iter()
                .map(|l| l.chars().count() + 1)
                .sum::<usize>()
                + cursor.1;
            // Starting the change at the cursor instead, if that explains the
            // edit just as briefly.
            if at < prefix {
                let alt = common_suffix(at);
                if old_chars.len() - at - alt == old_chars.len() - prefix - suffix {
                    prefix = at;
                    suffix = alt;
                }
            }
        }
        Some(Self {
            new,
            top,
            old_bottom_start: old.len() - bottom,
            new_bottom_start,
            old_len: old_chars.len(),
            new_len: new_chars.len(),
            old_lines: old,
            prefix,
            suffix,
        })
    }

    /// Where the text at `(row, col)` before the change is afterwards.
    pub fn map(&self, (row, col): (usize, usize)) -> (usize, usize) {
        if row < self.top {
            return (row, col);
        }
        if row >= self.old_bottom_start {
            return (row + self.new_bottom_start - self.old_bottom_start, col);
        }
        // Inside the changed lines: work in characters from their start.
        let offset: usize = self.old_lines[self.top..row]
            .iter()
            .map(|l| l.chars().count() + 1)
            .sum::<usize>()
            + col;
        let mapped = if offset <= self.prefix {
            offset
        } else if offset >= self.old_len - self.suffix {
            offset + self.new_len - self.old_len
        } else {
            self.prefix
        };
        self.position_in_new(mapped)
    }

    /// The `(row, col)` of character `offset` into the changed lines of `new`.
    fn position_in_new(&self, mut offset: usize) -> (usize, usize) {
        for (i, line) in self.new[self.top..self.new_bottom_start].iter().enumerate() {
            let len = line.chars().count();
            if offset <= len {
                return (self.top + i, offset);
            }
            offset -= len + 1;
        }
        // The changed lines were all deleted: the start of what follows.
        (self.top.min(self.new.len().saturating_sub(1)), 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Map `pos` across the change with no cursor hint (the cursor is placed
    /// past the changed lines); tests that care about ties use `map_at`.
    fn map(old: &[&str], new: &[&str], pos: (usize, usize)) -> (usize, usize) {
        map_at(old, new, pos, (new.len(), 0))
    }

    fn map_at(old: &[&str], new: &[&str], pos: (usize, usize), cursor: (usize, usize)) -> (usize, usize) {
        let (old, new) = (lines(old), lines(new));
        TextChange::new(&old, &new, cursor).unwrap().map(pos)
    }

    #[test]
    fn typing_before_a_position_on_its_line_shifts_it() {
        assert_eq!(map(&["Hello"], &["XHello"], (0, 3)), (0, 4));
        assert_eq!(map(&["Hello"], &["HelloX"], (0, 3)), (0, 3), "after it: unmoved");
    }

    #[test]
    fn inserted_and_deleted_lines_shift_later_rows() {
        assert_eq!(map(&["a", "b", "c"], &["a", "new", "b", "c"], (2, 1)), (3, 1));
        assert_eq!(map(&["a", "b", "c"], &["a", "c"], (2, 1)), (1, 1));
        assert_eq!(map(&["a", "b", "c"], &["a", "c"], (0, 1)), (0, 1));
    }

    #[test]
    fn splitting_and_joining_lines_keeps_the_text_attached() {
        // Enter in the middle of "Hello world" before the marker on "world".
        assert_eq!(map(&["Hello world"], &["Hello", "world"], (0, 8)), (1, 2));
        assert_eq!(map(&["Hello", "world"], &["Hello world"], (1, 2)), (0, 8));
    }

    #[test]
    fn a_position_inside_deleted_text_goes_to_where_it_was() {
        // ^T at column 4 deleted "two "; the cursor says so.
        assert_eq!(map_at(&["one two three"], &["one three"], (0, 5), (0, 4)), (0, 4));
        assert_eq!(map(&["a", "b", "c"], &["c"], (1, 0)), (0, 0));
    }
}
