use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{EditOp, edit};
use crate::error::{LapisError, Result};
use crate::notes::{self, HASH_ALGO};
use crate::write::{Guard, Written, today};

fn vault() -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lapis-edit-{}-{n}-{seq}", std::process::id()));
    std::fs::create_dir_all(d.join("notes")).unwrap();
    d
}

fn put(v: &Path, rel: &str, text: &str) {
    let p = v.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
}

fn disk(v: &Path, rel: &str) -> Vec<u8> {
    std::fs::read(v.join(rel)).unwrap()
}

fn repl(v: &Path, rel: &str, old: &str, new: &str, g: &Guard) -> Result<Written> {
    edit(v, rel, &EditOp::Replace { old: old.into(), new: new.into() }, g)
}

fn ins(v: &Path, rel: &str, anchor: &str, text: &str, before: bool, g: &Guard) -> Result<Written> {
    edit(v, rel, &EditOp::Insert { anchor: anchor.into(), text: text.into(), before }, g)
}

fn matches_of(err: LapisError) -> usize {
    match err {
        LapisError::Matches { matches, message } => {
            assert!(message.contains(&matches.to_string()), "{message}");
            let env =
                serde_json::to_value(crate::envelope::err(&LapisError::Matches { matches, message }, 0.0))
                    .unwrap();
            assert_eq!(env["ok"], false);
            assert_eq!(env["error"]["exit"], 1);
            assert_eq!(env["error"]["matches"], matches);
            assert!(env["error"]["message"].as_str().unwrap().contains(&matches.to_string()));
            matches
        }
        other => panic!("expected match error, got {other}"),
    }
}

const NOTE: &str = "---\nname: X\n# a comment\nweird-key: [1, 2]\nupdated: 2020-01-01\n---\nhello anchor\n";

#[test]
fn unique_replace_bumps_updated_and_keeps_other_frontmatter() {
    let v = vault();
    put(&v, "notes/a.md", NOTE);
    let before = disk(&v, "notes/a.md");
    let w = repl(&v, "notes/a.md", "anchor", "world", &Guard::default()).unwrap();
    let out = disk(&v, "notes/a.md");
    let text = String::from_utf8(out.clone()).unwrap();
    assert!(text.contains("# a comment\nweird-key: [1, 2]\n"), "{text}");
    assert!(text.contains(&format!("updated: {}\n", today())), "{text}");
    assert!(text.ends_with("hello world\n"), "{text}");
    assert!(!text.contains("anchor"), "{text}");
    assert_eq!(w.old_hash.as_deref(), Some(notes::content_hash(&before).as_str()));
    assert_eq!(w.new_hash.as_deref(), Some(notes::content_hash(&out).as_str()));
    assert_eq!(w.hash_algo, Some(HASH_ALGO));
    let j = serde_json::to_value(&w).unwrap();
    assert_eq!(j["hashAlgo"], HASH_ALGO);
    assert!(j["oldHash"].as_str().unwrap().starts_with("fnv1a64:"));
    assert!(j.get("diff").is_none());
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn zero_and_overlapping_matches_leave_the_file_identical() {
    let v = vault();
    put(&v, "notes/a.md", NOTE);
    let before = disk(&v, "notes/a.md");
    let err = repl(&v, "notes/a.md", "missing", "x", &Guard::default()).unwrap_err();
    assert_eq!(matches_of(err), 0);
    assert_eq!(disk(&v, "notes/a.md"), before);
    put(&v, "notes/a.md", "---\nname: N\n---\naaa\n");
    let before = disk(&v, "notes/a.md");
    let err = repl(&v, "notes/a.md", "aa", "b", &Guard::default()).unwrap_err();
    assert_eq!(matches_of(err), 2, "aa overlaps twice in aaa");
    assert_eq!(disk(&v, "notes/a.md"), before);
    put(&v, "notes/a.md", "---\nname: N\n---\nbeta beta\n");
    let err = repl(&v, "notes/a.md", "beta", "x", &Guard::default()).unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert_eq!(matches_of(err), 2);
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn stale_hash_and_mtime_write_nothing() {
    let v = vault();
    put(&v, "notes/a.md", NOTE);
    let before = disk(&v, "notes/a.md");
    let hash = notes::content_hash(&before);
    let bad = Guard { if_hash: Some("fnv1a64:0000000000000000".into()), ..Guard::default() };
    let err = repl(&v, "notes/a.md", "anchor", "world", &bad).unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert!(err.to_string().contains("stale"), "{err}");
    assert_eq!(disk(&v, "notes/a.md"), before);
    let bad_m = Guard { if_mtime: Some(1), ..Guard::default() };
    let err = repl(&v, "notes/a.md", "anchor", "world", &bad_m).unwrap_err();
    assert!(err.to_string().contains("mtime"), "{err}");
    assert_eq!(disk(&v, "notes/a.md"), before);
    let ok = Guard { if_hash: Some(hash), ..Guard::default() };
    repl(&v, "notes/a.md", "anchor", "world", &ok).unwrap();
    assert!(String::from_utf8(disk(&v, "notes/a.md")).unwrap().contains("hello world\n"));
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn dry_run_skips_write_updated_bump_and_returns_diff() {
    let v = vault();
    put(&v, "notes/a.md", NOTE);
    let before = disk(&v, "notes/a.md");
    let mtime = std::fs::metadata(v.join("notes/a.md")).unwrap().modified().unwrap();
    let g = Guard { dry_run: true, ..Guard::default() };
    let w = repl(&v, "notes/a.md", "anchor", "world", &g).unwrap();
    assert!(w.dry_run);
    assert_eq!(disk(&v, "notes/a.md"), before);
    assert_eq!(std::fs::metadata(v.join("notes/a.md")).unwrap().modified().unwrap(), mtime);
    let preview = w.text.unwrap();
    assert!(preview.contains("hello world\n"), "{preview}");
    assert!(preview.contains("updated: 2020-01-01\n"), "dry-run must not bump updated: {preview}");
    let diff = w.diff.unwrap();
    assert!(diff.contains("-hello anchor") && diff.contains("+hello world"), "{diff}");
    assert!(!diff.contains(&format!("updated: {}", today())), "{diff}");
    assert_eq!(w.new_hash.unwrap(), notes::content_hash(preview.as_bytes()));
    assert_eq!(w.old_hash.unwrap(), notes::content_hash(&before));
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn missing_updated_is_inserted_and_absent_frontmatter_is_left_alone() {
    let v = vault();
    put(&v, "notes/a.md", "---\nname: X\n# keep\n---\nhello\n");
    repl(&v, "notes/a.md", "hello", "world", &Guard::default()).unwrap();
    let text = String::from_utf8(disk(&v, "notes/a.md")).unwrap();
    assert_eq!(text, format!("---\nname: X\n# keep\nupdated: {}\n---\nworld\n", today()), "{text}");
    put(&v, "notes/b.md", "hello\n");
    repl(&v, "notes/b.md", "hello", "world", &Guard::default()).unwrap();
    assert_eq!(disk(&v, "notes/b.md"), b"world\n");
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn frontmatter_is_not_a_match_target_and_newlines_are_raw() {
    let v = vault();
    put(&v, "notes/a.md", "---\nname: secret\n---\nbody\n");
    let before = disk(&v, "notes/a.md");
    assert_eq!(matches_of(repl(&v, "notes/a.md", "secret", "nope", &Guard::default()).unwrap_err()), 0);
    assert_eq!(disk(&v, "notes/a.md"), before, "a frontmatter-only needle does not match");
    put(&v, "notes/a.md", "---\nname: N\n---\nhello");
    assert_eq!(matches_of(repl(&v, "notes/a.md", "hello\n", "x", &Guard::default()).unwrap_err()), 0);
    repl(&v, "notes/a.md", "hello", "world", &Guard::default()).unwrap();
    let text = String::from_utf8(disk(&v, "notes/a.md")).unwrap();
    assert!(text.ends_with("---\nworld"), "{text}");
    assert!(!text.ends_with("world\n"), "{text}");
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn crlf_and_unicode_match_raw_bytes() {
    let v = vault();
    let raw = "---\r\nname: Café\r\n# keep\r\nupdated: 2020-01-01\r\n---\r\nfoo\r\nbar\r\n";
    put(&v, "notes/a.md", raw);
    assert_eq!(matches_of(repl(&v, "notes/a.md", "foo\nbar", "x", &Guard::default()).unwrap_err()), 0);
    assert_eq!(disk(&v, "notes/a.md"), raw.as_bytes());
    repl(&v, "notes/a.md", "bar", "baz", &Guard::default()).unwrap();
    let want = format!("---\r\nname: Café\r\n# keep\r\nupdated: {}\r\n---\r\nfoo\r\nbaz\r\n", today());
    assert_eq!(String::from_utf8(disk(&v, "notes/a.md")).unwrap(), want);
    put(&v, "notes/a.md", "---\nname: N\n---\nsee 漢字 here\n");
    repl(&v, "notes/a.md", "漢字", "かな", &Guard::default()).unwrap();
    let text = String::from_utf8(disk(&v, "notes/a.md")).unwrap();
    assert!(text.contains("see かな here\n"), "{text}");
    assert!(!text.contains('漢'), "{text}");
    put(&v, "notes/a.md", "---\nname: N\n---\nééé\n");
    assert_eq!(matches_of(repl(&v, "notes/a.md", "éé", "x", &Guard::default()).unwrap_err()), 2);
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn insert_before_and_after_use_the_same_uniqueness_rule() {
    let v = vault();
    let raw = "---\nname: N\nupdated: 2020-01-01\n---\nAA BB CC\n";
    put(&v, "notes/a.md", raw);
    ins(&v, "notes/a.md", "BB", " XX", false, &Guard::default()).unwrap();
    assert!(String::from_utf8(disk(&v, "notes/a.md")).unwrap().ends_with("AA BB XX CC\n"));
    put(&v, "notes/a.md", raw);
    ins(&v, "notes/a.md", "BB", "XX ", true, &Guard::default()).unwrap();
    assert!(String::from_utf8(disk(&v, "notes/a.md")).unwrap().ends_with("AA XX BB CC\n"));
    put(&v, "notes/a.md", raw);
    let before = disk(&v, "notes/a.md");
    let err = ins(&v, "notes/a.md", "nope", "z", false, &Guard::default()).unwrap_err();
    assert_eq!(matches_of(err), 0);
    assert_eq!(disk(&v, "notes/a.md"), before);
    put(&v, "notes/a.md", "---\nname: N\n---\nAA AA\n");
    assert_eq!(matches_of(ins(&v, "notes/a.md", "AA", "!", false, &Guard::default()).unwrap_err()), 2);
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn sandbox_rejects_parent_and_absolute_without_touching_them() {
    let v = vault();
    let secret = std::env::temp_dir().join(format!("lapis-edit-secret-{}-{}", std::process::id(), unique()));
    std::fs::write(&secret, b"SECRET").unwrap();
    let abs = secret.to_str().unwrap();
    let err = repl(&v, abs, "SECRET", "nope", &Guard::default()).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert_eq!(std::fs::read(&secret).unwrap(), b"SECRET");
    let name = format!("secret-{}.md", unique());
    let sibling = v.parent().unwrap().join(&name);
    std::fs::write(&sibling, b"SECRET").unwrap();
    let err = repl(&v, &format!("../{name}"), "SECRET", "nope", &Guard::default()).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert_eq!(std::fs::read(&sibling).unwrap(), b"SECRET");
    let _ = std::fs::remove_file(&secret);
    let _ = std::fs::remove_file(&sibling);
    let _ = std::fs::remove_dir_all(&v);
}

fn unique() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

#[cfg(unix)]
#[test]
fn symlink_out_of_vault_exits_3_and_keeps_the_target() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let v = vault();
    let outside = std::env::temp_dir().join(format!("lapis-edit-link-{}-{}", std::process::id(), unique()));
    std::fs::write(&outside, b"SECRET").unwrap();
    symlink(&outside, v.join("notes/link.md")).unwrap();
    let err = repl(&v, "notes/link.md", "SECRET", "nope", &Guard::default()).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert!(err.to_string().contains("symlink"), "{err}");
    assert_eq!(std::fs::read(&outside).unwrap(), b"SECRET");
    assert!(std::fs::symlink_metadata(v.join("notes/link.md")).unwrap().file_type().is_symlink());
    let dir = std::env::temp_dir().join(format!("lapis-edit-dir-{}-{}", std::process::id(), unique()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("note.md"), b"SECRET").unwrap();
    symlink(&dir, v.join("out")).unwrap();
    let err = repl(&v, "out/note.md", "SECRET", "nope", &Guard::default()).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert_eq!(std::fs::read(dir.join("note.md")).unwrap(), b"SECRET");
    put(&v, "notes/a.md", NOTE);
    std::fs::set_permissions(v.join("notes/a.md"), std::fs::Permissions::from_mode(0o640)).unwrap();
    repl(&v, "notes/a.md", "anchor", "world", &Guard::default()).unwrap();
    let mode = std::fs::metadata(v.join("notes/a.md")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
    let tmp = std::fs::read_dir(v.join("notes"))
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".tmp"));
    assert!(!tmp, "atomic rename leaves no temp file");
    let _ = std::fs::remove_dir_all(&v);
    let _ = std::fs::remove_file(&outside);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reindex_touches_only_the_edited_note_and_dry_run_skips_it() {
    let v = vault();
    put(&v, "notes/a.md", "---\nname: A\n---\nalphatoken stays readable\n");
    put(&v, "notes/b.md", "---\nname: B\n---\nbetatoken stays readable\n");
    lapis_lattice::Engine::open(&v).unwrap().reindex().unwrap();
    let ctx = crate::ops::Ctx {
        json: true,
        vault: crate::vault::Vault { root: v.clone(), source: "test" },
        cfg: crate::config::Config::default(),
        lattice_url: crate::config::DEFAULT_LATTICE_URL.into(),
        force_http: false,
    };
    let w = repl(&v, "notes/a.md", "alphatoken", "alphareplaced", &Guard::default()).unwrap();
    let report = crate::ops::kick(&ctx, w, false).await.unwrap();
    assert_eq!(report.reindex.unwrap().path, "notes/a.md");
    let engine = lapis_lattice::Engine::open(&v).unwrap();
    let hits = engine.search("alphareplaced", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "notes/a.md");
    assert!(engine.search("alphatoken", 10).unwrap().is_empty());
    let other = engine.search("betatoken", 10).unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].path, "notes/b.md");
    let before = disk(&v, "notes/b.md");
    let w =
        repl(&v, "notes/b.md", "betatoken", "shouldnotland", &Guard { dry_run: true, ..Guard::default() })
            .unwrap();
    let report = crate::ops::kick(&ctx, w, false).await.unwrap();
    assert!(report.reindex.is_none());
    assert_eq!(disk(&v, "notes/b.md"), before);
    assert!(engine.search("shouldnotland", 10).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&v);
}

#[tokio::test]
async fn mcp_lapis_edit_round_trip() {
    let v = vault();
    put(&v, "notes/a.md", "---\nname: A\nupdated: 1999-01-01\n---\nhello\n");
    let server = crate::mcp::LapisServer::new(crate::ops::Ctx {
        json: true,
        vault: crate::vault::Vault { root: v.clone(), source: "test" },
        cfg: crate::config::Config::default(),
        lattice_url: crate::config::DEFAULT_LATTICE_URL.into(),
        force_http: false,
    });
    let ok = server
        .lapis_edit(rmcp::handler::server::wrapper::Parameters(crate::mcp::EditArg {
            path: "notes/a.md".into(),
            mode: "replace".into(),
            old: "hello".into(),
            new: "world".into(),
            dry_run: None,
            if_mtime: None,
            if_hash: Some(notes::content_hash(&disk(&v, "notes/a.md"))),
        }))
        .await
        .unwrap();
    let env = ok.structured_content.unwrap();
    assert_eq!(env["ok"], true);
    assert_eq!(env["data"]["hashAlgo"], HASH_ALGO);
    assert!(env["data"]["newHash"].as_str().unwrap().starts_with("fnv1a64:"));
    assert!(String::from_utf8(disk(&v, "notes/a.md")).unwrap().contains("world\n"));
    put(&v, "notes/a.md", "---\nname: A\n---\nhello hello\n");
    let before = disk(&v, "notes/a.md");
    let err = server
        .lapis_edit(rmcp::handler::server::wrapper::Parameters(crate::mcp::EditArg {
            path: "notes/a.md".into(),
            mode: "insert_after".into(),
            old: "hello".into(),
            new: "!".into(),
            dry_run: Some(true),
            if_mtime: None,
            if_hash: None,
        }))
        .await
        .unwrap_err();
    assert!(err.message.contains('2'), "{}", err.message);
    assert_eq!(err.data.as_ref().and_then(|d| d["matches"].as_u64()), Some(2));
    assert_eq!(disk(&v, "notes/a.md"), before);
    let _ = std::fs::remove_dir_all(&v);
}

#[test]
fn cli_parses_replace_and_insert_guards() {
    use crate::cli::{Cli, Command};
    use clap::Parser;
    let Command::Replace(a) = Cli::try_parse_from([
        "lapis",
        "replace",
        "notes/a.md",
        "--old",
        "aa",
        "--new",
        "b",
        "--dry-run",
        "--if-hash",
        "fnv1a64:0",
        "--if-mtime",
        "7",
    ])
    .unwrap()
    .command() else {
        panic!("replace");
    };
    let g = a.guard.guard();
    assert!(g.dry_run && g.if_mtime == Some(7) && g.if_hash.as_deref() == Some("fnv1a64:0"));
    assert_eq!((a.old.as_str(), a.new.as_deref()), ("aa", Some("b")));
    let Command::Insert(i) =
        Cli::try_parse_from(["lapis", "insert", "notes/a.md", "--before", "AA", "--stdin", "--dry-run"])
            .unwrap()
            .command()
    else {
        panic!("insert");
    };
    assert!(i.before.as_deref() == Some("AA") && i.after.is_none() && i.stdin && i.guard.dry_run);
    assert!(
        Cli::try_parse_from(["lapis", "insert", "a.md", "--after", "A", "--before", "B", "--text", "t"])
            .is_err()
    );
}
