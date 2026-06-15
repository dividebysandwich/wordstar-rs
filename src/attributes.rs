//! Inline attribute model and the parser that maps a cursor position in raw
//! markdown to the formatting active there.
//!
//! Encoding (per the project plan):
//!
//! - bold: `**text**`
//! - italic: `*text*`
//! - underline: `[text]{.underline}`
//! - font: `[text]{font="Courier"}`
//! - size: `[text]{size=14}`
//!
//! Document defaults live in YAML frontmatter (`font:` / `size:`).

/// Formatting active over a run of characters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunAttributes {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub font: Option<String>,
    pub size: Option<u32>,
}

/// Compute the attributes active at every character boundary of `line`.
///
/// The returned vector has `line.chars().count() + 1` entries; index `col`
/// gives the attributes that text inserted at column `col` would carry.
pub fn line_attributes(line: &str) -> Vec<RunAttributes> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut result: Vec<RunAttributes> = Vec::with_capacity(n + 1);
    let mut state = RunAttributes::default();
    let mut i = 0;

    while i < n {
        // Bold: `**`
        if chars[i] == '*' && i + 1 < n && chars[i + 1] == '*' {
            result.push(state.clone()); // first '*'
            result.push(state.clone()); // second '*'
            state.bold = !state.bold;
            i += 2;
            continue;
        }
        // Italic: `*`
        if chars[i] == '*' {
            result.push(state.clone());
            state.italic = !state.italic;
            i += 1;
            continue;
        }
        // Attribute span: `[text]{attrs}`
        if chars[i] == '['
            && let Some((rb, bo, bc)) = find_span(&chars, i)
        {
            let inner_attrs: String = chars[bo + 1..bc].iter().collect();
            let mut span_state = state.clone();
            apply_attr_tokens(&mut span_state, &inner_attrs);

            result.push(state.clone()); // '['
            for _ in (i + 1)..rb {
                result.push(span_state.clone()); // span content
            }
            for _ in rb..=bc {
                result.push(state.clone()); // ']', '{', …, '}'
            }
            i = bc + 1;
            continue;
        }
        // Ordinary character.
        result.push(state.clone());
        i += 1;
    }

    result.push(state);
    debug_assert_eq!(result.len(), n + 1);
    result
}

/// Locate an attribute span `[ … ]{ … }` starting at `start` (a `[`).
///
/// Returns `(close_bracket, brace_open, brace_close)` character indices, or
/// `None` if the text at `start` is not a well-formed span.
pub fn find_span(chars: &[char], start: usize) -> Option<(usize, usize, usize)> {
    let mut j = start + 1;
    while j < chars.len() && chars[j] != ']' {
        if chars[j] == '[' {
            return None; // nested brackets: not a simple span
        }
        j += 1;
    }
    let rb = j;
    if rb >= chars.len() {
        return None;
    }
    let bo = rb + 1;
    if bo >= chars.len() || chars[bo] != '{' {
        return None;
    }
    let mut k = bo + 1;
    while k < chars.len() && chars[k] != '}' {
        k += 1;
    }
    if k >= chars.len() {
        return None;
    }
    Some((rb, bo, k))
}

/// Remove inline markdown formatting markers from `s`, keeping the text.
pub fn strip_inline_markers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' {
            i += 1;
            if i < chars.len() && chars[i] == '*' {
                i += 1;
            }
            continue;
        }
        if chars[i] == '['
            && let Some((rb, _bo, bc)) = find_span(&chars, i)
        {
            out.extend(chars[i + 1..rb].iter());
            i = bc + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Parse YAML frontmatter for document-default font/size, if present.
pub fn document_defaults(lines: &[String]) -> (Option<String>, Option<u32>) {
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (None, None);
    }
    let mut font = None;
    let mut size = None;
    for line in &lines[1..] {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(v) = t.strip_prefix("font:") {
            font = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = t.strip_prefix("size:")
            && let Ok(n) = v.trim().parse()
        {
            size = Some(n);
        }
    }
    (font, size)
}

/// Tokenize the inside of a `{ … }` attribute list, respecting quotes.
fn tokenize_attrs(inner: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in inner.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Apply `{ … }` attribute tokens onto `state`.
fn apply_attr_tokens(state: &mut RunAttributes, inner: &str) {
    for tok in tokenize_attrs(inner) {
        if let Some(class) = tok.strip_prefix('.') {
            if class == "underline" {
                state.underline = true;
            }
        } else if let Some(eq) = tok.find('=') {
            let key = &tok[..eq];
            let val = tok[eq + 1..].trim_matches('"');
            match key {
                "font" => state.font = Some(val.to_string()),
                "size" => {
                    if let Ok(n) = val.parse() {
                        state.size = Some(n);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str, col: usize) -> RunAttributes {
        line_attributes(line)[col].clone()
    }

    #[test]
    fn bold_run_detected_inside_markers() {
        // "ab**cd**ef" — indices: a0 b1 *2 *3 c4 d5 *6 *7 e8 f9
        let line = "ab**cd**ef";
        assert!(!at(line, 1).bold, "before bold");
        assert!(at(line, 5).bold, "inside bold (between c and d)");
        assert!(!at(line, 9).bold, "after bold");
    }

    #[test]
    fn italic_run_detected() {
        let line = "x*y*z";
        assert!(at(line, 2).italic, "inside italic");
        assert!(!at(line, 0).italic);
    }

    #[test]
    fn attribute_span_font_size_underline() {
        let line = "[hi]{font=\"Courier\" size=14 .underline}";
        // 'h' is at index 1
        let a = at(line, 2);
        assert_eq!(a.font.as_deref(), Some("Courier"));
        assert_eq!(a.size, Some(14));
        assert!(a.underline);
    }

    #[test]
    fn strip_removes_markers() {
        assert_eq!(strip_inline_markers("**bold** and *it*"), "bold and it");
        assert_eq!(strip_inline_markers("[x]{font=\"Courier\"}"), "x");
    }

    #[test]
    fn frontmatter_defaults_parsed() {
        let lines: Vec<String> = ["---", "font: Courier", "size: 14", "---", "body"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (f, s) = document_defaults(&lines);
        assert_eq!(f.as_deref(), Some("Courier"));
        assert_eq!(s, Some(14));
    }
}
