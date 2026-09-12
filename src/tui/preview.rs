//! Markdown preview: pulldown-cmark events → styled ratatui `Line`s.
//! Wrapping is left to `Paragraph::wrap`, so each logical block is one Line.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::theme;

/// Source-like readers retain every character, including YAML document markers
/// and literal Markdown-looking text in extracted PDFs.
pub fn for_file(path: &str, text: &str) -> Vec<Line<'static>> {
    match crate::notes::kind_of(path) {
        crate::notes::Kind::Yaml => text
            .split('\n')
            .map(|line| {
                if line.trim_start().starts_with('#') {
                    Line::from(Span::styled(line.to_string(), theme::dim()))
                } else if let Some(colon) = line
                    .find(':')
                    .filter(|i| line[*i + 1..].starts_with(char::is_whitespace) || *i + 1 == line.len())
                {
                    Line::from(vec![
                        Span::styled(
                            line[..colon + 1].to_string(),
                            Style::default().fg(theme::current().regent),
                        ),
                        Span::styled(line[colon + 1..].to_string(), theme::code()),
                    ])
                } else {
                    Line::from(Span::styled(line.to_string(), theme::code()))
                }
            })
            .collect(),
        crate::notes::Kind::Pdf => text
            .split('\n')
            .map(|line| {
                Line::from(Span::styled(
                    line.to_string(),
                    if line.starts_with("Page ") {
                        theme::link()
                    } else {
                        Style::default().fg(theme::cream())
                    },
                ))
            })
            .collect(),
        _ => render(text),
    }
}

#[derive(Default)]
struct State {
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    style: Style,
    list_stack: Vec<Option<u64>>,
    in_code: bool,
    quote: usize,
    in_table: bool,
    row: Vec<String>,
    cell: String,
    link: Option<String>,
    heading: Option<u8>,
}

impl State {
    fn prefix(&self) -> String {
        let mut p = String::new();
        for _ in 0..self.quote {
            p.push_str("│ ");
        }
        p
    }
    fn push_text(&mut self, text: &str) {
        if self.in_table {
            self.cell.push_str(text);
            return;
        }
        let style = if self.in_code { theme::code() } else { self.style };
        // Highlight [[wikilinks]] inline.
        let mut rest = text;
        while let Some(start) = rest.find("[[") {
            let (before, after) = rest.split_at(start);
            if !before.is_empty() {
                self.cur.push(Span::styled(before.to_string(), style));
            }
            if let Some(end) = after.find("]]") {
                self.cur.push(Span::styled(after[..end + 2].to_string(), theme::link()));
                rest = &after[end + 2..];
            } else {
                self.cur.push(Span::styled(after.to_string(), style));
                rest = "";
            }
        }
        if !rest.is_empty() {
            self.cur.push(Span::styled(rest.to_string(), style));
        }
    }
    fn flush(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let mut spans = Vec::with_capacity(self.cur.len() + 1);
        let prefix = self.prefix();
        if !prefix.is_empty() {
            spans.push(Span::styled(prefix, theme::dim()));
        }
        spans.append(&mut self.cur);
        self.lines.push(Line::from(spans));
    }
    fn blank(&mut self) {
        if !self.lines.is_empty() && !self.lines.last().is_some_and(|l| l.spans.is_empty()) {
            self.lines.push(Line::default());
        }
    }
}

pub fn render(md: &str) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    let base = Style::default().fg(theme::cream());
    let mut st = State { style: base, ..Default::default() };
    for ev in Parser::new_ext(md, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    st.blank();
                    let lvl = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    st.heading = Some(lvl);
                    st.style = theme::heading(lvl);
                    st.cur.push(Span::styled(format!("{} ", "#".repeat(lvl as usize)), theme::dim()));
                }
                Tag::Paragraph => {}
                Tag::BlockQuote(_) => {
                    st.quote += 1;
                    st.style = Style::default().fg(theme::current().regent).add_modifier(Modifier::ITALIC);
                }
                Tag::CodeBlock(kind) => {
                    st.flush();
                    st.blank();
                    st.in_code = true;
                    if let CodeBlockKind::Fenced(lang) = kind
                        && !lang.is_empty()
                    {
                        st.lines.push(Line::from(Span::styled(format!("┌ {lang}"), theme::dim())));
                    }
                }
                Tag::List(start) => {
                    if st.list_stack.is_empty() {
                        st.flush();
                    }
                    st.list_stack.push(start);
                }
                Tag::Item => {
                    st.flush();
                    let depth = st.list_stack.len().saturating_sub(1);
                    let marker = match st.list_stack.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            m
                        }
                        _ => "• ".to_string(),
                    };
                    st.cur.push(Span::styled(format!("{}{marker}", "  ".repeat(depth)), theme::chrome()));
                }
                Tag::Emphasis => st.style = st.style.add_modifier(Modifier::ITALIC),
                Tag::Strong => st.style = st.style.add_modifier(Modifier::BOLD),
                Tag::Strikethrough => st.style = st.style.add_modifier(Modifier::CROSSED_OUT),
                Tag::Link { dest_url, .. } => {
                    st.link = Some(dest_url.to_string());
                    st.style = theme::link();
                }
                Tag::Image { dest_url, .. } => {
                    st.cur.push(Span::styled(format!("🖼 {dest_url}"), theme::dim()));
                }
                Tag::Table(_) => {
                    st.flush();
                    st.blank();
                    st.in_table = true;
                }
                Tag::TableHead | Tag::TableRow => st.row.clear(),
                Tag::TableCell => st.cell.clear(),
                Tag::HtmlBlock => st.style = theme::dim(),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    st.flush();
                    st.heading = None;
                    st.style = base;
                    st.blank();
                }
                TagEnd::Paragraph => {
                    st.flush();
                    if st.list_stack.is_empty() {
                        st.blank();
                    }
                }
                TagEnd::BlockQuote(_) => {
                    st.flush();
                    st.quote = st.quote.saturating_sub(1);
                    st.style = base;
                    st.blank();
                }
                TagEnd::CodeBlock => {
                    st.flush();
                    st.in_code = false;
                    st.blank();
                }
                TagEnd::List(_) => {
                    st.flush();
                    st.list_stack.pop();
                    if st.list_stack.is_empty() {
                        st.blank();
                    }
                }
                TagEnd::Item => st.flush(),
                TagEnd::Emphasis => st.style = st.style.remove_modifier(Modifier::ITALIC),
                TagEnd::Strong => st.style = st.style.remove_modifier(Modifier::BOLD),
                TagEnd::Strikethrough => st.style = st.style.remove_modifier(Modifier::CROSSED_OUT),
                TagEnd::Link => {
                    if let Some(url) = st.link.take()
                        && !url.starts_with('#')
                    {
                        st.cur.push(Span::styled(format!(" ({url})"), theme::dim()));
                    }
                    st.style =
                        if st.heading.is_some() { theme::heading(st.heading.unwrap_or(1)) } else { base };
                }
                TagEnd::TableCell => {
                    let c = std::mem::take(&mut st.cell);
                    st.row.push(c);
                }
                TagEnd::TableHead => {
                    let row = st.row.join(" │ ");
                    st.lines.push(Line::from(Span::styled(row, theme::accent())));
                    st.lines.push(Line::from(Span::styled("─".repeat(40), theme::dim())));
                }
                TagEnd::TableRow => {
                    let row = st.row.join(" │ ");
                    st.lines.push(Line::from(Span::styled(row, base)));
                }
                TagEnd::Table => {
                    st.in_table = false;
                    st.blank();
                }
                TagEnd::HtmlBlock => st.style = base,
                _ => {}
            },
            Event::Text(t) => {
                if st.in_code {
                    for (i, l) in t.lines().enumerate() {
                        if i > 0 {
                            st.flush();
                        }
                        st.cur.push(Span::styled(format!("  {l}"), theme::code()));
                    }
                    if t.ends_with('\n') {
                        st.flush();
                    }
                } else {
                    st.push_text(&t);
                }
            }
            Event::Code(c) => st.cur.push(Span::styled(format!("`{c}`"), theme::code())),
            Event::Html(h) | Event::InlineHtml(h) => st.cur.push(Span::styled(h.to_string(), theme::dim())),
            Event::SoftBreak => st.push_text(" "),
            Event::HardBreak => st.flush(),
            Event::Rule => {
                st.flush();
                st.lines.push(Line::from(Span::styled("─".repeat(60), theme::dim())));
                st.blank();
            }
            Event::TaskListMarker(done) => {
                let (glyph, style) = if done {
                    ("[x] ", Style::default().fg(theme::current().ok))
                } else {
                    ("[ ] ", Style::default().fg(theme::gold()))
                };
                st.cur.push(Span::styled(glyph, style));
            }
            Event::FootnoteReference(r) => st.cur.push(Span::styled(format!("[^{r}]"), theme::dim())),
            _ => {}
        }
    }
    st.flush();
    while st.lines.last().is_some_and(|l| l.spans.is_empty()) {
        st.lines.pop();
    }
    st.lines
}

/// Heading outline: (line index in the rendered preview, level, text).
pub fn outline(md: &str) -> Vec<(usize, u8, String)> {
    let mut out = Vec::new();
    for (i, l) in md.lines().enumerate() {
        let t = l.trim_start();
        let level = t.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && t.chars().nth(level) == Some(' ') {
            out.push((i, level as u8, t[level + 1..].trim().to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect::<String>()).collect()
    }

    #[test]
    fn renders_headings_lists_tasks_code_and_links() {
        let md = "# Title\n\nSome *em* and **strong** text with [[Wiki Link]] and [a](b.md).\n\n- one\n- [x] done\n- [ ] open\n  - nested\n\n```rust\nfn x() {}\n```\n\n> quoted\n\n---\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let lines = render(md);
        let t = text(&lines);
        assert_eq!(t[0], "# Title");
        assert!(
            t.iter().any(|l| l.contains("Some em and strong text with [[Wiki Link]] and a (b.md)")),
            "{t:?}"
        );
        assert!(t.iter().any(|l| l == "• one"), "{t:?}");
        assert!(t.iter().any(|l| l == "• [x] done"), "{t:?}");
        assert!(t.iter().any(|l| l == "  • nested"), "{t:?}");
        assert!(t.iter().any(|l| l == "┌ rust"), "{t:?}");
        assert!(t.iter().any(|l| l == "  fn x() {}"), "{t:?}");
        assert!(t.iter().any(|l| l == "│ quoted"), "{t:?}");
        assert!(t.iter().any(|l| l.starts_with("────")), "{t:?}");
        assert!(t.iter().any(|l| l == "a │ b"), "{t:?}");
        assert!(t.iter().any(|l| l == "1 │ 2"), "{t:?}");
        assert!(lines[0].spans.iter().any(|s| s.style.fg == Some(theme::gold())), "h1 is gold");
    }

    #[test]
    fn outline_finds_headings() {
        let o = outline("# A\ntext\n## B\n#notaheading\n### C\n");
        assert_eq!(o, vec![(0, 1, "A".into()), (2, 2, "B".into()), (4, 3, "C".into())]);
    }
}
