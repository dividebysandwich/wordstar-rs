# WordStar-rs

A faithful clone of DOS **WordStar 7** for the modern terminal — the blue
screen, the menu bar, the ruler, the status line, and above all the **control
diamond** and the `^K`/`^Q`/`^P` command chords you already have in your
fingers. Files are written as plain **Markdown**, so your manuscripts stay
readable, portable, and future-proof.

It is built for writers who learned to compose on WordStar and never quite found
anything that felt the same — but who also want arrow keys, a built-in file
browser, find-and-replace, and a live formatting preview when they want them.

---

## Installing and compiling

WordStar-rs is a single self-contained program. You need a **Rust toolchain,
version 1.85 or newer** (the project uses the 2024 edition). If you don't have
Rust, install it from <https://rustup.rs>.

```sh
# Clone, then from the project directory:
cargo build --release        # compiles to ./target/release/wordstar-rs

# Run it directly:
cargo run --release -- mynovel.md

# Or install it onto your PATH so you can call `wordstar-rs` anywhere:
cargo install --path .
```

### Starting the editor

```sh
wordstar-rs               # start with an empty, untitled document
wordstar-rs chapter1.md   # open (or create) a file
```

If you start without a file name, press **F3** at any time to open the file
browser, or just begin typing and save with **Save As** later.

---

## The screen

Top to bottom, the layout mirrors WordStar 7:

```
              WordStar    CHAPTER1.MD              <- title bar
 File  Edit  View  Insert  Style  Layout  Utilities      Help   <- menu bar
 Body Text    Default  12pt                       B  I  U   L C R J   <- style bar
L----!----!----!----!----!----!----!----R----!----   <- ruler
                                                   <- editing canvas
                          Insert   P1  L16  V3.00"  C65  H6.40"   <- status line
```

- **Title bar** — the program name and the current file (UNTITLED until saved).
- **Menu bar** — eight pull-down menus (see *Menus* below). Open with **F9**.
- **Style bar** — the paragraph style and, for the text **under the cursor**,
  the active font, point size, and the **B I U** emphasis indicators. The
  **L C R J** group shows the current paragraph alignment.
- **Ruler** — left/right margins (`L`/`R`) and tab stops (`!`).
- **Status line** — typing mode (Insert/Overtype), page, line, and the vertical
  and horizontal position in inches, exactly as WordStar reported them.

---

## How formatting works

WordStar-rs edits Markdown directly, so the formatting markers are visible in the
text — much like WordStar's old on-screen control codes. What you see is exactly
what is saved.

| You want      | It looks like in the text          |
| ------------- | ---------------------------------- |
| **Bold**      | `**bold**`                         |
| *Italic*      | `*italic*`                         |
| Underline     | `[underline]{.underline}`          |
| A font        | `[text]{font="Courier"}`           |
| A point size  | `[text]{size=14}`                  |

You rarely type those by hand. Select a block, then apply Bold/Italic/Underline
(from the **Insert** menu or `^PB`/`^PY`/`^PS`), or set a font or size from the
**Style** menu. With no selection, the markers are inserted at the cursor and you
type between them.

Document-wide defaults (the font and size shown on the style bar when the cursor
is in ordinary text) come from an optional YAML block at the very top of the
file:

```markdown
---
font: Courier
size: 12
---

Your manuscript begins here…
```

### Seeing it formatted

Press **F5** for a read-only **Preview**: headings, bold, italics, lists, and
block quotes are rendered with the markers hidden, the way the finished page
reads. Press **F5**, **Esc**, or **q** to return to editing. (A terminal cannot
show real fonts or point sizes — those are recorded in the file and surfaced on
the style bar, just as WordStar only previewed fonts in its own preview mode.)

---

## Keyboard commands

If you remember WordStar, you already know most of this. Every classic chord is
here, and the function keys are added for convenience.

### The cursor diamond and movement

| Keys        | Action                          |
| ----------- | ------------------------------- |
| `^E` / `^X` | Up / Down one line              |
| `^S` / `^D` | Left / Right one character      |
| `^A` / `^F` | Left / Right one word           |
| `^R` / `^C` | Page up / Page down             |
| `^Q` `^S` / `^Q` `^D` | Start / End of line   |
| `^Q` `^R` / `^Q` `^C` | Start / End of document |
| Arrows, Home, End, PgUp, PgDn | Modern equivalents |

### Editing

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^V`  | Toggle Insert / Overtype        |
| `^G`  | Delete the character at the cursor |
| `^T`  | Delete the word                 |
| `^Y`  | Delete the line                 |
| `^Q` `^Y` | Delete to end of line       |
| `^U`  | Undo                            |

### Blocks

Mark a block, then act on it. (Internally this uses a standard selection, so you
can also shift-click style selections with the arrow keys.)

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^KB` | Mark block beginning            |
| `^KK` | Mark block end (copies it to the buffer) |
| `^KC` | Copy block to the buffer        |
| `^KV` | Paste the block at the cursor   |
| `^KY` | Delete the block                |
| `^KH` | Hide the block markers          |

### Find and replace

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^QF` | Find                            |
| `^QA` | Find and replace                |
| `^L`  | Find next                       |

Searches are literal (not regular expressions). At a prompt, **Enter** confirms
and **Esc** cancels.

### Formatting

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^PB` | Bold (`**…**`)                  |
| `^PY` | Italic (`*…*`)                  |
| `^PS` | Underline (`[…]{.underline}`)   |

### Files and the program

| Keys          | Action                  |
| ------------- | ----------------------- |
| `^KS` / `F2`  | Save                    |
| `^KD`         | Save and keep editing   |
| `^KX`         | Save and exit           |
| `^KQ` / `F10` | Quit (asks before discarding unsaved changes) |
| `F3`          | Open the file browser   |
| `F5`          | Toggle the formatted preview |
| `F1` / `^J`   | Help                    |
| `F9`          | Open the menu bar       |

Press **F1** inside the program at any time for this command reference.

---

## Menus

Press **F9** to open the menu bar. Use **←/→** to move between menus, **↑/↓** to
move through items, **Enter** to choose, and **Esc** to close. Pressing a menu's
initial letter jumps straight to it.

- **File** — New, Open…, Save, Save As…, Save & Exit, Exit
- **Edit** — Undo, and the block operations (mark, copy, paste, delete)
- **View** — Preview, Insert/Overtype
- **Insert** — Bold, Italic, Underline
- **Style** — Font…, Font Size…, Clear Formatting
- **Layout** — Align Left, Center, Right, Justify
- **Utilities** — Find…, Replace…, Find Next
- **Help** — Help Topics, About

---

## The file browser

Press **F3** to browse. Directories are listed first, then files, across several
columns.

| Keys            | Action                          |
| --------------- | ------------------------------- |
| ↑ / ↓           | Move the highlight              |
| ← / →           | Jump a column                   |
| Enter           | Open a file, or enter a directory (`..` goes up) |
| Esc             | Close the browser               |

---

## File format

Documents are saved as **Markdown** (`.md`) — the canonical, human-readable
format. Bold, italic, headings, and lists are standard Markdown; underline,
fonts, and point sizes use the bracketed-attribute notation shown above, and
document defaults live in an optional YAML header. Everything is plain text you
can read, search, and version-control like any other manuscript.

> Reading original WordStar binary `.WS`/`.DOC` files is on the roadmap; today
> the editor reads and writes Markdown.
