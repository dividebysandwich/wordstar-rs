# WordStar-rs

A faithful clone of DOS **WordStar 7** for the modern terminal — the blue
screen, the menu bar, the ruler, the status line, and above all the **control
diamond** and the `^K`/`^Q`/`^P` command chords you already have in your
fingers. Files are written as plain **Markdown**, so your manuscripts stay
readable, portable, and future-proof.

It is built for writers who learned to compose on WordStar and never quite found
anything that felt the same — but who also want arrow keys, a built-in file
browser, find-and-replace, and a live formatting preview when they want them.

<img width="1007" height="472" alt="image" src="https://github.com/user-attachments/assets/37d2ccda-d788-4386-ab31-91c7aaf4a12e" />


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

Press **F5** for a read-only **Preview**: the document is rendered with the
markers hidden, the way the finished page reads. Press **F5**, **Esc**, or **q**
to return to editing.

If your terminal supports inline graphics (**Kitty**, **iTerm2**, **WezTerm**,
**Ghostty**, **Sixel**-capable terminals, …), the preview is shown as a real
**rendered image** — proper proportional type, true bold/italic, and scaled
headings, laid out with your system fonts. The document is paginated into
**A4 pages shown one at a time**, which you can page through and zoom:

| Keys | In the graphical preview |
| ---- | ------------------------ |
| **PgDn / PgUp** (or **n / p**, or ↑/↓) | Next / previous page |
| **Home / End** | First / last page |
| **+ / −** | Zoom in / out |
| **Arrow keys** (when zoomed) | Pan around the page |
| **Mouse wheel** | Page (or pan when zoomed) |
| **Esc / F5 / q** | Close |

On terminals without graphics support it automatically falls back to the
scrollable styled text preview, so it works everywhere. (In the text preview a
terminal still can't show real fonts or point sizes — those are recorded in the
file and surfaced on the style bar, just as WordStar only previewed fonts in its
own preview mode.)

### Markdown WordStar never had

You can type any standard Markdown by hand and the preview (and the `^OD` clean
view) will render it — features that have no WordStar equivalent:

- **Headings** (`# … ######`) and **horizontal rules** (`---`)
- **Ordered, bulleted, and nested lists**, plus **task lists** (`- [x] done`)
- **Tables** (GitHub-style), laid out with aligned columns and box borders:

  ```markdown
  | Item   | Qty | Price |
  | :---   | --: | :---: |
  | Apples |   3 | 1.50  |
  ```

- **Links** `[text](url)` and **images** `![alt](url)` (the URL is shown dimmed)
- **Inline code** `` `like this` `` and fenced **code blocks**
- **Block quotes** (`> …`) and **strikethrough** (`~~…~~`)

These are plain text in the editor — type them as you would in any Markdown file
— and they simply come to life in the preview.

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
| `^W` / `^Z` | Scroll the view up / down one line |
| `^Q` `^S` / `^Q` `^D` | Start / End of line   |
| `^Q` `^R` / `^Q` `^C` | Start / End of document |
| Arrows, Home, End, PgUp, PgDn | Modern equivalents |

### Editing

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^V`  | Toggle Insert / Overtype        |
| `^N`  | Insert a line (cursor stays put) |
| `^G`  | Delete the character at the cursor |
| `^T`  | Delete the word                 |
| `^Y`  | Delete the line                 |
| `^Q` `^Y` | Delete to end of line       |
| `^Q` `Del` | Delete to start of line    |
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
| `^PX` | Strikeout (`~~…~~`)             |

### On-screen format (`^O`)

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^OD` | Hide / show the formatting markup (a clean reading view) |
| `^OC` | Center the paragraph            |
| `^OL` / `^OR` | Align left / right       |
| `^OJ` | Justify                         |

`^OD` is the modern equivalent of WordStar's "display control characters"
toggle: it hides the Markdown markers and shows the text as it will read. It is a
read-only view — press `^OD` again (or `Esc`) to return to editing.

### Files and the program

| Keys          | Action                  |
| ------------- | ----------------------- |
| `^KS` / `F2`  | Save                    |
| `^KD`         | Save and keep editing   |
| `^KX`         | Save and exit           |
| `^KP`         | Export to PDF           |
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

You can also click a file with the mouse, or double-click to open it.

---

## Using the mouse

Like WordStar 7, the editor is fully usable with a mouse — handy alongside the
keyboard, never required.

| Action            | What it does                       |
| ----------------- | ---------------------------------- |
| Click             | Position the cursor                |
| Click and drag    | Mark a block (select text)         |
| Double-click      | Select the word under the pointer  |
| Click a menu title | Open that menu; click another to switch |
| Click a menu item | Run it                             |
| Click outside a menu | Close it                        |
| Scroll wheel      | Scroll the document, the file list, or any overlay |

Marked text works with the block commands (`^KC` copy, `^KV` paste, `^KY`
delete) just as a keyboard-marked block does.

---

## Exporting to PDF

Press **`^KP`** (or **File → Export PDF…**) to export. A dialog asks for the
output file name, pre-filled with a sensible default (`chapter1.md` →
`chapter1.pdf`); edit it as you like and press **Enter**, or **Esc** to cancel.
If the chosen file already exists, a confirmation box appears first — press
**Y** to overwrite or **N** (or **Esc**) to back out without touching it.

The PDF is laid out on A4 pages with page numbers, and renders your formatting:
headings, **bold**, *italic*, lists (bulleted, numbered, and task lists), block
quotes, code blocks, and tables. It is typeset in the Courier family — a
fixed-pitch, typewriter look in keeping with WordStar's manuscript heritage, and
one that needs no bundled fonts. Text is limited to the Latin-1 / Windows-1252
character set; anything outside it is shown as `?`.

## File format

Documents are saved as **Markdown** (`.md`) — the canonical, human-readable
format. Bold, italic, headings, and lists are standard Markdown; underline,
fonts, and point sizes use the bracketed-attribute notation shown above, and
document defaults live in an optional YAML header. Everything is plain text you
can read, search, and version-control like any other manuscript.

### Importing classic WordStar files

Open an original WordStar document (a `.WS` file, or any file beginning with the
WordStar header) and it is **imported automatically** — the binary header is
skipped, word-wrap soft returns are re-flowed, the high-bit word markers are
cleaned up, and inline effects (bold, italic, underline, strikeout) become their
Markdown equivalents. Dot commands (`.PA`, `.LM`, …) are dropped.

To protect your originals, an import opens as an **unsaved Markdown document**:
`CHAPTER.WS` becomes `CHAPTER.md` on save, leaving the `.WS` file untouched.
(Text is limited to the Latin-1 / Windows-1252 character set.)
