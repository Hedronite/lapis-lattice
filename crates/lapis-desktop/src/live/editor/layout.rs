use super::*;
use crate::live::Kind;
use gpui_kit::{
    FontStyle, FontWeight, StrikethroughStyle, TextAlign, TextRun, UnderlineStyle, WrappedLine, fill, font,
    size,
};
use std::sync::Arc;

pub(super) struct Line {
    pub shaped: Arc<WrappedLine>,
    /// Top of the line in content space; `origin` is placed from it and the scroll.
    pub y: Pixels,
    pub origin: Point<Pixels>,
    pub height: Pixels,
    pub block: usize,
    pub display: usize,
}
/// Everything besides the projection that shaped lines depend on.
#[derive(PartialEq)]
pub(super) struct LayoutKey {
    width: Pixels,
    font: gpui_kit::SharedString,
    accent: (f32, f32, f32, f32),
    foreground: (f32, f32, f32, f32),
}
fn channels(color: gpui_kit::Hsla) -> (f32, f32, f32, f32) {
    (color.h, color.s, color.l, color.a)
}
impl LiveEditor {
    pub(super) fn layout(
        &mut self,
        bounds: Bounds<Pixels>,
        theme: &gpui_omarchy::Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_source(cx);
        self.bounds = bounds;
        // A centred measure of about 72 characters, with at least 32 px of margin.
        let width = (bounds.size.width - px(64.)).clamp(px(40.), px(720.));
        self.left = ((bounds.size.width - width) / 2.).max(px(24.));
        self.measure = width;
        let key = LayoutKey {
            width,
            font: theme.font.clone(),
            accent: channels(theme.accent),
            foreground: channels(theme.foreground),
        };
        // GPUI redraws the whole window whenever any rendered view notifies, so this runs
        // on frames that changed nothing here, such as the widget caret blink or a redraw
        // elsewhere. Only a new projection, width, font or palette reshapes the lines.
        if self.relayout || self.layout_key.as_ref() != Some(&key) {
            self.shape(width, theme, window);
            self.layout_key = Some(key);
            self.relayout = false;
        }
        self.place(bounds);
        if let Some(scroll) = self.restored_scroll.take() {
            self.scroll = scroll;
            self.follow_cursor = false;
        }
        if self.follow_cursor
            && self.anchor.is_none()
            && let Some(p) = self.point_for_source(self.source.read(cx).cursor())
        {
            if p.y < bounds.top() + px(18.) {
                self.scroll -= bounds.top() + px(18.) - p.y;
            } else if p.y + px(28.) > bounds.bottom() - px(18.) {
                self.scroll += p.y + px(46.) - bounds.bottom();
            }
        }
        self.scroll = self.scroll.clamp(px(0.), (self.height - bounds.size.height).max(px(0.)));
        self.place(bounds);
        self.follow_cursor = false;
    }
    fn place(&mut self, bounds: Bounds<Pixels>) {
        for line in &mut self.lines {
            line.origin = bounds.origin + point(self.left, line.y - self.scroll);
        }
    }
    fn shape(&mut self, width: Pixels, theme: &gpui_omarchy::Theme, window: &mut Window) {
        self.generation = self.generation.wrapping_add(1);
        self.lines.clear();
        self.card = self.shape_card(width, theme, window, px(28.));
        let mut y = self.card.as_ref().map_or(px(32.), |c| c.top + c.height + px(28.));
        for (index, block) in self.projection.blocks.iter().enumerate() {
            let font_size = match block.kind {
                Kind::Heading(level) => px((32.0 - f32::from(level) * 3.).max(17.)),
                Kind::Code | Kind::Table => px(14.),
                _ => px(16.),
            };
            // Whole-pixel line heights: fractional ones accumulate, and adjacent lines
            // would overlap by a rounding error that hit testing then resolves wrongly.
            let line_height =
                (font_size * if matches!(block.kind, Kind::Heading(_)) { 1.3 } else { 1.6 }).round();
            // Headings breathe above more than below, like a printed page.
            if let Kind::Heading(level) = block.kind
                && index > 0
            {
                y += px(if level <= 2 { 14. } else { 8. });
            }
            let mut runs = Vec::new();
            for span in &block.spans {
                let mono = span.style.code || block.kind == Kind::Table;
                let mut font = font(if mono {
                    crate::theme::MONO.into()
                } else if matches!(block.kind, Kind::Heading(1 | 2)) {
                    crate::theme::SERIF.into()
                } else {
                    theme.font.clone()
                });
                if span.style.strong || matches!(block.kind, Kind::Heading(3..)) {
                    font.weight = FontWeight::BOLD;
                }
                if span.style.emphasis {
                    font.style = FontStyle::Italic;
                }
                let color = if span.style.link {
                    theme.accent
                } else if matches!(block.kind, Kind::Heading(_)) {
                    theme.bright
                } else if block.kind == Kind::Quote || block.kind == Kind::Rule {
                    theme.secondary
                } else {
                    theme.foreground
                };
                runs.push(TextRun {
                    len: span.text.len(),
                    font,
                    color,
                    // Inline code gets a faint wash; fenced blocks have their own panel.
                    background_color: (span.style.code && block.kind != Kind::Code)
                        .then(|| theme.foreground.opacity(0.07)),
                    underline: span.style.link.then(|| UnderlineStyle {
                        thickness: px(1.),
                        color: Some(theme.accent.opacity(0.4)),
                        wavy: false,
                    }),
                    strikethrough: span
                        .style
                        .strike
                        .then(|| StrikethroughStyle { thickness: px(1.), color: Some(theme.secondary) }),
                });
            }
            let mut text = block.text();
            if block.kind != Kind::Code {
                runs = mark_runs(runs, &text, theme);
            }
            // A block terminator ends its last row; it does not introduce another blank row.
            if text.ends_with('\n') {
                text.pop();
            }
            let shaped = window
                .text_system()
                .shape_text(text.clone().into(), font_size, &runs, Some(width), None)
                .unwrap_or_default();
            let mut display = 0;
            for line in shaped {
                let height = line.size(line_height).height;
                self.lines.push(Line {
                    display,
                    y,
                    origin: Point::default(),
                    height: line_height,
                    block: index,
                    shaped: Arc::new(line),
                });
                display += self.lines.last().unwrap().shaped.len() + 1;
                y += height;
            }

            y += if block.kind == Kind::Blank { px(0.) } else { px(8.) };
        }
        self.height = y + px(16.);
    }
    pub(super) fn paint(&self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = cx.omarchy().clone();
        let selection = self.source.read(cx).selected_range();
        let cursor = self.source.read(cx).cursor();
        window.paint_layer(self.bounds, |window| {
            self.paint_card(&theme, window, cx);
            for line in &self.lines {
                if line.origin.y + line.shaped.size(line.height).height < self.bounds.top()
                    || line.origin.y > self.bounds.bottom()
                {
                    continue;
                }
                let block = &self.projection.blocks[line.block];
                if block.kind == Kind::Code {
                    window.paint_quad(fill(
                        Bounds::new(
                            line.origin - point(px(12.), px(0.)),
                            size(self.measure + px(24.), line.shaped.size(line.height).height),
                        ),
                        theme.inset,
                    ));
                }
                if block.kind == Kind::Quote {
                    window.paint_quad(fill(
                        Bounds::new(
                            line.origin - point(px(14.), px(0.)),
                            size(px(2.), line.shaped.size(line.height).height),
                        ),
                        theme.accent.opacity(0.7),
                    ));
                }
                let a = block
                    .display_at(selection.start, crate::live::Bias::After)
                    .saturating_sub(line.display)
                    .min(line.shaped.len());
                let b = block
                    .display_at(selection.end, crate::live::Bias::Before)
                    .saturating_sub(line.display)
                    .min(line.shaped.len());
                if a < b && !selection.is_empty() {
                    for row in 0..=line.shaped.wrap_boundaries().len() {
                        let y = line.height * row as f32;
                        let lo = line
                            .shaped
                            .closest_index_for_position(point(px(0.), y), line.height)
                            .unwrap_or_else(|i| i);
                        let hi = line
                            .shaped
                            .closest_index_for_position(point(px(1e9), y), line.height)
                            .unwrap_or_else(|i| i);
                        let start = a.max(lo);
                        let end = b.min(hi);
                        if start >= end {
                            continue;
                        }
                        let x1 = if start == lo {
                            px(0.)
                        } else {
                            line.shaped.position_for_index(start, line.height).unwrap_or_default().x
                        };
                        let x2 = line.shaped.position_for_index(end, line.height).unwrap_or_default().x;
                        window.paint_quad(fill(
                            Bounds::new(line.origin + point(x1, y), size((x2 - x1).max(px(1.)), line.height)),
                            theme.accent.opacity(0.25),
                        ));
                    }
                }
                let _ = line.shaped.paint(line.origin, line.height, TextAlign::Left, None, window, cx);
            }
            if selection.is_empty()
                && self.source.read(cx).focus_handle(cx).is_focused(window)
                && let Some(p) = self.point_for_source(cursor)
            {
                // The caret is as tall as the text it sits in, headings included.
                let height = self
                    .lines
                    .iter()
                    .find(|l| p.y >= l.origin.y && p.y < l.origin.y + l.shaped.size(l.height).height)
                    .map_or(px(24.), |l| l.height);
                window.paint_quad(fill(Bounds::new(p, size(px(2.), height)), theme.accent));
            }
            // A thin thumb shows where the view sits in a long note.
            let view = self.bounds.size.height;
            if self.height > view + px(1.) {
                let thumb = (view * (view / self.height)).max(px(28.));
                let travel = (self.height - view).max(px(1.));
                let top = self.bounds.top() + (view - thumb) * (self.scroll / travel);
                window.paint_quad(
                    fill(
                        Bounds::new(
                            point(self.bounds.right() - px(7.), top + px(2.)),
                            size(px(4.), thumb - px(4.)),
                        ),
                        theme.foreground.opacity(0.16),
                    )
                    .corner_radii(px(2.)),
                );
            }
        });
    }
}

/// Byte ranges of `[[wikilinks]]` and `<!-- comments -->` in a block's display text.
fn marks(text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let mut found = Vec::new();
    for (open, close, link) in [("[[", "]]", true), ("<!--", "-->", false)] {
        let mut from = 0;
        while let Some(start) = text[from..].find(open).map(|i| from + i) {
            let Some(end) =
                text[start + open.len()..].find(close).map(|i| start + open.len() + i + close.len())
            else {
                break;
            };
            found.push((start..end, link));
            from = end;
        }
    }
    found.sort_by_key(|(r, _)| r.start);
    found
}

/// Recolour wikilinks in gold and dim HTML comments, splitting runs at their edges.
/// Only colour changes, so shaping and cursor mapping see the same text.
fn mark_runs(runs: Vec<TextRun>, text: &str, theme: &gpui_omarchy::Theme) -> Vec<TextRun> {
    let marks = marks(text);
    if marks.is_empty() {
        return runs;
    }
    let colour_at = |at: usize| marks.iter().find(|(r, _)| r.contains(&at)).map(|(_, link)| *link);
    let mut cuts: Vec<usize> = marks.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
    let mut out = Vec::with_capacity(runs.len() + cuts.len());
    let mut offset = 0;
    for run in runs {
        let end = offset + run.len;
        cuts.retain(|&c| c > offset);
        let mut start = offset;
        for &cut in cuts.iter().filter(|&&c| c < end).chain(std::iter::once(&end)) {
            if cut <= start {
                continue;
            }
            let mut piece = run.clone();
            piece.len = cut - start;
            match colour_at(start) {
                Some(true) => piece.color = theme.accent,
                Some(false) => piece.color = theme.secondary.opacity(0.6),
                None => {}
            }
            out.push(piece);
            start = cut;
        }
        offset = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(len: usize) -> TextRun {
        TextRun {
            len,
            font: font(".SystemUIFont"),
            color: gpui_kit::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn wikilinks_and_comments_split_runs_without_changing_length() {
        let theme = gpui_omarchy::Theme::tokyo_night();
        let text = "See [[a/b|B]] and <!-- note --> end";
        // Two source spans whose seam falls inside the wikilink.
        let runs = mark_runs(vec![run(8), run(text.len() - 8)], text, &theme);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), text.len());
        let mut at = 0;
        let colours: Vec<(&str, gpui_kit::Hsla)> = runs
            .iter()
            .map(|r| {
                let piece = &text[at..at + r.len];
                at += r.len;
                (piece, r.color)
            })
            .collect();
        assert_eq!(
            colours.iter().filter(|(_, c)| *c == theme.accent).map(|(p, _)| *p).collect::<String>(),
            "[[a/b|B]]"
        );
        assert!(colours.iter().any(|(p, c)| *p == "<!-- note -->" && *c == theme.secondary.opacity(0.6)));
        assert_eq!(colours.first().unwrap().0, "See ");
    }

    #[test]
    fn an_unclosed_wikilink_is_left_alone() {
        let theme = gpui_omarchy::Theme::tokyo_night();
        let runs = mark_runs(vec![run(9)], "[[pending", &theme);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].color, gpui_kit::black());
    }
}
