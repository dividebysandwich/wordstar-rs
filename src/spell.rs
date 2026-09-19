//! Spelling: a Hunspell dictionary (checked with `spellbook`), the tokenizer
//! that picks the words of prose out of Markdown, and the personal word list.
//!
//! US English is bundled so spelling works everywhere, including the browser.
//! A document can ask for another language with `language: en_GB` (or `de_DE`,
//! …) in its frontmatter; on native builds an installed Hunspell dictionary of
//! that name is used.

use std::collections::HashSet;

/// A loaded dictionary plus the words the user has accepted.
pub struct Speller {
    dict: spellbook::Dictionary,
    /// Words accepted for this session ("Ignore all").
    ignored: HashSet<String>,
    /// The dictionary in use, for messages (e.g. `en_US`).
    pub language: String,
}

impl Speller {
    /// Load the dictionary for `language` (default US English), and the
    /// personal word list. An English variant that isn't installed falls back to
    /// the bundled US dictionary; other missing languages are an error.
    pub fn load(language: Option<&str>) -> Result<Speller, String> {
        let wanted = language
            .map(|l| l.trim().replace('-', "_"))
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| "en_US".to_string());
        let (aff, dic, language) = match system_dictionary(&wanted) {
            Some((aff, dic)) => (aff, dic, wanted),
            None if wanted.to_ascii_lowercase().starts_with("en") => (
                include_str!("../assets/dict/en_US.aff").to_string(),
                include_str!("../assets/dict/en_US.dic").to_string(),
                "en_US".to_string(),
            ),
            None => return Err(format!("No {wanted} dictionary is installed.")),
        };
        let mut dict = spellbook::Dictionary::new(&aff, &dic)
            .map_err(|e| format!("Cannot read the {language} dictionary: {e}"))?;
        for word in crate::platform::personal_words() {
            let _ = dict.add(&word);
        }
        Ok(Speller {
            dict,
            ignored: HashSet::new(),
            language,
        })
    }

    /// Whether `word` is spelled correctly (or accepted).
    pub fn is_correct(&self, word: &str) -> bool {
        self.ignored.contains(word) || self.dict.check(word)
    }

    /// Up to nine likely corrections for `word`, best first.
    pub fn suggestions(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.dict.suggest(word, &mut out);
        out.truncate(9);
        out
    }

    /// Accept `word` everywhere for the rest of the session.
    pub fn ignore_all(&mut self, word: &str) {
        self.ignored.insert(word.to_string());
    }

    /// Accept `word` from now on: add it to the personal word list (kept across
    /// sessions — the place for your characters' names).
    pub fn add_to_personal(&mut self, word: &str) {
        let _ = self.dict.add(word);
        crate::platform::add_personal_word(word);
    }
}

/// An installed Hunspell dictionary for `language` (`.aff` and `.dic` text).
#[cfg(not(target_arch = "wasm32"))]
fn system_dictionary(language: &str) -> Option<(String, String)> {
    let mut dirs: Vec<std::path::PathBuf> = [
        "/usr/share/hunspell",
        "/usr/share/myspell",
        "/usr/share/myspell/dicts",
        "/usr/local/share/hunspell",
        "/opt/homebrew/share/hunspell",
        "/Library/Spelling",
    ]
    .iter()
    .map(Into::into)
    .collect();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/share/hunspell"));
        dirs.push(home.join("Library/Spelling"));
    }
    dirs.iter().find_map(|dir| {
        let aff = std::fs::read_to_string(dir.join(format!("{language}.aff"))).ok()?;
        let dic = std::fs::read_to_string(dir.join(format!("{language}.dic"))).ok()?;
        Some((aff, dic))
    })
}

/// The browser has no installed dictionaries.
#[cfg(target_arch = "wasm32")]
fn system_dictionary(_language: &str) -> Option<(String, String)> {
    None
}

/// For each line, whether it holds prose to check: not the frontmatter, a
/// fenced code block, a dot command, or a table's rule row.
pub fn prose_lines(lines: &[String]) -> Vec<bool> {
    let mut out = vec![true; lines.len()];
    let mut start = 0;
    if lines.first().map(|l| l.trim()) == Some("---")
        && let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---")
    {
        start = end + 2;
        out[..start].fill(false);
    }
    let mut fence = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        let t = line.trim_start();
        let is_fence = t.starts_with("```") || t.starts_with("~~~");
        if fence || is_fence || crate::attributes::is_dot_command(line) {
            out[i] = false;
        }
        if is_fence {
            fence = !fence;
        }
    }
    out
}

/// The words to check in a line of prose, as `(start, end)` character columns.
///
/// A word is a run of letters, with apostrophes inside it (`don't`, `it’s`);
/// hyphens split words. Left out: single letters, anything touching a digit
/// (`3rd`, `1920s`), inline code, link targets, `{…}` attributes, `<tags>`,
/// and web or email addresses.
pub fn words(line: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    // Characters not to check: code spans, link targets, attributes, tags, URLs.
    let mut skip = vec![false; n];
    let mut i = 0;
    while i < n {
        let close = match chars[i] {
            '`' => Some('`'),
            '{' => Some('}'),
            '<' => Some('>'),
            '(' if i > 0 && chars[i - 1] == ']' => Some(')'),
            _ => None,
        };
        if let Some(close) = close
            && let Some(len) = chars[i + 1..].iter().position(|&c| c == close)
        {
            skip[i..=i + 1 + len].fill(true);
            i += len + 2;
            continue;
        }
        i += 1;
    }
    // Web and email addresses: whole runs of text between spaces (and the
    // spans already skipped, so a link's text survives its URL).
    let separator = |i: usize| chars[i].is_whitespace() || skip[i];
    let mut addresses = Vec::new();
    let mut start = 0;
    while start < n {
        if separator(start) {
            start += 1;
            continue;
        }
        let end = (start..n).find(|&i| separator(i)).unwrap_or(n);
        let chunk: String = chars[start..end].iter().collect();
        if chunk.contains("://") || chunk.starts_with("www.") || chunk.contains('@') {
            addresses.push(start..end);
        }
        start = end;
    }
    for range in addresses {
        skip[range].fill(true);
    }

    let is_apostrophe = |c: char| c == '\'' || c == '\u{2019}';
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if skip[i] || !chars[i].is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < n
            && !skip[i]
            && (chars[i].is_alphabetic()
                || (is_apostrophe(chars[i]) && chars.get(i + 1).is_some_and(|c| c.is_alphabetic())))
        {
            i += 1;
        }
        let touches_digit = (start > 0 && chars[start - 1].is_ascii_digit())
            || chars.get(i).is_some_and(|c| c.is_ascii_digit());
        if i - start > 1 && !touches_digit {
            out.push((start, i));
        }
    }
    out
}

/// The word at (or just before) character column `col` of `line`, if any.
pub fn word_at(line: &str, col: usize) -> Option<(usize, usize)> {
    words(line).into_iter().find(|&(s, e)| s <= col && col <= e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picked(line: &str) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        words(line)
            .into_iter()
            .map(|(s, e)| chars[s..e].iter().collect())
            .collect()
    }

    #[test]
    fn words_are_picked_out_of_markdown_prose() {
        assert_eq!(
            picked("**Don't** say it’s *over*, well-known [link](http://x.io) `code`"),
            ["Don't", "say", "it’s", "over", "well", "known", "link"]
        );
        assert_eq!(picked("The 3rd of May, 1920s, a I x"), ["The", "of", "May"]);
        assert_eq!(picked("Mail me@example.com or www.site.org {.underline} <br>"), ["Mail", "or"]);
    }

    #[test]
    fn prose_lines_skip_frontmatter_code_and_dot_commands() {
        let lines: Vec<String> = ["---", "title: x", "---", "Text", "```", "code", "```", ".pa", "More"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            prose_lines(&lines),
            [false, false, false, true, false, false, false, false, true]
        );
    }

    #[test]
    fn bundled_dictionary_checks_and_suggests() {
        let mut s = Speller::load(None).unwrap();
        assert!(s.is_correct("receive") && s.is_correct("don't") && s.is_correct("it’s"));
        assert!(!s.is_correct("recieve"));
        assert_eq!(s.suggestions("recieve").first().map(String::as_str), Some("receive"));
        assert!(!s.is_correct("Zorblax"));
        s.ignore_all("Zorblax");
        assert!(s.is_correct("Zorblax"));
        // An English variant that isn't installed falls back to US English.
        assert_eq!(Speller::load(Some("en-XX")).unwrap().language, "en_US");
        assert!(Speller::load(Some("xx_YY")).is_err());
    }
}
