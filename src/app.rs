//! Application state and the top-level input dispatcher.
//!
//! [`App`] is the single source of truth: the text widget, the open file, the
//! current [`Mode`], the chord state, and transient status. All mutation flows
//! through here or through [`commands::execute`](crate::commands::execute).

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
use std::path::{Path, PathBuf};

use std::cell::Cell;
// `RefCell` only wraps the native image-protocol caches.
#[cfg(not(target_arch = "wasm32"))]
use std::cell::RefCell;

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
}

/// Which kind of single-line prompt is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptKind {
    #[default]
    Find,
    Replace,
    SaveAs,
    Font,
    FontSize,
    ExportPdf,
    /// Insert another file's contents at the cursor (^KR).
    InsertFile,
    /// Jump to a page number.
    GoToPage,
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
    /// Overwrite an existing file with the exported PDF.
    OverwritePdf(PathBuf),
    /// Save As onto a file that already exists.
    OverwriteSave(PathBuf),
    /// Unsaved changes before the given action: save / discard / cancel.
    SaveBefore(AfterSave),
    /// Restore this autosaved recovery text in place of the loaded document.
    Recover(String),
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

/// State backing the [`Mode::Prompt`] overlay.
#[derive(Debug, Clone, Default)]
pub struct PromptState {
    pub kind: PromptKind,
    pub label: String,
    pub input: String,
    /// For replace: the search term captured in the first step.
    pub pending_find: Option<String>,
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
    /// Active confirmation modal (present when `mode == Confirm`).
    pub confirm: Option<ConfirmState>,
    /// Active information modal (present when `mode == Info`).
    pub info: Option<InfoState>,
    /// Active calculator dialog (present when `mode == Calculator`).
    pub calc: Option<CalcState>,
    /// Active header/footer dialog (present when `mode == Header`).
    pub header_dialog: Option<HeaderState>,
    /// Active file browser (present when `mode == Browser`). Native only — the
    /// browser build opens files through the host's file picker.
    #[cfg(not(target_arch = "wasm32"))]
    pub browser: Option<crate::browser::Browser>,
    /// Scroll offset for the preview overlay.
    pub preview_scroll: u16,
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
    pub editor_area: Cell<Rect>,
    pub menu_bar_area: Cell<Rect>,
    pub dropdown_area: Cell<Rect>,
    /// Geometry of the open submenu panel, for mouse hit-testing.
    pub sub_dropdown_area: Cell<Rect>,
    pub browser_list_area: Cell<Rect>,
    /// First visible visual row of the editor viewport, tracked across frames so
    /// the scrollbar thumb reflects the textarea's internal scroll position.
    pub scroll_top: Cell<usize>,
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
            confirm: None,
            info: None,
            calc: None,
            header_dialog: None,
            #[cfg(not(target_arch = "wasm32"))]
            browser: None,
            preview_scroll: 0,
            help_scroll: 0,
            menu: crate::menu::MenuState::default(),
            align: AlignChoice::Left,
            wrap: true,
            block_buffer: String::new(),
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
            menu_bar_area: Cell::new(Rect::ZERO),
            dropdown_area: Cell::new(Rect::ZERO),
            sub_dropdown_area: Cell::new(Rect::ZERO),
            browser_list_area: Cell::new(Rect::ZERO),
            scroll_top: Cell::new(0),
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

    /// Apply the WordStar look to the text widget (and give it a deep undo
    /// history; the widget's default remembers only 50 keystrokes).
    fn apply_editor_theme(&mut self) {
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

    /// Open the find prompt.
    pub fn start_find(&mut self) {
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::Find,
            label: "Find:".into(),
            input: String::new(),
            pending_find: None,
        };
    }

    /// Open the find-and-replace prompt (two steps).
    pub fn start_replace(&mut self) {
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::Replace,
            label: "Find:".into(),
            input: String::new(),
            pending_find: None,
        };
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
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::SaveAs,
            label: "Save as:".into(),
            input: current,
            pending_find: None,
        };
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
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::Font,
            label: "Font name:".into(),
            input: String::new(),
            pending_find: None,
        };
    }

    /// Open the font-size prompt.
    pub fn start_size_prompt(&mut self) {
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::FontSize,
            label: "Font size:".into(),
            input: String::new(),
            pending_find: None,
        };
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Editor;
                self.after_save = None;
                self.set_status("Cancelled.");
            }
            KeyCode::Enter => self.confirm_prompt(),
            KeyCode::Backspace => {
                self.prompt.input.pop();
            }
            KeyCode::Char(c) => self.prompt.input.push(c),
            _ => {}
        }
    }

    fn confirm_prompt(&mut self) {
        match self.prompt.kind {
            PromptKind::Find => {
                let query = self.prompt.input.clone();
                self.mode = Mode::Editor;
                self.run_search(&query);
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
                        action: ConfirmAction::OverwritePdf(path),
                    });
                    self.mode = Mode::Confirm;
                } else {
                    self.do_export_pdf(&path);
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
            PromptKind::GoToPage => {
                let raw = self.prompt.input.trim().to_string();
                self.mode = Mode::Editor;
                match raw.parse::<usize>() {
                    Ok(n) => self.goto_page(n),
                    Err(_) => self.set_status("Page must be a number."),
                }
            }
            PromptKind::Replace => {
                if self.prompt.pending_find.is_none() {
                    // First step done: capture the search term, ask for replacement.
                    let find = self.prompt.input.clone();
                    if find.is_empty() {
                        self.mode = Mode::Editor;
                        self.set_status("Replace cancelled.");
                        return;
                    }
                    self.prompt.pending_find = Some(find);
                    self.prompt.label = "Replace with:".into();
                    self.prompt.input.clear();
                } else {
                    let find = self.prompt.pending_find.take().unwrap();
                    let with = self.prompt.input.clone();
                    self.mode = Mode::Editor;
                    self.replace_all(&find, &with);
                }
            }
        }
    }

    /// Set the search pattern (literal) and jump to the first match.
    fn run_search(&mut self, query: &str) {
        if query.is_empty() {
            let _ = self.textarea.set_search_pattern("");
            self.set_status("Search cleared.");
            return;
        }
        let pattern = regex_escape(query);
        match self.textarea.set_search_pattern(&pattern) {
            Ok(()) => {
                if self.textarea.search_forward(false) {
                    self.set_status(format!("Found \"{query}\". ^L finds next."));
                } else {
                    self.set_status(format!("\"{query}\" not found."));
                }
            }
            Err(e) => self.set_status(format!("Bad search: {e}")),
        }
    }

    /// Repeat the most recent search forward.
    pub fn find_next(&mut self) {
        if self.textarea.search_pattern().is_none() {
            self.set_status("No active search. Use ^QF to find.");
            return;
        }
        if !self.textarea.search_forward(false) {
            self.set_status("No more matches.");
        }
    }

    /// Replace every occurrence of `find` with `with`.
    ///
    /// Only the span between the first and last change is rewritten, as a single
    /// selection edit, so the cursor, search and undo history survive and one
    /// `^U` restores the original text.
    fn replace_all(&mut self, find: &str, with: &str) {
        let text = self.textarea.lines().join("\n");
        let count = text.matches(find).count();
        if count == 0 {
            self.set_status(format!("\"{find}\" not found."));
            return;
        }
        let replaced = text.replace(find, with);
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
            if self.graphics {
                // Start an incremental render; the main loop drives it while the
                // loading modal shows progress. `None` means no fonts → text view.
                self.preview_job = crate::gfx::Job::new(&self.textarea.lines().join("\n"));
            }
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

    /// `^KB` — mark the start of a block (begin selecting at the cursor).
    pub fn block_begin(&mut self) {
        self.textarea.cancel_selection();
        self.textarea.start_selection();
        self.marking = true;
        self.set_status("Block start marked — move the cursor, then ^KC copy / ^KY cut.");
    }

    /// `^KK` — mark the end of the block (the selection runs start → cursor).
    pub fn block_end(&mut self) {
        match self.selected_text() {
            Some(text) => {
                let n = text.chars().count();
                self.set_status(format!(
                    "Block marked: {n} chars.  ^KC copy · ^KY cut · ^KH clear"
                ));
            }
            None => self.set_status("Mark the block start first with ^KB."),
        }
    }

    /// `^KC` — copy the marked block to the block clipboard.
    pub fn block_copy(&mut self) {
        match self.selected_text() {
            Some(text) => {
                let n = text.chars().count();
                self.block_buffer = text;
                self.clear_marking();
                self.set_status(format!(
                    "Copied {n} chars — move the cursor and press ^KV to paste."
                ));
            }
            None => self.set_status("No block marked. Press ^KB, then move the cursor."),
        }
    }

    /// `^KY` — cut the marked block to the clipboard and remove it.
    pub fn block_delete(&mut self) {
        match self.selected_text() {
            Some(text) => {
                let n = text.chars().count();
                self.block_buffer = text;
                self.edit(|t| t.cut());
                self.clear_marking();
                self.set_status(format!("Cut {n} chars — press ^KV to paste."));
            }
            None => self.set_status("No block marked. Press ^KB, then move the cursor."),
        }
    }

    /// `^KV` — paste the block clipboard at the cursor.
    pub fn block_move(&mut self) {
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

    /// `^KH` — clear the block markers (cancel the selection).
    pub fn block_hide(&mut self) {
        self.clear_marking();
        self.set_status("Block markers cleared.");
    }

    /// Stop marking and drop any active selection highlight.
    fn clear_marking(&mut self) {
        self.marking = false;
        self.textarea.cancel_selection();
    }

    /// The currently selected text, if any (used for block copy/cut).
    fn selected_text(&self) -> Option<String> {
        let ((sr, sc), (er, ec)) = self.textarea.selection_range()?;
        if (sr, sc) == (er, ec) {
            return None; // empty selection
        }
        let lines = self.textarea.lines();
        if sr == er {
            let line = &lines[sr];
            Some(line.chars().skip(sc).take(ec - sc).collect())
        } else {
            let mut out: String = lines[sr].chars().skip(sc).collect();
            out.push('\n');
            for line in &lines[sr + 1..er] {
                out.push_str(line);
                out.push('\n');
            }
            out.extend(lines[er].chars().take(ec));
            Some(out)
        }
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
        if self.textarea.is_selecting() {
            self.textarea.cut();
            let inner = self.textarea.yank_text();
            self.textarea.insert_str(format!("{open}{inner}{close}"));
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
        if self.textarea.is_selecting() {
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
        self.textarea.insert_newline();
        self.textarea.move_cursor(CursorMove::Up);
        self.modified = true;
    }

    // ------------------------------------------------------------------
    // Insert / utility commands
    // ------------------------------------------------------------------

    /// Open the "insert file" prompt (^KR).
    pub fn start_insert_file(&mut self) {
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::InsertFile,
            label: "Insert file:".into(),
            input: String::new(),
            pending_find: None,
        };
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
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::GoToPage,
            label: "Go to page:".into(),
            input: String::new(),
            pending_find: None,
        };
    }

    /// Jump the cursor to the first line of the given 1-based page (55 lines/page,
    /// matching the status-line page metric).
    fn goto_page(&mut self, page: usize) {
        let page = page.max(1);
        let target =
            ((page - 1) * LINES_PER_PAGE).min(self.textarea.lines().len().saturating_sub(1));
        self.textarea
            .move_cursor(CursorMove::Jump(target as u16, 0));
        self.set_status(format!("Page {page}."));
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

    /// Show document statistics in an info modal (^K?).
    pub fn show_word_count(&mut self) {
        let mut words = 0usize;
        let mut chars = 0usize;
        let mut paragraphs = 0usize;
        let mut in_para = false;
        let lines = self.textarea.lines();
        for line in lines {
            if crate::attributes::is_dot_command(line) {
                continue;
            }
            let text = crate::attributes::strip_inline_markers(line);
            let n = text.split_whitespace().count();
            words += n;
            chars += text.chars().count();
            if text.trim().is_empty() {
                in_para = false;
            } else if !in_para {
                paragraphs += 1;
                in_para = true;
            }
        }
        self.info = Some(InfoState {
            title: "Word Count".into(),
            lines: vec![
                format!("Words:       {words}"),
                format!("Characters:  {chars}"),
                format!("Lines:       {}", lines.len()),
                format!("Paragraphs:  {paragraphs}"),
            ],
        });
        self.mode = Mode::Info;
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
            Ok(()) => {
                crate::platform::clear_recovery(self.path.as_deref());
                crate::platform::clear_recovery(Some(path));
                self.autosave_hash = None;
                self.modified = false;
                self.set_status(format!("Saved {}", path.display()));
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
    pub fn start_export_pdf(&mut self) {
        let default = match &self.path {
            Some(p) => p.with_extension("pdf"),
            None => PathBuf::from("untitled.pdf"),
        };
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::ExportPdf,
            label: "Export PDF as:".into(),
            input: default.to_string_lossy().into_owned(),
            pending_find: None,
        };
    }

    /// Write the PDF to `path`, reporting success or failure on the status line.
    fn do_export_pdf(&mut self, path: &Path) {
        let title = self.file_name();
        let markdown = self.textarea.lines().join("\n");
        let bytes = crate::pdf::export(&markdown, &title);
        #[cfg(not(target_arch = "wasm32"))]
        match fs::write(path, bytes) {
            Ok(()) => self.set_status(format!("Exported {}", path.display())),
            Err(e) => self.set_status(format!("PDF export failed: {e}")),
        }
        #[cfg(target_arch = "wasm32")]
        {
            let name = file_download_name(path, "pdf");
            match crate::platform::download(&name, "application/pdf", &bytes) {
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
                    Some(ConfirmAction::OverwritePdf(path)) => self.do_export_pdf(&path),
                    Some(ConfirmAction::OverwriteSave(path)) => self.save_as(path),
                    Some(ConfirmAction::Recover(text)) => self.restore_recovered(&text),
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

    /// Insert pasted text.
    pub fn handle_paste(&mut self, text: String) {
        if self.textarea.insert_str(text) {
            self.modified = true;
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
            Mode::Prompt | Mode::Confirm | Mode::Header | Mode::Calculator => {} // keyboard-only
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
                if let Some((r, c)) = self.editor_doc_pos(me.column, me.row) {
                    let double = self.register_click(me.column, me.row);
                    self.textarea.cancel_selection();
                    self.textarea.move_cursor(CursorMove::Jump(r, c));
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
                    && let Some((r, c)) = self.editor_doc_pos(me.column, me.row)
                {
                    self.textarea.move_cursor(CursorMove::Jump(r, c));
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

    /// Map an editor-area click to a document `(row, col)`, using the cursor's
    /// known screen position to recover the scroll offset.
    fn editor_doc_pos(&self, mx: u16, my: u16) -> Option<(u16, u16)> {
        let area = self.editor_area.get();
        if area.width == 0
            || my < area.y
            || my >= area.y + area.height
            || mx < area.x
            || mx >= area.x + area.width
        {
            return None;
        }
        let wrow = (my - area.y) as usize;
        let wcol = (mx - area.x) as usize;
        let sc = self.textarea.screen_cursor();
        let dc = self.textarea.cursor();
        let top = dc.0.saturating_sub(sc.row);
        let left = dc.1.saturating_sub(sc.col);
        Some(((top + wrow) as u16, (left + wcol) as u16))
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

    /// Cursor metrics for the status line, in WordStar units.
    pub fn cursor_metrics(&self) -> CursorMetrics {
        let cursor = self.textarea.cursor();
        let line = cursor.0; // 0-based row
        let col = cursor.1; // 0-based column
        CursorMetrics {
            line: line + 1,
            column: col + 1,
            // Page: ~54 text lines per page (9" at 6 lines/inch).
            page: line / LINES_PER_PAGE + 1,
            // Vertical position: 0.5" top margin + 6 lines/inch.
            vertical_inches: 0.5 + line as f32 / 6.0,
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

/// Escape regex metacharacters so a user's search term is matched literally
/// (WordStar's find is literal by default).
fn regex_escape(s: &str) -> String {
    const SPECIAL: &[char] = &[
        '\\', '.', '+', '*', '?', '(', ')', '|', '[', ']', '{', '}', '^', '$', '#', '&', '-', '~',
    ];
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if SPECIAL.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
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
        assert!(body.contains("Words:       5"), "got:\n{body}");
        assert!(body.contains("Paragraphs:  2"), "got:\n{body}");
        // Any key dismisses it.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Editor);
        assert!(app.info.is_none());
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

    #[test]
    fn replace_all_is_undone_by_a_single_undo() {
        let mut app = App::new(None).unwrap();
        app.textarea.insert_str("Ann met Anne.\nAnn left.");
        app.textarea.move_cursor(CursorMove::Jump(1, 3));
        app.replace_all("Ann", "Beth");
        assert_eq!(app.textarea.lines(), ["Beth met Bethe.", "Beth left."]);
        assert_eq!(app.textarea.cursor(), (1, 3), "cursor stays put");
        commands::execute(&mut app, commands::Command::Undo);
        assert_eq!(app.textarea.lines(), ["Ann met Anne.", "Ann left."]);
    }

    #[test]
    fn replace_with_nothing_is_undone_by_a_single_undo() {
        let mut app = App::new(None).unwrap();
        type_str(&mut app, "ab");
        app.replace_all("b", "");
        assert_eq!(app.textarea.lines(), ["a"]);
        commands::execute(&mut app, commands::Command::Undo);
        assert_eq!(app.textarea.lines(), ["ab"], "one undo, not two");
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
