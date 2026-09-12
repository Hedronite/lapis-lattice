use super::*;
use crate::live::Kind;
use gpui_kit::{FontStyle, FontWeight, TextAlign, TextRun, WrappedLine, fill, font, size};
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
        let width = (bounds.size.width - px(48.)).max(px(40.));
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
            line.origin = bounds.origin + point(px(24.), line.y - self.scroll);
        }
    }
    fn shape(&mut self, width: Pixels, theme: &gpui_omarchy::Theme, window: &mut Window) {
        self.generation = self.generation.wrapping_add(1);
        self.lines.clear();
        let mut y = px(18.);
        for (index, block) in self.projection.blocks.iter().enumerate() {
            let font_size = match block.kind {
                Kind::Heading(level) => px((28.0 - f32::from(level) * 2.).max(18.)),
                _ => px(16.),
            };
            let line_height = font_size * 1.5;
            let mut runs = Vec::new();
            for span in &block.spans {
                let mut font = font(if span.style.code || block.kind == Kind::Table {
                    "monospace".into()
                } else {
                    theme.font.clone()
                });
                if span.style.strong || matches!(block.kind, Kind::Heading(_)) {
                    font.weight = FontWeight::BOLD;
                }
                if span.style.emphasis {
                    font.style = FontStyle::Italic;
                }
                runs.push(TextRun {
                    len: span.text.len(),
                    font,
                    color: if span.style.link { theme.accent } else { theme.foreground },
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
            let mut text = block.text();
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
                            line.origin - point(px(8.), px(0.)),
                            size(self.bounds.size.width - px(32.), line.shaped.size(line.height).height),
                        ),
                        theme.normal_fill(),
                    ));
                }
                if block.kind == Kind::Quote {
                    window.paint_quad(fill(
                        Bounds::new(
                            line.origin - point(px(12.), px(0.)),
                            size(px(2.), line.shaped.size(line.height).height),
                        ),
                        theme.accent,
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
                window.paint_quad(fill(Bounds::new(p, size(px(2.), px(23.))), theme.accent));
            }
        });
    }
}
