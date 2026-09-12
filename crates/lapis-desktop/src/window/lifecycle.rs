//! Graph acquisition and previews never run on the paint/input thread.
use super::*;
use std::sync::Arc;

pub(crate) struct OpenNote(pub String);
impl gpui_kit::EventEmitter<OpenNote> for Root {}

impl Root {
    pub(crate) fn new(
        vault: PathBuf,
        services: Option<Arc<dyn crate::services::WorkspaceServices>>,
        seed: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            vault,
            services,
            seed,
            snapshot: None,
            loading: false,
            load_epoch: 0,
            preview_epoch: 0,
            visible: false,
            scene: Scene::default(),
            error: None,
            selected: None,
            peek: None,
            filters: Filters::default(),
            typing: false,
            depth: 2,
            layout: Layout::Global,
            groups: vec![],
            sim: ForceSim::empty(),
            meter: Meter::default(),
            camera: Camera::default(),
            fit_zoom: 1.,
            hover: None,
            press: None,
            pins: BTreeMap::new(),
            origin: Rc::new(Cell::new((0., 0.))),
            board_px: Rc::new(Cell::new((0., 0.))),
            fitted_for: (0., 0.),
            draw_ms: Rc::new(Cell::new(0.)),
            skipped_tick: false,
            focus: cx.focus_handle(),
        }
    }
    pub(super) fn next_layout(&self) -> Layout {
        if self.services.is_some() && self.layout == Layout::Local {
            Layout::Global
        } else {
            self.layout.next()
        }
    }
    pub(crate) fn show(&mut self, seed: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self.seed != seed;
        self.seed = seed;
        self.selected = self.seed.clone();
        for n in &mut self.scene.nodes {
            n.is_seed = Some(&n.id) == self.seed.as_ref();
        }
        self.visible = true;
        self.focus.focus(window, cx);
        if self.snapshot.is_none() || (changed && self.layout == Layout::Local) {
            self.reload(window, cx);
        }
        cx.notify();
    }
    pub(crate) fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
    }
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.press = None;
        self.hover = None;
        self.preview_epoch += 1;
        self.load_epoch += 1;
        if self.loading {
            self.snapshot = None;
        }
        self.loading = false;
        self.meter.idle();
        cx.notify();
    }
    pub(crate) fn invalidate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.snapshot = None;
        if self.visible {
            self.reload(window, cx);
        }
    }
    pub(super) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_epoch += 1;
        let epoch = self.load_epoch;
        self.loading = true;
        self.error = None;
        self.press = None;
        self.hover = None;
        self.preview_epoch += 1;
        self.peek = None;
        let (services, vault, cached) = (self.services.clone(), self.vault.clone(), self.snapshot.clone());
        let (seed, layout, depth) = (self.seed.clone().unwrap_or_default(), self.layout, self.depth);
        let task = cx.background_executor().spawn(async move {
            if layout == Layout::Rings {
                return graph_data::scene_for(&vault, &seed, 2, false)
                    .map(|scene| (None, graph_data::Laid { scene, sim: ForceSim::empty() }));
            }
            let snapshot = match cached {
                Some(snapshot) => snapshot,
                None => Arc::new(match services {
                    Some(service) => service.graph_snapshot()?,
                    None => lapis_lattice::Engine::open(&vault)
                        .and_then(|e| e.graph_snapshot())
                        .map_err(|e| e.to_string())?,
                }),
            };
            if layout == Layout::Local && seed.is_empty() {
                return Err("Open a note to choose the center of the local graph".to_string());
            }
            let scene = if layout == Layout::Local {
                graph_data::within(&snapshot, &seed, depth)
            } else {
                (*snapshot).clone()
            };
            // Settle off-thread. Subsequent dragging uses the existing live simulation.
            let laid = graph_data::lay_out(&scene, &seed, graph_data::SETTLE_TICKS);
            Ok((Some(snapshot), laid))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.load_epoch != epoch || !this.visible {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((snapshot, laid)) => {
                        this.snapshot = snapshot;
                        this.adopt(laid);
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
                window.refresh();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn preview(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_epoch += 1;
        let epoch = self.preview_epoch;
        self.peek = None;
        let Some(id) = id else { return };
        if id.starts_with("dangling:") {
            self.peek =
                Some(format!("{} · Unresolved link; no note to open", id.trim_start_matches("dangling:")));
            return;
        }
        self.peek = Some(format!("{id} · Loading preview…"));
        let services = self.services.clone();
        let vault = self.vault.clone();
        let task = cx.background_executor().spawn(async move {
            match services {
                Some(service) => service.graph_preview(&id).map(|text| format!("{id} · {text}")),
                None => graph_data::peek(&vault, &id, 600).map(|text| format!("{id} · {text}")),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.preview_epoch != epoch || !this.visible {
                    return;
                }
                this.peek = Some(result.unwrap_or_else(|e| format!("Preview unavailable: {e}")));
                cx.notify();
                window.refresh();
            });
        })
        .detach();
    }
    pub(crate) fn open_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.error.is_some() {
            return;
        }
        self.selected = Some(id.into());
        self.preview(Some(id.into()), window, cx);
        if !id.starts_with("dangling:") {
            if self.services.is_some() {
                cx.emit(OpenNote(id.into()));
            } else {
                let changed = self.seed.as_deref() != Some(id);
                self.seed = Some(id.into());
                if changed && self.layout == Layout::Local {
                    self.reload(window, cx);
                }
            }
        }
        cx.notify();
    }
}
