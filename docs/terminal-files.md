# Reading and editing files in the terminal

The file sidebar includes Markdown, YAML/YML, PDF and HTML/HTM. Enter opens the selected file; the usual Vim, pointer selection and clipboard actions remain available. `Space l c` copies the selected text in normal or visual mode; Ctrl+C is an alternative.

Markdown and YAML are editable. Save with `:w` or Ctrl+S. Markdown retains its frontmatter and existing update/newline policy. YAML is literal source: comments, document separators, indentation, invalid syntax and a missing trailing newline are retained. It is never parsed and reserialized during an editor save. CRLF input is normalized to LF in the editor.

Save refuses a conflicting external change and retains the buffer. `Space l S` saves a copy with the original extension, keeping both versions. Undo/redo share the same history across explicit saves and save-triggered file notifications. A clean externally changed file reloads; a dirty buffer stays intact.

PDF and HTML references are read-only. PDF text has page labels and separators. Pages without extractable text say so; use the desktop page reader for scanned/image content. HTML uses a static text representation with headings, paragraphs, lists, tables, links and code. Scripts/styles are excluded; opening a reference does not execute them or fetch web content. Paste and save cannot modify these reference files.

The CLI/MCP `read` content contract is unchanged: PDF page labels and the HTML reading representation are terminal presentation only. The desktop PDF renderer has separate [runtime packaging](pdf-runtime.md).

These readers are still being polished. Terminal reference extraction currently runs during opening, so very large PDFs need further work on cancellable background loading. Native platform, full visual and large-file performance acceptance are tracked separately from the PTY regression suite.
