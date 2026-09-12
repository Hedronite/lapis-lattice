//! Formatted source projection over the existing native text/undo owner.
mod input;
mod layout;
use super::{Bias, Projection};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    Bounds, Context, ElementInputHandler, Entity, Focusable, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render, Styled, Subscription, Window,
    canvas, div, point, px,
};
use gpui_omarchy::ActiveTheme;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub struct LiveEditor {
    pub source: Entity<TextareaState>,
    text: String,
    projection: Projection,
    selection: Range<usize>,
    changed: bool,
    input_error: Option<String>,
    follow_cursor: bool,
    lines: Vec<layout::Line>,
    /// Set whenever the projection is rebuilt; cleared once the lines are reshaped.
    relayout: bool,
    layout_key: Option<layout::LayoutKey>,
    generation: u32,
    bounds: Bounds<Pixels>,
    scroll: Pixels,
    restored_scroll: Option<Pixels>,
    height: Pixels,
    anchor: Option<usize>,
    _changes: Subscription,
    _observe: Subscription,
}
impl LiveEditor {
    pub fn new(source: Entity<TextareaState>, cx: &mut Context<Self>) -> Self {
        // Live paints its own steady caret over the hidden source widget. The widget's
        // blink would notify every 500 ms while focused and redraw the whole window, so
        // it stays off here; `Workspace::set_view` turns it back on for visible source.
        source.update(cx, |s, cx| s.set_cursor_blink(false, cx));
        let changes = cx.subscribe(&source, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.changed = true;
            }
            cx.notify();
        });
        // A blinking `TextareaState` (Source/Split share this entity) notifies its
        // observers on every caret toggle, not only on edits and selection moves. The
        // projection paints its own steady caret, so a blink must not rebuild the
        // projection or reshape every line: invalidate only when the source text or
        // selection differs from what the last layout consumed. `changed` is already true while an edit or
        // an unlaid-out selection is pending, so that redraw is never lost.
        let observe = cx.observe(&source, |this, source, cx| {
            let state = source.read(cx);
            let text_changed = state.text() != this.text.as_str();
            if text_changed || this.changed || state.selected_range() != this.selection {
                this.changed |= text_changed;
                cx.notify();
            }
        });
        Self {
            source,
            text: String::new(),
            projection: Projection { blocks: vec![] },
            selection: 0..0,
            changed: true,
            input_error: None,
            follow_cursor: true,
            lines: vec![],
            relayout: true,
            layout_key: None,
            generation: 0,
            bounds: Bounds::default(),
            scroll: px(0.),
            restored_scroll: None,
            height: px(0.),
            anchor: None,
            _changes: changes,
            _observe: observe,
        }
    }
    pub fn scroll_position(&self) -> f32 {
        self.scroll.as_f32()
    }
    /// Counts line reshapes, so tests can show that a frame without a projection,
    /// width, font or palette change keeps its lines.
    pub fn layout_generation(&self) -> u32 {
        self.generation
    }
    pub fn restore_scroll(&mut self, value: f32, cx: &mut Context<Self>) {
        self.restored_scroll = Some(px(value.max(0.)));
        cx.notify();
    }
    fn sync_source(&mut self, cx: &mut Context<Self>) {
        let selection = self.source.read(cx).selected_range();
        self.changed |= self.source.read(cx).text() != self.text.as_str();
        if self.changed {
            self.text = self.source.read(cx).value().to_string();
        }
        if self.changed || (self.anchor.is_none() && selection != self.selection) {
            self.follow_cursor = true;
            self.projection = Projection::new(&self.text, selection.clone());
            self.selection = selection;
            self.changed = false;
            self.relayout = true;
        }
    }
    pub fn source_at_point(&self, position: Point<Pixels>) -> usize {
        let Some(line) = self.lines.iter().min_by(|a, b| {
            let distance = |line: &layout::Line| {
                let top = line.origin.y;
                let bottom = top + line.shaped.size(line.height).height;
                if position.y < top {
                    top - position.y
                } else if position.y >= bottom {
                    position.y - bottom + px(0.01)
                } else {
                    px(0.)
                }
            };
            distance(a).partial_cmp(&distance(b)).unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            return 0;
        };
        let local = position - line.origin;
        let index = line
            .shaped
            .closest_index_for_position(
                point(
                    local.x.max(px(0.)),
                    local.y.max(px(0.)).min(line.shaped.size(line.height).height - px(1.)),
                ),
                line.height,
            )
            .unwrap_or_else(|i| i);
        let bias = if line.shaped.position_for_index(index, line.height).is_some_and(|p| local.x < p.x) {
            Bias::Before
        } else {
            Bias::After
        };
        self.projection.blocks[line.block].source_at(line.display + index, bias)
    }
    pub fn point_for_source(&self, source: usize) -> Option<Point<Pixels>> {
        let target = self
            .projection
            .blocks
            .iter()
            .rposition(|b| b.source.contains(&source) || b.source.is_empty() && b.source.start == source)
            .or_else(|| self.projection.blocks.len().checked_sub(1))?;
        for line in &self.lines {
            if line.block != target {
                continue;
            }
            let block = &self.projection.blocks[line.block];
            if source < block.source.start || source > block.source.end {
                continue;
            }
            let index = block.display_at(source, Bias::After);
            if index >= line.display && index <= line.display + line.shaped.len() {
                return line
                    .shaped
                    .position_for_index(index - line.display, line.height)
                    .map(|p| line.origin + p);
            }
        }
        None
    }
    fn down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.source_at_point(event.position);
        let current = self.source.read(cx).selected_range();
        let (anchor, head) = if event.click_count >= 2 {
            self.text
                .split_word_bound_indices()
                .find(|(a, word)| *a <= p && *a + word.len() > p)
                .map_or((p, p), |(a, w)| (a, a + w.len()))
        } else if event.modifiers.shift {
            (current.start, p)
        } else {
            (p, p)
        };
        self.anchor = Some(anchor);
        self.source.update(cx, |s, cx| {
            s.focus(window, cx);
            s.set_selected_range(anchor..head, cx);
        });
        cx.stop_propagation();
        cx.notify();
    }
    fn drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(anchor) = self.anchor else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.anchor = None;
            return;
        }
        let p = self.source_at_point(event.position);
        self.source.update(cx, |s, cx| s.set_selected_range(anchor..p, cx));
        if event.position.y < self.bounds.top() + px(18.) {
            self.scroll = (self.scroll - px(18.)).max(px(0.));
        }
        if event.position.y > self.bounds.bottom() - px(18.) {
            self.scroll = (self.scroll + px(18.)).min((self.height - self.bounds.size.height).max(px(0.)));
        }
        let _ = window;
        cx.notify();
    }
}
impl Render for LiveEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source = self.source.clone();
        let pre = cx.entity();
        let paint = cx.entity();
        let theme = cx.omarchy().clone();
        div()
            .id("live-preview")
            .relative()
            .size_full()
            .overflow_hidden()
            // Retain the widget's native key actions and history without using its source layout.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(1.))
                    .overflow_hidden()
                    .opacity(0.)
                    .child(gpui_kit::base::Textarea::new(&source)),
            )
            .child(
                canvas(
                    move |bounds, window, cx| {
                        pre.update(cx, |this, cx| this.layout(bounds, &theme, window, cx));
                    },
                    move |bounds, _, window, cx| {
                        paint.update(cx, |this, cx| this.paint(window, cx));
                        let focus = paint.read(cx).source.read(cx).focus_handle(cx);
                        window.handle_input(&focus, ElementInputHandler::new(bounds, paint.clone()), cx);
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .px_4()
                    .text_color(cx.omarchy().danger)
                    .child(self.input_error.clone().unwrap_or_default()),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e, w, cx| this.down(e, w, cx)))
            .on_mouse_move(cx.listener(|this, e, w, cx| this.drag(e, w, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.anchor = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.anchor = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, e: &gpui_kit::ScrollWheelEvent, _, cx| {
                this.scroll = (this.scroll - e.delta.pixel_delta(px(22.)).y)
                    .clamp(px(0.), (this.height - this.bounds.size.height).max(px(0.)));
                cx.stop_propagation();
                cx.notify();
            }))
    }
}
