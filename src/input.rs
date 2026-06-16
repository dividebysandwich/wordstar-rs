//! Backend-agnostic key/mouse event vocabulary.
//!
//! The editor was written against `crossterm`'s event types. `crossterm` does
//! not compile to `wasm32-unknown-unknown`, so on the browser target this module
//! provides hand-written types that are *API-compatible* with the subset of
//! `crossterm::event` the app actually uses — same names, variants, fields and
//! constructors — letting `app.rs`, `keymap.rs` and `ui` stay verbatim. On
//! native targets the types are simply re-exported from crossterm, so behaviour
//! is identical to before.
//!
//! [`key_to_input`] converts a [`KeyEvent`] into a `ratatui_textarea::Input` on
//! both targets, replacing the textarea's crossterm-only `From` impl.

#[cfg(not(target_arch = "wasm32"))]
pub use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

#[cfg(target_arch = "wasm32")]
pub use wasm_events::*;

use ratatui_textarea::{Input, Key};

/// Translate a key event into the textarea widget's input representation. Mirrors
/// `ratatui_textarea`'s own crossterm conversion, but works on every target.
// Native `KeyCode` (crossterm) has more variants than the browser shim, so the
// catch-all arm is live there but unreachable on wasm.
#[cfg_attr(target_arch = "wasm32", allow(unreachable_patterns))]
pub fn key_to_input(key: &KeyEvent) -> Input {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let editor_key = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Enter => Key::Enter,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Tab => Key::Tab,
        KeyCode::Delete => Key::Delete,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Esc => Key::Esc,
        KeyCode::F(n) => Key::F(n),
        _ => Key::Null,
    };
    Input {
        key: editor_key,
        ctrl,
        alt,
        shift,
    }
}

/// Browser-target event types mirroring the crossterm API surface used by the
/// app, plus conversions from Ratzilla's web events.
#[cfg(target_arch = "wasm32")]
mod wasm_events {
    /// Keyboard modifier flags. Bit-compatible subset of `crossterm`'s type.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyModifiers(u8);

    impl KeyModifiers {
        pub const NONE: KeyModifiers = KeyModifiers(0);
        pub const SHIFT: KeyModifiers = KeyModifiers(1 << 0);
        pub const CONTROL: KeyModifiers = KeyModifiers(1 << 1);
        pub const ALT: KeyModifiers = KeyModifiers(1 << 2);

        pub fn contains(&self, other: KeyModifiers) -> bool {
            (self.0 & other.0) == other.0
        }
    }

    impl std::ops::BitOr for KeyModifiers {
        type Output = KeyModifiers;
        fn bitor(self, rhs: KeyModifiers) -> KeyModifiers {
            KeyModifiers(self.0 | rhs.0)
        }
    }

    impl std::ops::BitOrAssign for KeyModifiers {
        fn bitor_assign(&mut self, rhs: KeyModifiers) {
            self.0 |= rhs.0;
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyCode {
        Backspace,
        Enter,
        Left,
        Right,
        Up,
        Down,
        Home,
        End,
        PageUp,
        PageDown,
        Tab,
        Delete,
        Esc,
        Char(char),
        F(u8),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyEventKind {
        Press,
        Repeat,
        Release,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyEvent {
        pub code: KeyCode,
        pub modifiers: KeyModifiers,
        pub kind: KeyEventKind,
    }

    impl KeyEvent {
        pub fn new(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
            KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MouseButton {
        Left,
        Right,
        Middle,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum MouseEventKind {
        Down(MouseButton),
        Up(MouseButton),
        Drag(MouseButton),
        Moved,
        ScrollDown,
        ScrollUp,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MouseEvent {
        pub kind: MouseEventKind,
        pub column: u16,
        pub row: u16,
        pub modifiers: KeyModifiers,
    }

    fn modifiers_from(ctrl: bool, alt: bool, shift: bool) -> KeyModifiers {
        let mut m = KeyModifiers::NONE;
        if ctrl {
            m |= KeyModifiers::CONTROL;
        }
        if alt {
            m |= KeyModifiers::ALT;
        }
        if shift {
            m |= KeyModifiers::SHIFT;
        }
        m
    }

    impl From<ratzilla::event::KeyCode> for KeyCode {
        fn from(code: ratzilla::event::KeyCode) -> KeyCode {
            use ratzilla::event::KeyCode as R;
            match code {
                R::Char(c) => KeyCode::Char(c),
                R::F(n) => KeyCode::F(n),
                R::Backspace => KeyCode::Backspace,
                R::Enter => KeyCode::Enter,
                R::Left => KeyCode::Left,
                R::Right => KeyCode::Right,
                R::Up => KeyCode::Up,
                R::Down => KeyCode::Down,
                R::Tab => KeyCode::Tab,
                R::Delete => KeyCode::Delete,
                R::Home => KeyCode::Home,
                R::End => KeyCode::End,
                R::PageUp => KeyCode::PageUp,
                R::PageDown => KeyCode::PageDown,
                R::Esc => KeyCode::Esc,
                // Anything the editor has no mapping for becomes an inert key.
                R::Unidentified => KeyCode::Char('\0'),
            }
        }
    }

    impl From<ratzilla::event::KeyEvent> for KeyEvent {
        fn from(ev: ratzilla::event::KeyEvent) -> KeyEvent {
            KeyEvent {
                code: ev.code.into(),
                modifiers: modifiers_from(ev.ctrl, ev.alt, ev.shift),
                kind: KeyEventKind::Press,
            }
        }
    }

    impl From<ratzilla::event::MouseButton> for MouseButton {
        fn from(b: ratzilla::event::MouseButton) -> MouseButton {
            use ratzilla::event::MouseButton as R;
            match b {
                R::Right => MouseButton::Right,
                R::Middle => MouseButton::Middle,
                _ => MouseButton::Left,
            }
        }
    }

    /// Convert a Ratzilla mouse event. Returns `None` for kinds the editor does
    /// not act on (enter/exit/unidentified), so callers can simply skip them.
    pub fn mouse_event_from(ev: ratzilla::event::MouseEvent) -> Option<MouseEvent> {
        use ratzilla::event::MouseEventKind as R;
        let kind = match ev.kind {
            R::ButtonDown(b) => MouseEventKind::Down(b.into()),
            R::ButtonUp(b) => MouseEventKind::Up(b.into()),
            R::Moved => MouseEventKind::Moved,
            // Ratzilla reports synthesized clicks too; the editor already derives
            // clicks from button-down, so ignore those to avoid double handling.
            _ => return None,
        };
        Some(MouseEvent {
            kind,
            column: ev.col,
            row: ev.row,
            modifiers: modifiers_from(ev.ctrl, ev.alt, ev.shift),
        })
    }
}
