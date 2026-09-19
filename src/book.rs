//! Master documents: WordStar's `.fi filename` dot command pulls another file
//! into the printout, so a novel can be kept as one file per chapter plus a
//! short master file listing them. The preview, the PDF and the word count of
//! the master then cover the whole book.

use std::path::{Path, PathBuf};

/// A document with its `.fi` includes expanded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// How many files were pulled in.
    pub files: usize,
    /// Includes that couldn't be read (or would include themselves).
    pub missing: Vec<String>,
}

/// Start of the line printed in place of an include that couldn't be read
/// (word counts leave it out).
pub const MISSING: &str = "[Missing file: ";

/// How deep includes may nest (a chapter including a scene including …).
const MAX_DEPTH: usize = 8;

/// The file named by a `.fi` line, if `line` is one.
pub fn include_target(line: &str) -> Option<&str> {
    let name = line.get(..3)?;
    let rest = line.get(3..)?;
    (name.eq_ignore_ascii_case(".fi") && rest.starts_with([' ', '\t']))
        .then(|| rest.trim())
        .filter(|f| !f.is_empty())
}

/// Whether the document includes other files.
pub fn has_includes(lines: &[String]) -> bool {
    lines.iter().any(|l| include_target(l).is_some())
}

/// Resolve an include's file name: `~/` is the home folder, and relative names
/// are relative to the folder of the file that includes them.
pub fn resolve(name: &str, base_dir: Option<&Path>) -> PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    if let (Some(rest), Some(home)) = (name.strip_prefix("~/"), dirs::home_dir()) {
        return home.join(rest);
    }
    let path = PathBuf::from(name);
    match base_dir {
        Some(dir) if path.is_relative() => dir.join(path),
        _ => path,
    }
}

/// Replace each `.fi` line of `text` with the included file's text (its
/// frontmatter dropped, its own includes expanded). A file that can't be read
/// leaves a visible `[Missing file: …]` line, so the gap shows in the proof.
pub fn expand(text: &str, base_dir: Option<&Path>) -> Expanded {
    let mut out = Expanded::default();
    let mut chain = Vec::new();
    expand_into(text, base_dir, &mut chain, &mut out);
    out
}

fn expand_into(text: &str, base_dir: Option<&Path>, chain: &mut Vec<PathBuf>, out: &mut Expanded) {
    for line in text.lines() {
        let Some(name) = include_target(line) else {
            out.text.push_str(line);
            out.text.push('\n');
            continue;
        };
        let path = resolve(name, base_dir);
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let loaded = if chain.contains(&key) || chain.len() >= MAX_DEPTH {
            None
        } else {
            read(&path)
        };
        match loaded {
            Some(included) => {
                out.files += 1;
                chain.push(key);
                let body = crate::attributes::strip_frontmatter(&included).to_string();
                expand_into(&body, path.parent(), chain, out);
                chain.pop();
            }
            None => {
                out.missing.push(name.to_string());
                out.text.push_str(&format!("\n{MISSING}{name}]\n\n"));
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read(path: &Path) -> Option<String> {
    crate::wordstar::load(path).ok().map(|l| l.text)
}

/// The browser has no filesystem to include from.
#[cfg(target_arch = "wasm32")]
fn read(_path: &Path) -> Option<String> {
    None
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn includes_are_expanded_recursively_and_safely() {
        let dir = std::env::temp_dir().join(format!("wsrs-book-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("parts")).unwrap();
        std::fs::write(dir.join("ch1.md"), "---\ntitle: ignored\n---\n# One\nFirst.\n").unwrap();
        std::fs::write(dir.join("ch2.md"), "# Two\n.fi parts/scene.md\n").unwrap();
        // Relative to the including file's folder; and a loop back to itself.
        std::fs::write(dir.join("parts/scene.md"), "A scene.\n.fi scene.md\n").unwrap();
        let master = ".pa\n.fi ch1.md\n.FI ch2.md\n.fi nowhere.md\nEnd.";
        let e = expand(master, Some(&dir));
        assert_eq!(e.files, 3);
        assert_eq!(e.missing, ["scene.md", "nowhere.md"], "the loop is cut off");
        assert!(e.text.starts_with(".pa\n# One\nFirst.\n# Two\nA scene.\n"), "{}", e.text);
        assert!(!e.text.contains("title: ignored"));
        assert!(e.text.contains("[Missing file: nowhere.md]"));
        assert!(e.text.ends_with("End.\n"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_real_fi_lines_count() {
        assert_eq!(include_target(".fi chapter1.md"), Some("chapter1.md"));
        assert_eq!(include_target(".FI  ch 2.md "), Some("ch 2.md"));
        assert_eq!(include_target(".fix"), None);
        assert_eq!(include_target(".fi"), None);
        assert_eq!(include_target("text .fi x"), None);
    }
}
