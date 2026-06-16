//! wordstar-rs — a faithful DOS WordStar 7 clone.
//!
//! Built on `ratatui` + `ratatui-textarea`. Edits Markdown and reads original
//! WordStar binary files. Runs both as a native terminal application (see
//! `main.rs`) and, compiled to WebAssembly, in the browser via Ratzilla (see
//! the `wasm` module). Platform-specific concerns (file I/O, time, the input
//! event vocabulary) are funnelled through the [`input`] and [`platform`]
//! modules so the bulk of the editor is shared verbatim across both targets.

pub mod app;
pub mod attributes;
pub mod commands;
pub mod gfx;
pub mod help;
pub mod input;
pub mod keymap;
pub mod menu;
pub mod pdf;
pub mod platform;
pub mod preview;
pub mod theme;
pub mod ui;
pub mod wordstar;
pub mod wrap;

// The in-app filesystem browser only exists on native targets; the browser
// build opens files through the host's file picker instead.
#[cfg(not(target_arch = "wasm32"))]
pub mod browser;

#[cfg(target_arch = "wasm32")]
pub mod wasm;
