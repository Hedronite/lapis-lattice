//! Guarded in-place edit: `replace` and insert-at-anchor.
//!
//! The needle is matched against the note body only (bytes after the closing
//! frontmatter delimiter, or the whole file when there is no frontmatter).
//! Matching is exact and raw: no regex, no CRLF folding, no trailing-newline
//! normalization. Overlapping occurrences count, so `aa` in `aaa` is two hits.
//! Zero or many hits is [`LapisError::Matches`] (exit 1) and the file is left
//! byte-identical.
//!
//! A real write bumps top-level `updated:` to today, or inserts that line
//! immediately before the closing `---` when the key is absent, and leaves
//! every other frontmatter byte unchanged (including newline style). A note
//! with no frontmatter block is edited as raw text and does not gain one.
//! `--dry-run` does not bump `updated:`, does not write, and returns a diff
//! of that preview. The caller reindexes only this path, and only after a
//! real write.

use std::path::Path;

use crate::error::{LapisError, Result};
use crate::write::{Guard, Written, today};
use crate::{hal, notes};

/// One guarded in-place edit.
#[derive(Debug, Clone)]
pub enum EditOp {
    Replace {
        old: String,
        new: String,
    },
    /// `before` inserts `text` immediately before the anchor; otherwise after it.
    Insert {
        anchor: String,
        text: String,
        before: bool,
    },
}

pub fn edit(root: &Path, rel: &str, op: &EditOp, guard: &Guard) -> Result<Written> {
    let (rel, abs) = notes::resolve(root, rel)?;
    notes::refuse_escaped_symlink(root, &rel)?;
    let bytes = std::fs::read(&abs)?;
    guard.check(&rel, &abs, &bytes)?;
    let original = utf8_text(&rel, &bytes)?;
    let (head, body) = markdown_body(&rel, original)?;
    let preview = format!("{head}{}", apply_op(&rel, body, op)?);
    let staged = if guard.dry_run { preview } else { commit_text(root, &rel, &preview)? };
    if !guard.dry_run {
        write_exact(&rel, &abs, &bytes, &staged)?;
    }
    Ok(edited_value(&rel, original, &staged, &bytes, guard))
}

fn utf8_text<'a>(rel: &str, bytes: &'a [u8]) -> Result<&'a str> {
    std::str::from_utf8(bytes).map_err(|_| LapisError::Usage(format!("{rel}: not UTF-8 text")))
}

fn markdown_body<'a>(rel: &str, text: &'a str) -> Result<(&'a str, &'a str)> {
    if notes::kind_of(rel) != notes::Kind::Markdown {
        return Err(LapisError::Usage(format!("{rel}: edit only supports markdown notes")));
    }
    Ok(hal::raw_parts(text))
}

fn apply_op(rel: &str, body: &str, op: &EditOp) -> Result<String> {
    match op {
        EditOp::Replace { old, new } => {
            let at = one_at(rel, body, old)?;
            Ok(stitch(body, at, at + old.len(), new))
        }
        EditOp::Insert { anchor, text, before } => {
            let at = one_at(rel, body, anchor)?;
            let pos = if *before { at } else { at + anchor.len() };
            Ok(stitch(body, pos, pos, text))
        }
    }
}

/// Byte offset of the only match. Empty needle is a usage error, not "0 matches".
fn one_at(rel: &str, hay: &str, needle: &str) -> Result<usize> {
    if needle.is_empty() {
        return Err(LapisError::Usage(format!("{rel}: match text is empty")));
    }
    match unique_offset(hay.as_bytes(), needle.as_bytes()) {
        Ok(i) if hay.is_char_boundary(i) && hay.is_char_boundary(i + needle.len()) => Ok(i),
        Ok(_) => Err(LapisError::Usage(format!("{rel}: match is not on a character boundary"))),
        Err(n) => Err(match_err(rel, n)),
    }
}

fn match_err(rel: &str, n: usize) -> LapisError {
    LapisError::Matches {
        message: format!("{rel}: {n} matches (need exactly 1); file unchanged"),
        matches: n,
    }
}

/// `Ok(offset)` when `needle` occurs once. `Err(count)` otherwise.
/// The scan steps one byte, so overlapping hits each count.
fn unique_offset(hay: &[u8], needle: &[u8]) -> std::result::Result<usize, usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return Err(0);
    }
    let mut found = None;
    let mut n = 0usize;
    let last = hay.len() - needle.len();
    let mut i = 0usize;
    while i <= last {
        if &hay[i..i + needle.len()] == needle {
            n += 1;
            found.get_or_insert(i);
        }
        i += 1;
    }
    match (n, found) {
        (1, Some(i)) => Ok(i),
        _ => Err(n),
    }
}

fn stitch(body: &str, start: usize, end: usize, insert: &str) -> String {
    let mut out = String::with_capacity(body.len() - (end - start) + insert.len());
    out.push_str(&body[..start]);
    out.push_str(insert);
    out.push_str(&body[end..]);
    out
}

fn commit_text(root: &Path, rel: &str, preview: &str) -> Result<String> {
    crate::rag_gate::apply_on_write(root, rel, &bump_updated(preview))
}

fn write_exact(rel: &str, abs: &Path, expected: &[u8], text: &str) -> Result<()> {
    match crate::safe_file::replace(abs, text.as_bytes(), Some(expected)) {
        Ok(()) => Ok(()),
        Err(e) if e.to_string().contains("symlink") => {
            Err(LapisError::Path(format!("{rel}: symlink escapes the vault")))
        }
        Err(e) if e.to_string().contains("file changed on disk") => {
            Err(LapisError::Usage(format!("{rel}: stale: {e}")))
        }
        Err(e) => Err(e.into()),
    }
}

fn edited_value(rel: &str, original: &str, staged: &str, disk: &[u8], guard: &Guard) -> Written {
    let parsed = hal::parse(staged);
    let title = hal::title_from_hal(&parsed.hal).unwrap_or_else(|| notes::stem_of(rel));
    Written {
        path: rel.to_string(),
        title,
        hal: parsed.hal,
        bytes: staged.len() as u64,
        taxonomy: "unchanged",
        dry_run: guard.dry_run,
        text: guard.dry_run.then(|| staged.to_string()),
        old_hash: Some(notes::content_hash(disk)),
        new_hash: Some(notes::content_hash(staged.as_bytes())),
        hash_algo: Some(notes::HASH_ALGO),
        diff: guard.dry_run.then(|| unified_diff(rel, original, staged)),
    }
}

/// Bump `updated:` only. Other frontmatter bytes, including their newlines, stay.
/// No frontmatter block → the text is unchanged (no header is invented).
/// A missing `updated:` line is inserted immediately before the closing `---`.
fn bump_updated(text: &str) -> String {
    let (head, body) = hal::raw_parts(text);
    if head.is_empty() {
        return text.to_string();
    }
    let nl = if head.contains("\r\n") { "\r\n" } else { "\n" };
    format!("{}{body}", rewrite_updated(head, nl, &today()))
}

fn rewrite_updated(head: &str, nl: &str, today: &str) -> String {
    let lines: Vec<&str> = head.split(nl).collect();
    let close = lines.iter().rposition(|l| *l == "---").unwrap_or(0);
    let mut out = String::with_capacity(head.len() + today.len() + 16);
    let mut done = false;
    for (i, line) in lines.iter().enumerate() {
        if i + 1 == lines.len() && line.is_empty() {
            break;
        }
        if !done && is_updated_line(i, close, line) {
            push_updated(&mut out, today);
            done = true;
        } else if !done && i == close {
            push_updated(&mut out, today);
            out.push_str(nl);
            out.push_str(line);
            done = true;
        } else {
            out.push_str(line);
        }
        if i + 1 < lines.len() {
            out.push_str(nl);
        }
    }
    out
}

fn is_updated_line(i: usize, close: usize, line: &str) -> bool {
    i != 0 && i != close && line.starts_with("updated:")
}

fn push_updated(out: &mut String, today: &str) {
    out.push_str("updated: ");
    out.push_str(today);
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    let (start, a_end, b_end) = diff_bounds(&a, &b);
    let a0 = start.saturating_sub(3);
    let a1 = (a_end + 3).min(a.len());
    let b1 = (b_end + 3).min(b.len());
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    if start == a_end && start == b_end {
        return out;
    }
    out.push_str(&format!("@@ -{},{} +{},{} @@\n", a0 + 1, a1 - a0, a0 + 1, b1 - a0));
    push_hunk(&mut out, ' ', &a[a0..start]);
    push_hunk(&mut out, '-', &a[start..a_end]);
    push_hunk(&mut out, '+', &b[start..b_end]);
    push_hunk(&mut out, ' ', &a[a_end..a1]);
    out
}

fn diff_bounds(a: &[&str], b: &[&str]) -> (usize, usize, usize) {
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut a_end = a.len();
    let mut b_end = b.len();
    while a_end > start && b_end > start && a[a_end - 1] == b[b_end - 1] {
        a_end -= 1;
        b_end -= 1;
    }
    (start, a_end, b_end)
}

fn push_hunk(out: &mut String, sigil: char, lines: &[&str]) {
    for line in lines {
        out.push(sigil);
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
