//! G2a golden: the exact bytes `lapis create` writes, recorded before the
//! create-set, slugging, inbox and containment move into `lapis-notes`. After
//! the move the CLI must still write these bytes, so this file stays unchanged.
//!
//! Each case drives the real binary on a fresh temp vault with `--no-reindex`,
//! so only the note is written, and compares every file in the vault tree.
//!
//! Dates: `created` and `updated` come from the clock and there is no hook to
//! pin it, so the goldens hold `{DATE}` in those two fields and nowhere else.
//! The test reads the date the binary wrote, checks it is today in UTC (the
//! child runs with `TZ=UTC`), and substitutes only that. Every other byte is
//! exact.

use std::path::{Path, PathBuf};
use std::process::Output;

/// Case 1: `create --path projects/` with tags, a body and a config operator.
const CREATE: &str = "---
name: Ship the Golden Note
title: Ship the Golden Note
type: note
doc_type: note
domain: projects
status: draft
created: {DATE}
updated: {DATE}
hal_version: '1.0'
operator: Golden Operator
tags:
- lapis
- golden
---
First line.

Second line.
";

/// Case 2: `create` with no `--path` lands in the overlay's inbox bucket, with
/// the default `# <title>` body. The title needs YAML quoting and slugs to
/// lowercase Unicode.
const INBOX: &str = "---
name: 'Inbox drop: Café & Ünïcode!'
title: 'Inbox drop: Café & Ünïcode!'
type: note
doc_type: note
domain: inbox
status: draft
created: {DATE}
updated: {DATE}
hal_version: '1.0'
tags: []
---
# Inbox drop: Café & Ünïcode!
";

/// Case 3: the first `create --title "Same Title"` in a vault with no overlay
/// (default bucket `inbox`). The second is refused and changes nothing.
const SAME: &str = "---
name: Same Title
title: Same Title
type: note
doc_type: note
domain: inbox
status: draft
created: {DATE}
updated: {DATE}
hal_version: '1.0'
tags: []
---
# Same Title
";

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lapis-golden-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn lapis(vault: &Path, config: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_lapis"))
        .arg("--vault")
        .arg(vault)
        .args(args)
        .env("XDG_CONFIG_HOME", config)
        .env("TZ", "UTC")
        .env_remove("LAPIS_VAULT")
        .output()
        .unwrap()
}

fn today_utc() -> String {
    jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).strftime("%Y-%m-%d").to_string()
}

/// Every file under `dir`, vault-relative, sorted.
fn tree(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            tree(root, &path, out);
        } else {
            out.push(path.strip_prefix(root).unwrap().to_str().unwrap().to_string());
        }
    }
    out.sort();
}

fn files(vault: &Path) -> Vec<String> {
    let mut out = Vec::new();
    tree(vault, vault, &mut out);
    out
}

/// `golden` with `{DATE}` replaced by the date the binary wrote, once that
/// date is checked to be today in UTC (`before` or `after` the run).
fn expected(golden: &str, written: &str, before: &str, after: &str) -> String {
    assert_eq!(golden.matches("{DATE}").count(), 2, "only created and updated are substituted");
    let date = written
        .lines()
        .find_map(|l| l.strip_prefix("created: "))
        .unwrap_or_else(|| panic!("no created line:\n{written}"));
    assert!(date == before || date == after, "created {date} is not today ({before} or {after}) in UTC");
    golden.replace("{DATE}", date)
}

fn assert_ok(out: &Output, stdout: &str) {
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), stdout, "{err}");
    assert_eq!(err, "");
}

#[test]
fn create_writes_the_golden_bytes() {
    let vault = temp_dir("create-vault");
    let config = temp_dir("create-config");
    std::fs::create_dir_all(config.join("lapis")).unwrap();
    std::fs::write(config.join("lapis/config.toml"), "[operator]\nname = \"Golden Operator\"\n").unwrap();

    let before = today_utc();
    let out = lapis(
        &vault,
        &config,
        &[
            "create",
            "--title",
            "Ship the Golden Note",
            "--path",
            "projects/",
            "--tag",
            "lapis",
            "--tag",
            "golden",
            "--body",
            "First line.\n\nSecond line.",
            "--no-reindex",
        ],
    );
    let after = today_utc();
    assert_ok(&out, "projects/ship-the-golden-note.md\n");
    assert_eq!(files(&vault), ["projects/ship-the-golden-note.md"]);
    let written = std::fs::read_to_string(vault.join("projects/ship-the-golden-note.md")).unwrap();
    assert_eq!(written, expected(CREATE, &written, &before, &after));

    let _ = std::fs::remove_dir_all(vault);
    let _ = std::fs::remove_dir_all(config);
}

#[test]
fn inbox_create_writes_the_golden_bytes() {
    let vault = temp_dir("inbox-vault");
    let config = temp_dir("inbox-config");
    let overlay = "{\"buckets\":{\"inbox\":\"Inbox/Drop\"}}\n";
    std::fs::create_dir_all(vault.join(".lapis")).unwrap();
    std::fs::write(vault.join(".lapis/vault.json"), overlay).unwrap();

    let before = today_utc();
    let out = lapis(&vault, &config, &["create", "--title", "Inbox drop: Café & Ünïcode!", "--no-reindex"]);
    let after = today_utc();
    let rel = "Inbox/Drop/inbox-drop-café-ünïcode.md";
    assert_ok(&out, &format!("{rel}\n"));
    assert_eq!(files(&vault), [".lapis/vault.json", rel]);
    assert_eq!(std::fs::read_to_string(vault.join(".lapis/vault.json")).unwrap(), overlay);
    let written = std::fs::read_to_string(vault.join(rel)).unwrap();
    assert_eq!(written, expected(INBOX, &written, &before, &after));

    let _ = std::fs::remove_dir_all(vault);
    let _ = std::fs::remove_dir_all(config);
}

/// A second create of the same title is refused: exit 1, the usage message on
/// stderr (and in the `--json` envelope), no new name, and the first note's
/// bytes untouched.
#[test]
fn collision_is_refused_and_keeps_the_first_bytes() {
    let vault = temp_dir("same-vault");
    let config = temp_dir("same-config");
    let args = ["create", "--title", "Same Title", "--no-reindex"];

    let before = today_utc();
    let out = lapis(&vault, &config, &args);
    let after = today_utc();
    assert_ok(&out, "inbox/same-title.md\n");
    let first = std::fs::read(vault.join("inbox/same-title.md")).unwrap();
    let text = String::from_utf8(first.clone()).unwrap();
    assert_eq!(text, expected(SAME, &text, &before, &after));

    let out = lapis(&vault, &config, &args);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "lapis: already exists: inbox/same-title.md\n");
    assert_eq!(files(&vault), ["inbox/same-title.md"]);
    assert_eq!(std::fs::read(vault.join("inbox/same-title.md")).unwrap(), first);

    let mut json = vec!["--json"];
    json.extend(args);
    let out = lapis(&vault, &config, &json);
    assert_eq!(out.status.code(), Some(1));
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["ok"], false);
    assert_eq!(
        reply["error"],
        serde_json::json!({"kind": "usage", "message": "already exists: inbox/same-title.md", "exit": 1})
    );
    assert_eq!(files(&vault), ["inbox/same-title.md"]);
    assert_eq!(std::fs::read(vault.join("inbox/same-title.md")).unwrap(), first);

    let _ = std::fs::remove_dir_all(vault);
    let _ = std::fs::remove_dir_all(config);
}
