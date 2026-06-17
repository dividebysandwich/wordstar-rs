# Bundled fonts

These fonts ship with the WebAssembly build of wordstar-rs.

## BigBlue Terminal — editor screen font (browser build)

`BigBlueTerminal.ttf` is the typeface used for the terminal grid in the browser.
It is a subset (the glyphs WordStar actually renders: ASCII, Latin-1, CP437
box-drawing / block / shape characters) of **BigBlue Terminal** by **VileR**,
released under the **Creative Commons Attribution-ShareAlike 4.0** license
(CC BY-SA 4.0).

- Original: <https://int10h.org/blog/2015/05/bigblue-terminal-oldschool-readable-cga-vga-font/>
- The subset was generated from the Nerd Fonts patched build
  (`BigBlueTermPlusNerdFontMono`); the Nerd Fonts patch is MIT-licensed, the
  underlying letterforms remain CC BY-SA 4.0.
- Two characters the editor draws but the font lacks (`▶` U+25B6, `▮` U+25AE)
  are aliased in the cmap to the near-identical CP437 glyphs the font does have
  (`►` U+25BA and `█` U+2588) so the screen never falls back mid-grid.

As a derivative, `BigBlueTerminal.ttf` is likewise made available under
CC BY-SA 4.0.

## DejaVu — graphical preview fonts (all builds)

`DejaVu*.ttf` (Sans, Serif, Sans Mono; regular / bold / italic / bold-italic)
are embedded into the binary and used to rasterize the document preview. They
are part of the **DejaVu Fonts** project and distributed under the
[DejaVu Fonts License](https://dejavu-fonts.github.io/License.html) (a permissive
Bitstream Vera–derived license).
