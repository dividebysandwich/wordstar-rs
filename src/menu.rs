//! The pull-down menu model and its navigation state.
//!
//! The menu tree is static data; selecting an item yields a [`Command`] that the
//! usual dispatcher executes. Titles mirror WordStar's bar (File / Edit / View /
//! Insert / Style / Layout / Utilities / Help).

use crate::commands::Command;

/// One entry in a drop-down menu.
pub struct MenuItem {
    pub label: &'static str,
    /// Shortcut hint shown right-aligned (empty for none).
    pub shortcut: &'static str,
    pub action: MenuAction,
}

/// What activating an item does.
pub enum MenuAction {
    /// Run a command and close the menu.
    Run(Command),
    /// A non-selectable divider.
    Separator,
}

/// A titled drop-down menu.
pub struct Menu {
    pub title: &'static str,
    pub items: &'static [MenuItem],
}

const fn item(label: &'static str, shortcut: &'static str, cmd: Command) -> MenuItem {
    MenuItem {
        label,
        shortcut,
        action: MenuAction::Run(cmd),
    }
}

const SEP: MenuItem = MenuItem {
    label: "",
    shortcut: "",
    action: MenuAction::Separator,
};

static FILE: &[MenuItem] = &[
    item("New", "", Command::New),
    item("Open...", "F3", Command::OpenBrowser),
    SEP,
    item("Save", "^KS / F2", Command::Save),
    item("Save As...", "", Command::SaveAs),
    item("Save & Exit", "^KX", Command::SaveExit),
    SEP,
    item("Exit", "^KQ / F10", Command::Quit),
];

static EDIT: &[MenuItem] = &[
    item("Undo", "^U", Command::Undo),
    SEP,
    item("Mark Block Begin", "^KB", Command::BlockBegin),
    item("Mark Block End", "^KK", Command::BlockEnd),
    item("Copy Block", "^KC", Command::BlockCopy),
    item("Paste Block", "^KV", Command::BlockMove),
    item("Delete Block", "^KY", Command::BlockDelete),
];

static VIEW: &[MenuItem] = &[
    item("Preview", "F5", Command::TogglePreview),
    SEP,
    item("Insert / Overtype", "^V", Command::ToggleInsert),
];

static INSERT: &[MenuItem] = &[
    item("Bold", "^PB", Command::InsertBold),
    item("Italic", "^PY", Command::InsertItalic),
    item("Underline", "^PS", Command::InsertUnderline),
];

static STYLE: &[MenuItem] = &[
    item("Font...", "", Command::FontPrompt),
    item("Font Size...", "", Command::SizePrompt),
    SEP,
    item("Clear Formatting", "", Command::ClearFormat),
];

static LAYOUT: &[MenuItem] = &[
    item("Align Left", "", Command::AlignLeft),
    item("Align Center", "", Command::AlignCenter),
    item("Align Right", "", Command::AlignRight),
    item("Justify", "", Command::AlignJustify),
];

static UTILITIES: &[MenuItem] = &[
    item("Find...", "^QF", Command::Find),
    item("Replace...", "^QA", Command::Replace),
    item("Find Next", "^L", Command::FindNext),
];

static HELP: &[MenuItem] = &[
    item("Help Topics", "F1", Command::Help),
    item("About", "", Command::About),
];

/// The full menu bar, left to right.
pub static MENUS: &[Menu] = &[
    Menu {
        title: "File",
        items: FILE,
    },
    Menu {
        title: "Edit",
        items: EDIT,
    },
    Menu {
        title: "View",
        items: VIEW,
    },
    Menu {
        title: "Insert",
        items: INSERT,
    },
    Menu {
        title: "Style",
        items: STYLE,
    },
    Menu {
        title: "Layout",
        items: LAYOUT,
    },
    Menu {
        title: "Utilities",
        items: UTILITIES,
    },
    Menu {
        title: "Help",
        items: HELP,
    },
];

/// Index of the Help menu (rendered right-aligned in the bar).
pub const HELP_INDEX: usize = MENUS.len() - 1;

/// Navigation state for an open menu.
#[derive(Debug, Clone, Copy, Default)]
pub struct MenuState {
    /// Selected top-level menu.
    pub menu: usize,
    /// Selected item within the open menu.
    pub item: usize,
}

impl MenuState {
    fn items(&self) -> &'static [MenuItem] {
        MENUS[self.menu].items
    }

    /// Move to the previous menu (wraps), selecting its first real item.
    pub fn prev_menu(&mut self) {
        self.menu = (self.menu + MENUS.len() - 1) % MENUS.len();
        self.item = self.first_selectable();
    }

    /// Move to the next menu (wraps).
    pub fn next_menu(&mut self) {
        self.menu = (self.menu + 1) % MENUS.len();
        self.item = self.first_selectable();
    }

    /// Move the highlight up, skipping separators (wraps within the menu).
    pub fn prev_item(&mut self) {
        let n = self.items().len();
        for _ in 0..n {
            self.item = (self.item + n - 1) % n;
            if !self.is_separator(self.item) {
                break;
            }
        }
    }

    /// Move the highlight down, skipping separators (wraps within the menu).
    pub fn next_item(&mut self) {
        let n = self.items().len();
        for _ in 0..n {
            self.item = (self.item + 1) % n;
            if !self.is_separator(self.item) {
                break;
            }
        }
    }

    /// The command for the currently highlighted item, if selectable.
    pub fn selected_command(&self) -> Option<Command> {
        match &self.items().get(self.item)?.action {
            MenuAction::Run(cmd) => Some(cmd.clone()),
            MenuAction::Separator => None,
        }
    }

    /// Jump to the menu whose title starts with `letter` (case-insensitive).
    pub fn jump_to_title(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_lowercase();
        for (i, m) in MENUS.iter().enumerate() {
            if m.title.chars().next().map(|c| c.to_ascii_lowercase()) == Some(letter) {
                self.menu = i;
                self.item = self.first_selectable();
                return true;
            }
        }
        false
    }

    fn first_selectable(&self) -> usize {
        MENUS[self.menu]
            .items
            .iter()
            .position(|it| !matches!(it.action, MenuAction::Separator))
            .unwrap_or(0)
    }

    fn is_separator(&self, idx: usize) -> bool {
        matches!(self.items()[idx].action, MenuAction::Separator)
    }
}
