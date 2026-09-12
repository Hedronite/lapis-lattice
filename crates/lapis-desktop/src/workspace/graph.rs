//! A lazy graph shares files, dirty buffers, retrieval services and navigation.
use super::*;
impl Workspace {
    pub(super) fn hide_graph(&mut self, cx: &mut Context<Self>) {
        self.graph_visible = false;
        if let Some(graph) = &self.graph {
            graph.update(cx, |g, cx| g.hide(cx));
        }
    }
    pub(super) fn toggle_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.graph_visible {
            self.hide_graph(cx);
            self.focus_active(window, cx);
        } else {
            self.show_graph(None, window, cx);
        }
        cx.notify();
    }
    pub(super) fn show_graph(&mut self, travel: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_epoch += 1;
        self.opening = None;
        self.command = None;
        self.graph_visible = true;
        let seed = self.tabs.get(self.active).map(|t| t.document.path.clone());
        if self.graph.is_none() {
            let graph = cx.new(|cx| {
                crate::window::Root::new(Default::default(), Some(self.services.clone()), seed.clone(), cx)
            });
            self.graph_events = Some(cx.subscribe_in(
                &graph,
                window,
                |this, _, event: &crate::window::OpenNote, window, cx| {
                    this.open_file(event.0.clone(), window, cx);
                },
            ));
            self.graph = Some(graph);
        }
        for tab in &self.tabs {
            if let Some(pdf) = &tab.pdf {
                pdf.update(cx, |p, cx| p.set_visible(false, window, cx));
            }
        }
        self.graph.as_ref().unwrap().update(cx, |g, cx| g.show(seed, window, cx));
        self.history.commit(history::Visit::Graph, travel);
        cx.notify();
    }
}
