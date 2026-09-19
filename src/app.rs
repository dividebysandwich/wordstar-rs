//! Application state and the top-level input dispatcher.
//!
//! [`App`] is the single source of truth: the text widget, the open file, the
//! current [`Mode`], the chord state, and transient status. All mutation flows
//! through here or through [`commands::execute`](crate::commands::execute).

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
use std::path::{Path, PathBuf};

use std::cell::{Cell, RefCell};

use anyhow::Result;
use crate::input::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Alignment, Rect};
use ratatui_textarea::{CursorMove, Input, Key, TextArea};

use crate::attributes::RunAttributes;
use crate::commands;
use crate::keymap::{self, ChordState, Resolution};
use crate::theme;

/// Derive a download filename from the document path, falling back to a generic
/// name with the given extension when there is none (browser builds only).
#[cfg(target_arch = "wasm32")]
fn file_download_name(path: &Path, default_ext: &str) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("document.{default_ext}"))
}

/// Which screen / interaction the app is currently in. Drives both input
/// routing and rendering so the two never drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Normal editing.
    #[default]
    Editor,
    /// Read-only "hide markup" reading view (^OD).
    Clean,
    /// A pull-down menu is open.
    Menu,
    /// A single-line prompt overlay (find / replace / save-as) is open.
    Prompt,
    /// A modal yes/no confirmation is open.
    Confirm,
    /// The file browser is open.
    Browser,
    /// The read-only formatted preview is open.
    Preview,
    /// The help overlay is open.
    Help,
    /// A dismissable information modal (e.g. Word Count).
    Info,
    /// The header / footer dialog.
    Header,
    /// The calculator dialog.
    Calculator,
    /// Find and replace is asking whether to replace the highlighted match.
    ReplaceAsk,
    /// The Go to Heading list.
    Outline,
    /// The spelling check is asking about an unknown word.
    Spell,
    /// File ▸ Recent Files.
    Recent,
}

/// Which kind of single-line prompt is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptKind {
    #[default]
    Find,
    /// Find's second step: the options (`B U W G`).
    FindOptions,
    /// Replace's steps: the text to find, its replacement, the options.
    Replace,
    ReplaceWith,
    ReplaceOptions,
    SaveAs,
    Font,
    FontSize,
    ExportPdf,
    /// Insert another file's contents at the cursor (^KR).
    InsertFile,
    /// Jump to a page number.
    GoToPage,
    /// Set the right margin (the wrap column), stored as a `.rm` dot command.
    RightMargin,
    /// Which place marker to set / go to (from the Edit menu).
    SetMarker,
    GotoMarker,
    /// A replacement typed during the spelling check.
    SpellReplace,
    /// Where to write the marked block (^KW).
    WriteBlock,
    /// A chapter file to include with `.fi`.
    IncludeFile,
}

/// Header vs. footer for the [`Mode::Header`] dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderKind {
    Header,
    Footer,
}

/// Which pages a header / footer applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeaderPages {
    #[default]
    Both,
    Odd,
    Even,
}

/// State backing the [`Mode::Header`] dialog.
#[derive(Debug, Clone)]
pub struct HeaderState {
    pub kind: HeaderKind,
    pub text: String,
    pub pages: HeaderPages,
}

/// A dismissable information modal (title + body lines).
#[derive(Debug, Clone, Default)]
pub struct InfoState {
    pub title: String,
    pub lines: Vec<String>,
}

/// State backing the [`Mode::Calculator`] dialog: the expression being typed and
/// the formatted result of the last calculation.
#[derive(Debug, Clone, Default)]
pub struct CalcState {
    pub input: String,
    pub result: String,
}

/// What to do once the document has been saved (or its changes deliberately
/// discarded). Lets Save As, and the unsaved-changes prompt, finish the action
/// that triggered them instead of dropping it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterSave {
    /// Keep editing.
    Nothing,
    /// Exit the program.
    Quit,
    /// Close the document, leaving a fresh untitled one (WordStar `^KD`).
    Close,
    /// Replace the document with this file (the native file browser).
    Open(PathBuf),
    /// Replace the document with the file chosen in the host picker (browser).
    ApplyPicked,
}

/// A pending action awaiting yes/no confirmation in [`Mode::Confirm`].
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Overwrite an existing file with the export (PDF or Word).
    OverwriteExport(PathBuf),
    /// Save As onto a file that already exists.
    OverwriteSave(PathBuf),
    /// Unsaved changes before the given action: save / discard / cancel.
    SaveBefore(AfterSave),
    /// Restore this autosaved recovery text in place of the loaded document.
    Recover(String),
    /// Write this block text over an existing file (^KW).
    OverwriteBlock(PathBuf, String),
}

/// Identifies a zoomed preview view: `(page, zoom×1000, offx×1000, offy×1000,
/// area_w, area_h)`. Used to skip re-encoding when the view hasn't changed.
type PreviewViewKey = (usize, u32, i32, i32, u16, u16);

/// State backing the [`Mode::Confirm`] modal.
#[derive(Debug, Clone)]
pub struct ConfirmState {
    pub message: String,
    pub action: ConfirmAction,
}

/// Paragraph alignment choice (Justify is tracked even though the widget
/// can only render Left/Center/Right).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignChoice {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// A WordStar block marked with `^KB` … `^KK`. Its ends stay put while the
/// cursor moves elsewhere, so it can be copied (`^KC`) or moved (`^KV`) there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedBlock {
    /// Start and end positions, `(row, col)` in characters, `start < end`.
    pub start: (usize, usize),
    pub end: (usize, usize),
    /// The block's text when marked; if the text there changes, the block is
    /// stale and dropped rather than acting on the wrong text.
    pub text: String,
    /// Hidden with `^KH`: kept, but neither highlighted nor acted on.
    pub hidden: bool,
}

/// State backing the [`Mode::Prompt`] overlay.
#[derive(Debug, Clone, Default)]
pub struct PromptState {
    pub kind: PromptKind,
    pub label: String,
    pub input: String,
    /// Cursor position in `input`, in characters.
    pub cursor: usize,
    /// The input is a suggested default (the previous answer): typing replaces
    /// it, while the arrow keys start editing it.
    pub fresh: bool,
    /// For find / replace: the search term captured in the first step.
    pub pending_find: Option<String>,
    /// For replace: the replacement captured in the second step.
    pub pending_replace: Option<String>,
    /// Help shown under the input (e.g. the find option letters).
    pub hint: &'static str,
}

/// Options for a find or replace, as WordStar asks for them after the search
/// text: `B` search backwards, `U` ignore case, `W` whole words only, `G` the
/// whole document (from the top, or the end when backwards), `N` replace
/// without asking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub backwards: bool,
    pub ignore_case: bool,
    pub whole_words: bool,
    pub whole_document: bool,
    pub no_ask: bool,
}

impl FindOptions {
    /// Parse option letters (any case; other characters are ignored).
    pub fn parse(s: &str) -> Self {
        let has = |c: char| s.chars().any(|x| x.eq_ignore_ascii_case(&c));
        Self {
            backwards: has('b'),
            ignore_case: has('u'),
            whole_words: has('w'),
            whole_document: has('g'),
            no_ask: has('n'),
        }
    }

    /// The option letters, for pre-filling the next prompt.
    pub fn letters(&self) -> String {
        [
            (self.backwards, 'B'),
            (self.ignore_case, 'U'),
            (self.whole_words, 'W'),
            (self.whole_document, 'G'),
            (self.no_ask, 'N'),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, c)| *c)
        .collect()
    }

    /// The regular expression matching `term` literally under these options.
    fn pattern(&self, term: &str) -> String {
        let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let mut p = regex::escape(term);
        // `\b` only makes sense next to a word character.
        if self.whole_words && is_word(term.chars().next()) {
            p.insert_str(0, "\\b");
        }
        if self.whole_words && is_word(term.chars().last()) {
            p.push_str("\\b");
        }
        if self.ignore_case {
            p.insert_str(0, "(?i)");
        }
        p
    }
}

/// The most recent find or replace, repeated by `^L` and offered as the default
/// the next time.
#[derive(Debug, Clone)]
pub struct LastFind {
    pub find: String,
    pub replace: Option<String>,
    pub options: FindOptions,
}

/// Case changes for a block (`^K"`, `^K'`, `^K.`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Upper,
    Lower,
    Sentence,
}

/// Sentence case: lower case, with a capital at the start and after each `.`,
/// `!` or `?` — and the word "I" (and I'm, I'd, …) kept upright.
fn sentence_case(text: &str) -> String {
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut capital = true;
    for (i, &c) in lower.iter().enumerate() {
        let alone_i = c == 'i'
            && !lower.get(i.wrapping_sub(1)).is_some_and(|p| i > 0 && p.is_alphanumeric())
            && !lower.get(i + 1).is_some_and(|n| n.is_alphanumeric());
        if c.is_alphabetic() && (capital || alone_i) {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        if c.is_alphanumeric() {
            capital = false;
        } else if matches!(c, '.' | '!' | '?') {
            capital = true;
        }
    }
    out
}

/// An unknown word found by the spelling check: its start, end and text.
type Misspelling = ((usize, usize), (usize, usize), String);

/// The spelling check's question about one unknown word ([`Mode::Spell`]).
#[derive(Debug, Clone)]
pub struct SpellSession {
    pub word: String,
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub suggestions: Vec<String>,
    /// Corrections made so far in this check.
    pub corrected: usize,
    /// Checking just the word at the cursor (`^QN`): stop after it.
    pub single: bool,
}

/// One heading in the Go to Heading list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    /// Document row of the heading.
    pub row: usize,
    /// Heading level, 1 for `#`.
    pub level: usize,
    pub title: String,
    /// Printed page it falls on.
    pub page: usize,
    /// For a `.fi` line in a master document: the chapter file it includes,
    /// opened (rather than jumped to) when chosen.
    pub file: Option<PathBuf>,
}

/// State backing the Go to Heading list ([`Mode::Outline`]).
#[derive(Debug, Clone, Default)]
pub struct OutlineState {
    pub items: Vec<OutlineItem>,
    pub selected: usize,
}

/// State backing File ▸ Recent Files ([`Mode::Recent`]).
#[derive(Debug, Clone, Default)]
pub struct RecentState {
    /// Documents, most recent first, with where their cursor was left.
    pub items: Vec<(PathBuf, (usize, usize))>,
    pub selected: usize,
}

/// What a key or click did in a pick-list (Go to Heading, Recent Files).
enum ListAction {
    Stay,
    Choose,
    Close,
}

/// Move a pick-list selection for `key` (arrows or the WordStar diamond, page
/// keys, Home/End) and report Enter / Esc.
fn list_key(selected: &mut usize, len: usize, key: &KeyEvent) -> ListAction {
    let last = len.saturating_sub(1);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match (key.code, ctrl) {
        (KeyCode::Up, _) | (KeyCode::Char('e'), true) => *selected = selected.saturating_sub(1),
        (KeyCode::Down, _) | (KeyCode::Char('x'), true) => *selected = (*selected + 1).min(last),
        (KeyCode::PageUp, _) | (KeyCode::Char('r'), true) => *selected = selected.saturating_sub(10),
        (KeyCode::PageDown, _) | (KeyCode::Char('c'), true) => *selected = (*selected + 10).min(last),
        (KeyCode::Home, _) => *selected = 0,
        (KeyCode::End, _) => *selected = last,
        (KeyCode::Enter, _) => return ListAction::Choose,
        (KeyCode::Esc, _) | (KeyCode::Char('q'), false) => return ListAction::Close,
        _ => {}
    }
    ListAction::Stay
}

/// Positions that follow the text through edits (see [`crate::track`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Anchors {
    markers: [Option<(usize, usize)>; 10],
    prev: Option<(usize, usize)>,
    block: Option<((usize, usize), (usize, usize))>,
}

/// An interactive replace in progress ([`Mode::ReplaceAsk`]).
#[derive(Debug, Clone)]
pub struct ReplaceSession {
    regex: regex::Regex,
    with: String,
    backwards: bool,
    no_ask: bool,
    /// Where the search for the next match starts.
    from: (usize, usize),
    /// The match currently highlighted and awaiting an answer.
    current: Option<((usize, usize), (usize, usize))>,
    replaced: usize,
    /// Widget undo steps the session has used, so one `^U` undoes it all.
    undo_steps: usize,
}

/// The whole application.
pub struct App {
    /// The editable text buffer (raw markdown).
    pub textarea: TextArea<'static>,
    /// Path of the open document, if any.
    pub path: Option<PathBuf>,
    /// Whether the buffer has unsaved changes.
    pub modified: bool,
    /// Set to true to break the main loop.
    pub should_quit: bool,
    /// Current interaction mode.
    pub mode: Mode,
    /// Insert (true) vs overtype (false), shown on the status line.
    pub insert_mode: bool,
    /// Pending WordStar chord prefix, if any.
    pub chord: ChordState,
    /// Transient message shown on the status line (cleared on next key).
    pub status_msg: Option<String>,
    /// Active prompt overlay state (meaningful when `mode == Prompt`).
    pub prompt: PromptState,
    /// The last find / replace (for `^L` and as the next default).
    pub last_find: Option<LastFind>,
    /// Where the cursor was before the last long jump (find, go to page, …),
    /// for `^QP`.
    pub prev_pos: Option<(usize, usize)>,
    /// Place markers `^K0`…`^K9`.
    pub markers: [Option<(usize, usize)>; 10],
    /// Words in the document when it was opened, for "this session".
    session_start_words: usize,
    /// The live word count, cached against the text it was counted for.
    word_count_cache: RefCell<Option<(u64, usize)>>,
    /// The interactive replace in progress (when `mode == ReplaceAsk`).
    replace_session: Option<ReplaceSession>,
    /// Active confirmation modal (present when `mode == Confirm`).
    pub confirm: Option<ConfirmState>,
    /// Active information modal (present when `mode == Info`).
    pub info: Option<InfoState>,
    /// Active calculator dialog (present when `mode == Calculator`).
    pub calc: Option<CalcState>,
    /// The Go to Heading list (present when `mode == Outline`).
    pub outline: Option<OutlineState>,
    /// File ▸ Recent Files (present when `mode == Recent`).
    pub recent: Option<RecentState>,
    /// The spelling dictionary, loaded on first use (or why it couldn't be).
    speller: std::cell::OnceCell<Result<crate::spell::Speller, String>>,
    /// Underline misspelled words as you write (View menu).
    pub spell_highlight: bool,
    /// The spelling check in progress (when `mode == Spell`).
    pub spell: Option<SpellSession>,
    /// Geometry of the Go to Heading list's rows, for mouse hit-testing.
    pub outline_area: Cell<Rect>,
    /// Active header/footer dialog (present when `mode == Header`).
    pub header_dialog: Option<HeaderState>,
    /// Active file browser (present when `mode == Browser`). Native only — the
    /// browser build opens files through the host's file picker.
    #[cfg(not(target_arch = "wasm32"))]
    pub browser: Option<crate::browser::Browser>,
    /// Scroll offset for the preview overlay.
    pub preview_scroll: u16,
    /// The text the preview shows (the document with includes expanded).
    pub preview_source: String,
    /// Scroll offset for the help overlay.
    pub help_scroll: u16,
    /// Open pull-down menu navigation state (used when `mode == Menu`).
    pub menu: crate::menu::MenuState,
    /// Current paragraph alignment choice.
    pub align: AlignChoice,
    /// Whether word wrap is enabled (WordStar wraps by default).
    pub wrap: bool,
    /// Persistent clipboard for block copy / cut / paste.
    pub block_buffer: String,
    /// The marked block (`^KB` … `^KK`), if any.
    pub marked: Option<MarkedBlock>,
    /// True while a block is being marked, so cursor movement extends the
    /// selection even with plain (un-shifted) movement keys.
    marking: bool,
    /// Terminal graphics picker (set at startup when a TTY is available). Native
    /// only — the browser draws the preview to an HTML canvas instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub picker: Option<ratatui_image::picker::Picker>,
    /// Whether a graphical (rather than text) preview is available: a real
    /// terminal graphics protocol on native, always true in the browser canvas.
    pub graphics: bool,
    /// In-progress incremental rasterization job (graphical preview loading).
    pub preview_job: Option<crate::gfx::Job>,
    /// Rasterized pages of the document (graphical preview).
    pub preview_pages: Vec<image::RgbaImage>,
    /// Per-page encoded protocols, built lazily and reused (the zoom == 1 view).
    #[cfg(not(target_arch = "wasm32"))]
    pub preview_page_protocols: RefCell<Vec<Option<ratatui_image::protocol::StatefulProtocol>>>,
    /// Encoded protocol for the current zoomed/panned crop (zoom > 1).
    #[cfg(not(target_arch = "wasm32"))]
    pub preview_zoom_protocol: RefCell<Option<ratatui_image::protocol::StatefulProtocol>>,
    /// The view the zoom protocol was built for, so it is only re-encoded when
    /// the view actually changes.
    pub preview_view_key: Cell<Option<PreviewViewKey>>,
    /// Currently shown page index.
    pub preview_page: usize,
    /// Zoom factor (1.0 = whole page fit to the pane).
    pub preview_zoom: f32,
    /// Normalized pan offset within the page when zoomed (0.0..=1.0).
    pub preview_off: (f32, f32),
    /// Last rendered preview area, for crop sizing.
    pub preview_area: Cell<Rect>,
    /// True while a mouse drag is extending a selection in the editor.
    mouse_selecting: bool,
    /// Time + cell of the last mouse press, for double-click detection.
    last_click: Option<(f64, u16, u16)>,
    /// Screen geometry recorded during the last render, for mouse hit-testing.
    /// `editor_area` is the text column; `editor_pane` also covers the blank
    /// space right of the margin (but not the flag column or scrollbar).
    pub editor_area: Cell<Rect>,
    pub editor_pane: Cell<Rect>,
    pub menu_bar_area: Cell<Rect>,
    pub dropdown_area: Cell<Rect>,
    /// Geometry of the open submenu panel, for mouse hit-testing.
    pub sub_dropdown_area: Cell<Rect>,
    pub browser_list_area: Cell<Rect>,
    /// First visible visual row of the editor viewport, tracked across frames so
    /// the scrollbar thumb reflects the textarea's internal scroll position.
    pub scroll_top: Cell<usize>,
    /// The wrapped layout of the document at the current text width, rebuilt by
    /// the renderer each frame, and the printed `(page, line)` of every row.
    pub visual_rows: RefCell<Vec<crate::wrap::VisualRow>>,
    pub row_pages: RefCell<Vec<(usize, usize)>>,
    /// Action to finish once a pending Save As completes (e.g. quit after naming
    /// an untitled document).
    after_save: Option<AfterSave>,
    /// A multi-step edit (replace-all) that `^U` should undo in one go: the
    /// content hash right after it and how many widget undo steps it took.
    compound_undo: Option<(u64, usize)>,
    /// A file chosen in the host picker, held while the user decides what to do
    /// with unsaved changes (browser only).
    #[cfg(target_arch = "wasm32")]
    pending_open: Option<(String, Vec<u8>)>,
    /// When the crash-recovery copy was last considered, and the content hash
    /// it was written for (native only).
    #[cfg(not(target_arch = "wasm32"))]
    last_autosave_ms: f64,
    #[cfg(not(target_arch = "wasm32"))]
    autosave_hash: Option<u64>,
}

impl App {
    /// Build the app, optionally loading a file from `path`.
    pub fn new(path: Option<String>) -> Result<Self> {
        let path = path.map(PathBuf::from);
        // `imported` is only assigned on native (the browser opens files later,
        // via the host picker, never at construction time).
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut imported = false;
        let textarea = match &path {
            #[cfg(not(target_arch = "wasm32"))]
            Some(p) if p.is_file() => {
                let loaded = crate::wordstar::load(p)?;
                imported = loaded.imported;
                TextArea::new(text_to_lines(&loaded.text))
            }
            _ => TextArea::default(),
        };
        // An imported WordStar file becomes an unsaved Markdown document so the
        // original .WS is never overwritten.
        let path = match (imported, &path) {
            (true, Some(p)) => Some(p.with_extension("md")),
            _ => path,
        };

        let mut app = Self {
            textarea,
            path,
            modified: imported,
            should_quit: false,
            mode: Mode::default(),
            insert_mode: true,
            chord: ChordState::default(),
            status_msg: None,
            prompt: PromptState::default(),
            last_find: None,
            prev_pos: None,
            markers: [None; 10],
            session_start_words: 0,
            word_count_cache: RefCell::new(None),
            replace_session: None,
            confirm: None,
            info: None,
            calc: None,
            outline: None,
            recent: None,
            speller: std::cell::OnceCell::new(),
            spell_highlight: true,
            spell: None,
            outline_area: Cell::new(Rect::ZERO),
            header_dialog: None,
            #[cfg(not(target_arch = "wasm32"))]
            browser: None,
            preview_scroll: 0,
            preview_source: String::new(),
            help_scroll: 0,
            menu: crate::menu::MenuState::default(),
            align: AlignChoice::Left,
            wrap: true,
            block_buffer: String::new(),
            marked: None,
            marking: false,
            #[cfg(not(target_arch = "wasm32"))]
            picker: None,
            // Native starts with no graphics until a protocol is detected; the
            // browser canvas is always able to show the graphical preview.
            graphics: cfg!(target_arch = "wasm32"),
            preview_job: None,
            preview_pages: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            preview_page_protocols: RefCell::new(Vec::new()),
            #[cfg(not(target_arch = "wasm32"))]
            preview_zoom_protocol: RefCell::new(None),
            preview_view_key: Cell::new(None),
            preview_page: 0,
            preview_zoom: 1.0,
            preview_off: (0.0, 0.0),
            preview_area: Cell::new(Rect::ZERO),
            mouse_selecting: false,
            last_click: None,
            editor_area: Cell::new(Rect::ZERO),
            editor_pane: Cell::new(Rect::ZERO),
            menu_bar_area: Cell::new(Rect::ZERO),
            dropdown_area: Cell::new(Rect::ZERO),
            sub_dropdown_area: Cell::new(Rect::ZERO),
            browser_list_area: Cell::new(Rect::ZERO),
            scroll_top: Cell::new(0),
            visual_rows: RefCell::new(Vec::new()),
            row_pages: RefCell::new(Vec::new()),
            after_save: None,
            compound_undo: None,
            #[cfg(target_arch = "wasm32")]
            pending_open: None,
            #[cfg(not(target_arch = "wasm32"))]
            last_autosave_ms: 0.0,
            #[cfg(not(target_arch = "wasm32"))]
            autosave_hash: None,
        };
        app.apply_editor_theme();
        if imported {
            app.set_status("Imported WordStar file — saving will write a new .md file.");
        }
        Ok(app)
    }

    /// Set up a freshly created text widget: the WordStar look and a deep undo
    /// history (the widget's default remembers only 50 keystrokes). Positions
    /// kept for the previous text are dropped.
    fn apply_editor_theme(&mut self) {
        // A new text: positions in the old one no longer mean anything.
        self.markers = [None; 10];
        self.prev_pos = None;
        self.marked = None;
        self.compound_undo = None;
        // The next text may ask for another language.
        self.speller = std::cell::OnceCell::new();
        self.session_start_words = crate::attributes::count_words(self.textarea.lines()).words;
        self.textarea.set_max_histories(UNDO_LEVELS);
        self.textarea.set_style(theme::canvas());
        // WordStar does not underline the current line; keep it plain.
        self.textarea.set_cursor_line_style(theme::canvas());
        self.textarea.set_selection_style(theme::selection());
        self.textarea.set_search_style(theme::search());
        self.textarea.set_wrap_mode(self.wrap_mode());
    }

    fn wrap_mode(&self) -> ratatui_textarea::WrapMode {
        if self.wrap {
            ratatui_textarea::WrapMode::Word
        } else {
            ratatui_textarea::WrapMode::None
        }
    }

    /// Toggle word wrap (WordStar `^OW` / View menu).
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.textarea.set_wrap_mode(self.wrap_mode());
        self.set_status(if self.wrap {
            "Word wrap on."
        } else {
            "Word wrap off."
        });
    }

    /// Document name for the title bar.
    pub fn file_name(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "UNTITLED".into()),
            None => "UNTITLED".into(),
        }
    }

    /// Route a key press according to the current mode.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ignore key-release events (Windows / kitty protocol report them).
        if key.kind == KeyEventKind::Release {
            return;
        }
        let before = self.tracking_snapshot();
        self.dispatch_key(key);
        self.follow_edits(before);
    }

    fn dispatch_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Editor => self.handle_editor_key(key),
            Mode::Clean => self.handle_clean_key(key),
            Mode::Menu => self.handle_menu_key(key),
            Mode::Prompt => self.handle_prompt_key(key),
            Mode::Confirm => self.handle_confirm_key(key),
            Mode::Browser => self.handle_browser_key(key),
            Mode::Preview => self.handle_preview_key(key),
            Mode::Help => self.handle_overlay_key(key, OverlayKind::Help),
            Mode::Info => self.handle_info_key(key),
            Mode::Header => self.handle_header_key(key),
            Mode::Calculator => self.handle_calc_key(key),
            Mode::ReplaceAsk => self.handle_replace_key(key),
            Mode::Outline => self.handle_outline_key(key),
            Mode::Spell => self.handle_spell_key(key),
            Mode::Recent => self.handle_recent_key(key),
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        // Any key other than a repeated quit clears the quit warning and status.
        self.status_msg = None;

        // Alt+<accelerator> opens the matching menu (Alt+F = File, Alt+E = Edit…).
        if key.modifiers.contains(KeyModifiers::ALT)
            && let KeyCode::Char(c) = key.code
            && let Some(idx) = crate::menu::menu_for_accelerator(c)
        {
            self.open_menu();
            self.menu.select_menu(idx);
            return;
        }

        match keymap::resolve(&mut self.chord, key) {
            Resolution::Command(cmd) => commands::execute(self, cmd),
            Resolution::Pending(hint) => {
                self.status_msg = Some(hint.to_string());
            }
            Resolution::Beep => {
                self.set_status("Unrecognized command.");
                ring_bell();
            }
            Resolution::PassThrough => self.pass_to_editor(key),
        }
    }

    /// Hand a key to the text widget, honoring overtype mode for printable input.
    fn pass_to_editor(&mut self, key: KeyEvent) {
        let mut input: Input = crate::input::key_to_input(&key);

        // While marking a block, keep movement keys extending the selection
        // (the widget cancels the selection on an un-shifted move otherwise).
        if self.marking {
            if matches!(
                input.key,
                Key::Left
                    | Key::Right
                    | Key::Up
                    | Key::Down
                    | Key::Home
                    | Key::End
                    | Key::PageUp
                    | Key::PageDown
            ) {
                input.shift = true;
            } else {
                self.marking = false;
            }
        }

        // Overtype: replace the character under the cursor instead of inserting,
        // unless we are at end-of-line (where overtype behaves like insert).
        if !self.insert_mode && matches!(input.key, Key::Char(_)) && !self.at_line_end() {
            self.textarea.delete_next_char();
        }

        // `input()` maps the PageUp/PageDown keys to a viewport scroll of ±one
        // screen height, internally, without going through our command path. Note
        // the delta so we can mirror it onto our tracked scroll position below;
        // otherwise the scrollbar thumb desyncs from the real viewport.
        let scroll_delta = match input.key {
            Key::PageUp => -(self.viewport_height() as isize),
            Key::PageDown => self.viewport_height() as isize,
            _ => 0,
        };

        if self.textarea.input(input) {
            self.modified = true;
        }

        if scroll_delta != 0 {
            self.scroll_viewport(scroll_delta);
        }
    }

    fn at_line_end(&self) -> bool {
        let cursor = self.textarea.cursor();
        let line_len = self
            .textarea
            .lines()
            .get(cursor.0)
            .map(|l| l.chars().count())
            .unwrap_or(0);
        cursor.1 >= line_len
    }

    // ------------------------------------------------------------------
    // Pull-down menus
    // ------------------------------------------------------------------

    /// Open the menu bar (F9).
    pub fn open_menu(&mut self) {
        self.mode = Mode::Menu;
        self.menu = crate::menu::MenuState::default();
        self.status_msg = None;
    }

    fn handle_menu_key(&mut self, key: KeyEvent) {
        use crate::menu::Activation;
        match key.code {
            KeyCode::Esc | KeyCode::F(9) => self.mode = Mode::Editor,
            KeyCode::Left => self.menu.move_left(),
            KeyCode::Right => self.menu.move_right(),
            KeyCode::Up => self.menu.prev_item(),
            KeyCode::Down => self.menu.next_item(),
            KeyCode::Enter => match self.menu.activate() {
                Activation::Run(cmd) => {
                    self.mode = Mode::Editor;
                    commands::execute(self, cmd);
                }
                Activation::OpenedSubmenu | Activation::None => {}
            },
            KeyCode::Char(c) => {
                // A letter jumps to the matching menu title.
                self.menu.jump_to_title(c);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Prompt overlay (find / replace / save-as)
    // ------------------------------------------------------------------

    /// Open the find prompt (`^QF`), offering the last search text.
    pub fn start_find(&mut self) {
        let last = self.last_find.as_ref().map(|f| f.find.clone()).unwrap_or_default();
        self.open_prompt(PromptKind::Find, "Find:", last);
    }

    /// Open find-and-replace (`^QA`): the text, its replacement, then options.
    pub fn start_replace(&mut self) {
        let last = self.last_find.as_ref().map(|f| f.find.clone()).unwrap_or_default();
        self.open_prompt(PromptKind::Replace, "Find:", last);
    }

    /// Open a prompt, offering `input` as the default answer.
    fn open_prompt(&mut self, kind: PromptKind, label: &str, input: String) {
        self.mode = Mode::Prompt;
        self.prompt = PromptState::default();
        self.prompt_step(kind, label, "", input);
    }

    /// Move the open prompt on to its next question, keeping what the earlier
    /// steps collected. `hint` is shown under the input.
    fn prompt_step(&mut self, kind: PromptKind, label: &str, hint: &'static str, input: String) {
        self.prompt.kind = kind;
        self.prompt.label = label.into();
        self.prompt.hint = hint;
        self.prompt.cursor = input.chars().count();
        self.prompt.fresh = !input.is_empty();
        self.prompt.input = input;
    }

    /// Open the save-as prompt, pre-filled with the current file name (relative
    /// names are saved next to the current document).
    pub fn start_save_as(&mut self) {
        let current = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.open_prompt(PromptKind::SaveAs, "Save as:", current);
    }

    /// Turn a typed Save As name into a path: `~/` expands to the home folder, a
    /// missing extension becomes `.md`, and a relative name lands in the current
    /// document's folder.
    fn resolve_save_path(&self, name: &str) -> PathBuf {
        #[cfg(not(target_arch = "wasm32"))]
        let mut path = match (name.strip_prefix("~/"), dirs::home_dir()) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(name),
        };
        #[cfg(target_arch = "wasm32")]
        let mut path = PathBuf::from(name);
        if path.extension().is_none() {
            path.set_extension("md");
        }
        if path.is_relative()
            && let Some(dir) = self
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .filter(|d| !d.as_os_str().is_empty())
        {
            path = dir.join(path);
        }
        path
    }

    /// Open the font-name prompt.
    pub fn start_font_prompt(&mut self) {
        self.open_prompt(PromptKind::Font, "Font name:", String::new());
    }

    /// Open the font-size prompt.
    pub fn start_size_prompt(&mut self) {
        self.open_prompt(PromptKind::FontSize, "Font size:", String::new());
    }

    /// Line editing in a prompt: arrows (or `^S`/`^D`), Home/End, Backspace,
    /// Del (or `^G`), `^Y` to clear. A pre-filled default is replaced by typing.
    fn handle_prompt_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let p = &mut self.prompt;
        let len = p.input.chars().count();
        p.cursor = p.cursor.min(len);
        let byte_at = |s: &str, i: usize| s.char_indices().nth(i).map_or(s.len(), |(b, _)| b);
        let clear = |p: &mut PromptState| {
            p.input.clear();
            p.cursor = 0;
        };
        match (key.code, ctrl) {
            (KeyCode::Esc, _) => {
                if p.kind == PromptKind::SpellReplace && self.spell.is_some() {
                    // Back to the spelling question.
                    self.mode = Mode::Spell;
                    return;
                }
                self.mode = Mode::Editor;
                self.after_save = None;
                self.set_status("Cancelled.");
                return;
            }
            (KeyCode::Enter, _) => {
                self.confirm_prompt();
                return;
            }
            (KeyCode::Left, _) | (KeyCode::Char('s'), true) => p.cursor = p.cursor.saturating_sub(1),
            (KeyCode::Right, _) | (KeyCode::Char('d'), true) => p.cursor = (p.cursor + 1).min(len),
            (KeyCode::Home, _) => p.cursor = 0,
            (KeyCode::End, _) => p.cursor = len,
            (KeyCode::Char('y'), true) => clear(p),
            (KeyCode::Backspace, _) | (KeyCode::Char('h'), true) => {
                if p.fresh {
                    clear(p);
                } else if p.cursor > 0 {
                    p.cursor -= 1;
                    let at = byte_at(&p.input, p.cursor);
                    p.input.remove(at);
                }
            }
            (KeyCode::Delete, _) | (KeyCode::Char('g'), true) => {
                if p.fresh {
                    clear(p);
                } else if p.cursor < len {
                    let at = byte_at(&p.input, p.cursor);
                    p.input.remove(at);
                }
            }
            (KeyCode::Char(c), false) if !alt => {
                if p.fresh {
                    clear(p);
                }
                let at = byte_at(&p.input, p.cursor);
                p.input.insert(at, c);
                p.cursor += 1;
            }
            _ => return,
        }
        // Any edit or cursor move turns a suggested default into ordinary text.
        self.prompt.fresh = false;
    }

    fn confirm_prompt(&mut self) {
        match self.prompt.kind {
            PromptKind::Find | PromptKind::Replace => {
                let find = self.prompt.input.clone();
                let replacing = self.prompt.kind == PromptKind::Replace;
                if find.is_empty() {
                    self.mode = Mode::Editor;
                    let _ = self.textarea.set_search_pattern("");
                    self.set_status(if replacing {
                        "Replace cancelled."
                    } else {
                        "Search cleared."
                    });
                    return;
                }
                self.prompt.pending_find = Some(find);
                let last = self.last_find.clone();
                if replacing {
                    let with = last.and_then(|f| f.replace).unwrap_or_default();
                    self.prompt_step(PromptKind::ReplaceWith, "Replace with:", "", with);
                } else {
                    // Offer the previous options (minus "don't ask", a replace one).
                    let options = last
                        .map(|f| FindOptions { no_ask: false, ..f.options }.letters())
                        .unwrap_or_default();
                    self.prompt_step(
                        PromptKind::FindOptions,
                        "Options:",
                        "B backwards · U ignore case · W whole words · G whole document",
                        options,
                    );
                }
            }
            PromptKind::ReplaceWith => {
                self.prompt.pending_replace = Some(self.prompt.input.clone());
                let options = self
                    .last_find
                    .as_ref()
                    .map(|f| f.options.letters())
                    .unwrap_or_default();
                self.prompt_step(
                    PromptKind::ReplaceOptions,
                    "Options:",
                    "B back · U ignore case · W whole words · G whole doc · N don't ask",
                    options,
                );
            }
            PromptKind::FindOptions | PromptKind::ReplaceOptions => {
                let options = FindOptions::parse(&self.prompt.input);
                let find = self.prompt.pending_find.take().unwrap_or_default();
                let replace = self.prompt.pending_replace.take();
                self.mode = Mode::Editor;
                self.last_find = Some(LastFind {
                    find: find.clone(),
                    replace: replace.clone(),
                    options: options.clone(),
                });
                match replace {
                    Some(with) => self.start_replace_session(&find, &with, &options, true),
                    None => self.run_find(&find, &options, true),
                }
            }
            PromptKind::SaveAs => {
                let name = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                if name.is_empty() {
                    self.after_save = None;
                    self.set_status("Save cancelled (no name).");
                    return;
                }
                let path = self.resolve_save_path(&name);
                if path.exists() && self.path.as_ref() != Some(&path) {
                    self.confirm = Some(ConfirmState {
                        message: format!("{} already exists. Overwrite?", path.display()),
                        action: ConfirmAction::OverwriteSave(path),
                    });
                    self.mode = Mode::Confirm;
                } else {
                    self.save_as(path);
                }
            }
            PromptKind::Font => {
                let name = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                if name.is_empty() {
                    self.set_status("Font unchanged.");
                } else {
                    let close = format!("]{{font=\"{name}\"}}");
                    self.apply_format("[", &close, &format!("Font: {name}"));
                }
            }
            PromptKind::ExportPdf => {
                let name = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                if name.is_empty() {
                    self.set_status("PDF export cancelled.");
                    return;
                }
                let path = PathBuf::from(name);
                if path.exists() {
                    self.confirm = Some(ConfirmState {
                        message: format!("{} already exists. Overwrite?", path.display()),
                        action: ConfirmAction::OverwriteExport(path),
                    });
                    self.mode = Mode::Confirm;
                } else {
                    self.do_export(&path);
                }
            }
            PromptKind::FontSize => {
                let raw = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                match raw.parse::<u32>() {
                    Ok(n) => {
                        let close = format!("]{{size={n}}}");
                        self.apply_format("[", &close, &format!("Size: {n}"));
                    }
                    Err(_) => self.set_status("Size must be a number."),
                }
            }
            PromptKind::InsertFile => {
                let name = self.prompt.input.clone();
                self.mode = Mode::Editor;
                self.insert_file(&name);
            }
            PromptKind::IncludeFile => {
                let name = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                if name.is_empty() {
                    return self.set_status("Cancelled.");
                }
                let found = crate::book::resolve(&name, self.document_dir().as_deref()).is_file();
                let note = if found {
                    format!("Included {name} — the preview, PDF and word count take it in.")
                } else {
                    format!("Included {name} (not found yet — it will print as missing).")
                };
                self.insert_dot_command(&format!(".fi {name}"), &note);
            }
            PromptKind::WriteBlock => {
                let name = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                let Some(text) = self.block_text() else {
                    return self.set_status("The block is gone — mark it again.");
                };
                if name.is_empty() {
                    return self.set_status("Write cancelled (no name).");
                }
                let path = self.resolve_save_path(&name);
                if path.exists() {
                    self.confirm = Some(ConfirmState {
                        message: format!("{} already exists. Overwrite?", path.display()),
                        action: ConfirmAction::OverwriteBlock(path, text),
                    });
                    self.mode = Mode::Confirm;
                } else {
                    self.write_block(&path, &text);
                }
            }
            PromptKind::SpellReplace => {
                let with = self.prompt.input.clone();
                match self.spell.clone() {
                    Some(session) if !with.is_empty() => self.spell_replace(&session, &with),
                    Some(_) => self.mode = Mode::Spell,
                    None => self.mode = Mode::Editor,
                }
            }
            PromptKind::SetMarker | PromptKind::GotoMarker => {
                let set = self.prompt.kind == PromptKind::SetMarker;
                self.mode = Mode::Editor;
                match self.prompt.input.trim().parse::<usize>() {
                    Ok(n) if n <= 9 && set => self.set_marker(n),
                    Ok(n) if n <= 9 => self.goto_marker(n),
                    _ => self.set_status("Markers are numbered 0 to 9."),
                }
            }
            PromptKind::RightMargin => {
                let raw = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                match raw.parse::<usize>() {
                    Ok(n) if (MIN_RIGHT_MARGIN..=MAX_RIGHT_MARGIN).contains(&n) => {
                        self.set_right_margin(n)
                    }
                    _ => self.set_status(format!(
                        "Right margin must be a column from {MIN_RIGHT_MARGIN} to {MAX_RIGHT_MARGIN}."
                    )),
                }
            }
            PromptKind::GoToPage => {
                let raw = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                match raw.parse::<usize>() {
                    Ok(n) => self.goto_page(n),
                    Err(_) => self.set_status("Page must be a number."),
                }
            }
        }
    }

    /// Find `find` with `options` and put the cursor on the match. `first` is
    /// the initial search (from `^QF`), where `G` means start from the top (or
    /// the end); repeats with `^L` carry on from the cursor.
    fn run_find(&mut self, find: &str, options: &FindOptions, first: bool) {
        let pattern = options.pattern(find);
        let regex = match regex::Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => return self.set_status(format!("Bad search: {e}")),
        };
        // The widget highlights every match.
        let _ = self.textarea.set_search_pattern(&pattern);
        let from_edge = first && options.whole_document;
        let from = if from_edge {
            self.document_edge(options.backwards)
        } else {
            self.cursor_pos()
        };
        match self.next_match(&regex, from, options.backwards, !from_edge) {
            Some((start, _)) => {
                self.remember_position();
                self.clear_marking();
                self.textarea.move_cursor(jump(start));
                self.set_status(format!("Found \"{find}\". ^L finds the next."));
            }
            None if first => self.set_status(format!("\"{find}\" not found.")),
            None => self.set_status("No more matches."),
        }
    }

    /// `^L` — repeat the last find, or carry on with the last replace.
    pub fn find_next(&mut self) {
        let Some(last) = self.last_find.clone() else {
            self.set_status("No active search. Use ^QF to find.");
            return;
        };
        match &last.replace {
            None => self.run_find(&last.find, &last.options, false),
            Some(with) => self.start_replace_session(&last.find, with, &last.options, false),
        }
    }

    /// The very start of the document, or its end.
    fn document_edge(&self, end: bool) -> (usize, usize) {
        if end {
            let lines = self.textarea.lines();
            let last = lines.len() - 1;
            (last, lines[last].chars().count())
        } else {
            (0, 0)
        }
    }

    /// The next match of `regex` after (or before, `backwards`) `from`, as
    /// `(start, end)` positions. With `skip_at`, a match starting exactly at
    /// `from` doesn't count. The search stops at the document's edge.
    fn next_match(
        &self,
        regex: &regex::Regex,
        from: (usize, usize),
        backwards: bool,
        skip_at: bool,
    ) -> Option<((usize, usize), (usize, usize))> {
        let lines = self.textarea.lines();
        let col_of = |line: &str, byte: usize| line[..byte].chars().count();
        let span = |row: usize, m: regex::Match| {
            let line = &lines[row];
            ((row, col_of(line, m.start())), (row, col_of(line, m.end())))
        };
        if backwards {
            for row in (0..=from.0.min(lines.len() - 1)).rev() {
                let line = &lines[row];
                let before = |m: &regex::Match| {
                    row != from.0 || {
                        let c = col_of(line, m.start());
                        if skip_at { c < from.1 } else { c <= from.1 }
                    }
                };
                if let Some(m) = regex.find_iter(line).filter(before).last() {
                    return Some(span(row, m));
                }
            }
            return None;
        }
        for (row, line) in lines.iter().enumerate().skip(from.0) {
            let min_col = if row == from.0 { from.1 + usize::from(skip_at) } else { 0 };
            if min_col > line.chars().count() {
                continue;
            }
            let at = line.char_indices().nth(min_col).map_or(line.len(), |(b, _)| b);
            if let Some(m) = regex.find_at(line, at) {
                return Some(span(row, m));
            }
        }
        None
    }

    /// Start replacing `find` with `with`. Unless the `N` option says not to
    /// ask, each match is highlighted in turn with a Y/N question. `G` covers the
    /// whole document; otherwise it runs from the cursor to the end (or start).
    fn start_replace_session(&mut self, find: &str, with: &str, options: &FindOptions, first: bool) {
        let pattern = options.pattern(find);
        let regex = match regex::Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => return self.set_status(format!("Bad search: {e}")),
        };
        let _ = self.textarea.set_search_pattern(&pattern);
        let from_edge = first && options.whole_document;
        if from_edge && options.no_ask {
            // Everything, without asking: one edit for the lot.
            self.replace_all(&regex, with);
            return;
        }
        let from = if from_edge {
            self.document_edge(options.backwards)
        } else {
            self.cursor_pos()
        };
        self.clear_marking();
        self.replace_session = Some(ReplaceSession {
            regex,
            with: with.to_string(),
            backwards: options.backwards,
            no_ask: options.no_ask,
            from,
            current: None,
            replaced: 0,
            undo_steps: 0,
        });
        self.advance_replace();
    }

    /// Find the session's next match: replace it straight away (`N`), or
    /// highlight it and ask.
    fn advance_replace(&mut self) {
        loop {
            let Some(s) = self.replace_session.as_ref() else {
                return;
            };
            let (regex, from, backwards, no_ask) = (s.regex.clone(), s.from, s.backwards, s.no_ask);
            let Some((start, end)) = self.next_match(&regex, from, backwards, backwards) else {
                self.finish_replace();
                return;
            };
            if no_ask {
                self.replace_match(start, end);
                continue;
            }
            self.textarea.cancel_selection();
            self.textarea.move_cursor(jump(start));
            self.textarea.start_selection();
            self.textarea.move_cursor(jump(end));
            if let Some(s) = self.replace_session.as_mut() {
                s.current = Some((start, end));
            }
            self.mode = Mode::ReplaceAsk;
            self.set_status("Replace this one?  Y)es  N)o  A)ll the rest  Esc) stop");
            return;
        }
    }

    /// Replace the match from `start` to `end` and move the search past it.
    fn replace_match(&mut self, start: (usize, usize), end: (usize, usize)) {
        let Some(with) = self.replace_session.as_ref().map(|s| s.with.clone()) else {
            return;
        };
        self.textarea.cancel_selection();
        self.textarea.move_cursor(jump(start));
        self.textarea.start_selection();
        self.textarea.move_cursor(jump(end));
        self.textarea.insert_str(&with);
        self.modified = true;
        let after = self.cursor_pos();
        if let Some(s) = self.replace_session.as_mut() {
            s.undo_steps += 1 + usize::from(!with.is_empty());
            s.replaced += 1;
            s.from = if s.backwards { start } else { after };
        }
    }

    /// Keys while a replace is asking about the highlighted match.
    fn handle_replace_key(&mut self, key: KeyEvent) {
        let current = self.replace_session.as_mut().and_then(|s| s.current.take());
        let Some((start, end)) = current else {
            return self.finish_replace();
        };
        match key.code {
            KeyCode::Char('y' | 'Y') => self.replace_match(start, end),
            KeyCode::Char('a' | 'A') => {
                if let Some(s) = self.replace_session.as_mut() {
                    s.no_ask = true;
                }
                self.replace_match(start, end);
            }
            KeyCode::Char('n' | 'N') => {
                if let Some(s) = self.replace_session.as_mut() {
                    s.from = if s.backwards { start } else { end };
                }
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => return self.finish_replace(),
            _ => {
                // Not an answer: keep asking about the same match.
                if let Some(s) = self.replace_session.as_mut() {
                    s.current = Some((start, end));
                }
                return;
            }
        }
        self.advance_replace();
    }

    /// End the replace session and report; one `^U` undoes all of it.
    fn finish_replace(&mut self) {
        let Some(s) = self.replace_session.take() else {
            return;
        };
        self.textarea.cancel_selection();
        self.mode = Mode::Editor;
        if s.undo_steps > 0 {
            self.compound_undo = Some((self.content_hash(), s.undo_steps));
        }
        match s.replaced {
            0 => self.set_status("No matches to replace."),
            n => self.set_status(format!("Replaced {n} occurrence(s) — ^U undoes.")),
        }
    }

    /// Replace every match of `regex` with `with`.
    ///
    /// Only the span between the first and last change is rewritten, as a single
    /// selection edit, so the cursor, search and undo history survive and one
    /// `^U` restores the original text.
    fn replace_all(&mut self, regex: &regex::Regex, with: &str) {
        let text = self.textarea.lines().join("\n");
        let count = regex.find_iter(&text).count();
        if count == 0 {
            self.set_status("No matches to replace.");
            return;
        }
        let replaced = regex.replace_all(&text, regex::NoExpand(with)).into_owned();
        let old: Vec<char> = text.chars().collect();
        let new: Vec<char> = replaced.chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let middle: String = new[prefix..new.len() - suffix].iter().collect();
        let start = char_pos(&old, prefix);
        let end = char_pos(&old, old.len() - suffix);

        let cursor = self.textarea.cursor();
        self.clear_marking();
        self.textarea.move_cursor(jump(start));
        self.textarea.start_selection();
        self.textarea.move_cursor(jump(end));
        // An empty selection or an empty insertion adds no undo step of its own.
        let steps = usize::from(start != end) + usize::from(!middle.is_empty());
        self.textarea.insert_str(middle);
        self.textarea.move_cursor(jump((cursor.0, cursor.1)));
        self.compound_undo = Some((self.content_hash(), steps));
        self.modified = true;
        self.set_status(format!("Replaced {count} occurrence(s) — ^U undoes."));
    }

    /// `^U` — undo the last edit, treating a replace-all as a single edit.
    pub fn undo(&mut self) {
        let steps = match self.compound_undo.take() {
            Some((hash, steps)) if hash == self.content_hash() => steps,
            _ => 1,
        };
        for _ in 0..steps {
            if self.textarea.undo() {
                self.modified = true;
            }
        }
    }

    /// A hash of the buffer's text, for cheap "has anything changed?" checks.
    fn content_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.textarea.lines().hash(&mut hasher);
        hasher.finish()
    }

    // ------------------------------------------------------------------
    // File browser
    // ------------------------------------------------------------------

    /// Open the file browser at the document's directory (or the cwd / home).
    /// Open the in-app file browser at the document's directory (or cwd / home).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_browser(&mut self) {
        let start = self
            .path
            .as_ref()
            .and_then(|p| p.parent().map(PathBuf::from))
            .filter(|p| p.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        match crate::browser::Browser::new(start) {
            Ok(b) => {
                self.browser = Some(b);
                self.mode = Mode::Browser;
            }
            Err(e) => self.set_status(format!("Cannot open browser: {e}")),
        }
    }

    /// In the browser there is no in-app file tree; defer to the host's native
    /// file picker. The chosen file arrives asynchronously and is picked up by
    /// [`poll_pending_open`](Self::poll_pending_open) on a later frame.
    #[cfg(target_arch = "wasm32")]
    pub fn open_browser(&mut self) {
        crate::platform::pick_file(crate::platform::stash_open);
        self.set_status("Choose a .ws or .md file…");
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn handle_browser_key(&mut self, key: KeyEvent) {
        let Some(browser) = self.browser.as_mut() else {
            self.mode = Mode::Editor;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Editor;
                self.browser = None;
            }
            KeyCode::Up => browser.select_prev(),
            KeyCode::Down => browser.select_next(),
            KeyCode::Left | KeyCode::PageUp => browser.select_up_column(),
            KeyCode::Right | KeyCode::PageDown => browser.select_down_column(),
            KeyCode::Enter => match browser.activate() {
                crate::browser::Activation::Stay => {}
                crate::browser::Activation::Open(path) => {
                    self.browser = None;
                    self.mode = Mode::Editor;
                    self.guard_unsaved(AfterSave::Open(path));
                }
            },
            _ => {}
        }
    }

    /// Browser mode never activates on wasm (Open uses the host picker), so this
    /// stub only keeps the mode dispatch exhaustive.
    #[cfg(target_arch = "wasm32")]
    fn handle_browser_key(&mut self, _key: KeyEvent) {
        self.mode = Mode::Editor;
    }

    /// Apply a freshly loaded document to the editor, replacing the buffer and
    /// updating the path / modified state. Shared by every open path.
    fn apply_loaded(&mut self, loaded: crate::wordstar::Loaded, path: PathBuf) {
        self.textarea = TextArea::new(text_to_lines(&loaded.text));
        self.apply_editor_theme();
        if loaded.imported {
            // Imported WordStar → unsaved Markdown; keep the original intact.
            let md = path.with_extension("md");
            self.set_status(format!(
                "Imported {} — save writes {}",
                path.display(),
                md.display()
            ));
            self.path = Some(md);
            self.modified = true;
        } else {
            self.set_status(format!("Opened {}", path.display()));
            self.path = Some(path);
            self.modified = false;
        }
    }

    /// Load a file from disk into the editor, replacing the current buffer.
    #[cfg(not(target_arch = "wasm32"))]
    fn load_file(&mut self, path: PathBuf) {
        match crate::wordstar::load(&path) {
            Ok(loaded) => {
                self.apply_loaded(loaded, path);
                self.restore_last_position();
                self.offer_recovery();
            }
            Err(e) => self.set_status(format!("Open failed: {e}")),
        }
    }

    /// If the user has chosen a file via the host picker, load it (asking first
    /// about unsaved changes). Called once per frame from the browser render loop.
    #[cfg(target_arch = "wasm32")]
    pub fn poll_pending_open(&mut self) {
        if let Some(file) = crate::platform::take_open() {
            self.pending_open = Some(file);
            self.guard_unsaved(AfterSave::ApplyPicked);
        }
    }

    /// The current document as Markdown (for the browser autosave).
    #[cfg(target_arch = "wasm32")]
    pub fn document_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// The current file name (empty if untitled), for the browser autosave.
    #[cfg(target_arch = "wasm32")]
    pub fn draft_path(&self) -> String {
        self.path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Restore an autosaved draft into the editor on browser startup.
    #[cfg(target_arch = "wasm32")]
    pub fn restore_draft(&mut self, text: &str, path: &str, modified: bool) {
        self.textarea = TextArea::new(text_to_lines(text));
        self.apply_editor_theme();
        self.path = (!path.is_empty()).then(|| PathBuf::from(path));
        self.modified = modified;
        self.set_status("Recovered your unsaved document from this browser.");
    }

    // ------------------------------------------------------------------
    // Preview / Help overlays
    // ------------------------------------------------------------------

    /// Record the terminal graphics picker detected at startup.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_picker(&mut self, picker: ratatui_image::picker::Picker) {
        use ratatui_image::picker::ProtocolType;
        self.graphics = picker.protocol_type() != ProtocolType::Halfblocks;
        self.picker = Some(picker);
    }

    /// Drop any cached preview render state. On native that means the encoded
    /// image protocols; the browser canvas keeps no per-page cache.
    #[cfg(not(target_arch = "wasm32"))]
    fn clear_preview_protocols(&self) {
        self.preview_page_protocols.borrow_mut().clear();
        *self.preview_zoom_protocol.borrow_mut() = None;
        self.preview_view_key.set(None);
    }

    #[cfg(target_arch = "wasm32")]
    fn clear_preview_protocols(&self) {
        self.preview_view_key.set(None);
    }

    /// Toggle the markdown preview. Uses the graphical preview (one page at a
    /// time, zoomable/scrollable) when a terminal graphics protocol is available
    /// (always, in the browser canvas), otherwise the text preview.
    pub fn toggle_preview(&mut self) {
        if self.mode == Mode::Preview {
            self.close_preview();
        } else {
            self.preview_scroll = 0;
            self.preview_page = 0;
            self.preview_zoom = 1.0;
            self.preview_off = (0.0, 0.0);
            self.preview_pages.clear();
            self.clear_preview_protocols();
            self.preview_job = None;
            let book = self.book_text();
            if self.graphics {
                // Start an incremental render; the main loop drives it while the
                // loading modal shows progress. `None` means no fonts → text view.
                self.preview_job = crate::gfx::Job::new(&book.text);
            }
            self.report_includes(&book);
            self.preview_source = book.text;
            self.mode = Mode::Preview;
        }
    }

    fn close_preview(&mut self) {
        self.mode = Mode::Editor;
        self.preview_job = None;
        self.preview_pages.clear();
        self.clear_preview_protocols();
    }

    /// True while the graphical preview is still being rasterized.
    pub fn preview_loading(&self) -> bool {
        self.preview_job.is_some()
    }

    /// Progress of the preview rasterization (0.0..=1.0).
    pub fn preview_progress(&self) -> f32 {
        self.preview_job
            .as_ref()
            .map(|j| j.progress())
            .unwrap_or(1.0)
    }

    /// Do a slice of preview rasterization work; finalize pages when complete.
    pub fn step_preview_job(&mut self) {
        let Some(mut job) = self.preview_job.take() else {
            return;
        };
        job.step(33.0);
        if job.is_done() {
            self.preview_pages = job.finish();
            // Native: allocate one lazy protocol slot per page. The browser draws
            // straight from `preview_pages`, so it needs no per-page cache.
            #[cfg(not(target_arch = "wasm32"))]
            {
                *self.preview_page_protocols.borrow_mut() =
                    (0..self.preview_pages.len()).map(|_| None).collect();
            }
        } else {
            self.preview_job = Some(job);
        }
    }

    /// Ensure the protocol needed for the current view exists (building/encoding
    /// it only when missing or when the zoomed view actually changed). Called
    /// from the renderer, so navigation itself does no image work. Native only —
    /// the browser paints `preview_pages` straight to a canvas.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn ensure_preview(&self, inner: Rect) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        if self.preview_zoom <= 1.001 {
            // Whole-page view: build this page's protocol once and reuse it.
            let mut cache = self.preview_page_protocols.borrow_mut();
            if let Some(slot) = cache.get_mut(self.preview_page)
                && slot.is_none()
                && let Some(page) = self.preview_pages.get(self.preview_page)
            {
                *slot =
                    Some(picker.new_resize_protocol(image::DynamicImage::ImageRgba8(page.clone())));
            }
        } else {
            // Zoomed view: re-encode only when (page, zoom, pan, area) changes.
            let key = self.zoom_view_key(inner);
            if self.preview_view_key.get() != Some(key)
                && let Some(img) = self.zoom_crop(inner)
            {
                *self.preview_zoom_protocol.borrow_mut() =
                    Some(picker.new_resize_protocol(image::DynamicImage::ImageRgba8(img)));
                self.preview_view_key.set(Some(key));
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn zoom_view_key(&self, inner: Rect) -> PreviewViewKey {
        (
            self.preview_page,
            (self.preview_zoom * 1000.0) as u32,
            (self.preview_off.0 * 1000.0) as i32,
            (self.preview_off.1 * 1000.0) as i32,
            inner.width,
            inner.height,
        )
    }

    /// Crop the current page to a window sized to the pane's aspect ratio,
    /// magnified by the zoom factor and positioned by the pan offset.
    #[cfg(not(target_arch = "wasm32"))]
    fn zoom_crop(&self, inner: Rect) -> Option<image::RgbaImage> {
        let page = self.preview_pages.get(self.preview_page)?;
        let (fw, fh) = self
            .picker
            .as_ref()
            .map(|p| {
                let f = p.font_size();
                (f.width.max(1) as f32, f.height.max(1) as f32)
            })
            .unwrap_or((8.0, 16.0));
        let pane_w = (inner.width as f32 * fw).max(1.0);
        let pane_h = (inner.height as f32 * fh).max(1.0);
        let cw = (page.width() as f32 / self.preview_zoom)
            .round()
            .clamp(1.0, page.width() as f32);
        let ch = (cw * pane_h / pane_w)
            .round()
            .clamp(1.0, page.height() as f32);
        let max_x = page.width().saturating_sub(cw as u32);
        let max_y = page.height().saturating_sub(ch as u32);
        let x = (self.preview_off.0 * max_x as f32).round() as u32;
        let y = (self.preview_off.1 * max_y as f32).round() as u32;
        Some(image::imageops::crop_imm(page, x, y, cw as u32, ch as u32).to_image())
    }

    fn preview_set_page(&mut self, page: usize) {
        if page < self.preview_pages.len() && page != self.preview_page {
            self.preview_page = page;
            self.preview_off = (0.0, 0.0);
        }
    }

    fn preview_zoom_by(&mut self, factor: f32) {
        let z = (self.preview_zoom * factor).clamp(1.0, 6.0);
        if (z - self.preview_zoom).abs() > f32::EPSILON {
            self.preview_zoom = z;
            if z <= 1.001 {
                self.preview_off = (0.0, 0.0);
            }
        }
    }

    fn preview_pan(&mut self, dx: f32, dy: f32) {
        self.preview_off.0 = (self.preview_off.0 + dx).clamp(0.0, 1.0);
        self.preview_off.1 = (self.preview_off.1 + dy).clamp(0.0, 1.0);
    }

    /// Key handling for the preview: graphical pages (navigate/zoom/pan) or the
    /// scrolling text preview as a fallback.
    fn handle_preview_key(&mut self, key: KeyEvent) {
        // While the graphical preview is still rendering, only allow cancelling.
        if self.preview_loading() {
            if matches!(key.code, KeyCode::Esc | KeyCode::F(5) | KeyCode::Char('q')) {
                self.close_preview();
            }
            return;
        }
        // Text preview (no graphical pages): reuse the scrolling overlay handler.
        if self.preview_pages.is_empty() {
            self.handle_overlay_key(key, OverlayKind::Preview);
            return;
        }
        let zoomed = self.preview_zoom > 1.001;
        match key.code {
            KeyCode::Esc | KeyCode::F(5) | KeyCode::Char('q') => self.close_preview(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.preview_zoom_by(1.25),
            KeyCode::Char('-') | KeyCode::Char('_') => self.preview_zoom_by(0.8),
            KeyCode::PageDown | KeyCode::Char('n') | KeyCode::Char(' ') => {
                self.preview_set_page(self.preview_page + 1)
            }
            KeyCode::PageUp | KeyCode::Char('p') => {
                self.preview_set_page(self.preview_page.wrapping_sub(1))
            }
            KeyCode::Home => self.preview_set_page(0),
            KeyCode::End => self.preview_set_page(self.preview_pages.len().saturating_sub(1)),
            KeyCode::Up if zoomed => self.preview_pan(0.0, -0.12),
            KeyCode::Down if zoomed => self.preview_pan(0.0, 0.12),
            KeyCode::Left if zoomed => self.preview_pan(-0.12, 0.0),
            KeyCode::Right if zoomed => self.preview_pan(0.12, 0.0),
            KeyCode::Up | KeyCode::Left => self.preview_set_page(self.preview_page.wrapping_sub(1)),
            KeyCode::Down | KeyCode::Right => self.preview_set_page(self.preview_page + 1),
            _ => {}
        }
    }

    /// Toggle the help overlay.
    pub fn toggle_help(&mut self) {
        self.mode = if self.mode == Mode::Help {
            Mode::Editor
        } else {
            self.help_scroll = 0;
            Mode::Help
        };
    }

    /// `^OD` — toggle the "hide formatting markup" reading view.
    pub fn toggle_markup(&mut self) {
        if self.mode == Mode::Clean {
            self.mode = Mode::Editor;
            self.set_status("Markup shown.");
        } else {
            self.preview_scroll = 0;
            self.mode = Mode::Clean;
            self.set_status("Markup hidden — Esc or ^OD to edit again.");
        }
    }

    /// Input handling for the read-only "hide markup" view: scroll and toggle.
    fn handle_clean_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Editor;
                self.set_status("Markup shown.");
                return;
            }
            KeyCode::Up => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
                return;
            }
            KeyCode::Down => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
                return;
            }
            KeyCode::PageUp => {
                self.preview_scroll = self.preview_scroll.saturating_sub(10);
                return;
            }
            KeyCode::PageDown => {
                self.preview_scroll = self.preview_scroll.saturating_add(10);
                return;
            }
            KeyCode::Home => {
                self.preview_scroll = 0;
                return;
            }
            _ => {}
        }
        // Let chords/function keys through so ^OD, F5, F9, F10, F1 still work.
        match keymap::resolve(&mut self.chord, key) {
            Resolution::Command(crate::commands::Command::ToggleMarkup) => self.toggle_markup(),
            Resolution::Command(crate::commands::Command::TogglePreview) => self.toggle_preview(),
            Resolution::Command(crate::commands::Command::Help) => self.toggle_help(),
            Resolution::Command(crate::commands::Command::Menu) => self.open_menu(),
            Resolution::Command(crate::commands::Command::Quit) => self.request_quit(),
            Resolution::Pending(hint) => self.set_status(hint),
            _ => {}
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent, kind: OverlayKind) {
        let scroll = match kind {
            OverlayKind::Preview => &mut self.preview_scroll,
            OverlayKind::Help => &mut self.help_scroll,
        };
        match key.code {
            KeyCode::Esc | KeyCode::F(1) | KeyCode::F(5) | KeyCode::Char('q') => {
                self.mode = Mode::Editor
            }
            KeyCode::Up => *scroll = scroll.saturating_sub(1),
            KeyCode::Down => *scroll = scroll.saturating_add(1),
            KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
            KeyCode::PageDown => *scroll = scroll.saturating_add(10),
            KeyCode::Home => *scroll = 0,
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Block operations (mapped onto the text widget's selection + yank buffer)
    // ------------------------------------------------------------------

    /// `^KB` — mark the start of a block: a live selection grows from here as the
    /// cursor moves, until `^KK` fixes the block's end.
    pub fn block_begin(&mut self) {
        self.marked = None;
        self.textarea.cancel_selection();
        self.textarea.start_selection();
        self.marking = true;
        self.set_status("Block start marked — move to the end of the block and press ^KK.");
    }

    /// `^KK` — mark the end of the block. The block then stays marked where it is
    /// (and is copied to the block buffer) while the cursor moves elsewhere, so
    /// it can be copied (`^KC`) or moved (`^KV`) to the cursor, WordStar-style.
    pub fn block_end(&mut self) {
        let Some((start, end)) = self.textarea.selection_range().filter(|(s, e)| s != e) else {
            self.set_status("Mark the block start first with ^KB.");
            return;
        };
        let text = self.text_between(start, end).unwrap_or_default();
        let n = text.chars().count();
        self.set_block_buffer(text.clone());
        self.marked = Some(MarkedBlock {
            start,
            end,
            text,
            hidden: false,
        });
        self.clear_marking();
        self.set_status(format!(
            "Block marked: {n} chars.  Move the cursor, then ^KC copy · ^KV move · ^KY delete · ^KH hide"
        ));
    }

    /// `^KC` — copy. A live selection (mouse, or a block still being marked) is
    /// copied to the block buffer; a marked block is copied to the cursor.
    pub fn block_copy(&mut self) {
        if let Some(text) = self.selected_text() {
            let n = text.chars().count();
            self.set_block_buffer(text);
            self.clear_marking();
            self.set_status(format!(
                "Copied {n} chars — move the cursor and press ^KV to paste."
            ));
            return;
        }
        let Some(block) = self.active_block() else {
            return;
        };
        let at = self.cursor_pos();
        self.textarea.insert_str(&block.text);
        let end = self.cursor_pos();
        self.textarea.move_cursor(jump(at));
        self.modified = true;
        self.set_block_buffer(block.text.clone());
        self.marked = Some(MarkedBlock {
            start: at,
            end,
            ..block
        });
        self.set_status("Block copied to the cursor.");
    }

    /// `^KY` — delete. A live selection or a marked block is removed and kept in
    /// the block buffer, so `^KV` can paste it back.
    pub fn block_delete(&mut self) {
        if let Some(text) = self.selected_text() {
            let n = text.chars().count();
            self.set_block_buffer(text);
            self.edit(|t| t.cut());
            self.clear_marking();
            self.set_status(format!("Cut {n} chars — press ^KV to paste."));
            return;
        }
        let Some(block) = self.active_block() else {
            return;
        };
        self.delete_range(block.start, block.end);
        self.textarea.move_cursor(jump(block.start));
        self.set_status(format!(
            "Deleted the block ({} chars) — ^KV pastes it.",
            block.text.chars().count()
        ));
        self.set_block_buffer(block.text);
        self.marked = None;
    }

    /// `^KV` — move a marked block to the cursor; with no marked block, paste the
    /// block buffer at the cursor.
    pub fn block_move(&mut self) {
        if !self.textarea.is_selecting()
            && self.marked.as_ref().is_some_and(|b| !b.hidden)
        {
            if let Some(block) = self.active_block() {
                self.move_block_to_cursor(block);
            }
            return;
        }
        if self.block_buffer.is_empty() {
            self.set_status("Block clipboard is empty. Copy (^KC) or cut (^KY) a block first.");
            return;
        }
        self.clear_marking();
        let text = self.block_buffer.clone();
        let n = text.chars().count();
        if self.textarea.insert_str(text) {
            self.modified = true;
        }
        self.set_status(format!("Pasted {n} chars at the cursor."));
    }

    /// Move `block` to the cursor; the cursor ends at the start of the moved text.
    fn move_block_to_cursor(&mut self, block: MarkedBlock) {
        let cur = self.cursor_pos();
        if block.start <= cur && cur <= block.end {
            self.set_status("The cursor is inside the block — move it elsewhere first.");
            ring_bell();
            return;
        }
        let len = block.text.chars().count();
        let cur_off = pos_to_offset(self.textarea.lines(), cur);
        let new_off = if cur > block.end {
            // Insert first so the block's own position is still valid, then
            // delete it; the inserted copy shifts left by the block's length.
            self.textarea.insert_str(&block.text);
            self.delete_range(block.start, block.end);
            cur_off - len
        } else {
            self.delete_range(block.start, block.end);
            self.textarea.move_cursor(jump(cur));
            self.textarea.insert_str(&block.text);
            cur_off
        };
        let lines = self.textarea.lines();
        let start = offset_to_pos(lines, new_off);
        let end = offset_to_pos(lines, new_off + len);
        self.textarea.move_cursor(jump(start));
        self.modified = true;
        self.marked = Some(MarkedBlock { start, end, ..block });
        self.set_status("Block moved to the cursor.");
    }

    /// `^KH` — hide or redisplay the marked block (a live selection is dropped).
    pub fn block_hide(&mut self) {
        if self.textarea.is_selecting() || self.marking {
            self.clear_marking();
            self.set_status("Block markers cleared.");
            return;
        }
        match self.marked.as_mut() {
            Some(b) => {
                b.hidden = !b.hidden;
                let shown = !b.hidden;
                self.set_status(if shown {
                    "Block displayed."
                } else {
                    "Block hidden — ^KH shows it again."
                });
            }
            None => self.set_status("No block marked."),
        }
    }

    /// `^QB` / `^QK` — jump to the beginning or end of the marked block.
    pub fn goto_block(&mut self, end: bool) {
        match &self.marked {
            Some(b) => {
                let pos = if end { b.end } else { b.start };
                self.prev_pos = Some(self.cursor_pos());
                self.clear_marking();
                self.textarea.move_cursor(jump(pos));
            }
            None => self.set_status("No block marked. Mark one with ^KB … ^KK."),
        }
    }

    /// The marked block, if it is displayed and still where it was marked. A
    /// block whose text was edited since is dropped, with an explanation.
    fn active_block(&mut self) -> Option<MarkedBlock> {
        match self.marked.clone() {
            Some(b) if b.hidden => {
                self.set_status("The block is hidden — ^KH displays it again.");
                None
            }
            Some(b) if self.text_between(b.start, b.end).as_deref() == Some(&b.text) => Some(b),
            Some(_) => {
                self.marked = None;
                self.set_status("The text changed since the block was marked — mark it again (^KB … ^KK).");
                ring_bell();
                None
            }
            None => {
                self.set_status("No block marked. Press ^KB, move the cursor, then ^KK.");
                None
            }
        }
    }

    /// The marked block if it should be highlighted: displayed, and unchanged.
    pub fn visible_block(&self) -> Option<&MarkedBlock> {
        self.marked
            .as_ref()
            .filter(|b| !b.hidden && self.text_between(b.start, b.end).as_deref() == Some(&b.text))
    }

    /// Stop marking and drop any active selection highlight.
    fn clear_marking(&mut self) {
        self.marking = false;
        self.textarea.cancel_selection();
    }

    /// Select the visible marked block (if there is no live selection), so a
    /// formatting command can act on it. Returns whether text is selected.
    fn select_block_for_format(&mut self) -> bool {
        self.marking = false;
        if self.textarea.is_selecting() {
            return true;
        }
        let Some(b) = self.visible_block().cloned() else {
            return false;
        };
        self.marked = None;
        self.textarea.move_cursor(jump(b.start));
        self.textarea.start_selection();
        self.textarea.move_cursor(jump(b.end));
        true
    }

    /// Delete the text between two positions as one undoable edit.
    fn delete_range(&mut self, start: (usize, usize), end: (usize, usize)) {
        if start == end {
            return;
        }
        self.textarea.cancel_selection();
        self.textarea.move_cursor(jump(start));
        self.textarea.start_selection();
        self.textarea.move_cursor(jump(end));
        // With a selection active this deletes exactly it, without yanking.
        if self.textarea.delete_line_by_end() {
            self.modified = true;
        }
    }

    /// The positions that follow the text as it is edited — place markers, the
    /// previous position, the marked block — and the text they refer to, taken
    /// before handling a key (only when there is something to track).
    fn tracking_snapshot(&self) -> Option<(Anchors, Vec<String>)> {
        let anchors = self.anchors();
        let empty = anchors.markers.iter().all(Option::is_none)
            && anchors.prev.is_none()
            && anchors.block.is_none();
        (!empty).then(|| (anchors, self.textarea.lines().to_vec()))
    }

    fn anchors(&self) -> Anchors {
        Anchors {
            markers: self.markers,
            prev: self.prev_pos,
            block: self.marked.as_ref().map(|b| (b.start, b.end)),
        }
    }

    /// After a key was handled, move the tracked positions along with any edit
    /// it made — except those the command itself just set.
    fn follow_edits(&mut self, before: Option<(Anchors, Vec<String>)>) {
        let Some((was, old)) = before else {
            return;
        };
        let now = self.anchors();
        let cursor = self.cursor_pos();
        let Some(change) = crate::track::TextChange::new(&old, self.textarea.lines(), cursor)
        else {
            return;
        };
        let follow = |p: Option<(usize, usize)>, then: Option<(usize, usize)>| {
            if p == then { p.map(|p| change.map(p)) } else { p }
        };
        let markers: Vec<_> = (0..10).map(|i| follow(now.markers[i], was.markers[i])).collect();
        let prev = follow(now.prev, was.prev);
        let block = match (now.block, was.block) {
            (Some(b), Some(w)) if b == w => Some((change.map(b.0), change.map(b.1))),
            (b, _) => b,
        };
        for (slot, m) in self.markers.iter_mut().zip(markers) {
            *slot = m;
        }
        self.prev_pos = prev;
        if let Some((start, end)) = block {
            // The block keeps its ends as the text moves, and takes in any
            // editing done inside it, as WordStar's does.
            match self.text_between(start, end).filter(|_| start < end) {
                Some(text) => {
                    if let Some(b) = self.marked.as_mut() {
                        b.start = start;
                        b.end = end;
                        b.text = text;
                    }
                }
                None => self.marked = None,
            }
        }
    }

    /// The folder of the current document, if it has one.
    fn document_dir(&self) -> Option<PathBuf> {
        self.path
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf)
    }

    /// The whole text to print: the document with its `.fi` includes expanded
    /// (a master document's chapters pulled in).
    pub fn book_text(&self) -> crate::book::Expanded {
        let text = self.textarea.lines().join("\n");
        if crate::book::has_includes(self.textarea.lines()) {
            crate::book::expand(&text, self.document_dir().as_deref())
        } else {
            crate::book::Expanded {
                text,
                ..Default::default()
            }
        }
    }

    /// Insert ▸ Include File: add a `.fi` line pulling a chapter file into the
    /// printout.
    pub fn start_include_file(&mut self) {
        self.open_prompt(PromptKind::IncludeFile, "Include file (.fi):", String::new());
    }

    /// The spelling dictionary for this document (its frontmatter `language:`,
    /// US English by default), loaded the first time it's needed.
    pub fn speller(&self) -> Option<&crate::spell::Speller> {
        self.speller
            .get_or_init(|| {
                let language = crate::attributes::frontmatter_value(self.textarea.lines(), "language");
                crate::spell::Speller::load(language.as_deref())
            })
            .as_ref()
            .ok()
    }

    fn speller_mut(&mut self) -> Option<&mut crate::spell::Speller> {
        self.speller();
        self.speller.get_mut()?.as_mut().ok()
    }

    /// Why spelling is unavailable, if it is.
    fn speller_error(&self) -> Option<String> {
        self.speller();
        self.speller.get()?.as_ref().err().cloned()
    }

    /// View ▸ Spelling Highlights: underline unknown words as you write.
    pub fn toggle_spell_highlight(&mut self) {
        self.spell_highlight = !self.spell_highlight;
        self.set_status(if self.spell_highlight {
            "Spelling highlights on."
        } else {
            "Spelling highlights off."
        });
    }

    /// `^QL` — check the spelling from the cursor to the end of the document,
    /// stopping at each unknown word.
    pub fn start_spell_check(&mut self) {
        if let Some(e) = self.speller_error() {
            return self.set_status(e);
        }
        self.remember_position();
        self.clear_marking();
        let from = self.cursor_pos();
        self.advance_spell(from, 0);
    }

    /// `^QN` — check the spelling of the word at the cursor.
    pub fn spell_check_word(&mut self) {
        if let Some(e) = self.speller_error() {
            return self.set_status(e);
        }
        let (row, col) = self.cursor_pos();
        let line = &self.textarea.lines()[row];
        let Some((s, e)) = crate::spell::word_at(line, col) else {
            return self.set_status("No word at the cursor.");
        };
        let word: String = line.chars().skip(s).take(e - s).collect();
        if self.speller().is_some_and(|sp| sp.is_correct(&word)) {
            return self.set_status(format!("\u{201C}{word}\u{201D} is spelled correctly."));
        }
        self.clear_marking();
        self.ask_about_word(word, (row, s), (row, e), 0, true);
    }

    /// The next word the dictionary doesn't know, from `from` on.
    fn next_misspelling(&self, from: (usize, usize)) -> Option<Misspelling> {
        let speller = self.speller()?;
        let lines = self.textarea.lines();
        let prose = crate::spell::prose_lines(lines);
        for (row, line) in lines.iter().enumerate().skip(from.0) {
            if !prose[row] {
                continue;
            }
            let chars: Vec<char> = line.chars().collect();
            for (s, e) in crate::spell::words(line) {
                if row == from.0 && s < from.1 {
                    continue;
                }
                let word: String = chars[s..e].iter().collect();
                if !speller.is_correct(&word) {
                    return Some(((row, s), (row, e), word));
                }
            }
        }
        None
    }

    /// Move on to the next unknown word, or finish the check.
    fn advance_spell(&mut self, from: (usize, usize), corrected: usize) {
        match self.next_misspelling(from) {
            Some((start, end, word)) => self.ask_about_word(word, start, end, corrected, false),
            None => {
                self.spell = None;
                self.mode = Mode::Editor;
                self.textarea.cancel_selection();
                self.set_status(match corrected {
                    0 => "Spelling check complete.".to_string(),
                    n => format!("Spelling check complete — {n} correction(s)."),
                });
            }
        }
    }

    /// Highlight an unknown word and ask what to do with it.
    fn ask_about_word(&mut self, word: String, start: (usize, usize), end: (usize, usize), corrected: usize, single: bool) {
        self.textarea.cancel_selection();
        self.textarea.move_cursor(jump(start));
        self.textarea.start_selection();
        self.textarea.move_cursor(jump(end));
        let suggestions = self.speller().map(|s| s.suggestions(&word)).unwrap_or_default();
        self.spell = Some(SpellSession {
            word,
            start,
            end,
            suggestions,
            corrected,
            single,
        });
        self.mode = Mode::Spell;
        self.status_msg = None;
    }

    /// Replace the unknown word with `with` and carry on.
    fn spell_replace(&mut self, session: &SpellSession, with: &str) {
        self.textarea.cancel_selection();
        self.textarea.move_cursor(jump(session.start));
        self.textarea.start_selection();
        self.textarea.move_cursor(jump(session.end));
        self.textarea.insert_str(with);
        self.modified = true;
        let after = self.cursor_pos();
        self.spell_continue(session, after, session.corrected + 1);
    }

    fn spell_continue(&mut self, session: &SpellSession, from: (usize, usize), corrected: usize) {
        if session.single {
            self.spell = None;
            self.mode = Mode::Editor;
            self.textarea.cancel_selection();
        } else {
            self.advance_spell(from, corrected);
        }
    }

    /// Keys while the spelling check asks about a word: a suggestion's number
    /// replaces it; I ignores it, G ignores it everywhere, A adds it to your
    /// dictionary, T types a replacement, Esc stops.
    fn handle_spell_key(&mut self, key: KeyEvent) {
        let Some(session) = self.spell.clone() else {
            self.mode = Mode::Editor;
            return;
        };
        match key.code {
            KeyCode::Char(d @ '1'..='9') => {
                if let Some(with) = session.suggestions.get(d as usize - '1' as usize) {
                    self.spell_replace(&session, &with.clone());
                }
            }
            KeyCode::Char('i' | 'I') => self.spell_continue(&session, session.end, session.corrected),
            KeyCode::Char('g' | 'G') => {
                if let Some(s) = self.speller_mut() {
                    s.ignore_all(&session.word);
                }
                self.spell_continue(&session, session.end, session.corrected);
            }
            KeyCode::Char('a' | 'A') => {
                if let Some(s) = self.speller_mut() {
                    s.add_to_personal(&session.word);
                }
                self.spell_continue(&session, session.end, session.corrected);
                if self.mode == Mode::Editor {
                    self.set_status(format!("Added \u{201C}{}\u{201D} to your dictionary.", session.word));
                }
            }
            KeyCode::Char('t' | 'T') => {
                self.open_prompt(PromptKind::SpellReplace, "Replace with:", session.word.clone());
                self.prompt.fresh = false; // edit the word rather than retype it
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                self.spell = None;
                self.mode = Mode::Editor;
                self.textarea.cancel_selection();
                self.set_status("Spelling check stopped.");
            }
            _ => {}
        }
    }

    /// The document's headings (`#` … `######` outside code blocks and the
    /// frontmatter; a lone `#` is a scene break, not a heading), with the page
    /// each falls on.
    pub fn headings(&self) -> Vec<OutlineItem> {
        let lines = self.textarea.lines();
        let rows = self.visual_rows.borrow();
        let pages = self.row_pages.borrow();
        let mut body_start = 0;
        if lines.first().map(|l| l.trim()) == Some("---")
            && let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---")
        {
            body_start = end + 2;
        }
        let mut in_fence = false;
        let mut out = Vec::new();
        let base = self.document_dir();
        for (row, line) in lines.iter().enumerate().skip(body_start) {
            let t = line.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                in_fence = !in_fence;
                continue;
            }
            if let Some(name) = crate::book::include_target(line) {
                // A chapter file of a master document.
                out.push(OutlineItem {
                    row,
                    level: 1,
                    title: format!("\u{25B8} {name}"),
                    page: 0,
                    file: Some(crate::book::resolve(name, base.as_deref())),
                });
                continue;
            }
            let level = t.chars().take_while(|&c| c == '#').count();
            if in_fence || !(1..=6).contains(&level) || !t[level..].starts_with(' ') {
                continue;
            }
            let title = crate::attributes::strip_inline_markers(t[level..].trim());
            let title = title.trim_end_matches('#').trim().to_string();
            if title.is_empty() {
                continue;
            }
            let first_row = rows.partition_point(|r| r.line < row);
            let page = pages.get(first_row).map_or(row / LINES_PER_PAGE + 1, |p| p.0);
            out.push(OutlineItem {
                row,
                level,
                title,
                page,
                file: None,
            });
        }
        out
    }

    /// Go to Heading (`^QG`): list the chapters and headings to jump to, with
    /// the one the cursor is in selected.
    pub fn open_outline(&mut self) {
        let items = self.headings();
        if items.is_empty() {
            self.set_status("No headings yet — a line starting with \"# \" is a chapter title.");
            return;
        }
        let row = self.cursor_pos().0;
        let selected = items.iter().rposition(|h| h.row <= row).unwrap_or(0);
        self.outline = Some(OutlineState { items, selected });
        self.mode = Mode::Outline;
    }

    fn handle_outline_key(&mut self, key: KeyEvent) {
        let Some(o) = self.outline.as_mut() else {
            self.mode = Mode::Editor;
            return;
        };
        match list_key(&mut o.selected, o.items.len(), &key) {
            ListAction::Stay => {}
            ListAction::Choose => self.outline_jump(),
            ListAction::Close => self.close_lists(),
        }
    }

    /// Close whichever pick-list is open.
    fn close_lists(&mut self) {
        self.outline = None;
        self.recent = None;
        self.mode = Mode::Editor;
    }

    /// File ▸ Recent Files: the documents edited lately, to reopen where you
    /// left off.
    pub fn open_recent(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let current = self.path.as_ref().and_then(|p| std::path::absolute(p).ok());
            let items: Vec<_> = crate::platform::recent_documents()
                .into_iter()
                .filter(|(p, _)| Some(p) != current.as_ref() && p.is_file())
                .collect();
            if items.is_empty() {
                return self.set_status("No recent files yet.");
            }
            self.recent = Some(RecentState { items, selected: 0 });
            self.mode = Mode::Recent;
        }
        #[cfg(target_arch = "wasm32")]
        self.set_status("Recent files aren't available in the browser — use Open (F3).");
    }

    fn handle_recent_key(&mut self, key: KeyEvent) {
        let Some(r) = self.recent.as_mut() else {
            self.mode = Mode::Editor;
            return;
        };
        match list_key(&mut r.selected, r.items.len(), &key) {
            ListAction::Stay => {}
            ListAction::Choose => self.recent_open(),
            ListAction::Close => self.close_lists(),
        }
    }

    /// Open the selected recent document (asking about unsaved changes first).
    fn recent_open(&mut self) {
        let chosen = self
            .recent
            .take()
            .and_then(|r| r.items.get(r.selected).map(|(p, _)| p.clone()));
        self.mode = Mode::Editor;
        if let Some(path) = chosen {
            self.guard_unsaved(AfterSave::Open(path));
        }
    }

    /// Record where the cursor is in the current document, for Recent Files and
    /// for reopening it at the same place.
    #[cfg(not(target_arch = "wasm32"))]
    fn note_position(&self) {
        if let Some(path) = &self.path {
            crate::platform::remember_document(path, self.cursor_pos());
        }
    }

    /// After opening a document, go back to where it was left last time.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn restore_last_position(&mut self) {
        let Some(path) = self.path.as_ref().and_then(|p| std::path::absolute(p).ok()) else {
            return;
        };
        let last = crate::platform::recent_documents()
            .into_iter()
            .find(|(p, _)| *p == path)
            .map(|(_, pos)| pos);
        if let Some(pos) = last.filter(|&p| p != (0, 0)) {
            self.textarea.move_cursor(jump(pos));
            let message = match self.status_msg.take() {
                Some(opened) => format!("{opened} — back where you left off."),
                None => "Back where you left off.".to_string(),
            };
            self.set_status(message);
        }
        self.note_position();
    }

    /// Jump to the selected heading and close the list.
    fn outline_jump(&mut self) {
        let Some(o) = self.outline.take() else {
            return;
        };
        self.mode = Mode::Editor;
        let Some(item) = o.items.get(o.selected) else {
            return;
        };
        if let Some(file) = item.file.clone() {
            return self.guard_unsaved(AfterSave::Open(file));
        }
        self.remember_position();
        self.clear_marking();
        self.textarea.move_cursor(jump((item.row, 0)));
        self.set_status(format!("{} — page {}.  ^QP goes back.", item.title, item.page));
    }

    /// `^K0`…`^K9` — set place marker `n` at the cursor (again, on the same
    /// spot, removes it).
    pub fn set_marker(&mut self, n: usize) {
        let here = self.cursor_pos();
        if self.markers[n] == Some(here) {
            self.markers[n] = None;
            self.set_status(format!("Marker {n} removed."));
        } else {
            self.markers[n] = Some(here);
            self.set_status(format!("Marker {n} set — ^Q{n} returns here."));
        }
    }

    /// Ask which place marker to set or go to (Edit menu).
    pub fn start_marker_prompt(&mut self, set: bool) {
        let (kind, label) = if set {
            (PromptKind::SetMarker, "Set marker (0-9):")
        } else {
            (PromptKind::GotoMarker, "Go to marker (0-9):")
        };
        self.open_prompt(kind, label, String::new());
    }

    /// `^Q0`…`^Q9` — jump to place marker `n`.
    pub fn goto_marker(&mut self, n: usize) {
        match self.markers[n] {
            Some(pos) => {
                self.remember_position();
                self.clear_marking();
                self.textarea.move_cursor(jump(pos));
            }
            None => self.set_status(format!("Marker {n} is not set. Set it with ^K{n}.")),
        }
    }

    /// Note the cursor position before a long jump, for `^QP`.
    pub fn remember_position(&mut self) {
        self.prev_pos = Some(self.cursor_pos());
    }

    /// `^QP` — return to where the cursor was before the last long jump (find,
    /// go to page, start/end of document, block ends…). Pressing it again
    /// swaps back.
    pub fn goto_previous_position(&mut self) {
        match self.prev_pos {
            Some(pos) => {
                let here = self.cursor_pos();
                self.clear_marking();
                self.textarea.move_cursor(jump(pos));
                self.prev_pos = Some(here);
            }
            None => self.set_status("No previous position yet."),
        }
    }

    /// The cursor as a plain `(row, col)`.
    fn cursor_pos(&self) -> (usize, usize) {
        let c = self.textarea.cursor();
        (c.0, c.1)
    }

    /// Put `text` in the block buffer (what `^KV` pastes) and on the system
    /// clipboard, so it can be pasted into other programs too.
    fn set_block_buffer(&mut self, text: String) {
        crate::platform::copy_to_clipboard(&text);
        self.block_buffer = text;
    }

    /// The text of the live selection, or else of the displayed marked block.
    fn block_text(&self) -> Option<String> {
        self.selected_text()
            .or_else(|| self.visible_block().map(|b| b.text.clone()))
    }

    /// `^KW` — write the marked block to a file (e.g. to keep a cut scene).
    pub fn start_write_block(&mut self) {
        if self.block_text().is_none() {
            return self.set_status("Mark a block first — ^KB, move, ^KK.");
        }
        self.open_prompt(PromptKind::WriteBlock, "Write block to file:", String::new());
    }

    fn write_block(&mut self, path: &Path, text: &str) {
        let mut content = text.to_string();
        if !content.ends_with('\n') {
            content.push('\n');
        }
        #[cfg(not(target_arch = "wasm32"))]
        match crate::platform::write_file_safely(path, content.as_bytes()) {
            Ok(_) => self.set_status(format!("Block written to {}.", path.display())),
            Err(e) => self.set_status(format!("Could not write {}: {e}", path.display())),
        }
        #[cfg(target_arch = "wasm32")]
        {
            let name = file_download_name(path, "md");
            match crate::platform::download(&name, "text/markdown", content.as_bytes()) {
                Ok(()) => self.set_status(format!("Downloaded {name}")),
                Err(e) => self.set_status(e),
            }
        }
    }

    /// `^K"` / `^K'` / `^K.` — change the case of the marked block (or the
    /// selection) to UPPER, lower or Sentence case.
    pub fn change_case(&mut self, case: Case) {
        let selection = self.textarea.selection_range().filter(|(s, e)| s != e);
        let (start, end, from_block) = match (selection, self.visible_block()) {
            (Some((s, e)), _) => (s, e, false),
            (None, Some(b)) => (b.start, b.end, true),
            (None, None) => return self.set_status("Mark a block first — ^KB, move, ^KK."),
        };
        let Some(text) = self.text_between(start, end) else {
            return;
        };
        let changed = match case {
            Case::Upper => text.to_uppercase(),
            Case::Lower => text.to_lowercase(),
            Case::Sentence => sentence_case(&text),
        };
        self.marking = false;
        if changed != text {
            self.textarea.cancel_selection();
            self.textarea.move_cursor(jump(start));
            self.textarea.start_selection();
            self.textarea.move_cursor(jump(end));
            self.textarea.insert_str(&changed);
            self.modified = true;
        }
        let end = self.cursor_pos();
        self.textarea.cancel_selection();
        if from_block {
            // The block still covers the (possibly longer: ß → SS) text.
            self.marked = Some(MarkedBlock {
                start,
                end,
                text: changed,
                hidden: false,
            });
        }
        self.set_status(match case {
            Case::Upper => "Block in UPPER CASE.",
            Case::Lower => "Block in lower case.",
            Case::Sentence => "Block in Sentence case.",
        });
    }

    /// The currently selected text, if any (used for block copy/cut).
    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.textarea.selection_range()?;
        if start == end {
            return None; // empty selection
        }
        self.text_between(start, end)
    }

    /// The text from `start` to `end` (character positions, `start <= end`), or
    /// `None` if either lies outside the document.
    fn text_between(&self, (sr, sc): (usize, usize), (er, ec): (usize, usize)) -> Option<String> {
        let lines = self.textarea.lines();
        let len = |r: usize| lines.get(r).map(|l| l.chars().count());
        if sc > len(sr)? || ec > len(er)? || (sr, sc) > (er, ec) {
            return None;
        }
        if sr == er {
            return Some(lines[sr].chars().skip(sc).take(ec - sc).collect());
        }
        let mut out: String = lines[sr].chars().skip(sc).collect();
        out.push('\n');
        for line in &lines[sr + 1..er] {
            out.push_str(line);
            out.push('\n');
        }
        out.extend(lines[er].chars().take(ec));
        Some(out)
    }

    /// `^Y` — delete the whole line the cursor is on, its text and its line
    /// break, as one undoable edit; the cursor lands at the start of what was
    /// the next line.
    pub fn delete_line(&mut self) {
        let row = self.cursor_pos().0;
        let lines = self.textarea.lines();
        let end = if row + 1 < lines.len() {
            (row + 1, 0)
        } else {
            (row, lines[row].chars().count())
        };
        self.clear_marking();
        self.delete_range((row, 0), end);
        self.textarea.move_cursor(jump((row, 0)));
    }

    /// Run an editing closure, marking the buffer modified if it changed.
    pub fn edit<F: FnOnce(&mut TextArea<'static>) -> bool>(&mut self, f: F) {
        if f(&mut self.textarea) {
            self.modified = true;
        }
    }

    /// Toggle insert / overtype.
    pub fn toggle_insert(&mut self) {
        self.insert_mode = !self.insert_mode;
    }

    // ------------------------------------------------------------------
    // Inline formatting & paragraph alignment
    // ------------------------------------------------------------------

    /// Wrap the current selection (or, with none, the cursor) in `open`/`close`
    /// markdown markers. With no selection the cursor is left between the markers
    /// so the next typed text is formatted.
    pub fn apply_format(&mut self, open: &str, close: &str, label: &str) {
        if self.select_block_for_format() {
            self.textarea.cut();
            let inner = self.textarea.yank_text();
            // Markdown emphasis can't start or end with whitespace (`**word **`
            // stays literal), so keep surrounding spaces outside the markers.
            let core = inner.trim();
            let wrapped = if core.is_empty() {
                format!("{open}{inner}{close}")
            } else {
                let lead = &inner[..inner.len() - inner.trim_start().len()];
                let trail = &inner[inner.trim_end().len()..];
                format!("{lead}{open}{core}{close}{trail}")
            };
            self.textarea.insert_str(wrapped);
            self.set_status(format!("{label} applied to selection."));
        } else {
            self.textarea.insert_str(format!("{open}{close}"));
            for _ in 0..close.chars().count() {
                self.textarea.move_cursor(CursorMove::Back);
            }
            self.set_status(format!("{label} on — type, then move past the marker."));
        }
        self.modified = true;
    }

    /// Strip inline formatting markers from the selected text.
    pub fn clear_formatting(&mut self) {
        if self.select_block_for_format() {
            self.textarea.cut();
            let inner = self.textarea.yank_text();
            let cleaned = crate::attributes::strip_inline_markers(&inner);
            self.textarea.insert_str(cleaned);
            self.modified = true;
            self.set_status("Formatting cleared from selection.");
        } else {
            self.set_status("Select text first, then clear formatting.");
        }
    }

    /// Set the paragraph alignment (also updates the widget where it can).
    pub fn set_align(&mut self, choice: AlignChoice) {
        self.align = choice;
        let (a, label) = match choice {
            AlignChoice::Left => (Alignment::Left, "Left"),
            AlignChoice::Center => (Alignment::Center, "Centered"),
            AlignChoice::Right => (Alignment::Right, "Right"),
            // The widget has no justify; render as left but remember the choice.
            AlignChoice::Justify => (Alignment::Left, "Justified"),
        };
        self.textarea.set_alignment(a);
        self.set_status(format!("Alignment: {label}"));
    }

    /// `^N` — insert a hard return at the cursor, leaving the cursor in place
    /// (opens a new line below the current text position).
    pub fn insert_line(&mut self) {
        let at = self.cursor_pos();
        self.textarea.insert_newline();
        self.textarea.move_cursor(jump(at));
        self.modified = true;
    }

    // ------------------------------------------------------------------
    // Insert / utility commands
    // ------------------------------------------------------------------

    /// Open the "insert file" prompt (^KR).
    pub fn start_insert_file(&mut self) {
        self.open_prompt(PromptKind::InsertFile, "Insert file:", String::new());
    }

    /// Read `path` and insert its text at the cursor. WordStar files are decoded
    /// to Markdown just like when opening them.
    fn insert_file(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            self.set_status("Insert cancelled (no name).");
            return;
        }
        // Inserting by path needs the filesystem; the browser build has none.
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            self.set_status("Insert file is not available in the browser build.");
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        match crate::wordstar::load(Path::new(path)) {
            Ok(loaded) => {
                if self.textarea.insert_str(&loaded.text) {
                    self.modified = true;
                }
                self.set_status(format!("Inserted {path}."));
            }
            Err(e) => {
                self.set_status(format!("Cannot insert {path}: {e}"));
                ring_bell();
            }
        }
    }

    /// Insert a WordStar dot command on its own line at the cursor.
    pub fn insert_dot_command(&mut self, dot: &str, status: &str) {
        // Dot commands must sit at column 1, so start a fresh line if needed.
        if self.textarea.cursor().1 != 0 {
            self.textarea.insert_newline();
        }
        self.textarea.insert_str(dot);
        self.textarea.insert_newline();
        self.modified = true;
        self.set_status(status.to_string());
    }

    /// Open the page-jump prompt.
    pub fn start_goto_page(&mut self) {
        self.open_prompt(PromptKind::GoToPage, "Go to page:", String::new());
    }

    /// Jump the cursor to the first printed line of the given 1-based page, using
    /// the same pagination as the status line (wrapped rows, `.pa` breaks).
    fn goto_page(&mut self, page: usize) {
        let page = page.max(1);
        self.remember_position();
        let target = {
            let rows = self.visual_rows.borrow();
            let pages = self.row_pages.borrow();
            let lines = self.textarea.lines();
            let idx = pages
                .iter()
                .position(|&(p, _)| p >= page)
                .or_else(|| pages.len().checked_sub(1));
            idx.and_then(|i| rows.get(i))
                .map(|r| (r.line, r.start_col(lines)))
        };
        match target {
            Some(pos) => self.textarea.move_cursor(jump(pos)),
            // No layout yet (nothing rendered): fall back to logical lines.
            None => self
                .textarea
                .move_cursor(jump(((page - 1) * LINES_PER_PAGE, 0))),
        }
        let reached = self.cursor_metrics().page;
        if reached < page {
            self.set_status(format!("The document ends on page {reached}."));
        } else {
            self.set_status(format!("Page {page}."));
        }
    }

    /// Open the right-margin prompt (`^OR`), pre-filled with the current margin.
    pub fn start_right_margin(&mut self) {
        let margin = self.right_margin().to_string();
        self.open_prompt(PromptKind::RightMargin, "Right margin (column):", margin);
    }

    /// The right margin — the column text wraps at: the document's first `.rm N`
    /// dot command, or WordStar's default of 65.
    pub fn right_margin(&self) -> usize {
        self.textarea
            .lines()
            .iter()
            .find_map(|l| right_margin_command(l))
            .unwrap_or(DEFAULT_RIGHT_MARGIN)
    }

    /// Store a new right margin as a `.rm` dot command: rewrite the existing one,
    /// or add it at the top of the document. The cursor stays on its text.
    fn set_right_margin(&mut self, margin: usize) {
        let cursor = self.textarea.cursor();
        let existing = self
            .textarea
            .lines()
            .iter()
            .position(|l| right_margin_command(l).is_some());
        let dot = format!(".rm {margin}");
        self.clear_marking();
        match existing {
            Some(row) => {
                let len = self.textarea.lines()[row].chars().count();
                self.textarea.move_cursor(jump((row, 0)));
                self.textarea.start_selection();
                self.textarea.move_cursor(jump((row, len)));
                self.textarea.insert_str(&dot);
                self.textarea.move_cursor(jump((cursor.0, cursor.1)));
            }
            None => {
                self.textarea.move_cursor(CursorMove::Top);
                self.textarea.move_cursor(CursorMove::Head);
                self.textarea.insert_str(&dot);
                self.textarea.insert_newline();
                self.textarea.move_cursor(jump((cursor.0 + 1, cursor.1)));
            }
        }
        self.modified = true;
        self.set_status(format!("Right margin: column {margin}."));
    }

    /// Open the header / footer dialog.
    pub fn start_header(&mut self, kind: HeaderKind) {
        self.mode = Mode::Header;
        self.header_dialog = Some(HeaderState {
            kind,
            text: String::new(),
            pages: HeaderPages::Both,
        });
    }

    /// Open the calculator dialog (Utilities ▸ Calculator, `^QM`).
    pub fn start_calculator(&mut self) {
        self.mode = Mode::Calculator;
        self.calc = Some(CalcState {
            input: String::new(),
            result: "0".into(),
        });
    }

    fn handle_calc_key(&mut self, key: KeyEvent) {
        let Some(calc) = self.calc.as_mut() else {
            self.mode = Mode::Editor;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.calc = None;
                self.mode = Mode::Editor;
                self.set_status("Calculator closed.");
            }
            // Enter / OK: evaluate the expression into "Result of Last Calculation".
            KeyCode::Enter => {
                let expr = calc.input.trim();
                if expr.is_empty() {
                    return;
                }
                match crate::calc::eval(expr) {
                    Ok(value) => {
                        calc.result = crate::calc::format_result(value);
                        calc.input.clear();
                    }
                    // Keep the expression on error so it can be corrected.
                    Err(e) => calc.result = format!("Error: {e}"),
                }
            }
            KeyCode::Backspace => {
                calc.input.pop();
            }
            KeyCode::Char(c) => calc.input.push(c),
            _ => {}
        }
    }

    fn handle_header_key(&mut self, key: KeyEvent) {
        let Some(h) = self.header_dialog.as_mut() else {
            self.mode = Mode::Editor;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.header_dialog = None;
                self.mode = Mode::Editor;
                self.set_status("Cancelled.");
            }
            KeyCode::Enter => self.commit_header(),
            KeyCode::Up => {
                h.pages = match h.pages {
                    HeaderPages::Both => HeaderPages::Even,
                    HeaderPages::Odd => HeaderPages::Both,
                    HeaderPages::Even => HeaderPages::Odd,
                };
            }
            KeyCode::Down | KeyCode::Tab => {
                h.pages = match h.pages {
                    HeaderPages::Both => HeaderPages::Odd,
                    HeaderPages::Odd => HeaderPages::Even,
                    HeaderPages::Even => HeaderPages::Both,
                };
            }
            KeyCode::Backspace => {
                h.text.pop();
            }
            KeyCode::Char(c) => h.text.push(c),
            _ => {}
        }
    }

    /// Insert the configured header/footer as a WordStar dot command at the top
    /// of the document.
    fn commit_header(&mut self) {
        let Some(h) = self.header_dialog.take() else {
            return;
        };
        self.mode = Mode::Editor;
        let text = h.text.trim();
        if text.is_empty() {
            self.set_status("Header cancelled (no text).");
            return;
        }
        // `.he`/`.fo` = both pages, `.oh`/`.of` = odd, `.eh`/`.ef` = even.
        let dot = match (h.kind, h.pages) {
            (HeaderKind::Header, HeaderPages::Both) => ".he",
            (HeaderKind::Header, HeaderPages::Odd) => ".oh",
            (HeaderKind::Header, HeaderPages::Even) => ".eh",
            (HeaderKind::Footer, HeaderPages::Both) => ".fo",
            (HeaderKind::Footer, HeaderPages::Odd) => ".of",
            (HeaderKind::Footer, HeaderPages::Even) => ".ef",
        };
        let line = format!("{dot} {text}");
        self.textarea.move_cursor(CursorMove::Top);
        self.textarea.move_cursor(CursorMove::Head);
        self.textarea.insert_str(&line);
        self.textarea.insert_newline();
        self.modified = true;
        let what = match h.kind {
            HeaderKind::Header => "Header",
            HeaderKind::Footer => "Footer",
        };
        self.set_status(format!("{what} set: {line}"));
    }

    /// Show document statistics in an info modal (^K?). Only the text a reader
    /// sees counts: not the frontmatter, dot commands or Markdown markup. With a
    /// block marked (or text selected), its count is shown too, along with this
    /// session's progress and the document's `goal:`, if it sets one.
    pub fn show_word_count(&mut self) {
        use crate::attributes::{count_words, group_digits};
        let block = self
            .selected_text()
            .or_else(|| self.visible_block().map(|b| b.text.clone()));
        let lines = self.textarea.lines();
        let stats = count_words(lines);
        let mut out = vec![
            format!("Words:        {}", group_digits(stats.words)),
            format!("Characters:   {}", group_digits(stats.chars)),
            format!("Lines:        {}", group_digits(lines.len())),
            format!("Paragraphs:   {}", group_digits(stats.paragraphs)),
        ];
        if crate::book::has_includes(lines) {
            let book = self.book_text();
            let book_lines: Vec<String> = book.text.lines().map(str::to_owned).collect();
            let total = count_words(&book_lines).words;
            out.push(format!(
                "Whole book:   {} words in {} included file(s)",
                group_digits(total),
                book.files
            ));
            if !book.missing.is_empty() {
                out.push(format!("Missing:      {}", book.missing.join(", ")));
            }
        }
        let lines = self.textarea.lines();
        let delta = stats.words as isize - self.session_start_words as isize;
        out.push(format!("This session: {}{}", if delta < 0 { "-" } else { "+" }, group_digits(delta.unsigned_abs())));
        if let Some(goal) = crate::attributes::word_goal(lines) {
            let pct = stats.words * 100 / goal;
            let left = goal.saturating_sub(stats.words);
            out.push(if left > 0 {
                format!("Goal:         {} ({pct}%, {} to go)", group_digits(goal), group_digits(left))
            } else {
                format!("Goal:         {} — reached! ({pct}%)", group_digits(goal))
            });
        }
        if let Some(text) = block {
            let block_lines: Vec<String> = text.lines().map(str::to_owned).collect();
            let b = count_words(&block_lines);
            out.push(String::new());
            out.push(format!("Block:        {} words, {} characters", group_digits(b.words), group_digits(b.chars)));
        }
        self.info = Some(InfoState {
            title: "Word Count".into(),
            lines: out,
        });
        self.mode = Mode::Info;
    }

    /// The document's word count, as `^K?` counts it — for a master document,
    /// the whole book's; cached between frames while the text is unchanged.
    pub fn word_count(&self) -> usize {
        let hash = self.content_hash();
        if let Some((h, n)) = *self.word_count_cache.borrow()
            && h == hash
        {
            return n;
        }
        let n = if crate::book::has_includes(self.textarea.lines()) {
            let book: Vec<String> = self.book_text().text.lines().map(str::to_owned).collect();
            crate::attributes::count_words(&book).words
        } else {
            crate::attributes::count_words(self.textarea.lines()).words
        };
        *self.word_count_cache.borrow_mut() = Some((hash, n));
        n
    }

    /// Insert ▸ Manuscript Setup: add the frontmatter that turns on standard
    /// manuscript format for PDF export, with placeholders to fill in. Keys the
    /// document already sets are left alone.
    pub fn insert_manuscript_template(&mut self) {
        const TEMPLATE: &[&str] = &[
            "format: manuscript",
            "title: Untitled",
            "author: Your Legal Name",
            "byline: Your Pen Name",
            "contact:",
            "  - Street Address",
            "  - City, State ZIP",
            "  - you@example.com",
            "paper: letter",
            "paragraphs: lines",
        ];
        let lines = self.textarea.lines();
        let has_frontmatter = lines.first().map(|l| l.trim()) == Some("---")
            && lines.iter().skip(1).any(|l| l.trim() == "---");
        if crate::attributes::render_options(&lines.join("\n"))
            .manuscript
            .is_some()
        {
            self.textarea.move_cursor(CursorMove::Top);
            self.set_status("Manuscript format is already set up — see the lines at the top.");
            return;
        }
        let existing: Vec<String> = if has_frontmatter {
            lines
                .iter()
                .skip(1)
                .take_while(|l| l.trim() != "---")
                .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().to_ascii_lowercase()))
                .collect()
        } else {
            Vec::new()
        };
        // Skip keys (and a skipped key's list items) the document already has.
        let mut block = Vec::new();
        let mut skipping = false;
        for line in TEMPLATE {
            if let Some((key, _)) = line.split_once(':') {
                skipping = existing.iter().any(|k| k == key);
            }
            if !skipping {
                block.push(*line);
            }
        }
        self.clear_marking();
        self.textarea.move_cursor(CursorMove::Top);
        self.textarea.move_cursor(CursorMove::Head);
        let text = if has_frontmatter {
            // Insert after the opening `---`.
            self.textarea.move_cursor(CursorMove::Down);
            format!("{}\n", block.join("\n"))
        } else {
            format!("---\n{}\n---\n\n", block.join("\n"))
        };
        self.textarea.insert_str(text);
        self.modified = true;
        // Put the cursor on the title, the first thing to fill in.
        if let Some(row) = self.textarea.lines().iter().position(|l| l.starts_with("title: ")) {
            let len = self.textarea.lines()[row].chars().count();
            self.textarea.move_cursor(jump((row, len)));
        }
        self.set_status("Manuscript format on: fill in your details, then export with ^KP.");
    }

    fn handle_info_key(&mut self, _key: KeyEvent) {
        // Any key dismisses the modal.
        self.info = None;
        self.mode = Mode::Editor;
    }

    /// Report that a menu feature is not implemented yet.
    pub fn not_implemented(&mut self, feature: &str) {
        self.set_status(format!("{feature} is not implemented yet."));
        ring_bell();
    }

    /// Start a new, empty document (no path).
    pub fn new_document(&mut self) {
        self.textarea = TextArea::default();
        self.apply_editor_theme();
        self.textarea.set_alignment(Alignment::Left);
        self.path = None;
        self.modified = false;
        self.align = AlignChoice::Left;
        self.set_status("New document.");
    }

    /// Attributes active at the cursor (drives the style bar B/I/U + font/size).
    pub fn attributes_at_cursor(&self) -> RunAttributes {
        let cursor = self.textarea.cursor();
        let line = self
            .textarea
            .lines()
            .get(cursor.0)
            .cloned()
            .unwrap_or_default();
        let attrs = crate::attributes::line_attributes(&line);
        attrs.get(cursor.1).cloned().unwrap_or_default()
    }

    /// Document-default font and size (from YAML frontmatter, with fallbacks).
    pub fn document_defaults(&self) -> (String, u32) {
        let (font, size) = crate::attributes::document_defaults(self.textarea.lines());
        (font.unwrap_or_else(|| "Default".into()), size.unwrap_or(12))
    }

    /// Save the buffer to its path as markdown, returning whether it was written.
    /// An untitled document opens the Save As prompt instead.
    pub fn save(&mut self) -> bool {
        match self.path.clone() {
            Some(path) => self.write_document(&path),
            None => {
                self.start_save_as();
                false
            }
        }
    }

    /// Save, then carry out `after` — once Save As has named the document, if it
    /// is untitled. Nothing happens after a failed save, so no work is lost.
    pub fn save_then(&mut self, after: AfterSave) {
        if self.path.is_none() {
            self.after_save = Some(after);
            self.start_save_as();
        } else if self.save() {
            self.run_after(after);
        }
    }

    /// Write the document to `path` under that name (Save As), then finish any
    /// action that was waiting for the save.
    fn save_as(&mut self, path: PathBuf) {
        if self.write_document(&path) {
            self.path = Some(path);
            if let Some(after) = self.after_save.take() {
                self.run_after(after);
            }
        } else {
            self.after_save = None;
        }
    }

    /// Write the buffer to `path`, reporting the outcome on the status line.
    fn write_document(&mut self, path: &Path) -> bool {
        let mut content = self.textarea.lines().join("\n");
        content.push('\n');
        #[cfg(not(target_arch = "wasm32"))]
        match crate::platform::write_file_safely(path, content.as_bytes()) {
            Ok(backup_error) => {
                crate::platform::remember_document(path, self.cursor_pos());
                crate::platform::clear_recovery(self.path.as_deref());
                crate::platform::clear_recovery(Some(path));
                self.autosave_hash = None;
                self.modified = false;
                match backup_error {
                    None => self.set_status(format!("Saved {}", path.display())),
                    Some(e) => self.set_status(format!(
                        "Saved {} — but the .bak backup could not be written: {e}",
                        path.display()
                    )),
                }
                true
            }
            Err(e) => {
                self.set_status(format!("Save failed: {e}"));
                ring_bell();
                false
            }
        }
        // In the browser there is no filesystem: hand the bytes to the host as a
        // download named after the document.
        #[cfg(target_arch = "wasm32")]
        {
            let name = file_download_name(path, "md");
            match crate::platform::download(&name, "text/markdown", content.as_bytes()) {
                Ok(()) => {
                    self.modified = false;
                    self.set_status(format!("Downloaded {name}"));
                    true
                }
                Err(e) => {
                    self.set_status(e);
                    false
                }
            }
        }
    }

    /// Carry out `after` now; unsaved changes are either saved already or being
    /// deliberately discarded.
    fn run_after(&mut self, after: AfterSave) {
        #[cfg(not(target_arch = "wasm32"))]
        if after != AfterSave::Nothing {
            // Leaving this document: remember where we were in it.
            self.note_position();
            crate::platform::clear_recovery(self.path.as_deref());
        }
        match after {
            AfterSave::Nothing => {}
            AfterSave::Quit => self.should_quit = true,
            AfterSave::Close => self.new_document(),
            AfterSave::Open(path) => {
                #[cfg(not(target_arch = "wasm32"))]
                self.load_file(path);
                #[cfg(target_arch = "wasm32")]
                let _ = path;
            }
            AfterSave::ApplyPicked => {
                #[cfg(target_arch = "wasm32")]
                if let Some((name, bytes)) = self.pending_open.take() {
                    let loaded = crate::wordstar::load_bytes(&name, &bytes);
                    self.apply_loaded(loaded, PathBuf::from(name));
                }
            }
        }
    }

    /// Run `after`, first asking whether to save if there are unsaved changes.
    pub fn guard_unsaved(&mut self, after: AfterSave) {
        if !self.modified {
            self.run_after(after);
            return;
        }
        let before = match after {
            AfterSave::Quit => "quitting",
            AfterSave::Close => "closing",
            AfterSave::Open(_) | AfterSave::ApplyPicked => "opening another file",
            AfterSave::Nothing => "continuing",
        };
        self.confirm = Some(ConfirmState {
            message: format!("Save changes to {} before {before}?", self.file_name()),
            action: ConfirmAction::SaveBefore(after),
        });
        self.mode = Mode::Confirm;
    }

    /// Offer to restore an autosaved copy of the current document left behind by
    /// a crash or a closed terminal, if it is newer than the file on disk.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn offer_recovery(&mut self) {
        let Some((text, written)) = crate::platform::load_recovery(self.path.as_deref()) else {
            return;
        };
        let current = self.textarea.lines().join("\n");
        let on_disk_is_newer = self
            .path
            .as_ref()
            .and_then(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
            .is_some_and(|disk| disk >= written);
        if text.trim_end_matches('\n') == current.trim_end_matches('\n') || on_disk_is_newer {
            crate::platform::clear_recovery(self.path.as_deref());
            return;
        }
        let minutes = written.elapsed().map(|d| d.as_secs() / 60).unwrap_or(0);
        let age = match minutes {
            0 => "less than a minute ago".to_string(),
            1..=119 => format!("{minutes} min ago"),
            _ => format!("{} h ago", minutes / 60),
        };
        self.confirm = Some(ConfirmState {
            message: format!("Recover unsaved changes to {} from {age}?", self.file_name()),
            action: ConfirmAction::Recover(text),
        });
        self.mode = Mode::Confirm;
    }

    /// Periodic housekeeping from the native main loop: keep a crash-recovery
    /// copy of unsaved work, rewritten at most every few seconds when it changed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn tick(&mut self) {
        const AUTOSAVE_INTERVAL_MS: f64 = 15_000.0;
        if !self.modified {
            return;
        }
        let now = crate::platform::now_ms();
        if now - self.last_autosave_ms < AUTOSAVE_INTERVAL_MS {
            return;
        }
        self.last_autosave_ms = now;
        let hash = self.content_hash();
        if self.autosave_hash == Some(hash) {
            return;
        }
        let text = self.textarea.lines().join("\n");
        if crate::platform::save_recovery(self.path.as_deref(), &text).is_ok() {
            self.autosave_hash = Some(hash);
        }
    }

    /// Open the "Export PDF as:" filename prompt, pre-filled with a default.
    /// (Typing a `.docx` name there exports a Word document instead.)
    pub fn start_export_pdf(&mut self) {
        self.start_export("pdf");
    }

    /// File ▸ Export Word Document: the export prompt with a `.docx` name.
    pub fn start_export_docx(&mut self) {
        self.start_export("docx");
    }

    fn start_export(&mut self, extension: &str) {
        let default = match &self.path {
            Some(p) => p.with_extension(extension),
            None => PathBuf::from(format!("untitled.{extension}")),
        };
        let word = extension == "docx";
        let manuscript = crate::attributes::render_options(&self.textarea.lines().join("\n"))
            .manuscript
            .is_some();
        let label = match (manuscript, word) {
            (true, true) => "Export manuscript .docx as:",
            (true, false) => "Export manuscript PDF as:",
            (false, true) => "Export Word document as:",
            (false, false) => "Export PDF as:",
        };
        self.open_prompt(PromptKind::ExportPdf, label, default.to_string_lossy().into_owned());
    }

    /// Write the PDF to `path`, reporting success or failure on the status line.
    /// Mention included chapter files that couldn't be read.
    fn report_includes(&mut self, book: &crate::book::Expanded) {
        if !book.missing.is_empty() {
            self.set_status(format!("Missing included file(s): {}", book.missing.join(", ")));
        }
    }

    /// Export to `path`: a Word document if it ends in `.docx`, else a PDF.
    fn do_export(&mut self, path: &Path) {
        let title = self.file_name();
        let book = self.book_text();
        let word = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("docx"));
        let bytes = if word {
            crate::docx::export(&book.text, &title)
        } else {
            crate::pdf::export(&book.text, &title)
        };
        #[cfg(not(target_arch = "wasm32"))]
        match fs::write(path, bytes) {
            Ok(()) if !book.missing.is_empty() => self.set_status(format!(
                "Exported {} — missing included file(s): {}",
                path.display(),
                book.missing.join(", ")
            )),
            Ok(()) => self.set_status(format!("Exported {}", path.display())),
            Err(e) => self.set_status(format!("Export failed: {e}")),
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (ext, mime) = if word {
                ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document")
            } else {
                ("pdf", "application/pdf")
            };
            let name = file_download_name(path, ext);
            match crate::platform::download(&name, mime, &bytes) {
                Ok(()) => self.set_status(format!("Downloaded {name}")),
                Err(e) => self.set_status(e),
            }
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        // The unsaved-changes prompt is three-way (Save / Don't save / Cancel).
        if let Some(ConfirmAction::SaveBefore(_)) = self.confirm.as_ref().map(|c| &c.action) {
            let take_after = |app: &mut App| match app.confirm.take().map(|c| c.action) {
                Some(ConfirmAction::SaveBefore(after)) => after,
                _ => AfterSave::Nothing,
            };
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let after = take_after(self);
                    self.mode = Mode::Editor;
                    // Continues only once the save succeeds (an untitled document
                    // goes through Save As first), so nothing is lost.
                    self.save_then(after);
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    let after = take_after(self);
                    self.mode = Mode::Editor;
                    self.run_after(after); // discard the changes
                }
                KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
                    self.confirm = None;
                    self.mode = Mode::Editor;
                    self.set_status("Cancelled — your changes are still here.");
                }
                _ => {}
            }
            return;
        }

        // Generic yes/no confirmations (overwrite, recovery).
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let action = self.confirm.take().map(|c| c.action);
                self.mode = Mode::Editor;
                match action {
                    Some(ConfirmAction::OverwriteExport(path)) => self.do_export(&path),
                    Some(ConfirmAction::OverwriteSave(path)) => self.save_as(path),
                    Some(ConfirmAction::Recover(text)) => self.restore_recovered(&text),
                    Some(ConfirmAction::OverwriteBlock(path, text)) => self.write_block(&path, &text),
                    _ => {}
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                let action = self.confirm.take().map(|c| c.action);
                self.mode = Mode::Editor;
                self.after_save = None;
                match action {
                    #[cfg(not(target_arch = "wasm32"))]
                    Some(ConfirmAction::Recover(_)) => {
                        crate::platform::clear_recovery(self.path.as_deref());
                        self.set_status("Recovery copy discarded.");
                    }
                    _ => self.set_status("Cancelled."),
                }
            }
            _ => {}
        }
    }

    /// Replace the buffer with recovered text, leaving it unsaved.
    fn restore_recovered(&mut self, text: &str) {
        self.textarea = TextArea::new(text_to_lines(text));
        self.apply_editor_theme();
        self.modified = true;
        self.set_status("Recovered your unsaved changes — save (^KS) to keep them.");
    }

    /// Quit, prompting to save first if there are unsaved changes.
    pub fn request_quit(&mut self) {
        self.guard_unsaved(AfterSave::Quit);
    }

    /// Insert pasted text (terminal bracketed paste, or the browser's paste
    /// event) where the user is typing: into the document as a single undoable
    /// edit, or into the open prompt or dialog (first line only).
    pub fn handle_paste(&mut self, text: String) {
        let before = self.tracking_snapshot();
        self.paste(text);
        self.follow_edits(before);
    }

    fn paste(&mut self, text: String) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let first_line = text.lines().next().unwrap_or("");
        match self.mode {
            Mode::Editor => {
                self.marking = false;
                if self.textarea.insert_str(&text) {
                    self.modified = true;
                }
            }
            Mode::Prompt => {
                let p = &mut self.prompt;
                if std::mem::take(&mut p.fresh) {
                    p.input.clear();
                    p.cursor = 0;
                }
                let at = p.input.char_indices().nth(p.cursor).map_or(p.input.len(), |(b, _)| b);
                p.input.insert_str(at, first_line);
                p.cursor += first_line.chars().count();
            }
            Mode::Header => {
                if let Some(h) = self.header_dialog.as_mut() {
                    h.text.push_str(first_line);
                }
            }
            Mode::Calculator => {
                if let Some(c) = self.calc.as_mut() {
                    c.input.push_str(first_line);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Mouse
    // ------------------------------------------------------------------

    /// Route a mouse event according to the current mode.
    pub fn handle_mouse(&mut self, me: MouseEvent) {
        match self.mode {
            Mode::Editor => self.mouse_editor(me),
            Mode::Clean => self.mouse_scroll(me, OverlayKind::Preview),
            Mode::Menu => self.mouse_menu(me),
            Mode::Browser => self.mouse_browser(me),
            Mode::Preview => self.mouse_preview(me),
            Mode::Help => self.mouse_scroll(me, OverlayKind::Help),
            Mode::Info => {
                if me.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.info = None;
                    self.mode = Mode::Editor;
                }
            }
            Mode::Outline | Mode::Recent => self.mouse_list(me),
            Mode::Spell => {}
            Mode::Prompt
            | Mode::Confirm
            | Mode::Header
            | Mode::Calculator
            | Mode::ReplaceAsk => {} // keyboard-only
        }
    }

    fn mouse_editor(&mut self, me: MouseEvent) {
        match me.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // A click on the menu bar opens that menu.
                let mb = self.menu_bar_area.get();
                if mb.height > 0 && me.row >= mb.y && me.row < mb.y + mb.height {
                    if let Some(idx) = self.menu_index_at(me.column) {
                        self.open_menu();
                        self.menu.select_menu(idx);
                    }
                    return;
                }
                // Otherwise position the cursor / begin a selection.
                if let Some(pos) = self.editor_doc_pos(me.column, me.row) {
                    let double = self.register_click(me.column, me.row);
                    self.textarea.cancel_selection();
                    self.textarea.move_cursor(jump(pos));
                    if double {
                        self.select_word();
                        self.mouse_selecting = false;
                    } else {
                        self.textarea.start_selection();
                        self.mouse_selecting = true;
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.mouse_selecting
                    && let Some(pos) = self.editor_doc_pos(me.column, me.row)
                {
                    self.textarea.move_cursor(jump(pos));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.mouse_selecting {
                    self.mouse_selecting = false;
                    // A plain click (no drag) leaves a zero-width selection; drop it.
                    match self.textarea.selection_range() {
                        Some((a, b)) if a != b => {}
                        _ => self.textarea.cancel_selection(),
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                self.textarea.scroll((1, 0));
                self.scroll_viewport(1);
            }
            MouseEventKind::ScrollUp => {
                self.textarea.scroll((-1, 0));
                self.scroll_viewport(-1);
            }
            _ => {}
        }
    }

    fn mouse_menu(&mut self, me: MouseEvent) {
        use crate::menu::Activation;
        if me.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let mb = self.menu_bar_area.get();
        if mb.height > 0 && me.row >= mb.y && me.row < mb.y + mb.height {
            if let Some(idx) = self.menu_index_at(me.column) {
                self.menu.select_menu(idx);
            }
            return;
        }

        // A click inside the open submenu panel selects and runs a leaf.
        let row_in = |r: Rect| -> Option<usize> {
            (r.height > 1
                && me.row > r.y
                && me.row < r.y + r.height - 1
                && me.column > r.x
                && me.column < r.x + r.width - 1)
                .then(|| (me.row - r.y - 1) as usize)
        };

        if self.menu.sub_open
            && let Some(items) = self.menu.submenu_items()
            && let Some(item) = row_in(self.sub_dropdown_area.get())
            && item < items.len()
        {
            self.menu.sub_item = item;
            if let Activation::Run(cmd) = self.menu.activate() {
                self.mode = Mode::Editor;
                commands::execute(self, cmd);
            }
            return;
        }

        if let Some(item) = row_in(self.dropdown_area.get()) {
            let count = crate::menu::MENUS[self.menu.menu].items.len();
            if item < count {
                self.menu.sub_open = false;
                self.menu.item = item;
                match self.menu.activate() {
                    Activation::Run(cmd) => {
                        self.mode = Mode::Editor;
                        commands::execute(self, cmd);
                    }
                    Activation::OpenedSubmenu | Activation::None => {}
                }
            }
            return;
        }
        // Clicked outside the menu: close it.
        self.mode = Mode::Editor;
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn mouse_browser(&mut self, me: MouseEvent) {
        match me.kind {
            MouseEventKind::ScrollDown => {
                if let Some(b) = self.browser.as_mut() {
                    b.select_next();
                }
            }
            MouseEventKind::ScrollUp => {
                if let Some(b) = self.browser.as_mut() {
                    b.select_prev();
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let la = self.browser_list_area.get();
                if la.height == 0
                    || me.row < la.y
                    || me.row >= la.y + la.height
                    || me.column < la.x
                    || me.column >= la.x + la.width
                {
                    return;
                }
                let col_width = 26u16;
                let (rows, num_cols, entries_len, cur_sel) = match self.browser.as_ref() {
                    Some(b) => (
                        b.col_height.get().max(1),
                        (la.width / col_width).max(1) as usize,
                        b.entries.len(),
                        b.selected,
                    ),
                    None => {
                        self.mode = Mode::Editor;
                        return;
                    }
                };
                let per_page = (rows * num_cols).max(1);
                let page_start = (cur_sel / per_page) * per_page;
                let col_idx = ((me.column - la.x) / col_width) as usize;
                let row_idx = (me.row - la.y) as usize;
                let idx = page_start + col_idx * rows + row_idx;
                if idx >= entries_len {
                    return;
                }
                let double = self.register_click(me.column, me.row) && cur_sel == idx;
                if let Some(b) = self.browser.as_mut() {
                    b.selected = idx;
                }
                if double {
                    let activation = self.browser.as_mut().map(|b| b.activate());
                    if let Some(crate::browser::Activation::Open(path)) = activation {
                        self.browser = None;
                        self.mode = Mode::Editor;
                        self.guard_unsaved(AfterSave::Open(path));
                    }
                }
            }
            _ => {}
        }
    }

    /// Browser mode never activates on wasm; stub keeps mouse dispatch exhaustive.
    #[cfg(target_arch = "wasm32")]
    fn mouse_browser(&mut self, _me: MouseEvent) {}

    fn mouse_preview(&mut self, me: MouseEvent) {
        if self.preview_pages.is_empty() {
            self.mouse_scroll(me, OverlayKind::Preview);
            return;
        }
        let zoomed = self.preview_zoom > 1.001;
        match me.kind {
            MouseEventKind::ScrollDown => {
                if zoomed {
                    self.preview_pan(0.0, 0.12);
                } else {
                    self.preview_set_page(self.preview_page + 1);
                }
            }
            MouseEventKind::ScrollUp => {
                if zoomed {
                    self.preview_pan(0.0, -0.12);
                } else {
                    self.preview_set_page(self.preview_page.wrapping_sub(1));
                }
            }
            _ => {}
        }
    }

    fn mouse_scroll(&mut self, me: MouseEvent, kind: OverlayKind) {
        let scroll = match kind {
            OverlayKind::Preview => &mut self.preview_scroll,
            OverlayKind::Help => &mut self.help_scroll,
        };
        match me.kind {
            MouseEventKind::ScrollDown => *scroll = scroll.saturating_add(1),
            MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(1),
            _ => {}
        }
    }

    /// Height of the editing viewport (rows), from the last render.
    pub fn viewport_height(&self) -> usize {
        self.editor_area.get().height as usize
    }

    /// Mirror an explicit `TextArea::scroll` on our tracked viewport top so the
    /// scrollbar thumb stays in sync. The widget shifts its stored viewport top
    /// by the same delta (saturating at zero) before pulling the cursor back into
    /// view; render-time `next_scroll_top` then settles both identically.
    pub fn scroll_viewport(&self, rows: isize) {
        let top = self.scroll_top.get() as isize;
        self.scroll_top.set((top + rows).max(0) as usize);
    }

    /// Mouse in the Go to Heading list: the wheel moves the selection, a click
    /// jumps to that heading, a click outside closes the list.
    fn mouse_list(&mut self, me: MouseEvent) {
        let area = self.outline_area.get();
        let (selected, len) = match self.mode {
            Mode::Outline => match self.outline.as_mut() {
                Some(o) => (&mut o.selected, o.items.len()),
                None => return,
            },
            _ => match self.recent.as_mut() {
                Some(r) => (&mut r.selected, r.items.len()),
                None => return,
            },
        };
        let last = len.saturating_sub(1);
        match me.kind {
            MouseEventKind::ScrollDown => *selected = (*selected + 1).min(last),
            MouseEventKind::ScrollUp => *selected = selected.saturating_sub(1),
            MouseEventKind::Down(MouseButton::Left) => {
                let inside = me.row >= area.y
                    && me.row < area.y + area.height
                    && me.column >= area.x
                    && me.column < area.x + area.width;
                if !inside {
                    self.close_lists();
                    return;
                }
                // The list scrolls to keep the selection in view; see the renderer.
                let first = crate::ui::outline_first_row(*selected, area.height as usize);
                let idx = first + (me.row - area.y) as usize;
                if idx <= last {
                    *selected = idx;
                    if self.mode == Mode::Outline {
                        self.outline_jump();
                    } else {
                        self.recent_open();
                    }
                }
            }
            _ => {}
        }
    }

    /// Map a click in the editor pane to a document `(row, col)` through the
    /// wrapped layout of the last render: the visual row under the pointer (the
    /// viewport's top row plus the offset), then the character at that column.
    /// Clicks right of the text, or below the end, land at the nearest position.
    fn editor_doc_pos(&self, mx: u16, my: u16) -> Option<(usize, usize)> {
        let pane = self.editor_pane.get();
        let area = self.editor_area.get();
        if area.width == 0
            || my < pane.y
            || my >= pane.y + pane.height
            || mx < pane.x
            || mx >= pane.x + pane.width
        {
            return None;
        }
        let rows = self.visual_rows.borrow();
        let lines = self.textarea.lines();
        let tab = self.textarea.tab_length();
        let v = self.scroll_top.get() + (my - area.y) as usize;
        let Some(row) = rows.get(v) else {
            // Below the end of the document: the end of the last line.
            let last = lines.len().saturating_sub(1);
            return Some((last, lines[last].chars().count()));
        };
        let text = row.text(lines);
        let len = text.chars().count();
        let offset = self.row_offset(text, area.width as usize);
        let x = ((mx - area.x) as usize).saturating_sub(offset);
        let mut col = crate::wrap::x_to_char(text, x, tab);
        // Just past the end of a wrapped row is the start of the next one; stay on
        // this row (normally on the space it ends with).
        if !row.last && col >= len && len > 0 {
            col = len - 1;
        }
        Some((row.line, row.start_col(lines) + col))
    }

    /// Select the word under the cursor (double-click).
    fn select_word(&mut self) {
        self.textarea.move_cursor(CursorMove::WordBack);
        self.textarea.start_selection();
        self.textarea.move_cursor(CursorMove::WordForward);
    }

    /// Which menu title (if any) sits under screen column `x`.
    fn menu_index_at(&self, x: u16) -> Option<usize> {
        let width = self.menu_bar_area.get().width;
        let anchors = crate::ui::menu_anchors(width);
        for (i, m) in crate::menu::MENUS.iter().enumerate() {
            if i == crate::menu::HELP_INDEX {
                if x >= anchors[i] {
                    return Some(i);
                }
            } else {
                let start = anchors[i];
                let end = start + m.title.chars().count() as u16;
                if x >= start && x < end {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Record a click and report whether it completes a double-click.
    fn register_click(&mut self, x: u16, y: u16) -> bool {
        let now = crate::platform::now_ms();
        let double = matches!(
            self.last_click,
            Some((t, px, py)) if px == x && py == y && now - t < 400.0
        );
        self.last_click = if double { None } else { Some((now, x, y)) };
        double
    }

    /// Set the transient status-line message.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some(msg.into());
    }

    /// Where a visual row's `text` starts within a text column `width` cells
    /// wide, given the paragraph alignment (centered / right-aligned rows are
    /// shifted right).
    pub fn row_offset(&self, text: &str, width: usize) -> usize {
        let used = crate::wrap::char_to_x(text, text.chars().count(), self.textarea.tab_length());
        match self.textarea.alignment() {
            Alignment::Center => width.saturating_sub(used) / 2,
            Alignment::Right => width.saturating_sub(used),
            Alignment::Left => 0,
        }
    }

    /// Rebuild the wrapped layout for a text column `width` cells wide, and the
    /// printed page/line of every row. Called by the renderer once per frame.
    pub fn refresh_layout(&self, width: usize) {
        let lines = self.textarea.lines();
        let rows = crate::wrap::layout(
            lines,
            self.textarea.wrap_mode(),
            width,
            self.textarea.tab_length(),
        );
        *self.row_pages.borrow_mut() = page_layout(&rows, lines);
        *self.visual_rows.borrow_mut() = rows;
    }

    /// Index of the visual row holding the cursor, from the cached layout.
    pub fn cursor_row_index(&self) -> Option<usize> {
        let (line, col) = {
            let c = self.textarea.cursor();
            (c.0, c.1)
        };
        let rows = self.visual_rows.borrow();
        let lines = self.textarea.lines();
        let first = rows.partition_point(|r| r.line < line);
        let mut found = None;
        for (i, r) in rows.iter().enumerate().skip(first) {
            if r.line != line || r.start_col(lines) > col {
                break;
            }
            found = Some(i);
        }
        found
    }

    /// The cursor's printed column within its visual row (formatting markers
    /// don't count), 0-based. Drives the ruler indicator and the status line.
    pub fn cursor_visible_column(&self) -> usize {
        let (line_idx, col) = {
            let c = self.textarea.cursor();
            (c.0, c.1)
        };
        let line = self
            .textarea
            .lines()
            .get(line_idx)
            .map(String::as_str)
            .unwrap_or("");
        let row_start = self
            .cursor_row_index()
            .and_then(|i| self.visual_rows.borrow().get(i).copied())
            .map_or(0, |r| r.start_col(self.textarea.lines()));
        crate::attributes::visible_column(line, col)
            - crate::attributes::visible_column(line, row_start)
    }

    /// Cursor metrics for the status line, in WordStar units: the page and line
    /// on it as printed (wrapped rows, `.pa` page breaks, 54 lines a page), and
    /// the column within the printed line.
    pub fn cursor_metrics(&self) -> CursorMetrics {
        let (page, line) = match self.cursor_row_index() {
            Some(i) => self.row_pages.borrow()[i],
            None => {
                let row = self.textarea.cursor().0;
                (row / LINES_PER_PAGE + 1, row % LINES_PER_PAGE + 1)
            }
        };
        let col = self.cursor_visible_column();
        CursorMetrics {
            line,
            column: col + 1,
            page,
            // Vertical position: 0.5" top margin + 6 lines/inch.
            vertical_inches: 0.5 + (line - 1) as f32 / 6.0,
            // Horizontal position: 10 chars/inch (pica).
            horizontal_inches: col as f32 / 10.0,
        }
    }
}

/// Derived cursor position for the status line.
pub struct CursorMetrics {
    pub line: usize,
    pub column: usize,
    pub page: usize,
    pub vertical_inches: f32,
    pub horizontal_inches: f32,
}

/// Which scrollable overlay a key event targets.
#[derive(Clone, Copy)]
enum OverlayKind {
    Preview,
    Help,
}

/// Approximate text lines per printed page (9" at 6 lines/inch), used for the
/// status-line page metric and "go to page".
const LINES_PER_PAGE: usize = 54;

/// WordStar's default right margin (the column text wraps at), and the range
/// `^OR` accepts.
pub const DEFAULT_RIGHT_MARGIN: usize = 65;
const MIN_RIGHT_MARGIN: usize = 20;
const MAX_RIGHT_MARGIN: usize = 250;

/// The column set by a `.rm N` (right margin) dot command, if `line` is one.
fn right_margin_command(line: &str) -> Option<usize> {
    let rest = line.get(..3)?.eq_ignore_ascii_case(".rm").then(|| &line[3..])?;
    rest.trim()
        .parse()
        .ok()
        .map(|n: usize| n.clamp(MIN_RIGHT_MARGIN, MAX_RIGHT_MARGIN))
}

/// True for a `.pa` (new page) dot command.
pub fn is_page_break(line: &str) -> bool {
    line.trim_end().eq_ignore_ascii_case(".pa")
}

/// The printed `(page, line)` of every visual row, both 1-based: pages hold
/// [`LINES_PER_PAGE`] printed lines, `.pa` starts a new page, and other dot
/// commands don't print (they report the line the next text would go on).
fn page_layout(rows: &[crate::wrap::VisualRow], lines: &[String]) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(rows.len());
    let (mut page, mut used) = (1usize, 0usize);
    for r in rows {
        let text = lines.get(r.line).map(String::as_str).unwrap_or("");
        if crate::attributes::is_dot_command(text) {
            out.push((page, used + 1));
            if is_page_break(text) && used > 0 {
                page += 1;
                used = 0;
            }
            continue;
        }
        if used == LINES_PER_PAGE {
            page += 1;
            used = 0;
        }
        used += 1;
        out.push((page, used));
    }
    out
}

/// The `(row, col)` of character `offset` in `chars`, a buffer joined with `\n`.
fn char_pos(chars: &[char], offset: usize) -> (usize, usize) {
    let (mut row, mut col) = (0, 0);
    for &c in &chars[..offset] {
        if c == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
}

/// The absolute character offset of `(row, col)` in `lines` joined with `\n`.
fn pos_to_offset(lines: &[String], (row, col): (usize, usize)) -> usize {
    lines[..row.min(lines.len())]
        .iter()
        .map(|l| l.chars().count() + 1)
        .sum::<usize>()
        + col
}

/// The `(row, col)` of absolute character `offset` in `lines` joined with `\n`.
fn offset_to_pos(lines: &[String], mut offset: usize) -> (usize, usize) {
    for (row, line) in lines.iter().enumerate() {
        let len = line.chars().count();
        if offset <= len {
            return (row, offset);
        }
        offset -= len + 1;
    }
    let last = lines.len().saturating_sub(1);
    (last, lines.get(last).map_or(0, |l| l.chars().count()))
}

/// A cursor jump to `(row, col)`, saturating at the widget's `u16` coordinates.
fn jump((row, col): (usize, usize)) -> CursorMove {
    CursorMove::Jump(
        row.min(u16::MAX as usize) as u16,
        col.min(u16::MAX as usize) as u16,
    )
}

/// Undo steps the editor remembers (the text widget defaults to only 50, and
/// every typed character is one step).
const UNDO_LEVELS: usize = 10_000;

/// Split loaded document text into editor lines (never empty).
fn text_to_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        vec![String::new()]
    } else {
        text.lines().map(str::to_owned).collect()
    }
}

fn ring_bell() {
    use std::io::Write;
    let _ = std::io::stdout().write_all(b"\x07");
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui_image::picker::Picker;

    fn page(color: u8) -> RgbaImage {
        RgbaImage::from_pixel(40, 60, Rgba([color, color, color, 255]))
    }

    #[test]
    fn page_protocol_built_lazily_and_cached() {
        let mut app = App::new(None).unwrap();
        app.picker = Some(Picker::from_fontsize((10, 20).into()));
        app.preview_pages = vec![page(255), page(0)];
        *app.preview_page_protocols.borrow_mut() = vec![None, None];
        app.preview_page = 0;
        app.preview_zoom = 1.0;
        let area = Rect::new(0, 0, 10, 8);

        app.ensure_preview(area);
        assert!(
            app.preview_page_protocols.borrow()[0].is_some(),
            "current page built"
        );
        assert!(
            app.preview_page_protocols.borrow()[1].is_none(),
            "other page untouched"
        );

        // Re-running for the same page must not rebuild a fresh protocol.
        let ptr_before = app.preview_page_protocols.borrow()[0]
            .as_ref()
            .map(|p| p as *const _ as usize);
        app.ensure_preview(area);
        let ptr_after = app.preview_page_protocols.borrow()[0]
            .as_ref()
            .map(|p| p as *const _ as usize);
        assert_eq!(ptr_before, ptr_after, "cached protocol reused, not rebuilt");
    }

    #[test]
    fn zoom_view_key_distinguishes_views() {
        let app = App::new(None).unwrap();
        let area = Rect::new(0, 0, 10, 8);
        let base = app.zoom_view_key(area);
        let mut z = App::new(None).unwrap();
        z.preview_zoom = 2.0;
        assert_ne!(base, z.zoom_view_key(area), "zoom changes the key");
        let mut p = App::new(None).unwrap();
        p.preview_off = (0.5, 0.0);
        assert_ne!(base, p.zoom_view_key(area), "pan changes the key");
    }

    fn select_world(app: &mut App) {
        // "hello world" with the cursor selecting "world" (cols 6..11).
        app.textarea.insert_str("hello world");
        app.textarea.move_cursor(CursorMove::Head);
        for _ in 0..6 {
            app.textarea.move_cursor(CursorMove::Forward);
        }
        app.block_begin(); // start selection at col 6
        app.textarea.move_cursor(CursorMove::End); // extends to col 11
    }

    #[test]
    fn block_copy_then_paste() {
        let mut app = App::new(None).unwrap();
        select_world(&mut app);
        app.block_copy();
        assert_eq!(app.block_buffer, "world");
        assert!(!app.textarea.is_selecting(), "copy clears the selection");
        app.textarea.move_cursor(CursorMove::Head);
        app.block_move(); // paste
        assert_eq!(app.textarea.lines(), ["worldhello world"]);
    }

    #[test]
    fn block_cut_then_paste() {
        let mut app = App::new(None).unwrap();
        select_world(&mut app);
        app.block_delete(); // cut
        assert_eq!(app.block_buffer, "world");
        assert_eq!(app.textarea.lines(), ["hello "]);
        app.textarea.move_cursor(CursorMove::Head);
        app.block_move(); // paste
        assert_eq!(app.textarea.lines(), ["worldhello "]);
    }

    #[test]
    fn plain_arrows_extend_selection_while_marking() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("abcdef");
        app.textarea.move_cursor(CursorMove::Head);
        app.block_begin();
        // Three plain Right arrows should extend the selection, not cancel it.
        for _ in 0..3 {
            app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        }
        app.block_copy();
        assert_eq!(app.block_buffer, "abc");
    }

    #[test]
    fn word_wrap_on_by_default_and_toggles() {
        let mut app = App::new(None).unwrap();
        assert!(app.wrap);
        assert_eq!(app.textarea.wrap_mode(), ratatui_textarea::WrapMode::Word);
        app.toggle_wrap();
        assert!(!app.wrap);
        assert_eq!(app.textarea.wrap_mode(), ratatui_textarea::WrapMode::None);
    }

    #[test]
    fn paste_with_empty_clipboard_is_noop() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("text");
        app.block_move();
        assert_eq!(app.textarea.lines(), ["text"]);
    }

    #[test]
    fn word_count_reports_words_and_paragraphs() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("one **two** three\n\nfour five");
        app.show_word_count();
        assert_eq!(app.mode, Mode::Info);
        let info = app.info.as_ref().unwrap();
        let body = info.lines.join("\n");
        // Markers are stripped, so "**two**" counts as one word: 5 total.
        assert!(body.contains("Words:        5"), "got:\n{body}");
        assert!(body.contains("Paragraphs:   2"), "got:\n{body}");
        assert!(body.contains("This session: +5"), "got:\n{body}");
        assert!(!body.contains("Block"), "no block marked");
        // Any key dismisses it.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        assert!(app.info.is_none());
    }

    #[test]
    fn word_count_shows_the_block_and_the_goal() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("---\ngoal: 10\n---\none two three four");
        app.textarea.move_cursor(CursorMove::Jump(3, 4));
        app.block_begin();
        app.textarea.move_cursor(CursorMove::Jump(3, 13));
        app.block_end();
        app.show_word_count();
        let body = app.info.as_ref().unwrap().lines.join("\n");
        assert!(body.contains("Words:        4"), "frontmatter not counted:\n{body}");
        assert!(body.contains("Goal:         10 (40%, 6 to go)"), "got:\n{body}");
        assert!(body.contains("Block:        2 words"), "got:\n{body}");
        assert_eq!(app.word_count(), 4);
    }

    #[test]
    fn insert_file_inserts_contents_at_cursor() {
        let dir = std::env::temp_dir();
        let path = dir.join("wsrs_insert_test.txt");
        std::fs::write(&path, "INSERTED").unwrap();
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("AB");
        app.textarea.move_cursor(CursorMove::Head); // cursor before "AB"
        app.insert_file(path.to_str().unwrap());
        assert_eq!(app.textarea.lines(), ["INSERTEDAB"]);
        assert!(app.modified);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn header_dialog_inserts_dot_command_at_top() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Body text");
        app.start_header(HeaderKind::Header);
        assert_eq!(app.mode, Mode::Header);
        for c in "My Title".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        // Down once: Both -> Odd.
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.textarea.lines()[0], ".oh My Title");
        assert_eq!(app.textarea.lines()[1], "Body text");
    }

    #[test]
    fn page_break_inserts_dot_command() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("para");
        app.insert_dot_command(".pa", "Page break inserted.");
        assert!(app.textarea.lines().iter().any(|l| l == ".pa"));
        assert!(app.modified);
    }

    #[test]
    fn header_dialog_cancels_without_inserting() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Body");
        app.start_header(HeaderKind::Footer);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.textarea.lines(), ["Body"]);
    }

    #[test]
    fn alt_accelerator_opens_matching_menu() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(None).unwrap();

        // Alt+F opens the File menu (index 0).
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(app.mode, Mode::Menu);
        assert_eq!(app.menu.menu, 0);

        // Case-insensitive: Alt+U opens Utilities.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        app.handle_key(KeyEvent::new(KeyCode::Char('U'), KeyModifiers::ALT));
        assert_eq!(app.mode, Mode::Menu);
        assert_eq!(app.menu.menu, crate::menu::menu_for_accelerator('u').unwrap());

        // Alt with a letter that isn't an accelerator does not open a menu.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT));
        assert_eq!(app.mode, Mode::Editor);
    }

    #[test]
    fn menu_submenu_opens_and_runs_leaf() {
        use crate::commands::Command;
        use crate::menu::Activation;
        let mut app = App::new(None).unwrap();
        app.open_menu();
        // Jump to the Layout menu and find the Headers/Footers submenu item.
        assert!(app.menu.jump_to_title('l'));
        while app.menu.submenu_items().is_none() {
            app.menu.next_item();
        }
        // Right opens the submenu; first leaf is "Header...".
        app.menu.move_right();
        assert!(app.menu.sub_open);
        match app.menu.activate() {
            Activation::Run(Command::Header) => {}
            other => panic!("expected Header command, got {other:?}"),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }

    /// A fresh, empty scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wsrs-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_exit_on_untitled_asks_for_a_name_instead_of_quitting() {
        let dir = scratch("save-exit");
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("precious words");
        app.modified = true;
        commands::execute(&mut app, commands::Command::SaveExit);
        assert!(!app.should_quit, "must not quit with the work unsaved");
        assert_eq!(app.mode, Mode::Prompt);
        assert_eq!(app.prompt.kind, PromptKind::SaveAs);

        // Naming the file saves it (adding .md), then finishes the exit.
        let target = dir.join("story");
        app.prompt.input = target.to_string_lossy().into_owned();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            std::fs::read_to_string(dir.join("story.md")).unwrap(),
            "precious words\n"
        );
        assert!(app.should_quit);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cancelled_save_as_forgets_the_pending_exit() {
        let mut app = App::new(None).unwrap();
        app.modified = true;
        commands::execute(&mut app, commands::Command::SaveExit);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Editor);
        assert!(!app.should_quit);
        assert!(app.after_save.is_none());
    }

    #[test]
    fn failed_save_does_not_quit() {
        let mut app = App::new(None).unwrap();
        app.path = Some(PathBuf::from("/nonexistent-dir-wsrs/story.md"));
        app.modified = true;
        commands::execute(&mut app, commands::Command::SaveExit);
        assert!(!app.should_quit);
        assert!(app.modified);
        assert!(app.status_msg.as_deref().unwrap().contains("Save failed"));
    }

    #[test]
    fn close_with_unsaved_changes_asks_first() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("draft");
        app.modified = true;
        commands::execute(&mut app, commands::Command::New);
        assert_eq!(app.mode, Mode::Confirm);
        assert_eq!(app.textarea.lines(), ["draft"], "nothing discarded yet");

        // Cancel keeps everything.
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.textarea.lines(), ["draft"]);

        // "No" discards and closes.
        commands::execute(&mut app, commands::Command::New);
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.textarea.lines(), [""]);
        assert!(!app.modified);
    }

    #[test]
    fn save_as_onto_an_existing_file_asks_to_overwrite() {
        let dir = scratch("overwrite");
        let other = dir.join("other.md");
        std::fs::write(&other, "someone else's chapter\n").unwrap();
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("mine");
        app.start_save_as();
        app.prompt.input = other.to_string_lossy().into_owned();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Confirm);
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "someone else's chapter\n");
        assert!(app.path.is_none(), "path only changes after a successful save");

        app.start_save_as();
        app.prompt.input = other.to_string_lossy().into_owned();
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char('y')));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "mine\n");
        assert_eq!(app.path.as_deref(), Some(other.as_path()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_as_resolves_relative_names_next_to_the_document() {
        let mut app = App::new(None).unwrap();
        app.path = Some(PathBuf::from("/novel/drafts/ch1.md"));
        assert_eq!(
            app.resolve_save_path("ch2"),
            PathBuf::from("/novel/drafts/ch2.md")
        );
        assert_eq!(app.resolve_save_path("/tmp/x.txt"), PathBuf::from("/tmp/x.txt"));
        app.start_save_as();
        assert_eq!(app.prompt.input, "ch1.md", "pre-filled with the current name");
    }

    #[test]
    fn undo_reaches_far_beyond_fifty_keystrokes() {
        let mut app = App::new(None).unwrap();
        let text = "x".repeat(300);
        type_str(&mut app, &text);
        for _ in 0..300 {
            commands::execute(&mut app, commands::Command::Undo);
        }
        assert_eq!(app.textarea.lines(), [""]);
    }

    fn literal(s: &str) -> regex::Regex {
        regex::Regex::new(&regex::escape(s)).unwrap()
    }

    /// Run `^QF` (or `^QA` with `replace`) through its prompts.
    fn find_via_prompts(app: &mut App, find: &str, replace: Option<&str>, options: &str) {
        chord(app, 'q', if replace.is_some() { 'a' } else { 'f' });
        app.handle_key(ctrl('y')); // clear any suggested default
        type_str(app, find);
        app.handle_key(key(KeyCode::Enter));
        if let Some(with) = replace {
            app.handle_key(ctrl('y'));
            type_str(app, with);
            app.handle_key(key(KeyCode::Enter));
        }
        app.handle_key(ctrl('y'));
        type_str(app, options);
        app.handle_key(key(KeyCode::Enter));
    }

    #[test]
    fn prompt_default_is_replaced_by_typing_but_editable_with_arrows() {
        let mut app = App::new(None).unwrap();
        app.open_prompt(PromptKind::GoToPage, "Page:", "12".into());
        type_str(&mut app, "7");
        assert_eq!(app.prompt.input, "7", "typing replaces the default");
        app.open_prompt(PromptKind::GoToPage, "Page:", "12".into());
        app.handle_key(key(KeyCode::Left));
        type_str(&mut app, "5");
        assert_eq!(app.prompt.input, "152", "arrows start editing it");
        app.handle_key(ctrl('s')); // ^S is left, WordStar-style
        app.handle_key(key(KeyCode::Delete));
        assert_eq!(app.prompt.input, "12");
        app.handle_key(ctrl('u')); // other control keys don't type letters
        assert_eq!(app.prompt.input, "12");
        app.handle_key(ctrl('y'));
        assert_eq!(app.prompt.input, "");
    }

    #[test]
    fn find_options_ignore_case_whole_words_and_backwards() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Anne met ann.\nANN left, then Ann.");
        app.textarea.move_cursor(CursorMove::Jump(0, 0));
        find_via_prompts(&mut app, "ann", None, "wu");
        assert_eq!(app.textarea.cursor(), (0, 9), "whole word, any case: not Anne");
        app.handle_key(ctrl('l'));
        assert_eq!(app.textarea.cursor(), (1, 0));
        app.handle_key(ctrl('l'));
        assert_eq!(app.textarea.cursor(), (1, 15));
        app.handle_key(ctrl('l'));
        assert!(app.status_msg.as_deref().unwrap().contains("No more"));
        // Backwards from the end of the document (G B).
        find_via_prompts(&mut app, "Ann", None, "GB");
        assert_eq!(app.textarea.cursor(), (1, 15));
        app.handle_key(ctrl('l'));
        assert_eq!(app.textarea.cursor(), (0, 0), "case-sensitive: skips ann and ANN");
        // The next ^QF offers the last text and options.
        chord(&mut app, 'q', 'f');
        assert_eq!(app.prompt.input, "Ann");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.prompt.input, "BG");
    }

    #[test]
    fn replace_asks_for_each_match_and_undoes_as_one() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Ann and Anne and Ann");
        find_via_prompts(&mut app, "Ann", Some("Beth"), "g");
        assert_eq!(app.mode, Mode::ReplaceAsk);
        assert_eq!(app.textarea.selection_range(), Some(((0, 0), (0, 3))));
        app.handle_key(key(KeyCode::Char('y')));
        assert_eq!(app.textarea.selection_range(), Some(((0, 9), (0, 12))), "then Anne");
        app.handle_key(key(KeyCode::Char('n')));
        app.handle_key(key(KeyCode::Char('y')));
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.textarea.lines(), ["Beth and Anne and Beth"]);
        assert!(app.status_msg.as_deref().unwrap().contains("Replaced 2"));
        app.handle_key(ctrl('u'));
        assert_eq!(app.textarea.lines(), ["Ann and Anne and Ann"]);
    }

    #[test]
    fn replace_whole_words_without_asking() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Ann and Anne and Ann");
        find_via_prompts(&mut app, "Ann", Some("Beth"), "GNW");
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.textarea.lines(), ["Beth and Anne and Beth"]);
        // "All the rest" from the question replaces the remaining ones.
        app.textarea.move_cursor(CursorMove::Jump(0, 0));
        find_via_prompts(&mut app, "and", Some("&"), "");
        app.handle_key(key(KeyCode::Char('a')));
        assert_eq!(app.textarea.lines(), ["Beth & Anne & Beth"]);
    }

    #[test]
    fn ctrl_qp_returns_to_the_position_before_a_jump() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("one\ntwo\nthree");
        app.textarea.move_cursor(CursorMove::Jump(1, 2));
        chord(&mut app, 'q', 'r');
        assert_eq!(app.textarea.cursor(), (0, 0));
        chord(&mut app, 'q', 'p');
        assert_eq!(app.textarea.cursor(), (1, 2));
        chord(&mut app, 'q', 'p');
        assert_eq!(app.textarea.cursor(), (0, 0), "and back again");
    }

    #[test]
    fn replace_all_is_undone_by_a_single_undo() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Ann met Anne.\nAnn left.");
        app.textarea.move_cursor(CursorMove::Jump(1, 3));
        app.replace_all(&literal("Ann"), "Beth");
        assert_eq!(app.textarea.lines(), ["Beth met Bethe.", "Beth left."]);
        assert_eq!(app.textarea.cursor(), (1, 3), "cursor stays put");
        commands::execute(&mut app, commands::Command::Undo);
        assert_eq!(app.textarea.lines(), ["Ann met Anne.", "Ann left."]);
    }

    #[test]
    fn replace_with_nothing_is_undone_by_a_single_undo() {
        let mut app = App::new(None).unwrap();
        type_str(&mut app, "ab");
        app.replace_all(&literal("b"), "");
        assert_eq!(app.textarea.lines(), ["a"]);
        commands::execute(&mut app, commands::Command::Undo);
        assert_eq!(app.textarea.lines(), ["ab"], "one undo, not two");
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// Press a two-key WordStar chord such as `^K` `K`.
    fn chord(app: &mut App, prefix: char, c: char) {
        app.handle_key(ctrl(prefix));
        app.handle_key(key(KeyCode::Char(c)));
    }

    #[test]
    fn ctrl_y_deletes_the_whole_line_in_one_press_and_one_undo() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("one\ntwo\nthree");
        app.textarea.move_cursor(CursorMove::Jump(1, 2));
        app.handle_key(ctrl('y'));
        assert_eq!(app.textarea.lines(), ["one", "three"]);
        assert_eq!(app.textarea.cursor(), (1, 0));
        app.handle_key(ctrl('u'));
        assert_eq!(app.textarea.lines(), ["one", "two", "three"]);
        // The last line is cleared rather than joined to the one above.
        app.textarea.move_cursor(CursorMove::Bottom);
        app.handle_key(ctrl('y'));
        assert_eq!(app.textarea.lines(), ["one", "two", ""]);
    }

    #[test]
    fn ctrl_q_r_and_c_go_to_the_very_start_and_end() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("first line\nlast line");
        app.textarea.move_cursor(CursorMove::Jump(1, 3));
        chord(&mut app, 'q', 'c');
        assert_eq!(app.textarea.cursor(), (1, 9));
        chord(&mut app, 'q', 'r');
        assert_eq!(app.textarea.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_n_splits_the_line_but_leaves_the_cursor_in_place() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("helloworld");
        app.textarea.move_cursor(CursorMove::Jump(0, 5));
        app.handle_key(ctrl('n'));
        assert_eq!(app.textarea.lines(), ["hello", "world"]);
        assert_eq!(app.textarea.cursor(), (0, 5));
    }

    /// "The quick brown fox." with `quick ` marked as a block via ^KB … ^KK.
    fn marked_quick() -> App {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("The quick brown fox.");
        app.textarea.move_cursor(CursorMove::Jump(0, 4));
        chord(&mut app, 'k', 'b');
        for _ in 0..6 {
            app.handle_key(key(KeyCode::Right));
        }
        chord(&mut app, 'k', 'k');
        app
    }

    #[test]
    fn ctrl_kk_fixes_the_block_so_the_cursor_can_move_away() {
        let mut app = marked_quick();
        assert_eq!(app.block_buffer, "quick ", "^KK copies the block, as documented");
        assert!(!app.textarea.is_selecting(), "the widget selection is replaced");
        // Moving on no longer stretches the block.
        app.handle_key(key(KeyCode::End));
        let b = app.marked.as_ref().unwrap();
        assert_eq!((b.start, b.end), ((0, 4), (0, 10)));
    }

    #[test]
    fn ctrl_kv_moves_the_marked_block_to_the_cursor() {
        let mut app = marked_quick();
        app.textarea.move_cursor(CursorMove::Jump(0, 16)); // before "fox"
        chord(&mut app, 'k', 'v');
        assert_eq!(app.textarea.lines(), ["The brown quick fox."]);
        assert_eq!(app.textarea.cursor(), (0, 10), "cursor at the moved block");
        // Moving it back to the front (cursor before the block).
        app.textarea.move_cursor(CursorMove::Head);
        chord(&mut app, 'k', 'v');
        assert_eq!(app.textarea.lines(), ["quick The brown fox."]);
    }

    #[test]
    fn ctrl_kc_copies_the_marked_block_to_the_cursor() {
        let mut app = marked_quick();
        app.textarea.move_cursor(CursorMove::End);
        chord(&mut app, 'k', 'c');
        assert_eq!(app.textarea.lines(), ["The quick brown fox.quick "]);
    }

    #[test]
    fn a_marked_block_follows_edits_made_while_it_is_marked() {
        let mut app = marked_quick();
        app.textarea.move_cursor(CursorMove::Head);
        type_str(&mut app, "X"); // before the block: it shifts along
        app.textarea.move_cursor(CursorMove::Jump(0, 7));
        type_str(&mut app, "Y"); // inside it: the block takes it in
        assert_eq!(app.marked.as_ref().unwrap().text, "quYick ");
        app.textarea.move_cursor(CursorMove::End);
        chord(&mut app, 'k', 'v');
        assert_eq!(app.textarea.lines(), ["XThe brown fox.quYick "]);
    }

    #[test]
    fn a_block_changed_behind_its_back_is_not_acted_on() {
        let mut app = marked_quick();
        // An edit that bypasses the key handling (and so the tracking).
        app.textarea.move_cursor(CursorMove::Head);
        app.textarea.insert_str("X");
        app.textarea.move_cursor(CursorMove::End);
        chord(&mut app, 'k', 'v');
        assert_eq!(app.textarea.lines(), ["XThe quick brown fox."], "nothing moved");
        assert!(app.marked.is_none());
        assert!(app.status_msg.as_deref().unwrap().contains("mark it again"));
    }

    #[test]
    fn place_markers_follow_the_text_and_toggle() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Chapter one\nThe storm broke.");
        app.textarea.move_cursor(CursorMove::Jump(1, 4));
        chord(&mut app, 'k', '3');
        assert_eq!(app.markers[3], Some((1, 4)));
        // Add a paragraph above; the marker stays on "storm".
        app.textarea.move_cursor(CursorMove::Jump(0, 11));
        app.handle_key(key(KeyCode::Enter));
        type_str(&mut app, "A new line.");
        chord(&mut app, 'q', 'r');
        chord(&mut app, 'q', '3');
        assert_eq!(app.textarea.cursor(), (2, 4));
        assert_eq!(&app.textarea.lines()[2][4..9], "storm");
        // ^QP goes back to where ^Q3 came from.
        chord(&mut app, 'q', 'p');
        assert_eq!(app.textarea.cursor(), (0, 0));
        // ^K3 on the marker's spot removes it.
        chord(&mut app, 'q', '3');
        chord(&mut app, 'k', '3');
        assert_eq!(app.markers[3], None);
        chord(&mut app, 'q', '3');
        assert!(app.status_msg.as_deref().unwrap().contains("not set"));
    }

    /// A small novel: frontmatter, two chapters, a scene, a code block and a
    /// scene break that must not count as headings.
    const NOVEL: &str = "---\ntitle: # not a heading\n---\n# Chapter **One**\nText.\n#\n## The Storm\n```\n# code, not a heading\n```\n.pa\n# Chapter Two #\nMore.";

    #[test]
    fn headings_skip_frontmatter_code_and_scene_breaks() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str(NOVEL);
        let h = app.headings();
        let titles: Vec<(usize, &str)> = h.iter().map(|h| (h.level, h.title.as_str())).collect();
        assert_eq!(titles, [(1, "Chapter One"), (2, "The Storm"), (1, "Chapter Two")]);
        assert_eq!(h[0].row, 3);
    }

    #[test]
    fn go_to_heading_jumps_and_ctrl_qp_comes_back() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str(NOVEL);
        app.textarea.move_cursor(CursorMove::Jump(7, 1)); // in "The Storm"
        chord(&mut app, 'q', 'g');
        assert_eq!(app.mode, Mode::Outline);
        assert_eq!(app.outline.as_ref().unwrap().selected, 1, "current heading selected");
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.textarea.cursor(), (11, 0));
        chord(&mut app, 'q', 'p');
        assert_eq!(app.textarea.cursor(), (7, 1));
        // Without headings there is nothing to list.
        let mut empty = App::new(None).unwrap();
        empty.textarea.insert_str("Just prose.");
        empty.open_outline();
        assert_eq!(empty.mode, Mode::Editor);
    }

    #[test]
    fn spelling_check_walks_the_unknown_words() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("---\ntitle: Zzyzx\n---\nThe stormm broke over Zorblax.\n`codde` is fine.\nZorblax again, and recieve.");
        app.textarea.move_cursor(CursorMove::Top);
        chord(&mut app, 'q', 'l');
        assert_eq!(app.mode, Mode::Spell);
        let s = app.spell.clone().unwrap();
        assert_eq!(s.word, "stormm", "frontmatter skipped");
        assert_eq!(s.suggestions[0], "storm");
        assert_eq!(app.textarea.selection_range(), Some(((3, 4), (3, 10))));
        app.handle_key(key(KeyCode::Char('1')));
        assert_eq!(app.textarea.lines()[3], "The storm broke over Zorblax.");
        assert_eq!(app.spell.as_ref().unwrap().word, "Zorblax");
        app.handle_key(key(KeyCode::Char('g'))); // ignore all: skips the second one too
        assert_eq!(app.spell.as_ref().unwrap().word, "recieve", "code span skipped");
        app.handle_key(key(KeyCode::Char('t')));
        assert_eq!(app.mode, Mode::Prompt);
        assert_eq!(app.prompt.input, "recieve", "the word, ready to edit");
        app.handle_key(ctrl('y'));
        type_str(&mut app, "receive");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.textarea.lines()[5], "Zorblax again, and receive.");
        assert_eq!(app.mode, Mode::Editor);
        assert!(app.status_msg.as_deref().unwrap().contains("2 correction"));
    }

    #[test]
    fn spell_check_word_at_cursor() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("A fine dya.");
        app.textarea.move_cursor(CursorMove::Jump(0, 3));
        chord(&mut app, 'q', 'n');
        assert!(app.status_msg.as_deref().unwrap().contains("spelled correctly"));
        app.textarea.move_cursor(CursorMove::Jump(0, 10)); // end of "dya"
        chord(&mut app, 'q', 'n');
        assert_eq!(app.mode, Mode::Spell);
        app.handle_key(key(KeyCode::Char('t')));
        app.handle_key(ctrl('y'));
        type_str(&mut app, "day");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.textarea.lines(), ["A fine day."]);
        assert_eq!(app.mode, Mode::Editor, "one word only");
    }

    #[test]
    fn sentence_case_capitalises_sentences_and_i() {
        assert_eq!(
            sentence_case("tHE STORM. i think i'm lost! yes? IT is iron"),
            "The storm. I think I'm lost! Yes? It is iron"
        );
    }

    #[test]
    fn case_commands_change_the_marked_block() {
        let mut app = marked_quick(); // "quick " in "The quick brown fox."
        app.textarea.move_cursor(CursorMove::End);
        app.handle_key(ctrl('k'));
        app.handle_key(key(KeyCode::Char('"')));
        assert_eq!(app.textarea.lines(), ["The QUICK brown fox."]);
        assert_eq!(app.marked.as_ref().unwrap().text, "QUICK ", "still marked");
        chord(&mut app, 'k', '\'');
        assert_eq!(app.textarea.lines(), ["The quick brown fox."]);
        // No block: nothing changes.
        chord(&mut app, 'k', 'h'); // hide it
        chord(&mut app, 'k', '"');
        assert_eq!(app.textarea.lines(), ["The quick brown fox."]);
        assert!(app.status_msg.as_deref().unwrap().contains("Mark a block"));
    }

    #[test]
    fn ctrl_kw_writes_the_block_to_a_file() {
        let dir = scratch("write-block");
        let mut app = marked_quick();
        app.path = Some(dir.join("novel.md"));
        chord(&mut app, 'k', 'w');
        assert_eq!(app.mode, Mode::Prompt);
        type_str(&mut app, "cut-scene");
        app.handle_key(key(KeyCode::Enter));
        let file = dir.join("cut-scene.md");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "quick \n");
        // Writing again over it asks first.
        chord(&mut app, 'k', 'w');
        type_str(&mut app, "cut-scene");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Confirm);
        app.handle_key(key(KeyCode::Char('n')));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_files_list_opens_the_chosen_document() {
        let dir = scratch("recent-list");
        let (a, b) = (dir.join("a.md"), dir.join("b.md"));
        std::fs::write(&a, "Chapter A\n").unwrap();
        std::fs::write(&b, "Chapter B\n").unwrap();
        let mut app = App::new(None).unwrap();
        app.recent = Some(RecentState {
            items: vec![(a.clone(), (0, 0)), (b.clone(), (0, 3))],
            selected: 0,
        });
        app.mode = Mode::Recent;
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Editor);
        assert_eq!(app.path.as_deref(), Some(b.as_path()));
        assert_eq!(app.textarea.lines(), ["Chapter B"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn master_document_covers_the_whole_book() {
        let dir = scratch("master");
        std::fs::write(dir.join("ch1.md"), "# One\nIt was a dark and stormy night.\n").unwrap();
        std::fs::write(dir.join("ch2.md"), "# Two\nMorning came.\n").unwrap();
        let mut app = App::new(None).unwrap();
        app.path = Some(dir.join("book.md"));
        app.textarea.insert_str("---\ngoal: 100\n---\n.fi ch1.md\n.pa\n.fi ch2.md");
        let book = app.book_text();
        assert_eq!(book.files, 2);
        app.textarea.insert_str("\n.fi missing.md");
        assert_eq!(app.word_count(), 11, "a missing file's marker isn't counted");
        assert!(book.text.contains("stormy night") && book.text.contains("Morning came"));
        assert_eq!(app.word_count(), 11, "the title bar counts the whole book");
        app.show_word_count();
        let body = app.info.as_ref().unwrap().lines.join("\n");
        assert!(body.contains("Whole book:   11 words in 2 included file(s)"), "{body}");
        app.mode = Mode::Editor;
        // Go to Heading lists the chapter files; choosing one opens it.
        app.open_outline();
        let titles: Vec<String> = app.outline.as_ref().unwrap().items.iter().map(|i| i.title.clone()).collect();
        assert_eq!(titles, ["\u{25B8} ch1.md", "\u{25B8} ch2.md", "\u{25B8} missing.md"]);
        app.handle_key(key(KeyCode::Home));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.path.as_deref(), Some(dir.join("ch2.md").as_path()));
        assert_eq!(app.textarea.lines(), ["# Two", "Morning came."]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_writes_word_for_a_docx_name() {
        let dir = scratch("export");
        let mut app = App::new(None).unwrap();
        app.path = Some(dir.join("story.md"));
        app.textarea.insert_str("# Title\nOnce upon a time.");
        commands::execute(&mut app, commands::Command::ExportDocx);
        assert_eq!(app.prompt.input, dir.join("story.docx").to_string_lossy());
        app.handle_key(key(KeyCode::Enter));
        let bytes = std::fs::read(dir.join("story.docx")).unwrap();
        assert!(bytes.starts_with(b"PK"), "a zip package");
        commands::execute(&mut app, commands::Command::ExportPdf);
        app.handle_key(key(KeyCode::Enter));
        assert!(std::fs::read(dir.join("story.pdf")).unwrap().starts_with(b"%PDF"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn opening_another_text_forgets_markers() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("text");
        chord(&mut app, 'k', '1');
        app.new_document();
        assert_eq!(app.markers[1], None);
    }

    #[test]
    fn ctrl_ky_deletes_the_marked_block_and_ctrl_kv_pastes_it_back() {
        let mut app = marked_quick();
        app.textarea.move_cursor(CursorMove::End);
        chord(&mut app, 'k', 'y');
        assert_eq!(app.textarea.lines(), ["The brown fox."]);
        app.textarea.move_cursor(CursorMove::End);
        chord(&mut app, 'k', 'v');
        assert_eq!(app.textarea.lines(), ["The brown fox.quick "]);
    }

    #[test]
    fn bold_applies_to_the_marked_block() {
        let mut app = marked_quick();
        app.textarea.move_cursor(CursorMove::End);
        chord(&mut app, 'p', 'b');
        assert_eq!(app.textarea.lines(), ["The **quick** brown fox."]);
        // Arrows after formatting move the cursor rather than select.
        app.handle_key(key(KeyCode::Right));
        assert!(!app.textarea.is_selecting());
    }

    #[test]
    fn right_margin_is_stored_as_a_dot_command() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("text");
        assert_eq!(app.right_margin(), DEFAULT_RIGHT_MARGIN);
        chord(&mut app, 'o', 'r');
        app.prompt.input = "72".into();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.textarea.lines(), [".rm 72", "text"]);
        assert_eq!(app.right_margin(), 72);
        assert_eq!(app.textarea.cursor(), (1, 4), "cursor stays on its text");
        // Setting it again rewrites the same line.
        app.start_right_margin();
        app.prompt.input = "50".into();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.textarea.lines(), [".rm 50", "text"]);
    }

    #[test]
    fn pages_count_wrapped_rows_and_honor_page_breaks() {
        use crate::wrap::VisualRow;
        let lines: Vec<String> = vec!["a".into(), ".pa".into(), "b".into()];
        let row = |line| VisualRow { line, start: 0, end: 1, last: true };
        let pages = page_layout(&[row(0), row(1), row(2)], &lines);
        assert_eq!(pages, [(1, 1), (1, 2), (2, 1)]);
        // 60 printed rows overflow the 54-line page.
        let lines = vec!["x".to_string(); 60];
        let rows: Vec<VisualRow> = (0..60).map(row).collect();
        let pages = page_layout(&rows, &lines);
        assert_eq!(pages[53], (1, 54));
        assert_eq!(pages[54], (2, 1));
    }

    #[test]
    fn paste_goes_to_the_open_prompt() {
        let mut app = App::new(None).unwrap();
        app.start_find();
        app.handle_paste("Anne\r\nsecond line".into());
        assert_eq!(app.prompt.input, "Anne");
        assert_eq!(app.textarea.lines(), [""], "the document is untouched");
        app.mode = Mode::Editor;
        app.handle_paste("a\r\nb".into());
        assert_eq!(app.textarea.lines(), ["a", "b"]);
    }

    #[test]
    fn manuscript_setup_adds_frontmatter_once() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Once upon a time.");
        commands::execute(&mut app, commands::Command::ManuscriptTemplate);
        let text = app.textarea.lines().join("\n");
        assert!(text.starts_with("---\nformat: manuscript\ntitle: Untitled"), "{text}");
        assert!(text.ends_with("---\n\nOnce upon a time."), "{text}");
        assert!(app.textarea.lines()[app.textarea.cursor().0].starts_with("title: "));
        app.start_export_pdf();
        assert_eq!(app.prompt.label, "Export manuscript PDF as:");
        // Running it again changes nothing.
        app.mode = Mode::Editor;
        commands::execute(&mut app, commands::Command::ManuscriptTemplate);
        assert_eq!(app.textarea.lines().join("\n"), text);
    }

    #[test]
    fn manuscript_setup_keeps_existing_frontmatter_keys() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("---\ntitle: The Red House\nfont: Courier\n---\nBody");
        app.insert_manuscript_template();
        let lines = app.textarea.lines();
        assert_eq!(lines.iter().filter(|l| l.starts_with("title:")).count(), 1);
        assert!(lines.contains(&"title: The Red House".to_string()));
        assert!(lines.contains(&"format: manuscript".to_string()));
        assert!(lines.contains(&"font: Courier".to_string()));
        assert_eq!(lines.iter().filter(|l| l.trim() == "---").count(), 2);
    }

    #[test]
    fn not_implemented_menu_item_beeps_with_status() {
        let mut app = App::new(None).unwrap();
        app.not_implemented("Printing");
        assert!(
            app.status_msg.as_deref().unwrap().contains("Printing"),
            "status: {:?}",
            app.status_msg
        );
    }
}
