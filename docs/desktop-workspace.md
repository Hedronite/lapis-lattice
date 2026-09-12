# Desktop workspace (development)

The `feat/ux` desktop surface opens a native notes workspace. It is under active development; the default CLI build does not include GPUI. Build with `--features desktop`, then run `lapis --vault /path/to/vault desktop`.

The sidebar opens folders and supported files independently of the search index. Find a note is available in the sidebar and through Ctrl/Cmd+P. Submit a query with Enter; use arrows and Enter to open a result. A fresh embedded index offers an explicit Build index action. HTTP indexing stays managed by the configured service. Backend failures and empty results are separate states.

Markdown opens in Live view. Source, Reading and Split reuse the same editor state and undo history. Persistent/resizable layout and the integrated graph are still being built. YAML is editable text; HTML is a static read-only reference. PDF page reading requires the packaged native runtime; see [PDF runtime packaging](pdf-runtime.md).

## Editing

Vim normal mode is the default. Native mouse selection and application clipboard shortcuts complement the command layer. Shift+arrow selection remains available. Supported commands in this development slice:

| Task | Keys |
| --- | --- |
| Move | h/j/k/l, arrows, w/b/e, W/B/E, 0/^/$, gg/G; counted motions |
| Insert | i/a/I/A, o/O with existing indentation; Escape returns to normal |
| Select | v for characters, V for lines; motions extend the selection |
| Change text | d/c/y plus a motion, dd/cc/yy, x, D/C/Y, r, J |
| Register paste | p/P; linewise and characterwise registers retain their distinction |
| Undo / redo | u / Ctrl+R; native undo/redo also use the same history |
| Find in document | /query, Enter, n/N |
| Explicit save | :w or Ctrl/Cmd+S |
| Close | :q or Ctrl/Cmd+W; dirty content is retained |
| Save and close | :wq or :x; closes only after a successful save if the buffer still matches |
| Discard current tab | :q! or Shift+Ctrl/Cmd+W |
| Save a copy | toolbar action or Shift+Ctrl/Cmd+S |

Yanks, deletes and changes update the workspace register and request a system clipboard copy. Native paste inserts literal text, including command-like content, normalizes CRLF to LF and creates one undo transaction. Insert-mode paste remains insert mode; a visual paste replaces the selected text. A 32 MiB limit rejects oversized payloads before insertion. Single-line command/search prompts reject multiline paste and retain the note underneath them.

Named register banks, text objects, macros, dot-repeat and continuous replace mode are not implemented yet. The displayed command state and explicit unmapped-key notices should make those limits visible. This is not a claim of complete Vim parity.

## Verification

The optional `gui-tests` feature exercises real GPUI components with a headless platform:

```sh
cargo test -p lapis-desktop --features gui-tests
```

Those checks cover command dispatch, native-widget undo, literal CRLF paste, separate prompt focus/text input, view-switch continuity and save-close success/failure. They supplement actual macOS/Linux mouse, clipboard and IME smoke; they do not prove native platform acceptance or performance thresholds.


### Live Markdown development checkpoint

Markdown opens in Live view. The block containing the caret reveals source syntax; other blocks render headings, emphasis, code, quotes and task markers. Source, Reading and Split remain available. All editing views retain the same source editor and undo history. YAML opens in Source; HTML opens in Reading.

The live surface maps pointer selection and UTF-16 input-method ranges back to source bytes. Dragging keeps the projection steady until release. Programmatic document replacements refresh the projection, and keyboard movement scrolls the caret into view. Clipboard copy contains the selected Markdown source. Both workspace Paste and platform input-handler paste normalize CRLF/lone CR to LF.

This is an implementation checkpoint, not full visual/platform acceptance. Table layout, link/task interaction, image rendering, accessibility semantics, source-reveal continuity and large-document layout caching still need work. Headless GPUI tests cover formatted Unicode mouse selection/copy, literal paste/undo, empty notes, composition ranges and long-note caret scrolling. Physical macOS/Linux input, IME and performance evidence remain required.
