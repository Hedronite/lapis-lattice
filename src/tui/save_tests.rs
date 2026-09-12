use super::*;
use crate::ops::Ctx;
use std::path::PathBuf;

fn fixture() -> (PathBuf, App) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "lapis-tui-yaml-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let ctx = Ctx {
        json: false,
        vault: crate::vault::Vault { root: root.clone(), source: "test" },
        cfg: crate::config::Config::default(),
        lattice_url: "http://127.0.0.1:9".into(),
        force_http: true,
    };
    (root, App::new(ctx))
}

#[tokio::test]
async fn yaml_edit_save_undo_and_conflict_copy_preserve_raw_source() {
    let (root, mut app) = fixture();
    let original = "---\n# retain this comment\nunknown: &data [a, b]\ninvalid: [\n...";
    std::fs::write(root.join("Settings.yml"), original).unwrap();
    app.open_note("Settings.yml");
    assert!(!app.tab().unwrap().readonly);
    assert_eq!(app.tab().unwrap().body(), original);
    app.focus = crate::tui::app::Focus::Editor;
    app.paste("# pasted\r\n  # two spaces\r\n".into());
    let edited = format!("# pasted\n  # two spaces\n{original}");
    assert_eq!(app.tab().unwrap().body(), edited);
    assert!(app.tab().unwrap().dirty);
    app.save();
    assert_eq!(std::fs::read_to_string(root.join("Settings.yml")).unwrap(), edited);
    assert!(!app.tab().unwrap().dirty);
    assert!(app.tab_mut().unwrap().undo_edit(false));
    assert_eq!(app.tab().unwrap().body(), original);
    assert!(app.tab().unwrap().dirty);
    assert!(app.tab_mut().unwrap().undo_edit(true));
    assert!(!app.tab().unwrap().dirty);
    // A save-triggered watcher event must retain history.
    app.reload_tab("Settings.yml");
    assert!(app.tab_mut().unwrap().undo_edit(false));
    std::fs::write(root.join("Settings.yml"), "agent: replacement\n").unwrap();
    app.save();
    assert!(app.status.contains("changed on disk"));
    assert_eq!(std::fs::read_to_string(root.join("Settings.yml")).unwrap(), "agent: replacement\n");
    app.save_copy();
    assert_eq!(app.tab().unwrap().rel, "Settings (Lapis copy 1).yml");
    assert_eq!(std::fs::read_to_string(root.join(&app.tab().unwrap().rel)).unwrap(), original);
    assert!(app.tabs[0].dirty);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn clean_yaml_reload_keeps_document_markers_and_literal_preview() {
    let (root, mut app) = fixture();
    std::fs::write(root.join("settings.yaml"), "old: 1\n").unwrap();
    app.open_note("settings.yaml");
    let changed = "---\n# comment\n  key: \"**literal**\"\n...\n";
    std::fs::write(root.join("settings.yaml"), changed).unwrap();
    app.reload_tab("settings.yaml");
    assert_eq!(app.tab().unwrap().body(), changed);
    assert!(!app.tab().unwrap().dirty);
    let lines = crate::tui::preview::for_file("settings.yaml", changed);
    let copied = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(copied, changed);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn shorter_external_reload_clears_visual_anchor_before_next_motion() {
    use ratatui_textarea::{Input, Key};
    let (root, mut app) = fixture();
    std::fs::write(root.join("selection.yaml"), "one\ntwo\nthree").unwrap();
    app.open_note("selection.yaml");
    let tab = app.tab_mut().unwrap();
    for c in "GVgg".chars() {
        tab.vim.input(Input { key: Key::Char(c), ..Default::default() }, &mut tab.text);
    }
    assert!(tab.text.is_selecting());
    std::fs::write(root.join("selection.yaml"), "replacement").unwrap();
    app.reload_tab("selection.yaml");
    let tab = app.tab_mut().unwrap();
    assert_eq!(tab.vim.mode, crate::tui::vim::Mode::Normal);
    assert!(!tab.text.is_selecting());
    tab.vim.input(Input { key: Key::Char('j'), ..Default::default() }, &mut tab.text);
    assert_eq!(tab.body(), "replacement");
    assert!(!tab.dirty);
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}
