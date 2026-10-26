//! The frontmatter card above the Live text. It is drawn on the same canvas and
//! scrolls with the note; it is read-only (Properties in the command menu edits).
use super::*;
use crate::live::properties::{self, Piece};
use gpui_kit::{TextAlign, TextRun, UnderlineStyle, WrappedLine, fill, font, size};
use std::sync::Arc;

const PAD: f32 = 14.;
const KEY_W: f32 = 128.;
const LINE: f32 = 20.;
const HEADER: f32 = 18.;

struct Row {
    key: Arc<WrappedLine>,
    value: Arc<WrappedLine>,
    /// Content-space top of the row.
    y: Pixels,
    links: Vec<(Range<usize>, String)>,
}
pub(super) struct Card {
    /// Content-space top and height of the panel. x is relative to the text column.
    pub top: Pixels,
    pub height: Pixels,
    header: Arc<WrappedLine>,
    toggle: Option<(Arc<WrappedLine>, Pixels)>,
    rows: Vec<Row>,
}
pub(super) enum Hit {
    Toggle,
    Link(String),
}

fn shape(
    text: String,
    px_size: f32,
    runs: Vec<TextRun>,
    width: Option<Pixels>,
    window: &mut Window,
) -> Arc<WrappedLine> {
    // One visual line per value: a newline inside a YAML string becomes a space, same length.
    let text = text.replace('\n', " ");
    let shaped =
        window.text_system().shape_text(text.into(), px(px_size), &runs, width, None).unwrap_or_default();
    Arc::new(shaped.into_iter().next().unwrap_or_default())
}
fn run(len: usize, colour: gpui_kit::Hsla, family: &gpui_kit::SharedString) -> TextRun {
    TextRun {
        len,
        font: font(family.clone()),
        color: colour,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

impl LiveEditor {
    pub(super) fn shape_card(
        &self,
        width: Pixels,
        theme: &gpui_omarchy::Theme,
        window: &mut Window,
        top: Pixels,
    ) -> Option<Card> {
        let all = properties::rows(&self.properties);
        if all.is_empty() {
            return None;
        }
        let ui = &theme.font;
        let title = format!("PROPERTIES · {}", all.len());
        let header = shape(title.clone(), 11., vec![run(title.len(), theme.secondary, ui)], None, window);
        let toggle = (all.len() > properties::SUMMARY).then(|| {
            let label =
                if self.card_open { "Show less".to_string() } else { format!("Show all {}", all.len()) };
            let line = shape(label.clone(), 12., vec![run(label.len(), theme.accent, ui)], None, window);
            let x = width - line.width();
            (line, x)
        });
        let shown = if self.card_open { all.len() } else { all.len().min(properties::SUMMARY) };
        let mut y = top + px(PAD + HEADER + 6.);
        let mut rows = Vec::new();
        for row in all.into_iter().take(shown) {
            let key = shape(
                row.key.clone(),
                12.,
                vec![run(row.key.len(), theme.secondary, ui)],
                Some(px(KEY_W - 10.)),
                window,
            );
            let (mut text, mut runs, mut links) = (String::new(), Vec::new(), Vec::new());
            for piece in row.pieces {
                match piece {
                    Piece::Text(t) => {
                        runs.push(run(t.len(), theme.foreground, ui));
                        text.push_str(&t);
                    }
                    Piece::Link { label, target } => {
                        let mut r = run(label.len(), theme.accent, ui);
                        r.underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(theme.accent.opacity(0.4)),
                            wavy: false,
                        });
                        runs.push(r);
                        links.push((text.len()..text.len() + label.len(), target));
                        text.push_str(&label);
                    }
                }
            }
            let value = shape(text, 13., runs, Some((width - px(KEY_W)).max(px(40.))), window);
            let height = value.size(px(LINE)).height.max(key.size(px(LINE)).height);
            rows.push(Row { key, value, y, links });
            y += height + px(4.);
        }
        Some(Card { top, height: y - top + px(PAD - 4.), header, toggle, rows })
    }

    /// Screen position of a content-space point, x relative to the text column.
    fn at(&self, x: Pixels, y: Pixels) -> Point<Pixels> {
        self.bounds.origin + point(self.left + x, y - self.scroll)
    }

    pub(super) fn paint_card(
        &self,
        theme: &gpui_omarchy::Theme,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        let Some(card) = &self.card else { return };
        let panel = Bounds::new(self.at(px(-12.), card.top), size(self.measure + px(24.), card.height));
        if panel.bottom() < self.bounds.top() || panel.top() > self.bounds.bottom() {
            return;
        }
        window.paint_quad(
            fill(panel, theme.inset).corner_radii(px(8.)).border_widths(px(1.)).border_color(theme.border),
        );
        let header_y = card.top + px(PAD);
        let _ = card.header.paint(
            self.at(px(0.), header_y + px(2.)),
            px(HEADER),
            TextAlign::Left,
            None,
            window,
            cx,
        );
        if let Some((line, x)) = &card.toggle {
            let _ = line.paint(self.at(*x, header_y), px(HEADER), TextAlign::Left, None, window, cx);
        }
        for row in &card.rows {
            let _ =
                row.key.paint(self.at(px(0.), row.y + px(1.)), px(LINE), TextAlign::Left, None, window, cx);
            let _ = row.value.paint(self.at(px(KEY_W), row.y), px(LINE), TextAlign::Left, None, window, cx);
        }
    }

    /// What a click at `position` lands on inside the card, if anything.
    pub(super) fn card_hit(&self, position: Point<Pixels>) -> Option<Hit> {
        let card = self.card.as_ref()?;
        let local = position - self.at(px(0.), px(0.));
        let (x, y) = (local.x, local.y);
        if let Some((line, tx)) = &card.toggle {
            let header_y = card.top + px(PAD);
            if x >= *tx && x <= *tx + line.width() && y >= header_y && y <= header_y + px(HEADER) {
                return Some(Hit::Toggle);
            }
        }
        for row in &card.rows {
            let inside = point(x - px(KEY_W), y - row.y);
            if inside.y < px(0.) || inside.y >= row.value.size(px(LINE)).height || inside.x < px(0.) {
                continue;
            }
            let index = row.value.index_for_position(inside, px(LINE)).ok()?;
            return row.links.iter().find(|(r, _)| r.contains(&index)).map(|(_, t)| Hit::Link(t.clone()));
        }
        None
    }

    /// Screen points on the toggle and on each card link, for headless tests.
    #[cfg(test)]
    pub fn card_targets(&self) -> (Option<Point<Pixels>>, Vec<Point<Pixels>>) {
        let Some(card) = &self.card else { return (None, vec![]) };
        let toggle = card.toggle.as_ref().map(|(_, x)| self.at(*x + px(4.), card.top + px(PAD + 6.)));
        let links = card
            .rows
            .iter()
            .flat_map(|row| {
                row.links.iter().filter_map(|(range, _)| {
                    let p = row.value.position_for_index(range.start + 1, px(LINE))?;
                    Some(self.at(px(KEY_W) + p.x, row.y + p.y + px(6.)))
                })
            })
            .collect();
        (toggle, links)
    }
    /// Rows the card currently shows.
    #[cfg(test)]
    pub fn card_rows(&self) -> usize {
        self.card.as_ref().map_or(0, |c| c.rows.len())
    }

    /// True when `position` falls on the card panel at all.
    pub(super) fn on_card(&self, position: Point<Pixels>) -> bool {
        self.card.as_ref().is_some_and(|card| {
            let y = position.y - self.at(px(0.), px(0.)).y;
            y >= card.top && y <= card.top + card.height
        })
    }
}
