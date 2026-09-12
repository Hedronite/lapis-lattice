//! Explicit clipboard requests; no clipboard polling. Subprocess IO stays off
//! the UI thread, and an unavailable system route leaves Vim's register intact.

use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::app::{App, Focus, Msg, Tab};
use super::vim::Mode;

const LIMIT: usize = 16 * 1024 * 1024;

#[derive(PartialEq, Eq)]
pub(crate) struct PasteTarget {
    revision: u64,
    cursor: (usize, usize),
    selection: Option<((usize, usize), (usize, usize))>,
    mode: Mode,
}

impl PasteTarget {
    fn capture(t: &Tab) -> Self {
        Self {
            revision: t.revision,
            cursor: (t.text.cursor().0, t.text.cursor().1),
            selection: t.text.selection_range().map(|(a, b)| ((a.0, a.1), (b.0, b.1))),
            mode: t.vim.mode,
        }
    }

    pub(crate) fn matches(&self, t: &Tab) -> bool {
        !t.readonly && t.vim.prompt.is_none() && *self == Self::capture(t)
    }
}

fn remote() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

fn commands(read: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        return vec![(if read { "pbpaste" } else { "pbcopy" }, vec![])];
    }
    let mut commands = vec![];
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        commands.push(if read { ("wl-paste", vec!["--no-newline"]) } else { ("wl-copy", vec![]) });
    }
    if std::env::var_os("DISPLAY").is_some() {
        commands.push(("xclip", vec!["-selection", "clipboard", if read { "-o" } else { "-i" }]));
        commands.push(("xsel", vec!["--clipboard", if read { "--output" } else { "--input" }]));
    }
    commands
}

async fn transfer(payload: Option<String>) -> Result<String, String> {
    let read = payload.is_none();
    for (program, args) in commands(read) {
        let mut command = Command::new(program);
        command.args(args).kill_on_drop(true).stderr(Stdio::null());
        command.stdin(if read { Stdio::null() } else { Stdio::piped() });
        command.stdout(if read { Stdio::piped() } else { Stdio::null() });
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{program}: {e}")),
        };
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            if let Some(payload) = &payload {
                let mut stdin =
                    child.stdin.take().ok_or_else(|| "clipboard input unavailable".to_string())?;
                stdin.write_all(payload.as_bytes()).await.map_err(|e| e.to_string())?;
                drop(stdin);
            }
            let mut bytes = Vec::new();
            if read {
                child
                    .stdout
                    .take()
                    .ok_or_else(|| "clipboard output unavailable".to_string())?
                    .take((LIMIT + 1) as u64)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            if bytes.len() > LIMIT {
                let _ = child.kill().await;
                return Err("clipboard exceeds 16 MiB limit".into());
            }
            let status = child.wait().await.map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("{program} exited {status}"));
            }
            String::from_utf8(bytes).map_err(|_| "clipboard is not UTF-8 text".into())
        })
        .await
        .map_err(|_| format!("{program} timed out"))?;
        return result;
    }
    Err("no clipboard tool: install wl-clipboard (Wayland) or xclip/xsel (X11); terminal paste still works"
        .into())
}

fn osc52(payload: &str, tmux: bool) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    if tmux { format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b")) } else { sequence }
}

impl App {
    pub(crate) fn copy_system(&mut self, payload: String) {
        if payload.len() > LIMIT {
            self.set_status("copy exceeds 16 MiB limit; Vim register retained");
            return;
        }
        if remote() {
            if payload.len() > 100_000 {
                self.set_status("SSH copy exceeds portable OSC 52 size; Vim register retained");
                return;
            }
            let sequence = osc52(&payload, std::env::var_os("TMUX").is_some());
            let result =
                std::io::stdout().write_all(sequence.as_bytes()).and_then(|()| std::io::stdout().flush());
            self.set_status(match result {
                Ok(()) => "copy sent via OSC 52; local terminal must allow clipboard writes".into(),
                Err(e) => format!("clipboard send failed: {e}; Vim register retained"),
            });
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = transfer(Some(payload)).await.map(|_| "copied to system clipboard".to_string());
            let _ = tx.send(Msg::ClipboardCopy(result));
        });
    }

    pub(crate) fn copy_selection(&mut self, cut: bool) {
        if self.focus == Focus::Preview {
            if cut {
                self.set_status("preview is read-only; use copy");
            } else if let Some(payload) = self.tab().and_then(|t| t.reader.selected()) {
                self.copy_system(payload);
            } else {
                self.set_status("select preview text first (drag or v)");
            }
            return;
        }
        if self.focus != Focus::Editor {
            self.set_status("focus text and select it to copy");
            return;
        }
        let Some(t) = self.tab_mut() else {
            return;
        };
        if cut && t.readonly {
            self.set_status("read-only buffer; use copy");
            return;
        }
        if !t.text.selection_range().is_some_and(|(a, b)| a != b) {
            self.set_status("select text first (drag or Vim v)");
            return;
        }
        if cut {
            let cursor = t.text.cursor();
            t.text.cut();
            t.record_edit((cursor.0, cursor.1));
        } else {
            t.text.copy();
        }
        let payload = t.text.yank_text();
        if t.vim.mode != Mode::Insert {
            t.vim.mode = Mode::Normal;
        }
        self.copy_system(payload);
    }

    pub(crate) fn paste_system(&mut self) {
        if remote() {
            self.set_status("SSH: paste with your local terminal shortcut (bracketed paste); remote clipboard is separate");
            return;
        }
        let Some(t) = self.tab() else {
            return;
        };
        if self.focus != Focus::Editor || self.overlay.is_some() || t.readonly {
            self.set_status("focus an editable note to paste");
            return;
        }
        let target = PasteTarget::capture(t);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(Msg::ClipboardPaste(target, transfer(None).await));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_paste_rejects_changed_selection_mode_document_or_readonly() {
        use ratatui_textarea::{CursorMove, TextArea};
        let mut tab = Tab {
            rel: "fixture.md".into(),
            text: TextArea::from(["anchor"]),
            vim: super::super::vim::Vim::new(),
            hal: Default::default(),
            hal_valid: true,
            dirty: false,
            reader: Default::default(),
            preview_for: "anchor".into(),
            readonly: false,
            saved_source: Some("anchor".into()),
            history: Default::default(),
            revision: 1,
        };
        tab.text.start_selection();
        tab.text.move_cursor(CursorMove::Forward);
        let request = PasteTarget::capture(&tab);
        assert!(request.matches(&tab));
        tab.text.cancel_selection();
        assert!(!request.matches(&tab));
        let request = PasteTarget::capture(&tab);
        tab.vim.mode = Mode::Insert;
        assert!(!request.matches(&tab));
        let request = PasteTarget::capture(&tab);
        tab.revision += 1;
        assert!(!request.matches(&tab));
        let request = PasteTarget::capture(&tab);
        tab.readonly = true;
        assert!(!request.matches(&tab));
    }

    #[test]
    fn osc52_encodes_literal_unicode_and_wraps_tmux() {
        let text = "a\n漢字\x1b]bad";
        let plain = osc52(text, false);
        let data = plain.strip_prefix("\x1b]52;c;").unwrap().strip_suffix('\x07').unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(data).unwrap(), text.as_bytes());
        assert_eq!(osc52(text, true), format!("\x1bPtmux;{}\x1b\\", plain.replace('\x1b', "\x1b\x1b")));
    }
}
