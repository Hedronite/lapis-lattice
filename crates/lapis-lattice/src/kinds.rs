//! What a file is, and how to get retrievable text out of it.
//!
//! Deliberately separate from the desktop HTML renderer: that one builds a
//! layout tree for drawing, this one wants headings and prose for FTS. Sharing
//! a type between them would make the index depend on a UI crate to answer
//! "what words are in this file".
//!
//! Nothing here is a browser. No scripts run, no styles are applied, nothing is
//! fetched. Tags become text and headings become chunk boundaries.

/// Values of the `documents.kind` column and the `kind` field on a search hit.
pub const MARKDOWN: &str = "markdown";
pub const HTML: &str = "html";
pub const YAML: &str = "yaml";

/// The kind for a vault-relative path, or `None` when it is not indexable.
pub fn kind_for(rel: &str) -> Option<&'static str> {
    let ext = rel.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => Some(MARKDOWN),
        "html" | "htm" => Some(HTML),
        "yaml" | "yml" => Some(YAML),
        _ => None,
    }
}

/// Body text plus retrievable chunks. `(heading, text)` pairs, in file order.
pub struct Extracted {
    pub title: Option<String>,
    pub body: String,
    pub chunks: Vec<(Option<String>, String)>,
    /// Top-level scalars usable as metadata: markdown frontmatter, or the YAML
    /// document itself. Empty for HTML.
    pub meta: std::collections::BTreeMap<String, String>,
}

// ---------------------------------------------------------------------- HTML

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let rep = match ent {
            "amp" => Some("&".into()),
            "lt" => Some("<".into()),
            "gt" => Some(">".into()),
            "quot" => Some("\"".into()),
            "apos" | "#39" => Some("'".into()),
            "nbsp" => Some(" ".to_string()),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| {
                    if let Some(h) = n.strip_prefix(['x', 'X']) {
                        u32::from_str_radix(h, 16).ok()
                    } else {
                        n.parse().ok()
                    }
                })
                .and_then(char::from_u32)
                .map(|c| c.to_string()),
        };
        match rep {
            Some(r) => {
                out.push_str(&r);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !space && !out.is_empty() {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out.trim().to_string()
}

/// Strip chrome and split on headings. `<script>`, `<style>`, comments and
/// doctypes never reach the index.
pub fn extract_html(raw: &str) -> Extracted {
    let mut title: Option<String> = None;
    let mut chunks: Vec<(Option<String>, String)> = Vec::new();
    let mut heading: Option<String> = None;
    let mut buf = String::new();
    let mut in_title = false;
    let mut in_heading = false;
    let mut head_buf = String::new();

    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // comments and declarations carry nothing we want
            if raw[i..].starts_with("<!--") {
                i = raw[i..].find("-->").map(|e| i + e + 3).unwrap_or(bytes.len());
                continue;
            }
            if raw[i..].starts_with("<!") || raw[i..].starts_with("<?") {
                i = raw[i..].find('>').map(|e| i + e + 1).unwrap_or(bytes.len());
                continue;
            }
            let Some(end) = raw[i..].find('>') else { break };
            let inner = &raw[i + 1..i + end];
            let closing = inner.starts_with('/');
            let name: String = inner
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            i += end + 1;

            // raw-text elements: swallow their contents entirely
            if !closing && (name == "script" || name == "style") {
                let close = format!("</{name}");
                let lower = raw[i..].to_ascii_lowercase();
                i = lower.find(&close).map(|e| i + e).unwrap_or(bytes.len());
                continue;
            }
            match (name.as_str(), closing) {
                ("title", c) => in_title = !c,
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                    if !buf.trim().is_empty() {
                        chunks.push((heading.take(), squeeze(&buf)));
                        buf.clear();
                    }
                    in_heading = true;
                    head_buf.clear();
                }
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                    in_heading = false;
                    let h = squeeze(&head_buf);
                    if !h.is_empty() {
                        heading = Some(h.clone());
                        // the heading is part of the chunk's searchable text
                        buf.push_str(&h);
                        buf.push('\n');
                    }
                }
                // block-ish tags become whitespace so words do not run together
                ("p" | "div" | "br" | "li" | "tr" | "td" | "th" | "section" | "article", _) => buf.push('\n'),
                _ => buf.push(' '),
            }
            continue;
        }
        let end = raw[i..].find('<').map(|e| i + e).unwrap_or(bytes.len());
        let text = decode_entities(&raw[i..end]);
        if in_title {
            title.get_or_insert_with(String::new).push_str(&text);
        } else if in_heading {
            head_buf.push_str(&text);
        } else {
            buf.push_str(&text);
        }
        i = end;
    }
    if !buf.trim().is_empty() {
        chunks.push((heading, squeeze(&buf)));
    }
    let title = title
        .map(|t| squeeze(&t))
        .filter(|t| !t.is_empty())
        .or_else(|| chunks.iter().find_map(|(h, _)| h.clone()));
    let body = chunks.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n\n");
    if chunks.is_empty() {
        chunks.push((None, String::new()));
    }
    Extracted { title, body, chunks, meta: Default::default() }
}

// ---------------------------------------------------------------------- YAML

/// A vault YAML note is its own metadata: top-level scalars are read the way
/// markdown frontmatter is, and each top-level key becomes a chunk so a large
/// config stays retrievable by section.
pub fn extract_yaml(raw: &str) -> Extracted {
    let mut meta = std::collections::BTreeMap::new();
    let mut chunks: Vec<(Option<String>, String)> = Vec::new();
    let mut key: Option<String> = None;
    let mut buf = String::new();

    for line in raw.lines() {
        let is_top = !line.starts_with([' ', '\t', '-']) && line.contains(':') && !line.starts_with('#');
        if is_top {
            if let Some(k) = key.take()
                && !buf.trim().is_empty()
            {
                chunks.push((Some(k), buf.trim_end().to_string()));
            }
            buf.clear();
            let (k, v) = line.split_once(':').unwrap_or((line, ""));
            let k = k.trim().to_string();
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                meta.insert(k.clone(), v);
            }
            key = Some(k);
        }
        buf.push_str(line);
        buf.push('\n');
    }
    if let Some(k) = key.take()
        && !buf.trim().is_empty()
    {
        chunks.push((Some(k), buf.trim_end().to_string()));
    }
    if chunks.is_empty() {
        chunks.push((None, raw.trim().to_string()));
    }
    let title = meta.get("name").or_else(|| meta.get("title")).cloned();
    Extracted { title, body: raw.to_string(), chunks, meta }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind_for("a/b.md"), Some(MARKDOWN));
        assert_eq!(kind_for("a.MARKDOWN"), Some(MARKDOWN));
        assert_eq!(kind_for("page.HTM"), Some(HTML));
        assert_eq!(kind_for("conf.yml"), Some(YAML));
        assert_eq!(kind_for("main.rs"), None, "source is not the corpus");
        assert_eq!(kind_for("noext"), None);
    }

    #[test]
    fn html_drops_chrome_and_splits_on_headings() {
        let raw = r##"<!doctype html><html><head><title>Hedron &amp; Lattice</title>
        <style>body{color:red}</style><script>alert('x')</script></head>
        <body><h1>First</h1><p>alpha beta</p><h2>Second</h2><p>gamma &lt;delta&gt;</p></body></html>"##;
        let e = extract_html(raw);
        assert_eq!(e.title.as_deref(), Some("Hedron & Lattice"));
        assert!(!e.body.contains("alert") && !e.body.contains("color:red"), "no script or style text");
        let heads: Vec<Option<&str>> = e.chunks.iter().map(|(h, _)| h.as_deref()).collect();
        assert_eq!(heads, [Some("First"), Some("Second")]);
        assert!(e.chunks[0].1.contains("alpha beta"));
        assert!(e.chunks[1].1.contains("gamma <delta>"), "entities decoded");
        // words must not run together where tags were
        assert!(!e.body.contains("Firstalpha"));
    }

    #[test]
    fn html_without_headings_is_one_chunk() {
        let e = extract_html("<p>just prose</p>");
        assert_eq!(e.chunks.len(), 1);
        assert_eq!(e.chunks[0].1, "just prose");
        assert!(e.title.is_none());
    }

    #[test]
    fn yaml_is_its_own_frontmatter() {
        let raw = "name: Deploy plan\nstatus: draft\nsteps:\n  - build\n  - ship\n";
        let e = extract_yaml(raw);
        assert_eq!(e.title.as_deref(), Some("Deploy plan"));
        assert_eq!(e.meta.get("status").map(String::as_str), Some("draft"));
        let heads: Vec<Option<&str>> = e.chunks.iter().map(|(h, _)| h.as_deref()).collect();
        assert_eq!(heads, [Some("name"), Some("status"), Some("steps")]);
        assert!(e.chunks[2].1.contains("- build"), "nested lines stay with their key");
    }
}
