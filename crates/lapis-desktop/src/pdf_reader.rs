//! Read-only, lazy page reader. Hidden tabs retain position, not bitmap allocations.
use crate::services::{ArcCancel, PdfPage, WorkspaceServices};
use gpui_kit::base::input::{InputState, TextareaState};
use gpui_kit::{
    AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, RenderImage, Role, StatefulInteractiveElement, Styled, Window, canvas, div, img,
    px,
};
use gpui_omarchy::ActiveTheme;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const CACHE_BYTES: usize = 32 * 1024 * 1024;
struct Page {
    number: u32,
    width: u32,
    height: u32,
    requested_width: u32,
    image: Arc<RenderImage>,
    text: String,
    bytes: usize,
    revision: String,
}
pub(crate) struct PdfReader {
    services: Arc<dyn WorkspaceServices>,
    path: String,
    page: u32,
    pages: Option<u32>,
    zoom: f32,
    viewport: f32,
    scale: f32,
    visible: bool,
    loading: bool,
    error: Option<String>,
    epoch: u64,
    cancel: ArcCancel,
    cache: VecDeque<Page>,
    current: Option<(u32, u32)>,
    focus: FocusHandle,
    page_input: Entity<InputState>,
    text: Entity<TextareaState>,
    show_text: bool,
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
            scale: window.scale_factor(),
            visible: false,
            loading: false,
            error: None,
            epoch: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            cache: VecDeque::new(),
            current: None,
            focus: cx.focus_handle(),
            page_input,
            text,
            show_text: false,
        }
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
    }
    fn clear_cache(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.current = None;
        for page in self.cache.drain(..) {
            cx.drop_image(page.image, Some(window));
        }
    }
    pub fn set_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.load(self.page, false, window, cx);
        } else {
            self.epoch += 1;
            self.cancel.store(true, Ordering::Relaxed);
            self.loading = false;
            self.clear_cache(window, cx);
            self.text.update(cx, |s, cx| s.set_value("", window, cx));
        }
    }
    fn raster_width(&self) -> u32 {
        ((self.viewport - 48.).max(200.) * self.zoom * self.scale).round().clamp(320., 2400.) as u32
    }
    fn load(&mut self, page: u32, reload: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        if let Some(pages) = self.pages
            && page >= pages
        {
            self.error = Some(format!("Choose a page from 1 to {pages}"));
            cx.notify();
            return;
        }
        self.epoch += 1;
        let epoch = self.epoch;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        if reload {
            self.clear_cache(window, cx);
        }
        self.page = page;
        self.error = None;
        self.current = None;
        self.page_input.update(cx, |s, cx| s.set_value((page + 1).to_string(), window, cx));
        let width = self.raster_width();
        if let Some(index) = self.cache.iter().position(|p| p.number == page && p.requested_width == width) {
            let found = self.cache.remove(index).unwrap();
            self.text.update(cx, |s, cx| s.set_value(found.text.clone(), window, cx));
            self.cache.push_back(found);
            self.current = Some((page, width));
            self.loading = false;
            cx.notify();
            return;
        }
        self.loading = true;
        let services = self.services.clone();
        let path = self.path.clone();
        let delay = cx.background_executor().timer(std::time::Duration::from_millis(80));
        let task = cx.background_executor().spawn(async move {
            // Coalesce rapid zoom/page requests before launching a native process.
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
                this.loading = false;
                match result {
                    Ok(page) => this.install(page, width, window, cx),
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn install(&mut self, page: PdfPage, requested_width: u32, window: &mut Window, cx: &mut Context<Self>) {
        let bytes = page.bgra.len();
        let Some(buffer) = image::RgbaImage::from_raw(page.info.width, page.info.height, page.bgra) else {
            self.error = Some("Invalid PDF bitmap".into());
            return;
        };
        if self.cache.front().is_some_and(|p| p.revision != page.revision) {
            self.clear_cache(window, cx);
        }
        while !self.cache.is_empty()
            && (self.cache.len() >= 3
                || self.cache.iter().map(|p| p.bytes).sum::<usize>() + bytes > CACHE_BYTES)
        {
            cx.drop_image(self.cache.pop_front().unwrap().image, Some(window));
        }
        if bytes > CACHE_BYTES {
            self.error = Some("This page exceeds the image cache budget; reduce zoom".into());
            return;
        }
        self.pages = Some(page.info.pages);
        self.text.update(cx, |s, cx| s.set_value(page.info.text.clone(), window, cx));
        self.current = Some((page.info.page, requested_width));
        self.cache.push_back(Page {
            number: page.info.page,
            width: page.info.width,
            height: page.info.height,
            requested_width,
            image: Arc::new(RenderImage::new([image::Frame::new(buffer)])),
            text: page.info.text,
            bytes,
            revision: page.revision,
        });
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if self.page_input.read(cx).focus_handle(cx).is_focused(window) {
            if key == "enter" {
                let value = self.page_input.read(cx).value();
                match value.trim().parse::<u32>().ok().filter(|n| *n > 0) {
                    Some(page) => self.load(page - 1, false, window, cx),
                    None => self.error = Some("Enter a page number starting at 1".into()),
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
        match key {
            "right" | "pagedown" | "j" => {
                if self.pages.is_some_and(|n| self.page + 1 < n) {
                    self.load(self.page + 1, false, window, cx)
                }
            }
            "left" | "pageup" | "k" => self.load(self.page.saturating_sub(1), false, window, cx),
            "home" => self.load(0, false, window, cx),
            "end" => {
                if let Some(pages) = self.pages {
                    self.load(pages - 1, false, window, cx)
                }
            }
            "+" | "=" => {
                self.zoom = (self.zoom + 0.25).min(3.);
                self.load(self.page, false, window, cx)
            }
            "-" => {
                self.zoom = (self.zoom - 0.25).max(0.5);
                self.load(self.page, false, window, cx)
            }
            "0" => {
                self.zoom = 1.;
                self.load(self.page, false, window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}
impl Render for PdfReader {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .child(label)
        };
        let mut toolbar = div()
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
                this.load(this.page.saturating_sub(1), false, w, cx);
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
                if this.pages.is_some_and(|n| this.page + 1 < n) {
                    this.load(this.page + 1, false, w, cx);
                }
                this.focus(w, cx)
            })))
            .child(div().flex_1())
            .child(button("pdf-zoom-out", "−").on_click(cx.listener(|this, _, w, cx| {
                this.zoom = (this.zoom - 0.25).max(0.5);
                this.load(this.page, false, w, cx)
            })))
            .child(format!("{:.0}%", self.zoom * 100.))
            .child(button("pdf-zoom-in", "+").on_click(cx.listener(|this, _, w, cx| {
                this.zoom = (this.zoom + 0.25).min(3.);
                this.load(this.page, false, w, cx)
            })))
            .child(button("pdf-fit", "Fit width").on_click(cx.listener(|this, _, w, cx| {
                this.zoom = 1.;
                this.load(this.page, false, w, cx)
            })))
            .child(button("pdf-text", "Page text").on_click(cx.listener(|this, _, w, cx| {
                this.show_text = !this.show_text;
                if this.show_text {
                    this.text.update(cx, |s, cx| s.focus(w, cx));
                } else {
                    this.focus(w, cx);
                }
                cx.notify()
            })));
        toolbar = toolbar.child(button("pdf-reload", "Reload").on_click(cx.listener(|this, _, w, cx| {
            this.pages = None;
            this.load(this.page, true, w, cx)
        })));
        let mut body =
            div().id("pdf-page-scroll").flex_1().min_h_0().overflow_scroll().p_6().bg(theme.normal_fill());
        if let Some(error) = &self.error {
            body = body.child(div().text_color(theme.danger).child(format!("{}: {error}", self.path))).child(
                button("pdf-retry", "Retry")
                    .on_click(cx.listener(|this, _, w, cx| this.load(this.page, true, w, cx))),
            );
        } else if self.loading {
            body = body.child(format!("Loading page {}…", self.page + 1));
        } else if let Some(page) =
            self.cache.iter().find(|p| Some((p.number, p.requested_width)) == self.current)
        {
            if self.show_text {
                if page.text.trim().is_empty() {
                    body = body.child("This page has no extractable text. Use the page image to read it.");
                } else {
                    self.text.update(cx, |s, _| s.set_editor_style(theme.input_style()));
                    body = body.child(gpui_kit::base::Textarea::new(&self.text));
                }
            } else {
                let width = (self.viewport - 48.).max(200.) * self.zoom;
                body = body.child(
                    img(page.image.clone())
                        .w(px(width))
                        .h(px(width * page.height as f32 / page.width as f32)),
                );
            }
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
                canvas(
                    move |bounds, w, cx| {
                        monitor.update(cx, |this, cx| {
                            let width = f32::from(bounds.size.width);
                            let old = this.raster_width();
                            this.viewport = width.max(200.);
                            this.scale = w.scale_factor();
                            if old.abs_diff(this.raster_width()) > 32 {
                                this.load(this.page, false, w, cx);
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .w_full()
                .h(px(0.)),
            )
            .child(body)
    }
}

#[cfg(all(test, feature = "gui-tests"))]
mod tests;
