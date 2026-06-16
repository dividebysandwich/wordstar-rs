//! High-level editor commands and the single dispatch point that executes them.
//!
//! [`Command`]s are produced by the [`keymap`](crate::keymap) chord machine (or by
//! menu selection later) and applied to the [`App`] here. Keeping all mutation in
//! one place makes the command set easy to test and extend.

use ratatui_textarea::{CursorMove, Scrolling};

use crate::app::{AlignChoice, App};

/// A resolved editor action, independent of which key produced it.
///
/// Some variants are wired to keys/menus in later phases; the set is allowed to
/// run ahead of the bindings.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    // --- file / app ---
    /// Save to the current path (markdown).
    Save,
    /// Save and keep editing (WordStar ^KD); same as [`Command::Save`] for now.
    SaveResume,
    /// Save then quit (WordStar ^KX).
    SaveExit,
    /// Quit, abandoning changes (WordStar ^KQ / F10).
    Quit,
    /// Open the help overlay (F1 / ^J).
    Help,
    /// Open the menu bar (F9).
    Menu,
    /// Open the file browser (F3).
    OpenBrowser,
    /// Save under a new name.
    SaveAs,
    /// Toggle the formatted preview (F5).
    TogglePreview,
    /// Toggle word wrap (^OW).
    ToggleWrap,
    /// Start a new, empty document.
    New,
    /// Export the document to a formatted PDF.
    ExportPdf,
    /// Show the About message.
    About,

    // --- cursor movement (the WordStar "diamond" and friends) ---
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
    /// Scroll the view up/down one line without moving the cursor (^W / ^Z).
    ScrollUpLine,
    ScrollDownLine,

    // --- deletion ---
    DeleteChar,
    DeleteCharBack,
    DeleteWord,
    DeleteLine,
    DeleteToLineEnd,
    DeleteToLineStart,
    /// Insert a hard return at the cursor, leaving the cursor in place (^N).
    InsertLine,

    // --- editing modes ---
    ToggleInsert,
    Undo,
    Redo,

    // --- search ---
    Find,
    Replace,
    FindNext,

    // --- blocks (mapped onto selection in Phase 2) ---
    BlockBegin,
    BlockEnd,
    BlockCopy,
    BlockMove,
    BlockDelete,
    BlockHide,

    // --- inline formatting (markdown markers) ---
    InsertBold,
    InsertItalic,
    InsertUnderline,
    /// Strikethrough (`~~…~~`), ^PX.
    InsertStrike,
    /// Toggle the "hide formatting markup" reading view (^OD).
    ToggleMarkup,
    /// Prompt for a font name and apply it to the selection.
    FontPrompt,
    /// Prompt for a font size and apply it to the selection.
    SizePrompt,
    /// Strip inline formatting markers from the selection.
    ClearFormat,

    // --- paragraph alignment ---
    AlignLeft,
    AlignCenter,
    AlignRight,
    AlignJustify,
}

/// Apply a [`Command`] to the application state.
pub fn execute(app: &mut App, cmd: Command) {
    use Command::*;
    match cmd {
        Save => app.save(),
        SaveResume => app.save(),
        SaveExit => {
            app.save();
            app.should_quit = true;
        }
        Quit => app.request_quit(),
        Help => app.toggle_help(),
        Menu => app.open_menu(),
        OpenBrowser => app.open_browser(),
        SaveAs => app.start_save_as(),
        TogglePreview => app.toggle_preview(),
        ToggleWrap => app.toggle_wrap(),
        New => app.new_document(),
        ExportPdf => app.start_export_pdf(),
        About => app.set_status("wordstar-rs — a WordStar 7 clone in Rust (ratatui)."),

        MoveUp => app.textarea.move_cursor(CursorMove::Up),
        MoveDown => app.textarea.move_cursor(CursorMove::Down),
        MoveLeft => app.textarea.move_cursor(CursorMove::Back),
        MoveRight => app.textarea.move_cursor(CursorMove::Forward),
        WordLeft => app.textarea.move_cursor(CursorMove::WordBack),
        WordRight => app.textarea.move_cursor(CursorMove::WordForward),
        LineStart => app.textarea.move_cursor(CursorMove::Head),
        LineEnd => app.textarea.move_cursor(CursorMove::End),
        PageUp => {
            let h = app.viewport_height();
            app.textarea.scroll(Scrolling::PageUp);
            app.scroll_viewport(-(h as isize));
        }
        PageDown => {
            let h = app.viewport_height();
            app.textarea.scroll(Scrolling::PageDown);
            app.scroll_viewport(h as isize);
        }
        DocStart => app.textarea.move_cursor(CursorMove::Top),
        DocEnd => app.textarea.move_cursor(CursorMove::Bottom),
        ScrollUpLine => {
            app.textarea.scroll((-1, 0));
            app.scroll_viewport(-1);
        }
        ScrollDownLine => {
            app.textarea.scroll((1, 0));
            app.scroll_viewport(1);
        }

        DeleteChar => app.edit(|t| t.delete_next_char()),
        DeleteCharBack => app.edit(|t| t.delete_char()),
        DeleteWord => app.edit(|t| t.delete_next_word()),
        DeleteLine => app.edit(|t| {
            t.move_cursor(CursorMove::Head);
            t.delete_line_by_end() || t.delete_newline()
        }),
        DeleteToLineEnd => app.edit(|t| t.delete_line_by_end()),
        DeleteToLineStart => app.edit(|t| t.delete_line_by_head()),
        InsertLine => app.insert_line(),

        ToggleInsert => app.toggle_insert(),
        Undo => app.edit(|t| t.undo()),
        Redo => app.edit(|t| t.redo()),

        Find => app.start_find(),
        Replace => app.start_replace(),
        FindNext => app.find_next(),

        BlockBegin => app.block_begin(),
        BlockEnd => app.block_end(),
        BlockCopy => app.block_copy(),
        BlockMove => app.block_move(),
        BlockDelete => app.block_delete(),
        BlockHide => app.block_hide(),

        InsertBold => app.apply_format("**", "**", "Bold"),
        InsertItalic => app.apply_format("*", "*", "Italic"),
        InsertUnderline => app.apply_format("[", "]{.underline}", "Underline"),
        InsertStrike => app.apply_format("~~", "~~", "Strikethrough"),
        ToggleMarkup => app.toggle_markup(),
        FontPrompt => app.start_font_prompt(),
        SizePrompt => app.start_size_prompt(),
        ClearFormat => app.clear_formatting(),

        AlignLeft => app.set_align(AlignChoice::Left),
        AlignCenter => app.set_align(AlignChoice::Center),
        AlignRight => app.set_align(AlignChoice::Right),
        AlignJustify => app.set_align(AlignChoice::Justify),
    }
}
