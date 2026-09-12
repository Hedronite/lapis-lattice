//! UTF-8 source motions shared by the native editor's command layer.
use unicode_segmentation::UnicodeSegmentation;

pub fn previous(s: &str, p: usize) -> usize {
    s[..p].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
}
pub fn next(s: &str, p: usize) -> usize {
    s[p..].graphemes(true).next().map_or(p, |g| p + g.len())
}
pub fn start(s: &str, p: usize) -> usize {
    s[..p].rfind('\n').map_or(0, |i| i + 1)
}
pub fn end(s: &str, p: usize) -> usize {
    s[p..].find('\n').map_or(s.len(), |i| p + i)
}
pub fn first(s: &str, p: usize) -> usize {
    let a = start(s, p);
    a + s[a..end(s, p)].find(|c: char| !c.is_whitespace()).unwrap_or(0)
}
pub fn vertical(s: &str, p: usize, down: bool, count: usize) -> usize {
    let a = start(s, p);
    let column = s[a..p].graphemes(true).count();
    let mut line = a;
    for _ in 0..count {
        if down {
            let e = end(s, line);
            if e == s.len() {
                break;
            }
            line = e + 1;
        } else if line > 0 {
            line = start(s, line - 1);
        } else {
            break;
        }
    }
    let e = end(s, line);
    s[line..e].grapheme_indices(true).nth(column).map_or(e, |(i, _)| line + i)
}
fn class(s: &str, p: usize, big: bool) -> u8 {
    match s[p..].chars().next() {
        None => 0,
        Some(c) if c.is_whitespace() => 0,
        Some(c) if big || c.is_alphanumeric() || c == '_' => 1,
        _ => 2,
    }
}
pub fn word(s: &str, mut p: usize, key: char) -> usize {
    let big = key.is_uppercase();
    match key.to_ascii_lowercase() {
        'w' => {
            let category = class(s, p, big);
            while p < s.len() && class(s, p, big) == category {
                p = next(s, p);
            }
            while p < s.len() && class(s, p, big) == 0 {
                p = next(s, p);
            }
        }
        'b' => {
            p = previous(s, p);
            while p > 0 && class(s, p, big) == 0 {
                p = previous(s, p);
            }
            let category = class(s, p, big);
            while p > 0 && class(s, previous(s, p), big) == category {
                p = previous(s, p);
            }
        }
        'e' => {
            p = next(s, p);
            while p < s.len() && class(s, p, big) == 0 {
                p = next(s, p);
            }
            let category = class(s, p, big);
            while p < s.len() && class(s, next(s, p), big) == category && next(s, p) < s.len() {
                p = next(s, p);
            }
        }
        _ => {}
    }
    p
}
pub fn line_range(s: &str, p: usize, count: usize) -> std::ops::Range<usize> {
    let a = start(s, p);
    let mut e = a;
    for _ in 0..count {
        e = next(s, end(s, e));
        if e == s.len() {
            break;
        }
    }
    a..e
}

pub fn word_end_here(s: &str, mut p: usize, big: bool) -> usize {
    let category = class(s, p, big);
    while next(s, p) < s.len() && class(s, next(s, p), big) == category {
        p = next(s, p);
    }
    p
}

pub fn normal_cursor(s: &str, p: usize) -> usize {
    if p == end(s, p) && p > start(s, p) { previous(s, p) } else { p }
}

/// Clamp stale native/undo offsets before source slicing.
pub fn clip(s: &str, p: usize) -> usize {
    let mut p = p.min(s.len());
    while !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}
