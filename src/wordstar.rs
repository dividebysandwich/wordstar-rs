//! Importer for classic WordStar (3–7) document files.
//!
//! WordStar stores text with a few quirks this decoder undoes:
//!
//! - WordStar 5+ documents begin with a binary header bracketed by `0x1D`.
//! - Word wrap uses *soft* returns (`0x8D 0x0A`) within a paragraph and *hard*
//!   returns (`0x0D 0x0A`) at paragraph ends; the high bit also marks soft
//!   spaces (`0xA0`) and the last byte of a justified word.
//! - Inline print effects are single control bytes (bold `0x02`, italic `0x19`,
//!   underline `0x13`, strikeout `0x18`, …).
//! - Lines beginning with `.` in column 1 are dot commands (layout directives).
//! - `0x1A` marks end of text; the file is padded with `0x1A` to a block size.
//!
//! The result is Markdown so it slots straight into the editor.

use std::fs;
use std::io;
use std::path::Path;

/// A loaded document plus whether it was imported from a WordStar file.
pub struct Loaded {
    pub text: String,
    pub imported: bool,
}

/// Read `path`, decoding it from WordStar if it looks like a WordStar file,
/// otherwise reading it as UTF-8 text (lossily).
pub fn load(path: &Path) -> io::Result<Loaded> {
    let bytes = fs::read(path)?;
    if is_wordstar(path, &bytes) {
        Ok(Loaded {
            text: decode(&bytes),
            imported: true,
        })
    } else {
        Ok(Loaded {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            imported: false,
        })
    }
}

/// Heuristic: should `path` / `bytes` be imported as a WordStar document?
pub fn is_wordstar(path: &Path, bytes: &[u8]) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("ws") | Some("wsd")) {
        return true;
    }
    // WordStar 5+ document header marker.
    bytes.first() == Some(&0x1D)
}

/// Where the text begins: after the `0x1D`-bracketed header, if present.
fn body_start(bytes: &[u8]) -> usize {
    if bytes.first() != Some(&0x1D) {
        return 0;
    }
    // The header is delimited by a second 0x1D; the documented size is 128.
    match bytes[1..].iter().position(|&b| b == 0x1D) {
        Some(pos) => (pos + 2).min(bytes.len()),
        None => 128.min(bytes.len()),
    }
}

/// Decode WordStar bytes into Markdown text.
pub fn decode(bytes: &[u8]) -> String {
    let body = &bytes[body_start(bytes)..];
    let mut out = String::new();
    let mut underline = false;
    let mut at_line_start = true;
    let mut i = 0;

    while i < body.len() {
        let b = body[i];

        // Dot command lines (e.g. `.PA`, `.LM 8`) are layout directives — drop them.
        if at_line_start && b == b'.' && body.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
            while i < body.len() && !matches!(body[i], 0x0D | 0x8D | 0x1A) {
                i += 1;
            }
            if i < body.len() && matches!(body[i], 0x0D | 0x8D) {
                i += 1;
                if body.get(i) == Some(&0x0A) {
                    i += 1;
                }
            }
            at_line_start = true;
            continue;
        }

        match b {
            0x1A => break, // end of text

            0x0D => {
                // Hard return: paragraph / line break.
                out.push('\n');
                i += 1;
                if body.get(i) == Some(&0x0A) {
                    i += 1;
                }
                at_line_start = true;
                continue;
            }
            0x8D => {
                // Soft return (word wrap): re-flow into a space.
                out.push(' ');
                i += 1;
                if body.get(i) == Some(&0x0A) {
                    i += 1;
                }
                at_line_start = false;
                continue;
            }
            0x0A => {
                i += 1; // stray LF
                continue;
            }
            0xA0 => out.push(' '), // soft space
            0x09 => out.push('\t'),

            // Inline print effects (toggles).
            0x02 | 0x04 => out.push_str("**"), // bold / double-strike
            0x19 => out.push('*'),             // italic
            0x18 => out.push_str("~~"),        // strikeout
            0x13 => {
                // Underline: asymmetric Markdown, so track open/close.
                out.push_str(if underline { "]{.underline}" } else { "[" });
                underline = !underline;
            }
            0x14 | 0x16 => {} // super/subscript: no Markdown equivalent, drop marker

            0x1E | 0x1F => {} // soft hyphen: drop

            0x1B => {
                // Extended-character escape: 0x1B <char> 0x1C.
                if body.get(i + 2) == Some(&0x1C) {
                    let ch = body[i + 1] & 0x7F;
                    if (0x20..0x7F).contains(&ch) {
                        out.push(ch as char);
                    }
                    i += 3;
                    at_line_start = false;
                    continue;
                }
                // otherwise drop the lone ESC
            }

            0x20..=0x7E => out.push(b as char), // printable ASCII

            _ if b >= 0x80 => {
                // High bit set: justify/word marker — mask it off.
                let c = b & 0x7F;
                if (0x20..0x7F).contains(&c) {
                    out.push(c as char);
                }
            }

            _ => {} // other low control bytes: ignore
        }

        at_line_start = false;
        i += 1;
    }

    if underline {
        out.push_str("]{.underline}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Wrap a body in a minimal 0x1D-bracketed header for testing.
    fn with_header(body: &[u8]) -> Vec<u8> {
        let mut v = vec![0x1D];
        v.extend(std::iter::repeat_n(0u8, 126));
        v.push(0x1D);
        v.extend_from_slice(body);
        v.push(0x1A);
        v
    }

    #[test]
    fn skips_header_and_reads_text() {
        let data = with_header(b"Hello\x0d\x0aWorld");
        assert_eq!(decode(&data), "Hello\nWorld");
    }

    #[test]
    fn soft_returns_become_spaces_hard_returns_newlines() {
        // "one" soft-wrap "two" hard-return "three"
        let body = b"one\x8d\x0atwo\x0d\x0athree";
        let data = with_header(body);
        assert_eq!(decode(&data), "one two\nthree");
    }

    #[test]
    fn high_bit_word_marker_is_masked() {
        // "test" with the last byte high-bit set (0x74 | 0x80 = 0xF4).
        let body = &[b't', b'e', b's', 0xF4];
        let data = with_header(body);
        assert_eq!(decode(&data), "test");
    }

    #[test]
    fn formatting_toggles_map_to_markdown() {
        // ^B bold ^B, ^Y italic ^Y, ^S underline ^S
        let body = &[
            0x02, b'b', 0x02, b' ', 0x19, b'i', 0x19, b' ', 0x13, b'u', 0x13,
        ];
        let data = with_header(body);
        assert_eq!(decode(&data), "**b** *i* [u]{.underline}");
    }

    #[test]
    fn dot_commands_are_dropped() {
        let body = b".PA\x0d\x0a.LM 8\x0d\x0aBody text";
        let data = with_header(body);
        assert_eq!(decode(&data), "Body text");
    }

    #[test]
    fn detects_by_extension_and_header() {
        assert!(is_wordstar(&PathBuf::from("doc.ws"), b""));
        assert!(is_wordstar(&PathBuf::from("DOC.WS"), b""));
        assert!(is_wordstar(&PathBuf::from("noext"), &[0x1D, 0x00]));
        assert!(!is_wordstar(&PathBuf::from("notes.md"), b"# hi"));
    }

    #[test]
    fn imports_the_bundled_test_file() {
        // TEST.WS lives in the crate root (the test working directory).
        let path = PathBuf::from("TEST.WS");
        if !path.is_file() {
            return;
        }
        let loaded = load(&path).unwrap();
        assert!(loaded.imported);
        assert!(
            loaded.text.contains("Test document"),
            "got: {:?}",
            loaded.text
        );
        assert!(loaded.text.contains("Wordstar test"));
        // Inline formatting must be imported too.
        assert!(
            loaded.text.contains("**This is bold**"),
            "got: {:?}",
            loaded.text
        );
        assert!(loaded.text.contains("*This is italic*"));
        assert!(loaded.text.contains("[This is underline]{.underline}"));
    }
}
