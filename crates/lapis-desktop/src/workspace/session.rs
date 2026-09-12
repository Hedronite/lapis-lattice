//! Restore metadata first, then only the selected document; remaining tabs stay lazy.
use super::*;
impl Workspace {
    pub(super) fn restore_session(
        &mut self,
        seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let service = self.services.clone();
        let epoch = self.open_epoch;
        let task = cx.background_executor().spawn(async move { service.load_session() });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // User navigation while metadata loads wins over the saved session.
                if this.open_epoch != epoch {
                    this.session_ready = result.is_ok();
                    return;
                }
                let session = match result.and_then(|s| {
                    if let Some(s) = &s {
                        s.validate()?;
                    }
                    Ok(s)
                }) {
                    Ok(session) => {
                        this.session_ready = true;
                        session.unwrap_or_default()
                    }
                    Err(error) => {
                        this.session_notice =
                            Some(format!("Workspace restore failed: {error}. Saved state retained."));
                        crate::session::Session::default()
                    }
                };
                this.sidebar_width = session.sidebar_width;
                this.context_width = session.context_width;
                this.layout = cx.new(|_| gpui_kit::base::ResizableState::default());
                this.context_layout = cx.new(|_| gpui_kit::base::ResizableState::default());
                this.context = session.context;
                this.tab_order = session.tabs.iter().map(|t| t.path.clone()).collect();
                this.restored = session.tabs;
                this.directory(session.folder, window, cx);
                if let Some(path) = seed.or(session.active).or_else(|| this.tab_order.first().cloned()) {
                    this.open_file(path, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn session_snapshot(&self, cx: &App) -> crate::session::Session {
        let tabs = self
            .tab_order
            .iter()
            .filter_map(|path| {
                if let Some(tab) = self.tabs.iter().find(|t| &t.document.path == path) {
                    let editor = tab.editor.read(cx);
                    let selection = editor.selected_range();
                    let source = editor.scroll_offset();
                    let reading = tab.reading_scroll.offset();
                    let (pdf_page, pdf_zoom) =
                        tab.pdf.as_ref().map(|p| p.read(cx).position()).unwrap_or((0, 1.));
                    Some(crate::session::Tab {
                        path: path.clone(),
                        title: tab.document.title.chars().take(1024).collect(),
                        view: tab.view,
                        selection: [selection.start, selection.end],
                        source_scroll: [source.x.as_f32().min(0.), source.y.as_f32().min(0.)],
                        reading_scroll: [reading.x.as_f32().min(0.), reading.y.as_f32().min(0.)],
                        split_width: tab
                            .split
                            .read(cx)
                            .sizes()
                            .first()
                            .map(|v| v.as_f32().clamp(100., 10000.))
                            .or(tab.split_width),
                        live_scroll: tab.live.read(cx).scroll_position(),
                        pdf_page,
                        pdf_zoom,
                    })
                } else {
                    self.restored.iter().find(|t| &t.path == path).cloned()
                }
            })
            .collect();
        crate::session::Session {
            version: 1,
            tabs,
            active: self
                .tabs
                .get(self.active)
                .map(|t| t.document.path.clone())
                .or_else(|| self.tab_order.first().cloned()),
            folder: self.folder.clone(),
            context: self.context,
            sidebar_width: self
                .layout
                .read(cx)
                .sizes()
                .first()
                .map(|p| p.as_f32().clamp(140., 480.))
                .unwrap_or(self.sidebar_width),
            context_width: self
                .context_layout
                .read(cx)
                .sizes()
                .get(1)
                .map(|p| p.as_f32().clamp(180., 520.))
                .unwrap_or(self.context_width),
        }
    }
    pub(super) fn close_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|t| t.document.path == path) {
            let current = self.tabs.get(self.active).map(|t| t.document.path.clone());
            self.active = index;
            self.close_tab(false, cx);
            if !self.tabs.iter().any(|t| t.document.path == path)
                && let Some(index) = self.tabs.iter().position(|t| Some(&t.document.path) == current.as_ref())
            {
                self.active = index;
            }
        } else {
            self.restored.retain(|t| t.path != path);
            self.tab_order.retain(|p| p != path);
            self.open_epoch += 1;
            self.opening = None;
        }
        self.focus_active(window, cx);
        cx.notify();
    }
}
