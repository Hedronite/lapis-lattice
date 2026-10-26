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
        let from = if wiki { None } else { self.tabs.get(self.active).map(|t| t.document.path.clone()) };
        let Some(link) = vault_link(from.as_deref(), &target) else {
            self.status = format!("{target} points outside the vault");
            self.error = true;
            cx.notify();
            return;
        };
        self.status = format!("Following {target}…");
        self.error = false;
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { service.resolve_link(&link) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let applied = this.update_in(cx, |this, window, cx| {
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
            if let Err(error) = applied {
                eprintln!("lapis workspace: link result dropped: {error}");
            }
        })
        .detach();
        cx.notify();
    }
}

/// The vault-relative path a link names. A Markdown link is relative to the note
/// `from` (`../` climbs, `./` stays) and percent-escapes are decoded; a wikilink, or a
/// link starting with `/`, is relative to the vault root. `None` when it climbs out.
fn vault_link(from: Option<&str>, target: &str) -> Option<String> {
    let target = if from.is_some() { percent_decode(target) } else { target.to_string() };
    let mut parts: Vec<&str> = match from {
        Some(from) if !target.starts_with('/') => from.split('/').collect(),
        _ => vec![],
    };
    parts.pop(); // the note itself, or nothing at the root
    for piece in target.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            piece => parts.push(piece),
        }
    }
    Some(parts.join("/"))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) =
                (bytes.get(i + 1).and_then(|b| hex(*b)), bytes.get(i + 2).and_then(|b| hex(*b)))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::vault_link;

    #[test]
    fn relative_markdown_links_climb_and_decode() {
        assert_eq!(vault_link(Some("notes/day/today.md"), "../plan.md").as_deref(), Some("notes/plan.md"));
        assert_eq!(vault_link(Some("notes/today.md"), "./a%20b.md").as_deref(), Some("notes/a b.md"));
        assert_eq!(vault_link(Some("top.md"), "x.md").as_deref(), Some("x.md"));
        assert_eq!(
            vault_link(Some("notes/today.md"), "/projects/demo.md").as_deref(),
            Some("projects/demo.md")
        );
        assert_eq!(vault_link(Some("notes/today.md"), "../../out.md"), None);
    }

    #[test]
    fn wikilinks_are_vault_rooted_and_literal() {
        assert_eq!(vault_link(None, "projects/demo").as_deref(), Some("projects/demo"));
        assert_eq!(vault_link(None, "100%25 done").as_deref(), Some("100%25 done"));
    }
}
