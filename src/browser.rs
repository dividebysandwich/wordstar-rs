//! The built-in file browser — WordStar's opening/Open screen.
//!
//! Shows the current directory as a multi-column listing (directories first),
//! with arrow-key navigation. Enter opens a file or descends into a directory.

use std::cell::Cell;
use std::fs;
use std::io;
use std::path::PathBuf;

/// One row in the listing.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Display name (`..` for the parent, `NAME/` style handled at render time).
    pub name: String,
    /// Full path the entry refers to.
    pub path: PathBuf,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// File size in bytes (0 for directories).
    pub size: u64,
}

/// Result of activating (Enter) the selected entry.
pub enum Activation {
    /// Stay in the browser (e.g. a directory was entered).
    Stay,
    /// Close the browser and open this file.
    Open(PathBuf),
}

/// File browser state.
pub struct Browser {
    /// Directory currently being listed.
    pub cwd: PathBuf,
    /// Entries in `cwd` (parent first, then dirs, then files).
    pub entries: Vec<Entry>,
    /// Index of the highlighted entry.
    pub selected: usize,
    /// Rows per column as last rendered (for Left/Right column jumps).
    /// Interior-mutable so the renderer can record it from `&Browser`.
    pub col_height: Cell<usize>,
}

impl Browser {
    /// Build a browser listing for `dir`.
    pub fn new(dir: PathBuf) -> io::Result<Self> {
        let mut b = Browser {
            cwd: dir,
            entries: Vec::new(),
            selected: 0,
            col_height: Cell::new(1),
        };
        b.reload()?;
        Ok(b)
    }

    /// Re-read the current directory.
    pub fn reload(&mut self) -> io::Result<()> {
        let mut dirs: Vec<Entry> = Vec::new();
        let mut files: Vec<Entry> = Vec::new();

        for entry in fs::read_dir(&self.cwd)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden files (dotfiles) to mirror WordStar's tidy listing.
            if name.starts_with('.') {
                continue;
            }
            let meta = entry.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let e = Entry {
                name,
                path,
                is_dir,
                size: if is_dir { 0 } else { size },
            };
            if is_dir {
                dirs.push(e);
            } else {
                files.push(e);
            }
        }

        dirs.sort_by_key(|a| a.name.to_lowercase());
        files.sort_by_key(|a| a.name.to_lowercase());

        let mut entries = Vec::with_capacity(dirs.len() + files.len() + 1);
        if let Some(parent) = self.cwd.parent() {
            entries.push(Entry {
                name: "..".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                size: 0,
            });
        }
        entries.extend(dirs);
        entries.extend(files);

        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        Ok(())
    }

    /// Activate the selected entry.
    pub fn activate(&mut self) -> Activation {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            return Activation::Stay;
        };
        if entry.is_dir {
            self.cwd = entry.path;
            self.selected = 0;
            let _ = self.reload();
            Activation::Stay
        } else {
            Activation::Open(entry.path)
        }
    }

    pub fn select_next(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_down_column(&mut self) {
        let step = self.col_height.get().max(1);
        self.selected = (self.selected + step).min(self.entries.len().saturating_sub(1));
    }

    pub fn select_up_column(&mut self) {
        let step = self.col_height.get().max(1);
        self.selected = self.selected.saturating_sub(step);
    }

    /// Free space (bytes) on the volume holding `cwd`, best-effort.
    pub fn free_space_hint(&self) -> String {
        // Portable free-space queries need a syscall crate; show the entry count
        // instead, which is still useful context (and matches the spirit of the
        // WordStar header without adding a dependency).
        format!("{} items", self.entries.len())
    }
}
