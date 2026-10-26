//! Following a link clicked in Live: wikilinks resolve through the vault, URLs open outside.
use super::*;

impl Workspace {
    /// A link clicked in Reading view: rewritten wikilinks carry [`crate::live::wiki::SCHEME`].
    pub(super) fn follow_reading_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        match url.strip_prefix(crate::live::wiki::SCHEME) {
            Some(target) => self.follow_link(target.to_string(), true, window, cx),
            None => self.follow_link(url.to_string(), false, window, cx),
        }
    }
    pub(super) fn follow_link(
        &mut self,
        target: String,
        wiki: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !wiki && ["http://", "https://", "mailto:"].iter().any(|p| target.starts_with(p)) {
            cx.open_url(&target);
            self.status = format!("Opened {target}");
            self.error = false;
            cx.notify();
            return;
        }
        // A relative Markdown link names a file next to the current note.
        let link = match self.tabs.get(self.active).map(|t| t.document.path.clone()) {
            Some(from) if !wiki && !target.starts_with('/') => match from.rsplit_once('/') {
                Some((folder, _)) => format!("{folder}/{target}"),
                None => target.clone(),
            },
            _ => target.trim_start_matches('/').to_string(),
        };
        self.status = format!("Following {target}…");
        self.error = false;
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { service.resolve_link(&link) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Some(path)) => {
                        this.status.clear();
                        this.open_file(path, window, cx);
                    }
                    Ok(None) => {
                        this.status = format!("No note matches {target}");
                        this.error = true;
                    }
                    Err(e) => {
                        this.status = e;
                        this.error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
