//! The pull-down menu model and its navigation state.
//!
//! The menu tree is static data; selecting an item yields a [`Command`] that the
//! usual dispatcher executes. Titles and layout mirror WordStar 7's bar (File /
//! Edit / View / Insert / Style / Layout / Utilities / Help), including the
//! nested submenus WordStar shows with a `▶` marker.

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
    /// Open a nested submenu.
    Submenu(&'static [MenuItem]),
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

/// An item that opens a submenu.
const fn sub(label: &'static str, items: &'static [MenuItem]) -> MenuItem {
    MenuItem {
        label,
        shortcut: "",
        action: MenuAction::Submenu(items),
    }
}

/// An item that is shown for fidelity but not yet implemented.
const fn todo(label: &'static str, shortcut: &'static str, feature: &'static str) -> MenuItem {
    MenuItem {
        label,
        shortcut,
        action: MenuAction::Run(Command::NotImplemented(feature)),
    }
}

const SEP: MenuItem = MenuItem {
    label: "",
    shortcut: "",
    action: MenuAction::Separator,
};

static FILE: &[MenuItem] = &[
    item("Open/Switch...", "^OK / F3", Command::OpenBrowser),
    item("Close", "", Command::New),
    SEP,
    item("Save", "^KS / F2", Command::Save),
    item("Save As...", "^KT", Command::SaveAs),
    item("Save and Close", "^KD", Command::SaveExit),
    SEP,
    todo("Print...", "^KP", "Printing"),
    item("Export PDF...", "", Command::ExportPdf),
    SEP,
    item("Exit WordStar", "^KQX / F10", Command::Quit),
];

static EDIT: &[MenuItem] = &[
    item("Undo", "^U", Command::Undo),
    SEP,
    item("Mark Block Beginning", "^KB", Command::BlockBegin),
    item("Mark Block End", "^KK", Command::BlockEnd),
    item("Copy", "^KC", Command::BlockCopy),
    item("Move", "^KV", Command::BlockMove),
    item("Delete", "^KY", Command::BlockDelete),
    SEP,
    item("Find...", "^QF", Command::Find),
    item("Find and Replace...", "^QA", Command::Replace),
    item("Next Find", "^L", Command::FindNext),
    item("Go to Page...", "^QI", Command::GoToPage),
];

static VIEW: &[MenuItem] = &[
    item("Preview", "^OP / F5", Command::TogglePreview),
    SEP,
    item("Command Tags", "^OD", Command::ToggleMarkup),
    item("Block Highlighting", "^KH", Command::BlockHide),
    item("Word Wrap", "^OW", Command::ToggleWrap),
    SEP,
    item("Insert / Overtype", "^V", Command::ToggleInsert),
];

static INSERT: &[MenuItem] = &[
    item("Page Break", ".pa", Command::PageBreak),
    item("Column Break", ".cb", Command::ColumnBreak),
    SEP,
    item("File...", "^KR", Command::InsertFile),
];

static STYLE: &[MenuItem] = &[
    item("Bold", "^PB", Command::InsertBold),
    item("Italic", "^PY", Command::InsertItalic),
    item("Underline", "^PS", Command::InsertUnderline),
    item("Strikeout", "^PX", Command::InsertStrike),
    item("Font...", "^P=", Command::FontPrompt),
    item("Font Size...", "", Command::SizePrompt),
    SEP,
    item("Clear Formatting", "", Command::ClearFormat),
];

static HEADERS_FOOTERS: &[MenuItem] = &[
    item("Header...", "", Command::Header),
    item("Footer...", "", Command::Footer),
];

static LAYOUT: &[MenuItem] = &[
    item("Center Line", "^OC", Command::AlignCenter),
    item("Right Align Line", "^OJ", Command::AlignRight),
    item("Left Align Line", "^OL", Command::AlignLeft),
    item("Justify", "^OS", Command::AlignJustify),
    SEP,
    sub("Headers/Footers", HEADERS_FOOTERS),
];

static UTILITIES: &[MenuItem] = &[
    item("Word Count", "^K?", Command::WordCount),
    SEP,
    todo("Spelling Check", "^QL", "Spelling check"),
    todo("Thesaurus...", "^QJ", "Thesaurus"),
    todo("Calculator", "^QM", "Calculator"),
    todo("Sort Block", "", "Sort block"),
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

/// First non-separator index in a list of items.
fn first_selectable(items: &[MenuItem]) -> usize {
    items
        .iter()
        .position(|it| !matches!(it.action, MenuAction::Separator))
        .unwrap_or(0)
}

/// Navigation state for an open menu (one level of submenu nesting).
#[derive(Debug, Clone, Copy, Default)]
pub struct MenuState {
    /// Selected top-level menu.
    pub menu: usize,
    /// Selected item within the open menu.
    pub item: usize,
    /// Whether the highlighted item's submenu is open.
    pub sub_open: bool,
    /// Selected item within the open submenu.
    pub sub_item: usize,
}

/// What activating the current selection does.
#[derive(Debug)]
pub enum Activation {
    /// Run this command and close the menu.
    Run(Command),
    /// A submenu was opened; stay in the menu.
    OpenedSubmenu,
    /// Nothing actionable (e.g. a separator).
    None,
}

impl MenuState {
    fn items(&self) -> &'static [MenuItem] {
        MENUS[self.menu].items
    }

    /// The submenu items of the highlighted top-level item, if it is a submenu.
    pub fn submenu_items(&self) -> Option<&'static [MenuItem]> {
        match self.items().get(self.item)?.action {
            MenuAction::Submenu(items) => Some(items),
            _ => None,
        }
    }

    /// The items of whichever level currently has focus.
    fn active_items(&self) -> &'static [MenuItem] {
        if self.sub_open {
            self.submenu_items().unwrap_or(self.items())
        } else {
            self.items()
        }
    }

    fn active_index(&self) -> usize {
        if self.sub_open {
            self.sub_item
        } else {
            self.item
        }
    }

    /// Move to the previous menu (wraps), selecting its first real item.
    pub fn prev_menu(&mut self) {
        self.menu = (self.menu + MENUS.len() - 1) % MENUS.len();
        self.reset_items();
    }

    /// Move to the next menu (wraps).
    pub fn next_menu(&mut self) {
        self.menu = (self.menu + 1) % MENUS.len();
        self.reset_items();
    }

    fn reset_items(&mut self) {
        self.sub_open = false;
        self.sub_item = 0;
        self.item = first_selectable(self.items());
    }

    /// Move the highlight up, skipping separators (wraps within the level).
    pub fn prev_item(&mut self) {
        self.step_item(false);
    }

    /// Move the highlight down, skipping separators (wraps within the level).
    pub fn next_item(&mut self) {
        self.step_item(true);
    }

    fn step_item(&mut self, forward: bool) {
        let items = self.active_items();
        let n = items.len();
        if n == 0 {
            return;
        }
        // Wrap-safe step: +1 forward, or +(n-1) ≡ -1 backward, all within `n`.
        let delta = if forward { 1 } else { n - 1 };
        let mut idx = self.active_index();
        for _ in 0..n {
            idx = (idx + delta) % n;
            if !matches!(items[idx].action, MenuAction::Separator) {
                break;
            }
        }
        if self.sub_open {
            self.sub_item = idx;
        } else {
            self.item = idx;
        }
    }

    /// Right arrow: open the highlighted submenu, else move to the next menu.
    pub fn move_right(&mut self) {
        if !self.sub_open && self.submenu_items().is_some() {
            self.open_submenu();
        } else {
            self.next_menu();
        }
    }

    /// Left arrow: close an open submenu, else move to the previous menu.
    pub fn move_left(&mut self) {
        if self.sub_open {
            self.sub_open = false;
        } else {
            self.prev_menu();
        }
    }

    fn open_submenu(&mut self) {
        if let Some(items) = self.submenu_items() {
            self.sub_open = true;
            self.sub_item = first_selectable(items);
        }
    }

    /// Activate the current selection (Enter / click).
    pub fn activate(&mut self) -> Activation {
        if self.sub_open {
            return match self.submenu_items().and_then(|its| its.get(self.sub_item)) {
                Some(it) => match &it.action {
                    MenuAction::Run(cmd) => Activation::Run(cmd.clone()),
                    _ => Activation::None,
                },
                None => Activation::None,
            };
        }
        match self.items().get(self.item).map(|it| &it.action) {
            Some(MenuAction::Run(cmd)) => Activation::Run(cmd.clone()),
            Some(MenuAction::Submenu(_)) => {
                self.open_submenu();
                Activation::OpenedSubmenu
            }
            _ => Activation::None,
        }
    }

    /// Select a top-level menu by index (e.g. from a mouse click), highlighting
    /// its first selectable item.
    pub fn select_menu(&mut self, idx: usize) {
        if idx < MENUS.len() {
            self.menu = idx;
            self.reset_items();
        }
    }

    /// Jump to the menu whose title starts with `letter` (case-insensitive).
    pub fn jump_to_title(&mut self, letter: char) -> bool {
        let letter = letter.to_ascii_lowercase();
        for (i, m) in MENUS.iter().enumerate() {
            if m.title.chars().next().map(|c| c.to_ascii_lowercase()) == Some(letter) {
                self.menu = i;
                self.reset_items();
                return true;
            }
        }
        false
    }
}

