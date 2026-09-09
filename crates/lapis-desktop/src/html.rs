//! HTML renderer: tag soup → a small layout tree → text.
//!
//! Deliberately not a browser: no scripts, no stylesheets, no fetching, no
//! DOM API. It understands the structural subset a captured page or a note
//! export needs (headings, paragraphs, lists, links, emphasis, code, breaks,
//! tables as rows) and lays it out as blocks with inline runs, which either
//! the terminal or the GPUI canvas can paint.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Strong(String),
    Em(String),
    Code(String),
    /// (text, href)
    Link(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Paragraph(Vec<Inline>),
    ListItem {
        ordered: bool,
        index: usize,
        runs: Vec<Inline>,
    },
    Pre(String),
    Quote(Vec<Inline>),
    /// One table row, cells already flattened to text.
    Row(Vec<String>),
    Rule,
}

/// A parsed document: block sequence plus the `<title>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

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
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" | "#39" => Some("'".to_string()),
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

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            // keep one leading space: "<b>x</b> and" needs it; block starts are trimmed by the caller
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

#[derive(Debug)]
enum Tok<'a> {
    Text(&'a str),
    /// (name, attrs, closing)
    Tag(String, &'a str, bool),
}

fn tokenize(html: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    let b = html.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            if html[i..].starts_with("<!--") {
                i = html[i..].find("-->").map(|e| i + e + 3).unwrap_or(b.len());
                continue;
            }
            if html[i..].starts_with("<!") || html[i..].starts_with("<?") {
                i = html[i..].find('>').map(|e| i + e + 1).unwrap_or(b.len());
                continue;
            }
            let Some(end) = html[i..].find('>') else { break };
            let inner = &html[i + 1..i + end];
            let closing = inner.starts_with('/');
            let inner = inner.trim_start_matches('/').trim_end_matches('/').trim();
            let (name, attrs) = match inner.find(char::is_whitespace) {
                Some(p) => (&inner[..p], inner[p..].trim()),
                None => (inner, ""),
            };
            let name = name.to_ascii_lowercase();
            i += end + 1;
            // raw-text elements: swallow until the close tag
            if !closing && (name == "script" || name == "style") {
                let close = format!("</{name}");
                let lower = html[i..].to_ascii_lowercase();
                i = lower.find(&close).map(|e| i + e).unwrap_or(b.len());
                continue;
            }
            out.push(Tok::Tag(name, attrs, closing));
        } else {
            let end = html[i..].find('<').map(|e| i + e).unwrap_or(b.len());
            out.push(Tok::Text(&html[i..end]));
            i = end;
        }
    }
    out
}

fn attr(attrs: &str, key: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let pos = lower.find(&format!("{key}="))?;
    let v = &attrs[pos + key.len() + 1..];
    let v = v.trim_start();
    let (q, rest) = match v.chars().next() {
        Some(c @ ('"' | '\'')) => (Some(c), &v[1..]),
        _ => (None, v),
    };
    let end = match q {
        Some(c) => rest.find(c).unwrap_or(rest.len()),
        None => rest.find(char::is_whitespace).unwrap_or(rest.len()),
    };
    Some(decode_entities(&rest[..end]))
}

/// Parse an HTML string into blocks. Never fails: unknown tags are transparent.
pub fn parse(html: &str) -> Document {
    let mut doc = Document::default();
    let mut runs: Vec<Inline> = Vec::new();
    let mut heading: Option<u8> = None;
    let mut in_title = false;
    let mut in_pre = false;
    let mut pre_buf = String::new();
    let mut in_quote = false;
    let mut in_code = false;
    let mut strong = 0usize;
    let mut em = 0usize;
    let mut link: Option<String> = None;
    let mut list_stack: Vec<(bool, usize)> = Vec::new();
    let mut in_li = false;
    let mut in_table = false;
    let mut row: Vec<String> = Vec::new();
    let mut cell: Option<String> = None;
    let mut skip_depth = 0usize; // <head> etc.

    fn flush(
        doc: &mut Document,
        runs: &mut Vec<Inline>,
        heading: &mut Option<u8>,
        in_quote: bool,
        li: Option<(bool, usize)>,
    ) {
        if runs.is_empty() {
            *heading = None;
            return;
        }
        let r = std::mem::take(runs);
        let block = match (heading.take(), li, in_quote) {
            (Some(l), _, _) => Block::Heading(l, r),
            (None, Some((ordered, index)), _) => Block::ListItem { ordered, index, runs: r },
            (None, None, true) => Block::Quote(r),
            _ => Block::Paragraph(r),
        };
        doc.blocks.push(block);
    }
    fn push_text(
        runs: &mut Vec<Inline>,
        text: String,
        strong: bool,
        em: bool,
        code: bool,
        link: &Option<String>,
    ) {
        if text.is_empty() {
            return;
        }
        let run = match (link, code, strong, em) {
            (Some(h), _, _, _) => Inline::Link(text, h.clone()),
            (None, true, _, _) => Inline::Code(text),
            (None, false, true, _) => Inline::Strong(text),
            (None, false, false, true) => Inline::Em(text),
            _ => Inline::Text(text),
        };
        // merge adjacent plain text
        if let (Some(Inline::Text(prev)), Inline::Text(t)) = (runs.last_mut(), &run) {
            prev.push_str(t);
            return;
        }
        runs.push(run);
    }

    for tok in tokenize(html) {
        match tok {
            Tok::Text(t) => {
                if skip_depth > 0 && !in_title {
                    continue;
                }
                if in_title {
                    let s = collapse_ws(&decode_entities(t));
                    if !s.is_empty() {
                        doc.title = Some(match doc.title.take() {
                            Some(mut p) => {
                                p.push_str(&s);
                                p
                            }
                            None => s,
                        });
                    }
                    continue;
                }
                if in_pre {
                    pre_buf.push_str(&decode_entities(t));
                    continue;
                }
                let s = collapse_ws(&decode_entities(t));
                if s.trim().is_empty() {
                    if !runs.is_empty() && !s.is_empty() {
                        push_text(&mut runs, " ".into(), false, false, false, &None);
                    }
                    continue;
                }
                if let Some(c) = cell.as_mut() {
                    c.push_str(&s);
                    continue;
                }
                let s = if runs.is_empty() { s.trim_start().to_string() } else { s };
                push_text(&mut runs, s, strong > 0, em > 0, in_code, &link);
            }
            Tok::Tag(name, attrs, closing) => match (name.as_str(), closing) {
                ("head", false) => skip_depth += 1,
                ("head", true) => skip_depth = skip_depth.saturating_sub(1),
                ("title", c) => in_title = !c,
                _ if skip_depth > 0 => {}
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                    flush(&mut doc, &mut runs, &mut heading, in_quote, None);
                    heading = Some(name.as_bytes()[1] - b'0');
                }
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                    flush(&mut doc, &mut runs, &mut heading, in_quote, None);
                }
                ("p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside", _) => {
                    let li = if in_li { list_stack.last().copied() } else { None };
                    flush(&mut doc, &mut runs, &mut heading, in_quote, li);
                }
                ("br", _) => push_text(&mut runs, "\n".into(), false, false, false, &None),
                ("hr", _) => {
                    flush(&mut doc, &mut runs, &mut heading, in_quote, None);
                    doc.blocks.push(Block::Rule);
                }
                ("pre", false) => {
                    flush(&mut doc, &mut runs, &mut heading, in_quote, None);
                    in_pre = true;
                    pre_buf.clear();
                }
                ("pre", true) => {
                    in_pre = false;
                    doc.blocks.push(Block::Pre(pre_buf.trim_matches('\n').to_string()));
                    pre_buf.clear();
                }
                ("code" | "kbd" | "samp" | "tt", c) if !in_pre => in_code = !c,
                ("strong" | "b", c) => strong = if c { strong.saturating_sub(1) } else { strong + 1 },
                ("em" | "i", c) => em = if c { em.saturating_sub(1) } else { em + 1 },
                ("a", false) => link = attr(attrs, "href"),
                ("a", true) => link = None,
                ("blockquote", c) => {
                    flush(&mut doc, &mut runs, &mut heading, in_quote, None);
                    in_quote = !c;
                }
                ("ul" | "ol", false) => {
                    flush(&mut doc, &mut runs, &mut heading, in_quote, None);
                    list_stack.push((name == "ol", 0));
                }
                ("ul" | "ol", true) => {
                    let li = if in_li { list_stack.last().copied() } else { None };
                    flush(&mut doc, &mut runs, &mut heading, in_quote, li);
                    list_stack.pop();
                    in_li = false;
                }
                ("li", false) => {
                    let prev = if in_li { list_stack.last().copied() } else { None };
                    flush(&mut doc, &mut runs, &mut heading, in_quote, prev);
                    if let Some(top) = list_stack.last_mut() {
                        top.1 += 1;
                    } else {
                        list_stack.push((false, 1));
                    }
                    in_li = true;
                }
                ("li", true) => {
                    let li = list_stack.last().copied();
                    flush(&mut doc, &mut runs, &mut heading, in_quote, li);
                    in_li = false;
                }
                ("table", c) => {
                    flush(&mut doc, &mut runs, &mut heading, in_quote, None);
                    in_table = !c;
                }
                ("tr", false) if in_table => row.clear(),
                ("tr", true) if in_table && !row.is_empty() => {
                    doc.blocks.push(Block::Row(std::mem::take(&mut row)));
                }
                ("td" | "th", false) if in_table => cell = Some(String::new()),
                ("td" | "th", true) if in_table => {
                    let c = cell.take().unwrap_or_default();
                    row.push(c.trim().to_string());
                }
                ("img", false) => {
                    let alt = attr(attrs, "alt").unwrap_or_default();
                    if !alt.is_empty() {
                        push_text(&mut runs, format!("[{alt}]"), false, false, false, &None);
                    }
                }
                _ => {}
            },
        }
    }
    let li = if in_li { list_stack.last().copied() } else { None };
    flush(&mut doc, &mut runs, &mut heading, in_quote, li);
    // trim trailing spaces on runs
    for b in &mut doc.blocks {
        if let Block::Paragraph(r) | Block::Heading(_, r) | Block::Quote(r) | Block::ListItem { runs: r, .. } =
            b
            && let Some(Inline::Text(t)) = r.last_mut()
        {
            let trimmed = t.trim_end().to_string();
            *t = trimmed;
        }
    }
    doc
}

/// Flatten inline runs to text (links as `text (href)`).
pub fn inline_text(runs: &[Inline]) -> String {
    let mut s = String::new();
    for r in runs {
        match r {
            Inline::Text(t) | Inline::Strong(t) | Inline::Em(t) | Inline::Code(t) => s.push_str(t),
            Inline::Link(t, h) => {
                s.push_str(t);
                if !h.is_empty() && h != t {
                    s.push_str(&format!(" ({h})"));
                }
            }
        }
    }
    s
}

/// One line-oriented layout step: blocks → lines wrapped at `width` columns.
/// This is what both the terminal preview and the canvas text pass consume.
pub fn layout(doc: &Document, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    let wrap = |s: &str, indent: &str, out: &mut Vec<String>| {
        for para in s.split('\n') {
            let mut line = String::from(indent);
            let mut started = false;
            for word in para.split_whitespace() {
                if started && line.chars().count() + 1 + word.chars().count() > width {
                    out.push(std::mem::replace(&mut line, indent.to_string()));
                    started = false;
                }
                if started {
                    line.push(' ');
                }
                line.push_str(word);
                started = true;
            }
            out.push(line);
        }
    };
    for b in &doc.blocks {
        match b {
            Block::Heading(l, r) => {
                let t = inline_text(r);
                out.push(format!("{} {t}", "#".repeat(*l as usize)));
                out.push(String::new());
            }
            Block::Paragraph(r) => {
                wrap(&inline_text(r), "", &mut out);
                out.push(String::new());
            }
            Block::Quote(r) => {
                wrap(&inline_text(r), "> ", &mut out);
                out.push(String::new());
            }
            Block::ListItem { ordered, index, runs } => {
                let bullet = if *ordered { format!("{index}. ") } else { "• ".into() };
                let text = inline_text(runs);
                let mut lines = Vec::new();
                wrap(&text, "  ", &mut lines);
                if let Some(first) = lines.first_mut() {
                    *first = format!("{bullet}{}", first.trim_start());
                }
                out.extend(lines);
            }
            Block::Pre(code) => {
                out.push("```".into());
                out.extend(code.lines().map(str::to_string));
                out.push("```".into());
                out.push(String::new());
            }
            Block::Row(cells) => out.push(cells.join(" | ")),
            Block::Rule => {
                out.push("-".repeat(width.min(40)));
                out.push(String::new());
            }
        }
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

/// Convenience: HTML → wrapped plain text.
pub fn to_text(html: &str, width: usize) -> String {
    layout(&parse(html), width).join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<!doctype html><html><head><title>Hedron &amp; Lattice</title>
<style>body{color:red}</style><script>alert(1)</script></head>
<body><h1>Title  here</h1><p>First <b>bold</b> and <a href="https://x.test/a">a link</a>.<br>Second line</p>
<ul><li>one</li><li>two <em>soft</em></li></ul><ol><li>first</li><li>second</li></ol>
<pre><code>let x = 1;
let y = 2;</code></pre><blockquote>quoted &lt;text&gt;</blockquote>
<table><tr><th>k</th><th>v</th></tr><tr><td>a</td><td>1</td></tr></table><hr><p>&#x2014; end &nbsp;</p><img alt="diagram" src="x.png"></body></html>"#;

    #[test]
    fn parses_structure_and_ignores_scripts_styles() {
        let d = parse(PAGE);
        assert_eq!(d.title.as_deref(), Some("Hedron & Lattice"));
        assert!(matches!(&d.blocks[0], Block::Heading(1, r) if inline_text(r) == "Title here"));
        let Block::Paragraph(r) = &d.blocks[1] else { panic!("{:?}", d.blocks[1]) };
        assert_eq!(
            r,
            &[
                Inline::Text("First ".into()),
                Inline::Strong("bold".into()),
                Inline::Text(" and ".into()),
                Inline::Link("a link".into(), "https://x.test/a".into()),
                Inline::Text(".\nSecond line".into()),
            ]
        );
        let items: Vec<(bool, usize, String)> = d
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem { ordered, index, runs } => Some((*ordered, *index, inline_text(runs))),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            [
                (false, 1, "one".into()),
                (false, 2, "two soft".into()),
                (true, 1, "first".into()),
                (true, 2, "second".into())
            ]
        );
        assert!(d.blocks.iter().any(|b| matches!(b, Block::Pre(c) if c == "let x = 1;\nlet y = 2;")));
        assert!(d.blocks.iter().any(|b| matches!(b, Block::Quote(r) if inline_text(r) == "quoted <text>")));
        let rows: Vec<&Vec<String>> =
            d.blocks.iter().filter_map(|b| if let Block::Row(c) = b { Some(c) } else { None }).collect();
        assert_eq!(rows, [&vec!["k".to_string(), "v".to_string()], &vec!["a".to_string(), "1".to_string()]]);
        assert!(d.blocks.contains(&Block::Rule));
        let text = to_text(PAGE, 80);
        assert!(!text.contains("alert") && !text.contains("color:red"), "no script/style leakage");
        assert!(text.contains("— end") && text.contains("[diagram]"));
        assert!(text.contains("a link (https://x.test/a)"));
    }

    #[test]
    fn layout_wraps_and_never_panics_on_garbage() {
        let d = parse("<p>alpha beta gamma delta epsilon zeta</p>");
        let lines = layout(&d, 12);
        assert_eq!(lines, ["alpha beta", "gamma delta", "epsilon zeta"]);
        for junk in
            ["<", "<<>>", "</p></div>", "&", "&#xZZ;", "<a href=x", "<pre>unclosed", "<ul><li>a<li>b</ul>"]
        {
            let _ = to_text(junk, 20);
        }
        assert_eq!(to_text("<ul><li>a<li>b</ul>", 20), "• a\n• b");
        assert_eq!(decode_entities("a &amp; b &#65; &unknown; &"), "a & b A &unknown; &");
    }
}
