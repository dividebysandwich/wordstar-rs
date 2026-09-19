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

/// Private-use sentinels that wrap underlined text in the source handed to the
/// formatted renderers (preview / PDF / graphical preview). They never occur in
/// real documents, so the renderers can toggle underline on seeing them.
pub const UNDERLINE_START: char = '\u{E000}';
pub const UNDERLINE_END: char = '\u{E001}';

/// Private-use sentinel standing alone in a paragraph where a `.pa` page break
/// was; the renderers turn that paragraph into a new page.
pub const PAGE_BREAK: char = '\u{E002}';

/// True if `line` is a WordStar dot command (a `.` at column 1 followed by a
/// letter, e.g. `.he`, `.pa`). Such lines are print directives, not body text.
pub fn is_dot_command(line: &str) -> bool {
    let mut chars = line.chars();
    chars.next() == Some('.') && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// The name of a dot command, lowercased (`.HE Title` → `he`), and its argument.
fn dot_command(line: &str) -> Option<(String, &str)> {
    if !is_dot_command(line) {
        return None;
    }
    let body = &line[1..];
    let split = body
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(body.len());
    Some((body[..split].to_ascii_lowercase(), body[split..].trim()))
}

/// Preprocess raw Markdown (frontmatter already stripped) for the formatted
/// renderers: handle dot commands, apply the paragraph mode, and rewrite pandoc
/// attribute spans so the renderers can show them.
///
/// - `.pa` becomes a paragraph holding only [`PAGE_BREAK`]; other dot commands
///   are print directives and are dropped (see [`page_setup`]).
/// - A line that is just `#` is a manuscript scene break and becomes a rule
///   (on its own it would be an empty heading).
/// - With `paragraphs: lines`, every line of prose is its own paragraph and
///   leading indentation is dropped (it would otherwise make a code block).
/// - `[text]{.underline}` becomes `text` wrapped in [`UNDERLINE_START`] /
///   [`UNDERLINE_END`] sentinels (turned into a real underline downstream).
///   `[text]{font=… size=…}` collapses to its visible `text` — terminals and the
///   monospaced PDF can't change font, but the markers must not leak as literals.
///   Strikethrough (`~~…~~`) is standard Markdown and passes through untouched.
pub fn prepare_render_source(src: &str, opts: &RenderOptions) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<&str> = None;
    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            out.push(line.to_string());
            continue;
        }
        if let Some(marker) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
            fence = Some(marker);
            out.push(line.to_string());
            continue;
        }
        if let Some((name, _)) = dot_command(line) {
            if name == "pa" {
                out.extend([String::new(), PAGE_BREAK.to_string(), String::new()]);
            }
            continue;
        }
        if line.trim() == "#" {
            out.extend([String::new(), "* * *".to_string(), String::new()]);
            continue;
        }
        let text = if opts.smart && !trimmed.starts_with('|') && !is_thematic_break(trimmed) {
            em_dashes(line)
        } else {
            line.to_string()
        };
        let mut rewritten = String::with_capacity(text.len());
        if opts.prose_paragraphs && !is_structural(line) {
            rewrite_spans(text.trim_start(), &mut rewritten);
            out.extend([String::new(), rewritten, String::new()]);
        } else {
            rewrite_spans(&text, &mut rewritten);
            out.push(rewritten);
        }
    }
    out.join("\n")
}

/// Typewriter dashes: in a manuscript a double hyphen means an em dash, so turn
/// `--` into `—` before smart punctuation would make it an en dash (`---` still
/// becomes an em dash). Code spans are left alone.
fn em_dashes(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            in_code = !in_code;
        } else if c == '-' && !in_code {
            let run = chars[i..].iter().take_while(|&&c| c == '-').count();
            if run == 2 {
                out.push('\u{2014}');
            } else {
                out.extend(std::iter::repeat_n('-', run));
            }
            i += run;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// True for a line that is Markdown structure rather than prose: blank, a
/// heading, quote, list item, table row, or thematic break.
fn is_structural(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() || t.starts_with('#') || t.starts_with('>') || t.starts_with('|') {
        return true;
    }
    if ["- ", "* ", "+ "].iter().any(|m| t.starts_with(m)) {
        return true;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") ")) {
        return true;
    }
    is_thematic_break(t)
}

/// A Markdown thematic break (`---`, `***`, `* * *`, `___`): three or more of
/// the same marker, optionally spaced.
fn is_thematic_break(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|m| t.chars().all(|c| c.to_string() == *m))
}

/// Drop a leading YAML frontmatter block (`--- … ---`) so it is not rendered.
pub fn strip_frontmatter(src: &str) -> &str {
    let mut lines = src.lines();
    if lines.next().map(str::trim) != Some("---") {
        return src;
    }
    let mut offset = src.find('\n').map_or(src.len(), |i| i + 1);
    for line in lines {
        offset += line.len() + 1;
        if line.trim() == "---" {
            return src.get(offset..).unwrap_or("");
        }
    }
    src
}

/// The YAML frontmatter as `(key, values)` pairs, keys lowercased. Supports the
/// subset documents need: `key: value`, a key followed by `- item` lines,
/// quoted values, and `# comments`.
fn frontmatter(src: &str) -> Vec<(String, Vec<String>)> {
    frontmatter_of(src.lines())
}

fn frontmatter_of<'a>(mut lines: impl Iterator<Item = &'a str>) -> Vec<(String, Vec<String>)> {
    if lines.next().map(str::trim) != Some("---") {
        return Vec::new();
    }
    let unquote = |v: &str| {
        let v = v.trim();
        for q in ['"', '\''] {
            if let Some(rest) = v.strip_prefix(q) {
                return rest.split(q).next().unwrap_or("").to_string();
            }
        }
        let v = if v.starts_with('#') { "" } else { v };
        v.split(" #").next().unwrap_or("").trim().to_string()
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in lines {
        let t = line.trim();
        if t == "---" {
            break;
        }
        if let Some(item) = t.strip_prefix("- ").or_else(|| (t == "-").then_some("")) {
            if let Some((_, values)) = out.last_mut() {
                values.push(unquote(item));
            }
        } else if let Some((key, value)) = t.split_once(':') {
            let value = unquote(value);
            let values = if value.is_empty() { Vec::new() } else { vec![value] };
            out.push((key.trim().to_ascii_lowercase(), values));
        }
    }
    out
}

/// The document's word-count goal (`goal: 80000`, `goal: 80,000` or
/// `goal: 80k` in the frontmatter), if it sets one.
pub fn word_goal(lines: &[String]) -> Option<usize> {
    let fm = frontmatter_of(lines.iter().map(String::as_str));
    let raw = fm.iter().find(|(k, _)| k == "goal")?.1.first()?.to_ascii_lowercase();
    let (digits, scale) = match raw.strip_suffix('k') {
        Some(d) => (d, 1000),
        None => (raw.as_str(), 1),
    };
    let digits: String = digits.chars().filter(|c| !matches!(c, ',' | '_' | ' ')).collect();
    digits.parse::<usize>().ok().map(|n| n * scale).filter(|&n| n > 0)
}

/// `n` with thousands separators: `12345` → `12,345`.
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Paper size for the PDF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    A4,
    Letter,
}

/// The author details for a standard-manuscript-format PDF (`format:
/// manuscript` in the frontmatter).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manuscript {
    pub title: String,
    /// The author's legal name (for the contact block).
    pub author: String,
    /// The name the story is published under ("by …").
    pub byline: String,
    /// Surname for the running page header.
    pub surname: String,
    /// Contact block lines (address, email, …), printed under the author.
    pub contact: Vec<String>,
    /// `italics: underline` — the classic convention of underlining instead.
    pub underline_italics: bool,
}

/// Document-level rendering options, read from the YAML frontmatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOptions {
    /// `paragraphs: lines` — each line of prose is its own paragraph (the way
    /// WordStar writers type: one Enter per paragraph, maybe a Tab indent), and
    /// paragraphs after the first get a first-line indent instead of a gap.
    pub prose_paragraphs: bool,
    /// Curly quotes, en/em dashes and ellipses (`smart: false` turns it off).
    pub smart: bool,
    /// Standard manuscript format, when `format: manuscript`.
    pub manuscript: Option<Manuscript>,
    /// `paper: letter` / `paper: a4`; `None` uses the format's default.
    pub paper: Option<Paper>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            prose_paragraphs: false,
            smart: true,
            manuscript: None,
            paper: None,
        }
    }
}

/// Read the rendering options from the document's frontmatter.
pub fn render_options(src: &str) -> RenderOptions {
    let fm = frontmatter(src);
    let get = |key: &str| {
        fm.iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.first())
            .map(|v| v.to_ascii_lowercase())
    };
    let text = |key: &str| {
        fm.iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.first())
            .cloned()
            .unwrap_or_default()
    };
    let manuscript = (get("format").as_deref() == Some("manuscript")).then(|| {
        let author = text("author");
        let byline = Some(text("byline"))
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| author.clone());
        let surname = Some(text("surname"))
            .filter(|s| !s.is_empty())
            .or_else(|| author.split_whitespace().last().map(str::to_string))
            .unwrap_or_default();
        Manuscript {
            title: text("title"),
            author,
            byline,
            surname,
            contact: fm
                .iter()
                .find(|(k, _)| k == "contact")
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
            underline_italics: get("italics").as_deref() == Some("underline"),
        }
    });
    RenderOptions {
        prose_paragraphs: get("paragraphs").as_deref() == Some("lines"),
        smart: !matches!(get("smart").as_deref(), Some("false" | "no" | "off")),
        manuscript,
        paper: match get("paper").as_deref() {
            Some("letter" | "us-letter" | "usletter") => Some(Paper::Letter),
            Some("a4") => Some(Paper::A4),
            _ => None,
        },
    }
}

/// Running headers / footers and page numbering, from WordStar dot commands:
/// `.he`/`.oh`/`.eh` (header on all / odd / even pages), `.fo`/`.of`/`.ef`
/// (footers), `.op` (omit page numbers) and `.pn N` (first page number). In the
/// text, `#` stands for the page number. The first of each command wins, which
/// is the most recent one the Header/Footer dialog inserted (it adds at the top).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSetup {
    pub header_odd: Option<String>,
    pub header_even: Option<String>,
    pub footer_odd: Option<String>,
    pub footer_even: Option<String>,
    pub omit_page_numbers: bool,
    pub first_page: usize,
}

impl Default for PageSetup {
    fn default() -> Self {
        Self {
            header_odd: None,
            header_even: None,
            footer_odd: None,
            footer_even: None,
            omit_page_numbers: false,
            first_page: 1,
        }
    }
}

impl PageSetup {
    /// The header for printed page number `page`, with `#` filled in.
    pub fn header(&self, page: usize) -> Option<String> {
        let text = if page % 2 == 1 { &self.header_odd } else { &self.header_even };
        text.as_ref().map(|t| t.replace('#', &page.to_string()))
    }

    /// The footer for printed page number `page`, with `#` filled in.
    pub fn footer(&self, page: usize) -> Option<String> {
        let text = if page % 2 == 1 { &self.footer_odd } else { &self.footer_even };
        text.as_ref().map(|t| t.replace('#', &page.to_string()))
    }
}

/// Collect the page setup from the document's dot commands.
pub fn page_setup(src: &str) -> PageSetup {
    let mut setup = PageSetup::default();
    let mut first_page = None;
    for line in src.lines() {
        let Some((name, arg)) = dot_command(line) else {
            continue;
        };
        let text = || Some(arg.to_string());
        let set = |slot: &mut Option<String>| {
            if slot.is_none() {
                *slot = text();
            }
        };
        match name.as_str() {
            "he" => {
                set(&mut setup.header_odd);
                set(&mut setup.header_even);
            }
            "oh" => set(&mut setup.header_odd),
            "eh" => set(&mut setup.header_even),
            "fo" => {
                set(&mut setup.footer_odd);
                set(&mut setup.footer_even);
            }
            "of" => set(&mut setup.footer_odd),
            "ef" => set(&mut setup.footer_even),
            "op" => setup.omit_page_numbers = true,
            "pn" if first_page.is_none() => first_page = arg.parse().ok(),
            _ => {}
        }
    }
    setup.first_page = first_page.unwrap_or(1).max(1);
    setup
}

/// Document statistics for the Word Count dialog and the manuscript title page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextStats {
    pub words: usize,
    pub chars: usize,
    pub paragraphs: usize,
}

/// Count the words, characters and paragraphs a reader would see: frontmatter,
/// dot commands, Markdown structure (heading `#`, quote `>`, list markers, rules,
/// table rules, code fences) and inline formatting markers are left out, and
/// only tokens with a letter or digit count as words (not a spaced `—`).
pub fn count_words(lines: &[String]) -> TextStats {
    let mut stats = TextStats::default();
    let mut in_para = false;
    let mut body = lines;
    if lines.first().map(|l| l.trim()) == Some("---")
        && let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---")
    {
        body = &lines[end + 2..];
    }
    for line in body {
        let t = line.trim();
        if is_dot_command(line)
            || t.starts_with("```")
            || t.starts_with("~~~")
            || is_thematic_break(t)
            || t == "#"
            || (t.starts_with('|') && t.chars().all(|c| "|-: ".contains(c)))
        {
            in_para = false;
            continue;
        }
        let mut rest = t.trim_start_matches('#').trim_start();
        while let Some(r) = rest.strip_prefix('>') {
            rest = r.trim_start();
        }
        for marker in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
            if let Some(r) = rest.strip_prefix(marker) {
                rest = r;
                break;
            }
        }
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits > 0
            && let Some(r) = rest[digits..]
                .strip_prefix(". ")
                .or_else(|| rest[digits..].strip_prefix(") "))
        {
            rest = r;
        }
        let text = strip_inline_markers(&rest.replace('|', " "));
        stats.words += text
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphanumeric))
            .count();
        stats.chars += text.chars().count();
        if text.trim().is_empty() {
            in_para = false;
        } else if !in_para {
            stats.paragraphs += 1;
            in_para = true;
        }
    }
    stats
}

/// Append `line` to `out`, rewriting any `[text]{attrs}` attribute spans.
fn rewrite_spans(line: &str, out: &mut String) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '['
            && let Some((rb, bo, bc)) = find_span(&chars, i)
        {
            let inner: String = chars[i + 1..rb].iter().collect();
            let attr_str: String = chars[bo + 1..bc].iter().collect();
            let mut attrs = RunAttributes::default();
            apply_attr_tokens(&mut attrs, &attr_str);
            if attrs.underline {
                out.push(UNDERLINE_START);
                out.push_str(&inner);
                out.push(UNDERLINE_END);
            } else {
                out.push_str(&inner);
            }
            i = bc + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
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

/// Flag which characters of `chars` are formatting markers (not printed text):
/// `*`, `**`, `~~`, and the `[`/`]{…}` of attribute spans.
fn marker_mask(chars: &[char]) -> Vec<bool> {
    let n = chars.len();
    let mut mask = vec![false; n];
    let mut i = 0;
    while i < n {
        if (chars[i] == '*' || chars[i] == '~') && i + 1 < n && chars[i + 1] == chars[i] {
            mask[i] = true;
            mask[i + 1] = true;
            i += 2;
            continue;
        }
        if chars[i] == '*' {
            mask[i] = true;
            i += 1;
            continue;
        }
        if chars[i] == '['
            && let Some((rb, _bo, bc)) = find_span(chars, i)
        {
            mask[i] = true; // '['
            for m in mask.iter_mut().take(bc + 1).skip(rb) {
                *m = true; // ']', '{', …, '}'
            }
            i = bc + 1;
            continue;
        }
        i += 1;
    }
    mask
}

/// The visible (printed) column for a raw cursor column, ignoring inline
/// formatting markers.
pub fn visible_column(line: &str, raw_col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mask = marker_mask(&chars);
    let limit = raw_col.min(chars.len());
    mask[..limit].iter().filter(|m| !**m).count()
}

/// Remove inline markdown formatting markers from `s`, keeping the text.
pub fn strip_inline_markers(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' || chars[i] == '~' {
            let marker = chars[i];
            i += 1;
            if i < chars.len() && chars[i] == marker {
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

    fn prepare(src: &str) -> String {
        prepare_render_source(src, &RenderOptions::default())
    }

    #[test]
    fn prepare_wraps_underline_and_strips_font() {
        let out = prepare("a [hi]{.underline} b [yo]{font=\"Courier\"} c");
        // Underline span: text kept, wrapped in sentinels; brackets/attrs gone.
        assert!(
            out.contains(&format!("{UNDERLINE_START}hi{UNDERLINE_END}")),
            "got: {out:?}"
        );
        // Font span: only the visible text survives.
        assert!(out.contains("yo"), "got: {out:?}");
        assert!(!out.contains("font="), "attrs leaked: {out:?}");
        assert!(!out.contains('['), "brackets leaked: {out:?}");
    }

    #[test]
    fn prepare_drops_dot_command_lines() {
        let out = prepare(".he Title\nBody\n.pa\nMore");
        assert!(!out.contains("Title"), "dot command leaked: {out:?}");
        assert!(!out.contains(".pa"), "dot command leaked: {out:?}");
        assert!(out.contains("Body") && out.contains("More"));
        assert!(out.contains(&format!("\n\n{PAGE_BREAK}\n\n")), "page break: {out:?}");
    }

    #[test]
    fn prepare_leaves_links_untouched() {
        // `[text](url)` is a real link, not an attribute span.
        let out = prepare("see [the site](https://example.com)");
        assert_eq!(out, "see [the site](https://example.com)");
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
    fn visible_column_ignores_markers() {
        // plain text: identity
        assert_eq!(visible_column("abc", 2), 2);
        // "**bold**": cursor right after opening ** (raw 2) → visible 0
        assert_eq!(visible_column("**bold**", 2), 0);
        // cursor after "bold" (raw 6) → visible 4
        assert_eq!(visible_column("**bold**", 6), 4);
        // span: "[hi]{.underline}" — cursor after 'hi' (raw 3 = ']') → visible 2
        assert_eq!(visible_column("[hi]{.underline}", 3), 2);
        // end of "x*y*z" (raw 5) → visible 3 (x, y, z)
        assert_eq!(visible_column("x*y*z", 5), 3);
    }

    #[test]
    fn prose_mode_makes_each_line_a_paragraph_without_code_blocks() {
        let opts = RenderOptions {
            prose_paragraphs: true,
            ..RenderOptions::default()
        };
        let out = prepare_render_source("First.\n\tSecond, indented.\n- a list\n```\n  code\n```", &opts);
        assert!(out.contains("\n\nSecond, indented.\n"), "got {out:?}");
        assert!(!out.contains("\tSecond"), "indent would make a code block");
        assert!(out.contains("- a list"));
        assert!(out.contains("  code"), "fenced code left alone");
    }

    #[test]
    fn double_hyphens_become_em_dashes_outside_code() {
        let out = prepare("Wait -- no. Use `--flag` and a---b.");
        assert_eq!(out, "Wait \u{2014} no. Use `--flag` and a---b.");
        let plain = RenderOptions {
            smart: false,
            ..RenderOptions::default()
        };
        assert_eq!(prepare_render_source("a -- b", &plain), "a -- b");
    }

    #[test]
    fn lone_hash_line_is_a_scene_break() {
        assert!(prepare("one\n#\ntwo").contains("\n* * *\n"));
    }

    #[test]
    fn render_options_read_frontmatter() {
        let src = "---\nformat: manuscript\ntitle: \"The Red House\"\nauthor: Jane Q. Writer  # legal name\ncontact:\n  - 1 Elm St\n  - jane@example.com\nparagraphs: lines\nsmart: false\npaper: a4\n---\nBody";
        let o = render_options(src);
        assert!(o.prose_paragraphs);
        assert!(!o.smart);
        assert_eq!(o.paper, Some(Paper::A4));
        let m = o.manuscript.unwrap();
        assert_eq!(m.title, "The Red House");
        assert_eq!((m.byline.as_str(), m.surname.as_str()), ("Jane Q. Writer", "Writer"));
        assert_eq!(m.contact, ["1 Elm St", "jane@example.com"]);
        assert_eq!(strip_frontmatter(src), "Body");
        assert_eq!(render_options("Body"), RenderOptions::default());
    }

    #[test]
    fn page_setup_reads_header_footer_dot_commands() {
        let s = page_setup(".OH Odd #\n.eh Even #\n.fo - # -\n.pn 5\n.he later\nBody");
        assert_eq!(s.header(1).as_deref(), Some("Odd 1"));
        assert_eq!(s.header(2).as_deref(), Some("Even 2"));
        assert_eq!(s.footer(7).as_deref(), Some("- 7 -"));
        assert_eq!(s.first_page, 5);
        assert!(!s.omit_page_numbers);
        assert!(page_setup(".op").omit_page_numbers);
    }

    #[test]
    fn word_goal_accepts_common_spellings() {
        let doc = |g: &str| -> Vec<String> {
            ["---".to_string(), format!("goal: {g}"), "---".to_string()].to_vec()
        };
        assert_eq!(word_goal(&doc("80000")), Some(80_000));
        assert_eq!(word_goal(&doc("80,000")), Some(80_000));
        assert_eq!(word_goal(&doc("50k")), Some(50_000));
        assert_eq!(word_goal(&doc("lots")), None);
        assert_eq!(word_goal(&["No frontmatter".to_string()]), None);
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn word_count_skips_structure() {
        let lines: Vec<String> = [
            "---", "title: Not Counted", "---", "# Chapter One", "", "- item one",
            "> quoted text", "***", ".pa", "She paused — then **left**.", "| a | b |", "|---|---|",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let s = count_words(&lines);
        // Chapter One (2) + item one (2) + quoted text (2) + She paused then left (4) + a b (2)
        assert_eq!(s.words, 12);
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
