//! Application state and the top-level input dispatcher.
//!
//! [`App`] is the single source of truth: the text widget, the open file, the
//! current [`Mode`], the chord state, and transient status. All mutation flows
//! through here or through [`commands::execute`](crate::commands::execute).

use std::fs;
use std::path::{Path, PathBuf};

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Alignment, Rect};
use ratatui_textarea::{CursorMove, Input, Key, TextArea};

use crate::attributes::RunAttributes;
use crate::commands;
use crate::keymap::{self, ChordState, Resolution};
use crate::theme;

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
}

/// A pending action awaiting yes/no confirmation in [`Mode::Confirm`].
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Overwrite an existing file with the exported PDF.
    OverwritePdf(PathBuf),
    /// Quitting with unsaved changes: save / discard / cancel.
    SaveBeforeQuit,
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
    /// Active file browser (present when `mode == Browser`).
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
    /// Terminal graphics picker (set at startup when a TTY is available).
    pub picker: Option<ratatui_image::picker::Picker>,
    /// Whether a real graphics protocol (Kitty/iTerm2/Sixel) is available.
    pub graphics: bool,
    /// In-progress incremental rasterization job (graphical preview loading).
    pub preview_job: Option<crate::gfx::Job>,
    /// Rasterized pages of the document (graphical preview).
    pub preview_pages: Vec<image::RgbaImage>,
    /// Per-page encoded protocols, built lazily and reused (the zoom == 1 view).
    pub preview_page_protocols: RefCell<Vec<Option<ratatui_image::protocol::StatefulProtocol>>>,
    /// Encoded protocol for the current zoomed/panned crop (zoom > 1).
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
    last_click: Option<(Instant, u16, u16)>,
    /// Screen geometry recorded during the last render, for mouse hit-testing.
    pub editor_area: Cell<Rect>,
    pub menu_bar_area: Cell<Rect>,
    pub dropdown_area: Cell<Rect>,
    pub browser_list_area: Cell<Rect>,
}

impl App {
    /// Build the app, optionally loading a file from `path`.
    pub fn new(path: Option<String>) -> Result<Self> {
        let path = path.map(PathBuf::from);
        let mut imported = false;
        let textarea = match &path {
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
            browser: None,
            preview_scroll: 0,
            help_scroll: 0,
            menu: crate::menu::MenuState::default(),
            align: AlignChoice::Left,
            wrap: true,
            block_buffer: String::new(),
            marking: false,
            picker: None,
            graphics: false,
            preview_job: None,
            preview_pages: Vec::new(),
            preview_page_protocols: RefCell::new(Vec::new()),
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
            browser_list_area: Cell::new(Rect::ZERO),
        };
        app.apply_editor_theme();
        if imported {
            app.set_status("Imported WordStar file — saving will write a new .md file.");
        }
        Ok(app)
    }

    /// Apply the WordStar look to the text widget.
    fn apply_editor_theme(&mut self) {
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
        }
    }

    fn handle_editor_key(&mut self, key: KeyEvent) {
        // Any key other than a repeated quit clears the quit warning and status.
        self.status_msg = None;

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
        let mut input: Input = key.into();

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

        if self.textarea.input(input) {
            self.modified = true;
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
        match key.code {
            KeyCode::Esc | KeyCode::F(9) => self.mode = Mode::Editor,
            KeyCode::Left => self.menu.prev_menu(),
            KeyCode::Right => self.menu.next_menu(),
            KeyCode::Up => self.menu.prev_item(),
            KeyCode::Down => self.menu.next_item(),
            KeyCode::Enter => {
                if let Some(cmd) = self.menu.selected_command() {
                    self.mode = Mode::Editor;
                    commands::execute(self, cmd);
                }
            }
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

    /// Open the save-as prompt.
    pub fn start_save_as(&mut self) {
        self.mode = Mode::Prompt;
        self.prompt = PromptState {
            kind: PromptKind::SaveAs,
            label: "Save as:".into(),
            input: String::new(),
            pending_find: None,
        };
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
                    self.set_status("Save cancelled (no name).");
                } else {
                    self.path = Some(PathBuf::from(name));
                    self.save();
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

    /// Replace every occurrence of `find` with `with`, rebuilding the buffer.
    ///
    /// Note: this resets undo history (acceptable for the MVP).
    fn replace_all(&mut self, find: &str, with: &str) {
        let text = self.textarea.lines().join("\n");
        let count = text.matches(find).count();
        if count == 0 {
            self.set_status(format!("\"{find}\" not found."));
            return;
        }
        let replaced = text.replace(find, with);
        let lines: Vec<String> = if replaced.is_empty() {
            vec![String::new()]
        } else {
            replaced.split('\n').map(str::to_owned).collect()
        };
        self.textarea = TextArea::new(lines);
        self.apply_editor_theme();
        self.modified = true;
        self.set_status(format!("Replaced {count} occurrence(s)."));
    }

    // ------------------------------------------------------------------
    // File browser
    // ------------------------------------------------------------------

    /// Open the file browser at the document's directory (or the cwd / home).
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
                    self.load_file(path);
                }
            },
            _ => {}
        }
    }

    /// Load a file into the editor, replacing the current buffer.
    fn load_file(&mut self, path: PathBuf) {
        match crate::wordstar::load(&path) {
            Ok(loaded) => {
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
            Err(e) => self.set_status(format!("Open failed: {e}")),
        }
    }

    // ------------------------------------------------------------------
    // Preview / Help overlays
    // ------------------------------------------------------------------

    /// Record the terminal graphics picker detected at startup.
    pub fn set_picker(&mut self, picker: ratatui_image::picker::Picker) {
        use ratatui_image::picker::ProtocolType;
        self.graphics = picker.protocol_type() != ProtocolType::Halfblocks;
        self.picker = Some(picker);
    }

    /// Toggle the markdown preview. Uses the in-terminal graphical preview (one
    /// page at a time, zoomable/scrollable) when the terminal supports a
    /// graphics protocol, otherwise the text preview.
    pub fn toggle_preview(&mut self) {
        if self.mode == Mode::Preview {
            self.close_preview();
        } else {
            self.preview_scroll = 0;
            self.preview_page = 0;
            self.preview_zoom = 1.0;
            self.preview_off = (0.0, 0.0);
            self.preview_pages.clear();
            self.preview_page_protocols.borrow_mut().clear();
            *self.preview_zoom_protocol.borrow_mut() = None;
            self.preview_view_key.set(None);
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
        self.preview_page_protocols.borrow_mut().clear();
        *self.preview_zoom_protocol.borrow_mut() = None;
        self.preview_view_key.set(None);
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
        job.step(Duration::from_millis(33));
        if job.is_done() {
            self.preview_pages = job.finish();
            *self.preview_page_protocols.borrow_mut() =
                (0..self.preview_pages.len()).map(|_| None).collect();
        } else {
            self.preview_job = Some(job);
        }
    }

    /// Ensure the protocol needed for the current view exists (building/encoding
    /// it only when missing or when the zoomed view actually changed). Called
    /// from the renderer, so navigation itself does no image work.
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

    /// Save the buffer to its path as markdown.
    pub fn save(&mut self) {
        let Some(path) = self.path.clone() else {
            self.set_status("No file name yet — Save As arrives in Phase 2.");
            return;
        };
        let mut content = self.textarea.lines().join("\n");
        content.push('\n');
        match fs::write(&path, content) {
            Ok(()) => {
                self.modified = false;
                self.set_status(format!("Saved {}", path.display()));
            }
            Err(e) => self.set_status(format!("Save failed: {e}")),
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
        match fs::write(path, bytes) {
            Ok(()) => self.set_status(format!("Exported {}", path.display())),
            Err(e) => self.set_status(format!("PDF export failed: {e}")),
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        // The quit prompt is three-way (Save / Don't save / Cancel).
        if matches!(
            self.confirm.as_ref().map(|c| &c.action),
            Some(ConfirmAction::SaveBeforeQuit)
        ) {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.confirm = None;
                    self.mode = Mode::Editor;
                    self.save();
                    // Quit only if the save actually succeeded (e.g. an untitled
                    // document still needs a name; then we stay so nothing is lost).
                    if !self.modified {
                        self.should_quit = true;
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.confirm = None;
                    self.should_quit = true; // discard changes and quit
                }
                KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
                    self.confirm = None;
                    self.mode = Mode::Editor;
                    self.set_status("Quit cancelled.");
                }
                _ => {}
            }
            return;
        }

        // Generic yes/no confirmations (e.g. PDF overwrite).
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                let action = self.confirm.take().map(|c| c.action);
                self.mode = Mode::Editor;
                if let Some(ConfirmAction::OverwritePdf(path)) = action {
                    self.do_export_pdf(&path);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.confirm = None;
                self.mode = Mode::Editor;
                self.set_status("Cancelled.");
            }
            _ => {}
        }
    }

    /// Quit, prompting to save first if there are unsaved changes.
    pub fn request_quit(&mut self) {
        if self.modified {
            self.confirm = Some(ConfirmState {
                message: format!("Save changes to {} before quitting?", self.file_name()),
                action: ConfirmAction::SaveBeforeQuit,
            });
            self.mode = Mode::Confirm;
        } else {
            self.should_quit = true;
        }
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
            Mode::Prompt | Mode::Confirm => {} // keyboard-only
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
            MouseEventKind::ScrollDown => self.textarea.scroll((1, 0)),
            MouseEventKind::ScrollUp => self.textarea.scroll((-1, 0)),
            _ => {}
        }
    }

    fn mouse_menu(&mut self, me: MouseEvent) {
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
        let dd = self.dropdown_area.get();
        let inside = dd.height > 1
            && me.row > dd.y
            && me.row < dd.y + dd.height - 1
            && me.column > dd.x
            && me.column < dd.x + dd.width - 1;
        if inside {
            let item = (me.row - dd.y - 1) as usize;
            let count = crate::menu::MENUS[self.menu.menu].items.len();
            if item < count {
                self.menu.item = item;
                if let Some(cmd) = self.menu.selected_command() {
                    self.mode = Mode::Editor;
                    commands::execute(self, cmd);
                }
            }
            return;
        }
        // Clicked outside the menu: close it.
        self.mode = Mode::Editor;
    }

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
                        self.load_file(path);
                    }
                }
            }
            _ => {}
        }
    }

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
        let now = Instant::now();
        let double = matches!(
            self.last_click,
            Some((t, px, py)) if px == x && py == y && now.duration_since(t) < Duration::from_millis(400)
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
            page: line / 54 + 1,
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
}
