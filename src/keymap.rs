//! WordStar keyboard model: the multi-key "chord" state machine plus modern keys.
//!
//! WordStar drives nearly everything from Ctrl combinations, including two-step
//! chords that start with a prefix (`^K`, `^Q`, `^O`, `^P`). We intercept keys
//! here *before* handing anything unclaimed to the text widget, so the classic
//! command set and modern amenities (arrows, function keys) coexist.

use crate::input::{KeyCode, KeyEvent, KeyModifiers};

use crate::commands::Command;

/// Where the chord machine currently is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChordState {
    /// No prefix pending.
    #[default]
    Idle,
    /// `^K` block / file prefix pending.
    K,
    /// `^Q` quick-movement prefix pending.
    Q,
    /// `^O` onscreen-format prefix pending.
    O,
    /// `^P` print/format prefix pending.
    P,
}

/// The outcome of feeding one key to [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A fully resolved command to execute.
    Command(Command),
    /// A prefix was consumed; show this hint and wait for the next key.
    Pending(&'static str),
    /// Unrecognized chord; reset and ring the bell.
    Beep,
    /// Not a WordStar command — let the text widget handle it.
    PassThrough,
}

/// Resolve a key press against the current chord state, advancing the state.
pub fn resolve(state: &mut ChordState, key: KeyEvent) -> Resolution {
    match *state {
        ChordState::Idle => resolve_idle(state, key),
        ChordState::K => finish(state, resolve_k(key)),
        ChordState::Q => finish(state, resolve_q(key)),
        ChordState::O => finish(state, resolve_o(key)),
        ChordState::P => finish(state, resolve_p(key)),
    }
}

/// Reset the chord state and return the chord result.
fn finish(state: &mut ChordState, res: Resolution) -> Resolution {
    *state = ChordState::Idle;
    res
}

/// Lowercased letter of a `Char` key, ignoring the Ctrl modifier (so both
/// `^K S` and `^K ^S` resolve identically).
fn letter(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

fn ctrl(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
}

fn resolve_idle(state: &mut ChordState, key: KeyEvent) -> Resolution {
    use Command::*;

    // Escape cancels any partial state (already Idle here, but harmless).
    if key.code == KeyCode::Esc {
        return Resolution::PassThrough;
    }

    // Modern function keys (no modifier required).
    if let KeyCode::F(n) = key.code {
        return match n {
            1 => Resolution::Command(Help),
            2 => Resolution::Command(Save),
            3 => Resolution::Command(OpenBrowser),
            5 => Resolution::Command(TogglePreview),
            9 => Resolution::Command(Menu),
            10 => Resolution::Command(Quit),
            _ => Resolution::PassThrough,
        };
    }

    // Beyond this point we only handle Ctrl-letter combinations.
    if !ctrl(&key) {
        return Resolution::PassThrough;
    }
    let Some(c) = letter(&key) else {
        return Resolution::PassThrough;
    };

    match c {
        // Prefixes.
        'k' => {
            *state = ChordState::K;
            Resolution::Pending(
                "^K  Block & files:  S)ave  X)exit  Q)uit  P)df  R)ead file  ?)count  B/K/C/V/Y block",
            )
        }
        'q' => {
            *state = ChordState::Q;
            Resolution::Pending("^Q  Quick:  S/D line ends  R/C file ends  F)ind  A)replace")
        }
        'o' => {
            *state = ChordState::O;
            Resolution::Pending(
                "^O  Onscreen:  D)isplay markup  W)ord wrap  C)enter  J)ustify  L)eft  R)ight",
            )
        }
        'p' => {
            *state = ChordState::P;
            Resolution::Pending("^P  Format:  B)old  Y)italic  S)underline  X)strikeout")
        }
        // The movement "diamond".
        'e' => Resolution::Command(MoveUp),
        'x' => Resolution::Command(MoveDown),
        's' => Resolution::Command(MoveLeft),
        'd' => Resolution::Command(MoveRight),
        'a' => Resolution::Command(WordLeft),
        'f' => Resolution::Command(WordRight),
        'r' => Resolution::Command(PageUp),
        'c' => Resolution::Command(PageDown),
        'w' => Resolution::Command(ScrollUpLine),
        'z' => Resolution::Command(ScrollDownLine),
        // Deletion.
        'g' => Resolution::Command(DeleteChar),
        't' => Resolution::Command(DeleteWord),
        'y' => Resolution::Command(DeleteLine),
        'n' => Resolution::Command(InsertLine),
        // Misc.
        'v' => Resolution::Command(ToggleInsert),
        'u' => Resolution::Command(Undo),
        'l' => Resolution::Command(FindNext),
        'j' => Resolution::Command(Help),
        // Everything else (^H backspace, ^M enter, ^I tab, plain chars) → widget.
        _ => Resolution::PassThrough,
    }
}

fn resolve_k(key: KeyEvent) -> Resolution {
    use Command::*;
    match letter(&key) {
        Some('s') => Resolution::Command(Save),
        Some('d') => Resolution::Command(SaveResume),
        Some('x') => Resolution::Command(SaveExit),
        Some('q') => Resolution::Command(Quit),
        Some('p') => Resolution::Command(ExportPdf),
        Some('b') => Resolution::Command(BlockBegin),
        Some('k') => Resolution::Command(BlockEnd),
        Some('c') => Resolution::Command(BlockCopy),
        Some('v') => Resolution::Command(BlockMove),
        Some('y') => Resolution::Command(BlockDelete),
        Some('h') => Resolution::Command(BlockHide),
        Some('r') => Resolution::Command(InsertFile),
        Some('?') => Resolution::Command(WordCount),
        _ if key.code == KeyCode::Esc => Resolution::PassThrough,
        _ => Resolution::Beep,
    }
}

fn resolve_q(key: KeyEvent) -> Resolution {
    use Command::*;
    // ^Q Backspace / ^Q Del: delete to start of line.
    if matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
        return Resolution::Command(DeleteToLineStart);
    }
    match letter(&key) {
        Some('s') => Resolution::Command(LineStart),
        Some('d') => Resolution::Command(LineEnd),
        Some('r') => Resolution::Command(DocStart),
        Some('c') => Resolution::Command(DocEnd),
        Some('f') => Resolution::Command(Find),
        Some('a') => Resolution::Command(Replace),
        Some('y') => Resolution::Command(DeleteToLineEnd),
        _ if key.code == KeyCode::Esc => Resolution::PassThrough,
        _ => Resolution::Beep,
    }
}

fn resolve_o(key: KeyEvent) -> Resolution {
    use Command::*;
    match letter(&key) {
        Some('d') => Resolution::Command(ToggleMarkup),
        Some('w') => Resolution::Command(ToggleWrap),
        Some('c') => Resolution::Command(AlignCenter),
        Some('j') => Resolution::Command(AlignJustify),
        Some('l') => Resolution::Command(AlignLeft),
        Some('r') => Resolution::Command(AlignRight),
        _ if key.code == KeyCode::Esc => Resolution::PassThrough,
        _ => Resolution::Beep,
    }
}

fn resolve_p(key: KeyEvent) -> Resolution {
    use Command::*;
    match letter(&key) {
        Some('b') => Resolution::Command(InsertBold),
        Some('y') => Resolution::Command(InsertItalic),
        Some('s') => Resolution::Command(InsertUnderline),
        Some('x') => Resolution::Command(InsertStrike),
        _ if key.code == KeyCode::Esc => Resolution::PassThrough,
        _ => Resolution::Beep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn diamond_movement_resolves_directly() {
        let mut st = ChordState::Idle;
        assert_eq!(
            resolve(&mut st, ctrl_key('e')),
            Resolution::Command(Command::MoveUp)
        );
        assert_eq!(st, ChordState::Idle);
        assert_eq!(
            resolve(&mut st, ctrl_key('s')),
            Resolution::Command(Command::MoveLeft)
        );
    }

    #[test]
    fn ctrl_k_s_saves_via_two_step_chord() {
        let mut st = ChordState::Idle;
        assert!(matches!(
            resolve(&mut st, ctrl_key('k')),
            Resolution::Pending(_)
        ));
        assert_eq!(st, ChordState::K);
        // Second key without ctrl still resolves.
        assert_eq!(
            resolve(&mut st, plain('s')),
            Resolution::Command(Command::Save)
        );
        assert_eq!(st, ChordState::Idle);
    }

    #[test]
    fn ctrl_k_then_ctrl_s_also_saves() {
        let mut st = ChordState::Idle;
        resolve(&mut st, ctrl_key('k'));
        assert_eq!(
            resolve(&mut st, ctrl_key('s')),
            Resolution::Command(Command::Save)
        );
    }

    #[test]
    fn unknown_chord_beeps_and_resets() {
        let mut st = ChordState::Idle;
        resolve(&mut st, ctrl_key('k'));
        assert_eq!(resolve(&mut st, plain('z')), Resolution::Beep);
        assert_eq!(st, ChordState::Idle);
    }

    #[test]
    fn function_keys_map_to_commands() {
        let mut st = ChordState::Idle;
        let f2 = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        assert_eq!(resolve(&mut st, f2), Resolution::Command(Command::Save));
        let f10 = KeyEvent::new(KeyCode::F(10), KeyModifiers::NONE);
        assert_eq!(resolve(&mut st, f10), Resolution::Command(Command::Quit));
    }

    #[test]
    fn plain_text_passes_through() {
        let mut st = ChordState::Idle;
        assert_eq!(resolve(&mut st, plain('a')), Resolution::PassThrough);
    }
}
