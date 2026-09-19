# WordStar-rs

A reimplementation of DOS **WordStar 7** for the modern terminal — the blue
screen, the menu bar, the ruler, the status line, and above all the **control
diamond** and the `^K`/`^Q`/`^P` command chords you already have in your
fingers. Files are written as plain **Markdown**, so your manuscripts stay
readable, portable, and future-proof.

It is built for writers who learned to compose on WordStar and never quite found
anything that felt the same — but who also want arrow keys, a built-in file
browser, find-and-replace, and a live formatting preview when they want them.

<img width="1230" height="516" alt="image" src="https://github.com/user-attachments/assets/bcc00785-db24-434e-a86c-c302fa126fd9" />

<img width="1007" height="692" alt="image" src="https://github.com/user-attachments/assets/db97e20f-303f-4c0c-9170-e02c99158038" />


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

A document always reopens where you left off, and **File → Recent Files**
lists the ones you've worked on lately.

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

- **Title bar** — the program name and the current file (UNTITLED until saved),
  and at the right a live word count — `12,345 / 80,000 words` if the document
  sets a `goal:` (see below).
- **Menu bar** — eight pull-down menus (see *Menus* below). Open with **F9**.
- **Style bar** — the paragraph style and, for the text **under the cursor**,
  the active font, point size, and the **B I U** emphasis indicators. The
  **L C R J** group shows the current paragraph alignment.
- **Ruler** — left/right margins (`L`/`R`) and tab stops (`!`). Text wraps at
  the right margin — column 65 unless you change it with `^OR` (stored in the
  document as a `.rm` dot command).
- **Flag column** — at the right edge: `<` ends a paragraph (a hard return),
  `.` marks a dot-command line, `P` marks the first line of a new printed page,
  and a blank means the line continues on the next row.
- **Status line** — typing mode (Insert/Overtype), page, line on that page, and
  the vertical and horizontal position in inches, as WordStar reported them.
  Pages hold 54 printed lines; wrapped lines count as the rows they print on,
  `.pa` starts a new page, and other dot commands don't print.

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

The same block holds a few options for the preview and the PDF:

| Setting | Effect |
| ------- | ------ |
| `paragraphs: lines` | Type the WordStar way: every line you end with **Enter** is its own paragraph, and a Tab or spaces at its start are just an indent. Paragraphs are printed book-style, indented with no gap between them. (Without it, the file is standard Markdown: paragraphs are separated by a blank line, and an indented line after a blank one is a code block.) |
| `smart: false` | Turn off typographic punctuation. By default `"quotes"` and `'apostrophes'` print curly, a typewriter `--` (or `---`) becomes an em dash (—), and `...` an ellipsis. |
| `paper: letter` | Print on US Letter instead of A4 (`paper: a4`). |
| `goal: 80000` | A word-count goal (`80,000` and `80k` work too), shown on the title bar and in **Word Count**. |
| `format: manuscript` | Export in standard manuscript format — see *Exporting to PDF*. |

A line holding just `#` is a scene break, as in a typed manuscript.

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
| `^Q` `^I`   | Go to page                      |
| `^Q` `^G`   | Go to heading — a list of your chapters (`#` lines) and their pages |
| `^Q` `^B` / `^Q` `^K` | Beginning / End of the marked block |
| `^K` `0`…`9` | Set place marker 0–9 at the cursor (again on the same spot removes it) |
| `^Q` `0`…`9` | Go to place marker 0–9 |
| `^Q` `^P`   | Back to where you were before the last jump |
| Arrows, Home, End, PgUp, PgDn | Modern equivalents |

### Editing

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^V`  | Toggle Insert / Overtype        |
| `^N`  | Insert a line (cursor stays put) |
| `^G`  | Delete the character at the cursor |
| `^T`  | Delete the word                 |
| `^Y`  | Delete the whole line           |
| `^Q` `^Y` | Delete to end of line       |
| `^Q` `Del` | Delete to start of line    |
| `^U`  | Undo                            |

### Blocks

Blocks work the WordStar way: press `^KB` at the start of the text, move to its
end (the block highlights as you go), and press `^KK`. The block then stays
marked where it is while you move the cursor anywhere else, ready to be copied
or moved there.

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^KB` | Mark block beginning            |
| `^KK` | Mark block end (also copies it to the block buffer) |
| `^KC` | Copy the marked block to the cursor |
| `^KV` | Move the marked block to the cursor — or, with no block marked, paste the block buffer |
| `^KY` | Delete the block (it stays in the buffer, so `^KV` pastes it back) |
| `^KH` | Hide / redisplay the block      |
| `^KW` | Write the block to a file (e.g. to keep a scene you're cutting) |
| `^K"` / `^K'` / `^K.` | Block to UPPER case / lower case / Sentence case |

Bold, italic, and the other **Style** commands apply to the marked block too.
The block stays attached to its text while you keep writing: typing before it
moves it along, and typing inside it becomes part of it.

**Place markers** work the same way. `^K3` drops marker 3 at the cursor — its
number appears in the flag column and the spot is highlighted — and `^Q3` jumps
back to it from anywhere, even after you have written pages above it. Use them
to bookmark the scene you're revising, a character's first appearance, or where
you stopped. (Markers belong to the editing session; they aren't saved in the
file.)

A selection made with the mouse (or still being marked, before `^KK`) acts like
a clipboard selection instead: `^KC` copies it to the block buffer, `^KY` cuts
it, and `^KV` pastes the buffer at the cursor.

### Find and replace

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^QF` | Find                            |
| `^QA` | Find and replace                |
| `^L`  | Find next (or carry on replacing) |

As in WordStar, after the text (and, for a replace, the replacement) you are
asked for **options** — type any of these letters, or just press **Enter**:

| Option | Meaning |
| ------ | ------- |
| `U` | Ignore case: `ann` finds `Ann` and `ANN` |
| `W` | Whole words only: `Ann` doesn't find `Anne` or `Planning` |
| `B` | Search backwards |
| `G` | The whole document, from the top (or the end, with `B`) |
| `N` | Replace without asking |

A replace highlights each match and asks: **Y** replaces it, **N** skips it,
**A** replaces all the rest, **Esc** stops. However many it changed, one `^U`
undoes the lot. Renaming a character everywhere is `^QA`, the old and new name,
then `GWN`.

Searches are literal (not regular expressions). Prompts offer your previous
answer, highlighted: type to replace it, or use ←/→ (`^S`/`^D`), Home/End,
Backspace and Del (`^G`) to edit it; `^Y` clears the line. **Enter** confirms
and **Esc** cancels.

### Formatting

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^PB` | Bold (`**…**`)                  |
| `^PY` | Italic (`*…*`)                  |
| `^PS` | Underline (`[…]{.underline}`)   |
| `^PX` | Strikeout (`~~…~~`)             |
| `^P=` | Font…                           |

### On-screen format (`^O`)

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^OD` | Hide / show the formatting markup (a clean reading view) |
| `^OW` | Word wrap on / off              |
| `^OR` | Set the right margin (the column text wraps at) |
| `^OC` | Center the paragraph            |
| `^OL` / `^O]` | Align left / right       |
| `^OJ` | Justify                         |
| `^OP` | Preview (same as F5)            |

`^OD` is the modern equivalent of WordStar's "display control characters"
toggle: it hides the Markdown markers and shows the text as it will read. It is a
read-only view — press `^OD` again (or `Esc`) to return to editing.

### Files and the program

| Keys          | Action                  |
| ------------- | ----------------------- |
| `^KS` / `F2`  | Save and keep editing   |
| `^KD`         | Save and close the document (a fresh, untitled one takes its place) |
| `^KX`         | Save and exit           |
| `^KT`         | Save As                 |
| `^KR`         | Insert another file at the cursor |
| `^KP`         | Export to PDF           |
| `^KQ` / `F10` | Quit (asks before discarding unsaved changes) |

Saving an untitled document opens **Save As** first, and whatever you were
doing (closing, exiting, opening another file) carries on once it has a name. If
a save fails, nothing is closed. **Close**, opening another file, and quitting
all ask before discarding unsaved changes.

### Backups and crash recovery

Every save is written safely: the new text goes to a temporary file that
replaces the document only once it is completely on disk, so a crash or a full
disk can never leave a half-written manuscript. The previous version is kept
next to it as a WordStar-style backup, `chapter1.md.bak`.

While you have unsaved changes, a recovery copy is written every few seconds
(to `~/.local/share/wordstar-rs/recovery/` on Linux). If the program or the
terminal dies, the next time you open that document — or start without a file,
for untitled work — WordStar-rs offers to restore it. The copy is removed once
you save. Undo (`^U`) remembers the last 10,000 edits, and a Find-and-Replace
counts as one.
| `F3` / `^OK`  | Open the file browser   |
| `F5`          | Toggle the formatted preview |
| `F1` / `^J`   | Help                    |
| `F9`          | Open the menu bar       |

### Insert and utilities

| Keys  | Action                          |
| ----- | ------------------------------- |
| `^KR` | Insert another file at the cursor (WordStar `.WS` files are decoded too) |
| `^K?` | Word count — words, characters, lines, and paragraphs |
| `^QI` | Go to page                      |
| —     | **Page Break** / **Column Break** (Insert menu) insert `.pa` / `.cb` |
| —     | **Headers / Footers** (Layout menu) — see below |

Press **F1** inside the program at any time for this command reference.

### Headers, footers, and page breaks

The **Layout → Headers/Footers** submenu opens a dialog for a header or footer
line. Type the text, choose whether it applies to **Both**, **Odd**, or **Even**
pages (↑/↓), and press **Enter**. These are stored as WordStar **dot commands**
at the top of the document (`.he`/`.oh`/`.eh` for headers, `.fo`/`.of`/`.ef` for
footers); **Page Break** and **Column Break** insert `.pa` and `.cb`.

Dot commands are print directives, not body text: while editing they are plain
lines (marked `.` in the flag column), and in the preview and the PDF they take
effect instead of appearing as text:

| Dot command | Effect |
| ----------- | ------ |
| `.pa` | Start a new page (the `^OD` view shows a *page break* line) |
| `.he` / `.oh` / `.eh` *text* | Header on every / odd / even page |
| `.fo` / `.of` / `.ef` *text* | Footer on every / odd / even page (replaces the page number) |
| `.op` | Omit page numbers |
| `.pn` *n* | Number the first page *n* |
| `.rm` *n* | Right margin: wrap text at column *n* (`^OR` sets it) |

In header and footer text, `#` prints the page number: `.fo Page #` gives
"Page 1", "Page 2", and so on. Original WordStar files keep these dot commands
when imported.

---

## Menus

Press **F9** to open the menu bar. The eight menus follow WordStar 7's layout.
Use **←/→** to move between menus, **↑/↓** to move through items, **Enter** to
choose, and **Esc** to close. Pressing a menu's initial letter jumps straight to
it. Items that open a **submenu** are marked with a `▶`; press **→** (or click)
to open it and **←** to step back.

- **File** — Open/Switch…, Recent Files…, Close, Save, Save As…, Save and
  Close, Save and Exit, Export PDF…, Exit WordStar
- **Edit** — Undo · mark/copy/move/delete a block · Find…, Find and Replace…,
  Next Find, Go to Page…
- **View** — Preview · Command Tags (show/hide markup), Block Highlighting,
  Word Wrap · Insert / Overtype
- **Insert** — Page Break, Column Break · **File…** (insert another file)
- **Style** — Bold, Italic, Underline, Strikeout, Font…, Font Size… ·
  Clear Formatting
- **Layout** — Center / Right / Left / Justify line · **Headers/Footers ▶**
  (Header…, Footer…)
- **Utilities** — Word Count · Spelling Check, Thesaurus, Calculator, Sort Block
- **Help** — Help Topics, About

A few WordStar features that have no equivalent here yet (printing, the
thesaurus, block sort) appear in the menus for familiarity but report that they
are not implemented when chosen.

---

## Spelling

Words the dictionary doesn't know are underlined in red as you write (turn
that off with **View → Spelling Highlights**). The word you're still typing is
left alone until you move on.

| Keys  | Action |
| ----- | ------ |
| `^QL` | Check the spelling from the cursor to the end |
| `^QN` | Check the word at the cursor |

The check stops at each unknown word, highlights it, and lists suggestions:

| Key | Does |
| --- | ---- |
| `1`–`9` | Replace the word with that suggestion |
| `I` | Ignore it here |
| `G` | Ignore it everywhere, for this session |
| `A` | Add it to your personal dictionary — the place for your characters' names and invented words |
| `T` | Type the replacement yourself |
| `Esc` | Stop checking |

Your personal dictionary is a plain word list in
`~/.config/wordstar-rs/words.txt` (Linux; in the browser it's kept in the
browser's storage). US English is built in. For another language, name it in
the frontmatter — `language: en_GB`, `language: de_DE` — and WordStar-rs uses
that Hunspell dictionary if it's installed (as on most Linux systems). The
frontmatter, dot commands, code, links and web addresses are never checked.
(The built-in dictionary is the SCOWL-based en_US Hunspell dictionary; see
`assets/dict/LICENSE-en_US.txt` for its license.)

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
| Paste (terminal's paste, or Ctrl+Shift+V / Cmd+V in the browser) | Insert the text as one edit — plain Ctrl+V stays WordStar's `^V` |
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

The PDF is laid out on A4 pages (or US Letter, with `paper: letter`) with page
numbers, your headers and footers, and page breaks where you put `.pa`. It
renders your formatting: headings, **bold**, *italic*, lists (bulleted,
numbered, and task lists), block quotes, code blocks, tables, and line breaks
you forced inside a paragraph (end a line with `\` — for verse or letters). It
is typeset in the Courier family — a fixed-pitch, typewriter look in keeping
with WordStar's manuscript heritage, and one that needs no bundled fonts. Text
is limited to the Latin-1 / Windows-1252 character set (which includes curly
quotes and dashes); anything outside it is shown as `?`.

### Standard manuscript format

Magazines, agents and publishers ask for fiction in *standard manuscript
format*. Choose **Insert → Manuscript Setup** and fill in the lines it adds at
the top of your document:

```markdown
---
format: manuscript
title: The Red House
author: Jane Q. Writer        # your legal name, for the contact block
byline: J. Q. Writer          # the name to publish under (optional)
contact:
  - 12 Elm Street, Springfield
  - jane@example.com
paper: letter
paragraphs: lines
---
```

`^KP` then exports the manuscript the way editors expect it: 12-point Courier,
double-spaced, one-inch margins, and every paragraph indented half an inch with
no blank lines between them. Page one has your name and contact details in the
top left, the word count rounded to the nearest hundred ("about 4,300 words")
in the top right, and the title and byline halfway down, where the story
begins. Every later page carries the header `Surname / TITLE / page`. Scene
breaks (`#` or `***`) print as a centered `#`, each `#` chapter heading starts a
new page a third of the way down, and `END` marks the finish. Add
`italics: underline` for the older convention of underlining italics. (The
on-screen preview keeps its usual look; the layout applies to the PDF.)

The **Word Count** (`^K?`) counts what a reader would see, leaving out the
frontmatter, dot commands, and Markdown markup. It also shows how many words
you've added since opening the document, your progress towards the `goal:`,
and — with a block marked — the words in the block, handy for checking a single
scene.

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
