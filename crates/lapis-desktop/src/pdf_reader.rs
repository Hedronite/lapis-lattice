//! Read-only PDF reader: one continuous column of pages. Only the pages near the
//! viewport are rasterized, one at a time; hidden tabs keep their place, not bitmaps.
mod strip;

use crate::services::{ArcCancel, PdfPage, WorkspaceServices};
use gpui_kit::base::input::{InputState, TextareaState};
use gpui_kit::{
    AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, RenderImage, Role, StatefulInteractiveElement, Styled, Window, canvas, div, img,
    point, px,
};
use gpui_omarchy::ActiveTheme;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use strip::{GAP, PAD, Strip};

/// Bitmaps kept at once, and their byte budget. Pages farthest from the reader go first.
const CACHE_PAGES: usize = 6;
const CACHE_BYTES: usize = 96 * 1024 * 1024;
/// Pages rendered ahead of and behind the visible ones.
const PREFETCH: u32 = 1;
/// A page height used until any page has reported its size: A4 portrait.
const DEFAULT_ASPECT: f32 = 1.414;
/// Arrow keys scroll by this much.
const LINE_STEP: f32 = 64.;

struct Page {
    number: u32,
    requested_width: u32,
    image: Arc<RenderImage>,
    text: String,
    bytes: usize,
    revision: String,
}
pub(crate) struct PdfReader {
    services: Arc<dyn WorkspaceServices>,
    path: String,
    /// The page at the reading line, about a third of the way down the viewport.
    page: u32,
    pages: Option<u32>,
    zoom: f32,
    /// Viewport width and height in logical pixels, and the display scale.
    viewport: f32,
    view_height: f32,
    scale: f32,
    visible: bool,
    error: Option<String>,
    epoch: u64,
    cancel: ArcCancel,
    /// The page being rasterized, and at what width.
    inflight: Option<(u32, u32)>,
    cache: VecDeque<Page>,
    /// Height over width for each page that has rendered.
    aspects: BTreeMap<u32, f32>,
    /// Scroll to the top of this page once the column can be laid out.
    pending_jump: Option<u32>,
    /// Which page the text panel shows.
    text_page: Option<u32>,
    focus: FocusHandle,
    page_input: Entity<InputState>,
    text: Entity<TextareaState>,
    show_text: bool,
    scroll: gpui_kit::ScrollHandle,
}
impl PdfReader {
    pub fn new(
        path: String,
        services: Arc<dyn WorkspaceServices>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page_input = cx.new(|cx| InputState::new(window, cx).default_value("1"));
        let text = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).rows(30);
            s.set_readonly(true, cx);
            s
        });
        cx.on_release(|this, cx| {
            this.cancel.store(true, Ordering::Relaxed);
            for page in this.cache.drain(..) {
                cx.drop_image(page.image, None);
            }
        })
        .detach();
        Self {
            services,
            path,
            page: 0,
            pages: None,
            zoom: 1.,
            viewport: 800.,
            view_height: 0.,
            scale: window.scale_factor(),
            visible: false,
            error: None,
            epoch: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            inflight: None,
            cache: VecDeque::new(),
            aspects: BTreeMap::new(),
            pending_jump: None,
            text_page: None,
            focus: cx.focus_handle(),
            page_input,
            text,
            show_text: false,
            scroll: gpui_kit::ScrollHandle::new(),
        }
    }
    pub fn position(&self) -> (u32, f32) {
        (self.page, self.zoom)
    }
    pub fn restore_position(&mut self, page: u32, zoom: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(0.5, 3.);
        self.jump_to(page, window, cx);
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
    }
    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            // Back where the reader left off: the column is rebuilt from the saved page.
            self.pending_jump = Some(self.page);
            self.pump(window, cx);
        } else {
            self.stop();
            self.clear_cache(window, cx);
            self.text_page = None;
            self.text.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    /// Width a page is drawn at, and the bitmap width that fills it on this display.
    fn page_width(&self) -> f32 {
        (self.viewport - 48.).max(200.) * self.zoom
    }
    fn raster_width(&self) -> u32 {
        (self.page_width() * self.scale).round().clamp(320., 2400.) as u32
    }
    fn aspect(&self, page: u32) -> f32 {
        self.aspects.get(&page).or_else(|| self.aspects.values().next()).copied().unwrap_or(DEFAULT_ASPECT)
    }
    fn strip(&self) -> Strip {
        Strip::new(self.pages.unwrap_or(0), self.page_width(), |p| self.aspect(p))
    }
    /// Pages worth rasterizing now, nearest to the reading line first.
    fn wanted(&self) -> Vec<u32> {
        let Some(pages) = self.pages else { return vec![self.page] };
        let range = if self.view_height <= 0. || self.pending_jump.is_some() {
            self.page..self.page + 1
        } else {
            let y0 = -f32::from(self.scroll.offset().y);
            self.strip().visible(y0, y0 + self.view_height)
        };
        let lo = range.start.saturating_sub(PREFETCH);
        let hi = (range.end + PREFETCH).min(pages);
        let mut wanted: Vec<u32> = (lo..hi).collect();
        wanted.sort_by_key(|p| p.abs_diff(self.page));
        wanted
    }
    fn cached(&self, page: u32, width: u32) -> bool {
        self.cache.iter().any(|p| p.number == page && p.requested_width == width)
    }
    fn stop(&mut self) {
        self.epoch += 1;
        self.cancel.store(true, Ordering::Relaxed);
        self.inflight = None;
    }

    /// Start the next render the viewport needs. A render for a page that scrolled
    /// out of reach is cancelled; only one runs at a time.
    fn pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible || self.error.is_some() {
            return;
        }
        let width = self.raster_width();
        let wanted = self.wanted();
        if let Some((page, w)) = self.inflight {
            if w == width && wanted.contains(&page) {
                return;
            }
            self.stop();
        }
        let Some(page) = wanted.into_iter().find(|p| !self.cached(*p, width)) else { return };
        self.epoch += 1;
        let epoch = self.epoch;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.inflight = Some((page, width));
        let services = self.services.clone();
        let path = self.path.clone();
        let delay = cx.background_executor().timer(std::time::Duration::from_millis(60));
        let task = cx.background_executor().spawn(async move {
            // Coalesce a fast scroll or zoom before launching a native render.
            delay.await;
            if cancel.load(Ordering::Relaxed) {
                return Err("PDF loading cancelled".into());
            }
            services.pdf_page(&path, page, width, cancel)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.epoch != epoch || !this.visible {
                    return;
                }
                this.inflight = None;
                match result {
                    Ok(page) => this.install(page, width, window, cx),
                    Err(error) => this.error = Some(error),
                }
                this.pump(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn install(&mut self, page: PdfPage, requested_width: u32, window: &mut Window, cx: &mut Context<Self>) {
        let bytes = page.bgra.len();
        if bytes > CACHE_BYTES {
            self.error = Some("This page exceeds the image cache budget; reduce zoom".into());
            return;
        }
        let Some(buffer) = image::RgbaImage::from_raw(page.info.width, page.info.height, page.bgra) else {
            self.error = Some("Invalid PDF bitmap".into());
            return;
        };
        if self.cache.front().is_some_and(|p| p.revision != page.revision) {
            // The file changed on disk: every bitmap and page size is stale.
            self.clear_cache(window, cx);
            self.aspects.clear();
        }
        self.pages = Some(page.info.pages);
        self.aspects.insert(page.info.page, page.info.height as f32 / page.info.width.max(1) as f32);
        self.cache.retain(|p| p.number != page.info.page || p.requested_width == requested_width);
        self.cache.push_back(Page {
            number: page.info.page,
            requested_width,
            image: Arc::new(RenderImage::new([image::Frame::new(buffer)])),
            text: page.info.text,
            bytes,
            revision: page.revision,
        });
        self.evict(window, cx);
    }
    /// Keep the cache inside its budget, dropping the pages farthest from the reader.
    fn evict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.raster_width();
        while self.cache.len() > CACHE_PAGES
            || self.cache.iter().map(|p| p.bytes).sum::<usize>() > CACHE_BYTES
        {
            let anchor = self.page;
            let Some(victim) = self
                .cache
                .iter()
                .enumerate()
                .max_by_key(|(_, p)| (p.requested_width != width, p.number.abs_diff(anchor)))
                .map(|(i, _)| i)
            else {
                break;
            };
            let page = self.cache.remove(victim).unwrap();
            cx.drop_image(page.image, Some(window));
        }
    }
    fn clear_cache(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for page in self.cache.drain(..) {
            cx.drop_image(page.image, Some(window));
        }
    }

    /// Put the top of `page` at the top of the viewport.
    fn jump_to(&mut self, page: u32, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.pages.map_or(page, |n| page.min(n.saturating_sub(1)));
        self.page = page;
        self.pending_jump = Some(page);
        self.page_input.update(cx, |s, cx| s.set_value((page + 1).to_string(), window, cx));
        self.pump(window, cx);
        cx.notify();
    }
    fn scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        let offset = self.scroll.offset();
        let max = f32::from(self.scroll.max_offset().y);
        let y = (f32::from(offset.y) - dy).clamp(-max, 0.);
        self.scroll.set_offset(point(offset.x, px(y)));
        cx.notify();
    }
    fn zoom_to(&mut self, zoom: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(0.5, 3.);
        // Keep the reader on the same page as the column grows or shrinks.
        self.jump_to(self.page, window, cx);
    }

    /// Per frame: apply a pending jump, follow the reading line, keep the text panel current.
    fn sync_to_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pages) = self.pages else { return };
        if self.view_height <= 0. {
            return;
        }
        let strip = self.strip();
        if let Some(page) = self.pending_jump.take() {
            let page = page.min(pages.saturating_sub(1));
            let x = self.scroll.offset().x;
            self.scroll.set_offset(point(x, px(-(strip.top(page) - PAD).max(0.))));
            self.page = page;
        } else {
            let y0 = -f32::from(self.scroll.offset().y);
            let page = strip.at(y0 + self.view_height / 3.);
            if page != self.page {
                self.page = page;
                if !self.page_input.read(cx).focus_handle(cx).is_focused(window) {
                    self.page_input.update(cx, |s, cx| s.set_value((page + 1).to_string(), window, cx));
                }
            }
        }
        if self.show_text
            && self.text_page != Some(self.page)
            && let Some(text) =
                self.cache.iter().rev().find(|p| p.number == self.page).map(|p| p.text.clone())
        {
            self.text.update(cx, |s, cx| s.set_value(text, window, cx));
            self.text_page = Some(self.page);
        }
        self.pump(window, cx);
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.page_input.read(cx).focus_handle(cx).is_focused(window) {
            if key == "enter" {
                let value = self.page_input.read(cx).value();
                match value.trim().parse::<u32>().ok().filter(|n| *n > 0) {
                    Some(page) if self.pages.is_none_or(|n| page <= n) => {
                        self.error = None;
                        self.jump_to(page - 1, window, cx)
                    }
                    _ => {
                        self.error = Some(match self.pages {
                            Some(n) => format!("Choose a page from 1 to {n}"),
                            None => "Enter a page number starting at 1".into(),
                        })
                    }
                }
                self.focus(window, cx);
                cx.stop_propagation();
                cx.notify();
            } else if key == "escape" {
                self.focus(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if !self.focus.is_focused(window)
            || event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.alt
        {
            return;
        }
        let last = self.pages.map_or(self.page, |n| n.saturating_sub(1));
        match key {
            "right" | "pagedown" | "j" => self.jump_to((self.page + 1).min(last), window, cx),
            "left" | "pageup" | "k" => self.jump_to(self.page.saturating_sub(1), window, cx),
            "down" => self.scroll_by(LINE_STEP, cx),
            "up" => self.scroll_by(-LINE_STEP, cx),
            "space" => self.scroll_by((self.view_height - LINE_STEP).max(LINE_STEP), cx),
            "home" => self.jump_to(0, window, cx),
            "end" => self.jump_to(last, window, cx),
            "+" | "=" => self.zoom_to(self.zoom + 0.25, window, cx),
            "-" => self.zoom_to(self.zoom - 0.25, window, cx),
            "0" => self.zoom_to(1., window, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}
impl Render for PdfReader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_to_scroll(window, cx);
        let theme = cx.omarchy().clone();
        let monitor = cx.entity();
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(match id {
                    "pdf-zoom-in" => "Zoom in",
                    "pdf-zoom-out" => "Zoom out",
                    "pdf-text" => "Toggle selectable page text",
                    _ => label,
                })
                .px_2()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(theme.hover_fill()))
                .child(label)
        };
        let toolbar = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .px_4()
            .py_2()
            .text_sm()
            .border_b_1()
            .border_color(theme.border)
            .child(button("pdf-prev", "Previous").on_click(cx.listener(|this, _, w, cx| {
                this.jump_to(this.page.saturating_sub(1), w, cx);
                this.focus(w, cx)
            })))
            .child(div().w(px(54.)).child(gpui_omarchy::input(
                "pdf-page-number",
                &self.page_input,
                window,
                cx,
            )))
            .child(format!("/ {}", self.pages.map_or("…".into(), |n| n.to_string())))
            .child(button("pdf-next", "Next").on_click(cx.listener(|this, _, w, cx| {
                let last = this.pages.map_or(this.page, |n| n.saturating_sub(1));
                this.jump_to((this.page + 1).min(last), w, cx);
                this.focus(w, cx)
            })))
            .child(div().flex_1())
            .child(
                button("pdf-zoom-out", "−")
                    .on_click(cx.listener(|this, _, w, cx| this.zoom_to(this.zoom - 0.25, w, cx))),
            )
            .child(format!("{:.0}%", self.zoom * 100.))
            .child(
                button("pdf-zoom-in", "+")
                    .on_click(cx.listener(|this, _, w, cx| this.zoom_to(this.zoom + 0.25, w, cx))),
            )
            .child(
                button("pdf-fit", "Fit width")
                    .on_click(cx.listener(|this, _, w, cx| this.zoom_to(1., w, cx))),
            )
            .child(button("pdf-text", "Page text").on_click(cx.listener(|this, _, w, cx| {
                this.show_text = !this.show_text;
                this.text_page = None;
                if this.show_text {
                    this.text.update(cx, |s, cx| s.focus(w, cx));
                } else {
                    this.focus(w, cx);
                }
                cx.notify()
            })))
            .child(button("pdf-reload", "Reload").on_click(cx.listener(|this, _, w, cx| {
                this.error = None;
                this.stop();
                this.clear_cache(w, cx);
                this.aspects.clear();
                this.jump_to(this.page, w, cx);
            })));
        let mut body = div()
            .id("pdf-page-scroll")
            .track_scroll(&self.scroll)
            .size_full()
            .overflow_scroll()
            .bg(theme.inset);
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_4()
                    .text_color(theme.danger)
                    .child(format!("{}: {error}", self.path))
                    .child(button("pdf-retry", "Retry").text_color(theme.accent).on_click(cx.listener(
                        |this, _, w, cx| {
                            this.error = None;
                            this.pump(w, cx);
                            cx.notify();
                        },
                    ))),
            );
        }
        if self.show_text {
            let text = self.cache.iter().rev().find(|p| p.number == self.page).map(|p| p.text.clone());
            body = body.child(
                div().p_6().child(match text {
                    None => {
                        div().text_color(theme.secondary).child(format!("Loading page {}…", self.page + 1))
                    }
                    Some(t) if t.trim().is_empty() => div()
                        .text_color(theme.secondary)
                        .child("This page has no extractable text. Use the page image to read it."),
                    Some(_) => {
                        self.text.update(cx, |s, _| s.set_editor_style(theme.input_style()));
                        div().child(gpui_kit::base::Textarea::new(&self.text))
                    }
                }),
            );
        } else if let Some(pages) = self.pages {
            body = body.child(self.column(pages, &theme));
        } else if self.error.is_none() {
            body = body.child(
                div().p_6().text_color(theme.secondary).child(format!("Loading page {}…", self.page + 1)),
            );
        }
        div()
            .id("pdf-reader")
            .role(Role::Group)
            .aria_label(format!("PDF reader: {}", self.path))
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, e, w, cx| this.key(e, w, cx)))
            .capture_action(cx.listener(|this, _: &gpui_kit::base::input::Paste, w, cx| {
                if this.page_input.read(cx).focus_handle(cx).is_focused(w)
                    && cx
                        .read_from_clipboard()
                        .and_then(|i| i.text())
                        .is_some_and(|s| s.contains(['\r', '\n']))
                {
                    this.error = Some("Page number accepts one line; paste was not inserted".into());
                    cx.stop_propagation();
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .child(toolbar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    // Measures the viewport each layout; a new width re-renders at the new size.
                    .child(
                        canvas(
                            move |bounds, w, cx| {
                                monitor.update(cx, |this, cx| {
                                    let (width, height) =
                                        (f32::from(bounds.size.width), f32::from(bounds.size.height));
                                    let old = this.raster_width();
                                    let first = this.view_height <= 0.;
                                    this.viewport = width.max(200.);
                                    this.view_height = height;
                                    this.scale = w.scale_factor();
                                    if first || old.abs_diff(this.raster_width()) > 32 {
                                        cx.notify();
                                    }
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
    }
}
impl PdfReader {
    /// The page column: spacers stand in for pages out of view, so a long book costs
    /// the same to lay out as a short one.
    fn column(&self, pages: u32, theme: &gpui_omarchy::Theme) -> impl IntoElement + use<> {
        let strip = self.strip();
        let width = self.page_width();
        let y0 = -f32::from(self.scroll.offset().y);
        let shown = if self.view_height > 0. {
            let r = strip.visible(y0, y0 + self.view_height);
            r.start.saturating_sub(PREFETCH)..(r.end + PREFETCH).min(pages)
        } else {
            self.page..(self.page + 1).min(pages)
        };
        let raster = self.raster_width();
        let mut column = div()
            .flex()
            .flex_col()
            .items_center()
            // Never narrower than a zoomed page, so wide pages scroll instead of clipping left.
            .w(px((width + 48.).max(self.viewport)))
            .h(px(strip.total))
            .flex_none()
            .child(div().flex_none().h(px(strip.top(shown.start))));
        for page in shown.clone() {
            let height = strip.height(page);
            // An exact-width bitmap, else any bitmap of the page scaled until it re-renders.
            let bitmap = self
                .cache
                .iter()
                .rev()
                .find(|p| p.number == page && p.requested_width == raster)
                .or_else(|| self.cache.iter().rev().find(|p| p.number == page));
            let mut frame = div()
                .flex_none()
                .w(px(width))
                .h(px(height))
                .mb(px(GAP))
                .rounded(px(2.))
                .shadow_lg()
                .bg(theme.surface);
            frame = match bitmap {
                Some(p) => frame.child(img(p.image.clone()).w(px(width)).h(px(height))),
                None => frame.flex().items_center().justify_center().text_color(theme.secondary).child(
                    if self.inflight.is_some_and(|(p, _)| p == page) {
                        format!("Rendering page {}…", page + 1)
                    } else {
                        format!("Page {}", page + 1)
                    },
                ),
            };
            column = column.child(frame);
        }
        column
    }
}

#[cfg(all(test, feature = "gui-tests"))]
mod tests;
