use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Before,
    After,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub link: bool,
    pub strike: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    Heading(u8),
    Code,
    Quote,
    List,
    Table,
    Rule,
    Blank,
}
#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub source: Range<usize>,
    pub style: Style,
    /// Exact spans differ only in presentation. Replacements are indivisible source units.
    exact: bool,
}
#[derive(Clone, Debug)]
pub struct Block {
    pub source: Range<usize>,
    pub kind: Kind,
    pub spans: Vec<Span>,
    pub revealed: bool,
}
impl Block {
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
    pub fn source_at(&self, display: usize, bias: Bias) -> usize {
        let mut start = 0;
        for span in &self.spans {
            let end = start + span.text.len();
            if display < end || (display == end && bias == Bias::Before) {
                let relative = display.saturating_sub(start).min(span.text.len());
                return if span.exact {
                    span.source.start + clip(&span.text, relative)
                } else if relative == span.text.len() {
                    span.source.end
                } else if relative == 0 || bias == Bias::Before {
                    span.source.start
                } else {
                    span.source.end
                };
            }
            start = end;
        }
        self.spans.last().map_or(self.source.end, |s| s.source.end)
    }
    pub fn display_at(&self, source: usize, bias: Bias) -> usize {
        let mut display = 0;
        for span in &self.spans {
            if source < span.source.start {
                return display;
            }
            if source < span.source.end || (source == span.source.end && bias == Bias::Before) {
                return display
                    + if span.exact {
                        clip(&span.text, source.saturating_sub(span.source.start))
                    } else if source == span.source.end {
                        span.text.len()
                    } else if source == span.source.start || bias == Bias::Before {
                        0
                    } else {
                        span.text.len()
                    };
            }
            display += span.text.len();
        }
        display
    }
}
fn clip(s: &str, p: usize) -> usize {
    let mut p = p.min(s.len());
    while !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

#[derive(Clone, Debug)]
pub struct Projection {
    pub blocks: Vec<Block>,
}
impl Projection {
    pub fn new(source: &str, selection: Range<usize>) -> Self {
        let options = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
        let mut ranges = Vec::new();
        let mut depth = 0usize;
        let mut pending = None;
        for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
            match event {
                Event::Start(tag) => {
                    if depth == 0 {
                        let kind = match tag {
                            Tag::Heading { level, .. } => Kind::Heading(level as u8),
                            Tag::CodeBlock(_) => Kind::Code,
                            Tag::BlockQuote(_) => Kind::Quote,
                            Tag::List(_) => Kind::List,
                            Tag::Table(_) => Kind::Table,
                            _ => Kind::Paragraph,
                        };
                        pending = Some((range.clone(), kind));
                    }
                    depth += 1;
                }
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0
                        && let Some((mut whole, kind)) = pending.take()
                    {
                        whole.end = whole.end.max(range.end);
                        ranges.push((whole, kind));
                    }
                }
                Event::Rule if depth == 0 => ranges.push((range, Kind::Rule)),
                Event::Html(_) if depth == 0 => ranges.push((range, Kind::Code)),
                _ => {}
            }
        }
        let mut blocks = Vec::new();
        let mut end = 0;
        for (range, kind) in ranges {
            if range.start > end {
                blocks.push(Self::block(source, end..range.start, Kind::Blank, &selection, options));
            }
            end = end.max(range.end);
            blocks.push(Self::block(source, range, kind, &selection, options));
        }
        if end < source.len() {
            blocks.push(Self::block(source, end..source.len(), Kind::Blank, &selection, options));
        }
        if blocks.is_empty() || selection.start == source.len() {
            blocks.push(Self::block(source, source.len()..source.len(), Kind::Blank, &selection, options));
        }
        Self { blocks }
    }
    fn block(
        source: &str,
        range: Range<usize>,
        kind: Kind,
        selection: &Range<usize>,
        options: Options,
    ) -> Block {
        let revealed = if selection.is_empty() {
            range.contains(&selection.start) || range.is_empty() && selection.start == range.start
        } else {
            selection.start < range.end && selection.end > range.start
        };
        let raw = &source[range.clone()];
        let spans = if revealed || kind == Kind::Blank {
            vec![Span {
                text: raw.into(),
                source: range.clone(),
                style: Style { code: kind == Kind::Code, ..Style::default() },
                exact: true,
            }]
        } else if kind == Kind::Table {
            table_spans(raw, range.start, options)
        } else {
            formatted(raw, range.start, options)
        };
        Block { source: range, kind, spans, revealed }
    }
}
fn formatted(raw: &str, base: usize, options: Options) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut stack = Vec::new();
    let mut lists = Vec::new();
    for (event, range) in Parser::new_ext(raw, options).into_offset_iter() {
        let global = base + range.start..base + range.end;
        match event {
            Event::Start(tag) => {
                stack.push(style);
                match tag {
                    Tag::Strong => style.strong = true,
                    Tag::Emphasis => style.emphasis = true,
                    Tag::Strikethrough => style.strike = true,
                    Tag::Link { .. } => style.link = true,
                    Tag::CodeBlock(_) => style.code = true,
                    Tag::List(start) => lists.push(start),
                    Tag::Item => {
                        let prefix = if let Some(Some(number)) = lists.last_mut() {
                            let result = format!("{number}. ");
                            *number += 1;
                            result
                        } else {
                            "• ".into()
                        };
                        let prefix = format!("{}{prefix}", "  ".repeat(lists.len().saturating_sub(1)));
                        spans.push(Span {
                            text: prefix,
                            source: global.start..global.start,
                            style,
                            exact: false,
                        });
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                match tag {
                    TagEnd::List(_) => {
                        lists.pop();
                    }
                    TagEnd::Item | TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock
                        if !spans.last().is_some_and(|s| s.text.ends_with('\n')) =>
                    {
                        spans.push(Span {
                            text: "\n".into(),
                            source: global.end..global.end,
                            style,
                            exact: false,
                        });
                    }
                    _ => {}
                }
                style = stack.pop().unwrap_or_default();
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                push_text(&mut spans, &text, raw, &range, base, style)
            }
            Event::Code(text) => {
                push_text(&mut spans, &text, raw, &range, base, Style { code: true, ..style })
            }
            Event::SoftBreak => spans.push(Span {
                text: if style.code { "\n" } else { " " }.into(),
                source: global,
                style,
                exact: false,
            }),
            Event::HardBreak => spans.push(Span { text: "\n".into(), source: global, style, exact: false }),
            Event::TaskListMarker(checked) => spans.push(Span {
                text: if checked { "☑ " } else { "☐ " }.into(),
                source: global,
                style,
                exact: false,
            }),
            Event::Rule => {
                spans.push(Span {
                    text: "────────────────".into(), source: global, style, exact: false
                })
            }
            _ => {}
        }
    }
    while spans.last().is_some_and(|s| s.source.is_empty() && s.text == "\n") {
        spans.pop();
    }
    spans
}
fn push_text(spans: &mut Vec<Span>, text: &str, raw: &str, range: &Range<usize>, base: usize, style: Style) {
    let slice = &raw[range.clone()];
    let (source, exact) = if slice == text {
        (base + range.start..base + range.end, true)
    } else if slice.strip_prefix('\\') == Some(text) {
        (base + range.start + 1..base + range.end, true)
    } else if style.code
        && slice.starts_with('`')
        && slice.ends_with('`')
        && let Some(offset) = slice.find(text)
    {
        (base + range.start + offset..base + range.start + offset + text.len(), true)
    } else {
        (base + range.start..base + range.end, false)
    };
    spans.push(Span { text: text.into(), source, style, exact });
}
/// A closed table renders as aligned columns: the whole table is buffered
/// first, so a body cell can widen its column. Cell text stays an exact span
/// into the source; padding, the ` │ ` separator and the header rule are
/// non-exact spans. The open block keeps the raw source with its pipes.
fn table_spans(raw: &str, base: usize, options: Options) -> Vec<Span> {
    let mut aligns: Vec<Alignment> = Vec::new();
    let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut row: Vec<Vec<Span>> = Vec::new();
    let mut cell: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut stack: Vec<Style> = Vec::new();
    for (event, range) in Parser::new_ext(raw, options).into_offset_iter() {
        let global = base + range.start..base + range.end;
        match event {
            Event::Start(tag) => {
                stack.push(style);
                match tag {
                    Tag::Table(a) => aligns = a,
                    Tag::TableRow => row.clear(),
                    Tag::TableCell => cell.clear(),
                    Tag::Strong => style.strong = true,
                    Tag::Emphasis => style.emphasis = true,
                    Tag::Strikethrough => style.strike = true,
                    Tag::Link { .. } => style.link = true,
                    Tag::CodeBlock(_) => style.code = true,
                    _ => {}
                }
            }
            Event::End(tag) => {
                match tag {
                    TagEnd::TableCell => row.push(std::mem::take(&mut cell)),
                    TagEnd::TableHead | TagEnd::TableRow if !row.is_empty() => {
                        rows.push(std::mem::take(&mut row));
                    }
                    _ => {}
                }
                style = stack.pop().unwrap_or_default();
            }
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                push_text(&mut cell, &t, raw, &range, base, style)
            }
            Event::Code(t) => push_text(&mut cell, &t, raw, &range, base, Style { code: true, ..style }),
            Event::SoftBreak => cell.push(Span { text: " ".into(), source: global, style, exact: false }),
            Event::HardBreak => cell.push(Span { text: "\n".into(), source: global, style, exact: false }),
            Event::TaskListMarker(checked) => cell.push(Span {
                text: if checked { "☑ " } else { "☐ " }.into(),
                source: global,
                style,
                exact: false,
            }),
            _ => {}
        }
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for r in &rows {
        for (j, c) in r.iter().enumerate() {
            let w = c.iter().map(|s| s.text.width()).sum::<usize>();
            widths[j] = widths[j].max(w);
        }
    }
    let align = |j: usize| match aligns.get(j).copied() {
        Some(a @ (Alignment::Right | Alignment::Center)) => a,
        _ => Alignment::Left,
    };
    let pad_span =
        |text: String, at: usize| Span { text, source: at..at, style: Style::default(), exact: false };
    let last_col = cols.saturating_sub(1);
    let mut spans: Vec<Span> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let mut row_w = 0usize;
        for (j, &w) in widths.iter().enumerate() {
            let parts: &[Span] = r.get(j).map_or(&[], Vec::as_slice);
            let start = parts.first().map_or(base, |s| s.source.start);
            let end = parts.last().map_or(base + raw.len(), |s| s.source.end);
            let text: String = parts.iter().map(|s| s.text.as_str()).collect();
            let pad = w.saturating_sub(text.width());
            let last = j == last_col;
            if j > 0 {
                spans.push(pad_span(" │ ".into(), start));
                row_w += " │ ".width();
            }
            row_w += if matches!(align(j), Alignment::Left) && last { text.width() } else { w };
            match align(j) {
                Alignment::Right => {
                    if pad > 0 {
                        spans.push(pad_span(" ".repeat(pad), start));
                    }
                    spans.extend(parts.iter().cloned());
                }
                Alignment::Center => {
                    if pad / 2 > 0 {
                        spans.push(pad_span(" ".repeat(pad / 2), start));
                    }
                    spans.extend(parts.iter().cloned());
                    if !last && pad - pad / 2 > 0 {
                        spans.push(pad_span(" ".repeat(pad - pad / 2), end));
                    }
                }
                _ => {
                    spans.extend(parts.iter().cloned());
                    if !last && pad > 0 {
                        spans.push(pad_span(" ".repeat(pad), end));
                    }
                }
            }
        }
        let nl = spans.last().map_or(base, |s| s.source.end);
        spans.push(pad_span("\n".into(), nl));
        if i == 0 {
            spans.push(pad_span("─".repeat(row_w), nl));
            spans.push(pad_span("\n".into(), nl));
        }
    }
    while spans.last().is_some_and(|s| s.source.is_empty() && s.text == "\n") {
        spans.pop();
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    fn closed(source: &str) -> Projection {
        Projection::new(source, usize::MAX..usize::MAX)
    }
    #[test]
    fn reveals_only_the_edit_location_and_retains_exact_source() {
        let s = "# Heading\n\nA **bold** paragraph.\n\nOther *text*.\n";
        let p = Projection::new(s, 18..18);
        let revealed: Vec<_> = p.blocks.iter().filter(|b| b.revealed).collect();
        assert_eq!(revealed.len(), 1);
        assert!(revealed[0].text().contains("**bold**"));
        assert_eq!(p.blocks[0].text(), "Heading");
        assert_eq!(p.blocks.last().unwrap().text(), "Other text.");
        assert_eq!(p.blocks.iter().map(|b| &s[b.source.clone()]).collect::<String>(), s);
    }
    #[test]
    fn formatted_glyph_offsets_map_to_source_without_markers() {
        let s = "A **漢🙂** and `code`.\n";
        let p = closed(s);
        let b = &p.blocks[0];
        let t = b.text();
        assert_eq!(t, "A 漢🙂 and code.");
        let a = t.find('漢').unwrap();
        let end = a + "漢🙂".len();
        let r = b.source_at(a, Bias::After)..b.source_at(end, Bias::Before);
        assert_eq!(&s[r.clone()], "漢🙂");
        assert_eq!(b.display_at(r.start, Bias::After), a);
        assert_eq!(b.display_at(r.end, Bias::Before), end);
        assert!(b.spans.iter().any(|s| s.style.strong && s.text == "漢🙂"));
        assert!(b.spans.iter().any(|s| s.style.code && s.text == "code"));
    }
    #[test]
    fn entity_and_escape_mapping_stays_on_utf8_boundaries() {
        let s = "A &amp; \\* e\u{301}.\n";
        let p = closed(s);
        let b = &p.blocks[0];
        assert_eq!(b.text(), "A & * e\u{301}.");
        let i = b.text().find('&').unwrap();
        let r = b.source_at(i, Bias::After)..b.source_at(i + 1, Bias::Before);
        assert_eq!(&s[r], "&amp;");
        for i in 0..=b.text().len() {
            assert!(s.is_char_boundary(b.source_at(i, Bias::After)));
        }
    }
    #[test]
    fn code_and_tasks_keep_content_and_locations() {
        let s = "- [ ] task\n- [x] done\n\n```yaml\n  key: value\n```\n";
        let p = closed(s);
        assert!(p.blocks[0].text().contains("☐ task"));
        assert!(p.blocks[0].text().contains("☑ done"));
        let code = p.blocks.iter().find(|b| b.kind == Kind::Code).unwrap();
        assert_eq!(code.text(), "  key: value\n");
        let a = code.source_at(2, Bias::After);
        assert_eq!(&s[a..a + 3], "key");
    }
    #[test]
    fn trailing_blank_eof_has_an_editable_empty_block() {
        for source in ["one\n", "one\n\n", "one\n\n\n", "\n\n"] {
            let p = Projection::new(source, source.len()..source.len());
            let last = p.blocks.last().unwrap();
            assert_eq!(last.source, source.len()..source.len());
            assert!(last.revealed);
        }
    }
    #[test]
    fn selection_reveals_each_intersected_block_and_empty_note_is_editable() {
        let s = "one\n\ntwo\n\nthree";
        let p = Projection::new(s, 1..7);
        assert!(p.blocks.iter().take(3).all(|b| b.revealed));
        assert!(!p.blocks.last().unwrap().revealed);
        let p = Projection::new("", 0..0);
        assert_eq!(p.blocks.len(), 1);
        assert!(p.blocks[0].revealed);
        assert_eq!(p.blocks[0].source_at(0, Bias::After), 0);
    }
    #[test]
    fn closed_tables_align_and_open_tables_stay_raw() {
        let s = "| name | v |\n| --- | --- |\n| Alpha | 17 |\n";
        let p = closed(s);
        let b = &p.blocks[0];
        assert!(!b.revealed);
        let t = b.text();
        assert!(t.contains("name  │ v"), "{t:?}");
        assert!(t.contains(&"─".repeat(9)), "{t:?}");
        assert!(t.contains("Alpha │ 17"), "{t:?}");
        assert!(!t.contains("| name |"), "{t}");
        let d = t.find("Alpha").unwrap();
        let r = b.source_at(d, Bias::After)..b.source_at(d + 5, Bias::Before);
        assert_eq!(&s[r.clone()], "Alpha");
        assert_eq!(b.display_at(r.start, Bias::After), d);
        assert_eq!(b.display_at(r.end, Bias::Before), d + 5);
        let open = Projection::new(s, 2..2);
        assert!(open.blocks[0].revealed);
        assert!(open.blocks[0].text().contains("| name |"));
    }
}
