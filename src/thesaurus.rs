//! The thesaurus (`^QJ`), read from an installed MyThes thesaurus — the format
//! LibreOffice and most Linux distributions ship (`th_en_US_v2.dat` / `.idx`).
//! It is too large to bundle, so it is available where one is installed; the
//! `WORDSTAR_THESAURUS` environment variable can point at a `.dat` file.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// One sense of a word and the words that can stand in for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meaning {
    /// Part of speech, e.g. `adj`.
    pub part: String,
    /// Alternatives, each with its relation when the thesaurus gives one
    /// (`similar term`, `antonym`, …).
    pub words: Vec<(String, Option<String>)>,
}

/// An installed thesaurus: its data file and the sorted word index.
pub struct Thesaurus {
    dat: PathBuf,
    /// Lower-cased headwords and their byte offsets in `dat`, sorted.
    index: Vec<(String, u64)>,
}

impl Thesaurus {
    /// Find and load the thesaurus for `language` (e.g. `en_US`; other English
    /// variants fall back to it).
    pub fn find(language: &str) -> Option<Thesaurus> {
        if let Ok(dat) = std::env::var("WORDSTAR_THESAURUS") {
            return Thesaurus::open(Path::new(&dat));
        }
        let mut names = vec![language.to_string()];
        if language.starts_with("en") && language != "en_US" {
            names.push("en_US".into());
        }
        search_dirs().iter().find_map(|dir| {
            names.iter().find_map(|lang| {
                [format!("th_{lang}_v2.dat"), format!("th_{lang}.dat")]
                    .iter()
                    .find_map(|file| Thesaurus::open(&dir.join(file)))
            })
        })
    }

    /// Load the thesaurus whose data file is `dat` (with its `.idx` beside it).
    pub fn open(dat: &Path) -> Option<Thesaurus> {
        if !dat.is_file() {
            return None;
        }
        let idx = std::fs::read_to_string(dat.with_extension("idx")).ok()?;
        let mut index: Vec<(String, u64)> = idx
            .lines()
            .skip(2) // encoding, entry count
            .filter_map(|l| {
                let (word, offset) = l.rsplit_once('|')?;
                Some((word.to_lowercase(), offset.parse().ok()?))
            })
            .collect();
        index.sort();
        Some(Thesaurus {
            dat: dat.to_path_buf(),
            index,
        })
    }

    /// The meanings of `word`, trying simple base forms if the word itself
    /// isn't listed (`storms` → `storm`, `walked` → `walk`).
    pub fn lookup(&self, word: &str) -> Vec<Meaning> {
        let word = word.to_lowercase().replace('\u{2019}', "'");
        base_forms(&word)
            .iter()
            .find_map(|w| self.entry(w).filter(|m| !m.is_empty()))
            .unwrap_or_default()
    }

    fn entry(&self, word: &str) -> Option<Vec<Meaning>> {
        let at = self.index.binary_search_by(|(w, _)| w.as_str().cmp(word)).ok()?;
        let offset = self.index[at].1;
        let mut file = BufReader::new(std::fs::File::open(&self.dat).ok()?);
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut lines = file.lines();
        let head = lines.next()?.ok()?;
        let count: usize = head.rsplit_once('|')?.1.trim().parse().ok()?;
        let mut meanings = Vec::new();
        for line in lines.take(count) {
            let line = line.ok()?;
            let mut parts = line.split('|');
            let part = parts.next().unwrap_or("").trim_matches(|c| c == '(' || c == ')').to_string();
            let words = parts
                .filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case(word))
                .map(|w| match w.rsplit_once(" (") {
                    Some((term, note)) if note.ends_with(')') => {
                        (term.to_string(), Some(note.trim_end_matches(')').to_string()))
                    }
                    _ => (w.to_string(), None),
                })
                .collect::<Vec<_>>();
            if !words.is_empty() {
                meanings.push(Meaning { part, words });
            }
        }
        Some(meanings)
    }
}

/// `word` and the base forms it might be an inflection of.
fn base_forms(word: &str) -> Vec<String> {
    let mut out = vec![word.to_string()];
    let mut add = |w: String| {
        if w.len() > 2 && !out.contains(&w) {
            out.push(w);
        }
    };
    if let Some(stem) = word.strip_suffix("ies") {
        add(format!("{stem}y"));
    }
    for suffix in ["es", "s", "ed", "d", "ing", "ly"] {
        if let Some(stem) = word.strip_suffix(suffix) {
            add(stem.to_string());
            if matches!(suffix, "ed" | "ing") {
                add(format!("{stem}e"));
            }
        }
    }
    out
}

/// Where MyThes thesauri are commonly installed.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/usr/share/mythes",
        "/usr/share/myspell/dicts",
        "/usr/share/myspell",
        "/usr/local/share/mythes",
        "/usr/lib/libreoffice/share/extensions/dict-en",
        "/usr/lib64/libreoffice/share/extensions/dict-en",
        "/opt/homebrew/share/mythes",
        "/Applications/LibreOffice.app/Contents/Resources/extensions/dict-en",
        "C:\\Program Files\\LibreOffice\\share\\extensions\\dict-en",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/mythes"));
    }
    // LibreOffice from its own installer lives under /opt/libreoffice<version>.
    if let Ok(entries) = std::fs::read_dir("/opt") {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("libreoffice") {
                dirs.push(e.path().join("share/extensions/dict-en"));
            }
        }
    }
    dirs
}

/// `replacement` given the capitalisation of `original` (`Happy` → `Glad`,
/// `HAPPY` → `GLAD`).
pub fn match_case(original: &str, replacement: &str) -> String {
    let letters: Vec<char> = original.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        return replacement.to_uppercase();
    }
    if letters.first().is_some_and(|c| c.is_uppercase()) {
        let mut chars = replacement.chars();
        return chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
    }
    replacement.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny MyThes thesaurus in a temp folder.
    fn sample() -> (PathBuf, Thesaurus) {
        let dir = std::env::temp_dir().join(format!("wsrs-thes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let entries = [
            ("happy", "(adj)|glad|content (related term)|unhappy (antonym)\n(adj)|felicitous|happy"),
            ("storm", "(noun)|tempest|violent storm (generic term)"),
            ("walk", "(verb)|stroll|amble"),
        ];
        let mut dat = String::from("UTF-8\n");
        let mut idx = format!("UTF-8\n{}\n", entries.len());
        for (word, senses) in entries {
            idx.push_str(&format!("{word}|{}\n", dat.len()));
            dat.push_str(&format!("{word}|{}\n{senses}\n", senses.lines().count()));
        }
        std::fs::write(dir.join("th_en_US_v2.dat"), dat).unwrap();
        std::fs::write(dir.join("th_en_US_v2.idx"), idx).unwrap();
        let t = Thesaurus::open(&dir.join("th_en_US_v2.dat")).unwrap();
        (dir, t)
    }

    #[test]
    fn looks_up_meanings_and_base_forms() {
        let (dir, t) = sample();
        let happy = t.lookup("Happy");
        assert_eq!(happy.len(), 2);
        assert_eq!(happy[0].part, "adj");
        assert_eq!(
            happy[0].words,
            [
                ("glad".to_string(), None),
                ("content".to_string(), Some("related term".to_string())),
                ("unhappy".to_string(), Some("antonym".to_string()))
            ]
        );
        assert_eq!(happy[1].words, [("felicitous".to_string(), None)], "the headword itself is left out");
        assert_eq!(t.lookup("storms")[0].words[0].0, "tempest");
        assert_eq!(t.lookup("walked")[0].words[0].0, "stroll");
        assert!(t.lookup("zorblax").is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn replacements_keep_the_capitalisation() {
        assert_eq!(match_case("Happy", "glad"), "Glad");
        assert_eq!(match_case("HAPPY", "glad"), "GLAD");
        assert_eq!(match_case("happy", "glad"), "glad");
    }
}
