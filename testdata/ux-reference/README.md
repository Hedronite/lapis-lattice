# Synthetic reference fixtures

Generated for Lapis reader regression tests; no private document content.

- `reference.pdf`: three pages of text, tables and colored shapes. SHA-256 `b3b74b923c47b63e3772444cc8695a094f0324968f78b28fd5d7608a72b1867a`.
- `scanned.pdf`: one page with an image and no extractable text. SHA-256 `7bfc37356d627ea278f5aa00161e858ecbe03d2f7115a3e030638b5019c9f01e`.

The PTY regression opens them as read-only references and checks visible text/extraction limits plus unchanged source bytes after attempted paste/save. CLI/MCP extraction semantics are checked separately.
