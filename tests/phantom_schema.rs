//! G5a-G5c, G5e, G5g: the envelope `schema` check, the `version` probe Lapis
//! already sends, the guard that stops a child on a panic, and `truncated`
//! read from `data`. Fixtures follow Phantom at `486e5f3`
//! (`crates/phantom-fractal-proto/src/lib.rs`, and `version.rs`, `main.rs`
//! and `calls.rs` in `crates/phantom-fractal/src`). Phantom runs as stub
//! scripts.

#![cfg(unix)]

#[allow(dead_code)]
#[path = "../src/phantom/mod.rs"]
mod phantom;

use phantom::discover;
use phantom::envelope::{self, PHANTOM_SCHEMA, ParseError, Schema};
use phantom::run::{self, RunError};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Writing a stub while another thread forks can leave it busy (ETXTBSY) on
/// Linux. Tests that write stubs or spawn hold this lock.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn temp_dir(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lapis-phantom-schema-{name}-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// A stub `phantom-fractal` with `body` after the shebang.
fn script(name: &str, body: &str) -> PathBuf {
    let bin = temp_dir(name).join(discover::BINARY);
    std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// A stub that prints `stdout` for any arguments and exits 0.
fn printing(name: &str, stdout: &str) -> PathBuf {
    assert!(!stdout.contains('\''), "fixture must not contain a single quote");
    script(name, &format!("printf '%s' '{stdout}'"))
}

fn parse(raw: &str) -> Result<envelope::Reply, ParseError> {
    envelope::parse(raw.as_bytes())
}

fn alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only asks whether the pid exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// True once `pid` is gone, polling for up to 2 s while init reaps it.
fn gone(pid: libc::pid_t) -> bool {
    (0..200).any(|_| {
        let up = alive(pid);
        if up {
            std::thread::sleep(Duration::from_millis(10));
        }
        !up
    })
}

/// `phantom-fractal --json version` at `486e5f3`, built with a commit. Fields
/// in `VersionInfo` order: `name`, `version`, `git_sha`, `extractor_version`,
/// `store_schema`.
const VERSION: &str = r#"{"ok":true,"data":{"name":"phantom-fractal","version":"0.0.0","git_sha":"486e5f3fa359a0abf1029c4df413aa581896cd7f","extractor_version":5,"store_schema":3},"error":null,"schema":1}"#;

/// The same line from a build that found no commit.
const VERSION_NO_SHA: &str = r#"{"ok":true,"data":{"name":"phantom-fractal","version":"0.0.0","git_sha":null,"extractor_version":5,"store_schema":3},"error":null,"schema":1}"#;

/// `VERSION` with its top-level `schema` replaced by `schema` (raw JSON), or
/// removed when `None`.
fn with_schema(schema: Option<&str>) -> String {
    let base = VERSION.strip_suffix(r#","schema":1}"#).unwrap();
    match schema {
        Some(s) => format!(r#"{base},"schema":{s}}}"#),
        None => format!("{base}}}"),
    }
}

// ------------------------------------------------------------------ G5a

/// `schema_field_is_what_a_reader_checks` and `ambiguous_failure_keeps_candidate_data`
/// at `486e5f3`: `schema: 1` parses, a success and a failure alike.
#[test]
fn g5a_schema_1_parses() {
    assert_eq!(PHANTOM_SCHEMA, 1);
    let reply = parse(r#"{"ok":true,"data":"row","error":null,"schema":1}"#).unwrap();
    assert_eq!(reply.envelope.data, Some(json!("row")));
    let raw = r#"{"ok":false,"data":["a","b"],"error":{"kind":"ambiguous","message":"two hits","exit":6},"schema":1}"#;
    let reply = parse(raw).unwrap();
    assert_eq!(reply.envelope.data, Some(json!(["a", "b"])));
    assert_eq!(reply.envelope.error.unwrap().exit, 6);
    // The same success without `schema` is refused: the field is what decides.
    let missing = r#"{"ok":true,"data":"row","error":null}"#;
    assert_eq!(parse(missing), Err(ParseError::Schema(Schema::Missing)));
}

/// The real `version` line parses, and unknown extra fields still parse equal
/// to it (G3b holds with `schema`).
#[test]
fn g5a_version_line_parses_and_extra_fields_still_parse_equal() {
    let reply = parse(VERSION).unwrap();
    assert!(reply.envelope.ok);
    let extra =
        VERSION.replacen(r#"{"ok":true,"#, r#"{"api_version":"1.0","meta":{"latency_ms":4},"ok":true,"#, 1);
    let extra = extra.replacen(r#""store_schema":3}"#, r#""store_schema":3,"later":[1]}"#, 1);
    let extra = parse(&extra).unwrap();
    assert_eq!(extra.envelope.ok, reply.envelope.ok);
    assert_eq!(extra.envelope.error, reply.envelope.error);
    assert_eq!(
        extra.envelope.data.as_ref().unwrap()["git_sha"],
        reply.envelope.data.as_ref().unwrap()["git_sha"]
    );
    assert_eq!(parse(&with_schema(Some("2"))), Err(ParseError::Schema(Schema::Number(2.into()))));
}

// ------------------------------------------------------------------ G5b

/// Missing, `0`, `2`, `-1`, `"1"` and `1.5` are each the typed schema error.
/// The message names only the received and the supported number.
#[test]
fn g5b_other_schemas_are_refused_with_a_typed_error() {
    let cases: [(Option<&str>, Schema, &str); 7] = [
        (None, Schema::Missing, "Phantom envelope has no schema; Lapis supports schema 1"),
        (Some("null"), Schema::Missing, "Phantom envelope has no schema; Lapis supports schema 1"),
        (
            Some("0"),
            Schema::Number(0.into()),
            "Phantom envelope schema 0 is not supported; Lapis supports schema 1",
        ),
        (
            Some("2"),
            Schema::Number(2.into()),
            "Phantom envelope schema 2 is not supported; Lapis supports schema 1",
        ),
        (
            Some("-1"),
            Schema::Number((-1).into()),
            "Phantom envelope schema -1 is not supported; Lapis supports schema 1",
        ),
        (
            Some(r#""1""#),
            Schema::NotANumber,
            "Phantom envelope schema is not a number; Lapis supports schema 1",
        ),
        (
            Some("1.5"),
            Schema::Number(serde_json::Number::from_f64(1.5).unwrap()),
            "Phantom envelope schema 1.5 is not supported; Lapis supports schema 1",
        ),
    ];
    for (schema, want, text) in cases {
        let raw = with_schema(schema);
        let err = parse(&raw).unwrap_err();
        assert_eq!(err, ParseError::Schema(want), "{raw}");
        let msg = err.to_string();
        assert_eq!(msg, text, "{raw}");
        for content in ["phantom-fractal\"", "486e5f3", "store_schema", "git_sha"] {
            assert!(!msg.contains(content), "{msg} repeats envelope content");
        }
    }
}

/// No `data` is read on a refusal: data that would fail every other check
/// (an out-of-range exit, a contradictory envelope, a huge array) still only
/// yields the schema error, and nothing panics.
#[test]
fn g5b_refusal_comes_before_data() {
    let rows: Vec<Value> = (0..1000).map(|i| json!(format!("/abs/{i}.db"))).collect();
    let raws = [
        json!({"ok": false, "data": null, "error": {"kind": "usage", "message": "m", "exit": 99}, "schema": 2}),
        json!({"ok": false, "data": null, "error": null, "schema": 2}),
        json!({"ok": "yes", "data": rows, "schema": 2}),
        json!({"data": {"path": "/abs/phantom.db"}}),
    ];
    for raw in raws {
        let err = parse(&raw.to_string()).unwrap_err();
        assert!(matches!(err, ParseError::Schema(_)), "{raw}: {err:?}");
        assert!(!err.to_string().contains("/abs/"), "{err}");
    }
}

/// `run` refuses a wrong schema as a typed error, whatever the exit status.
#[test]
fn g5b_run_refuses_a_wrong_schema() {
    let _g = serial();
    let bin = printing("run-schema-2", r#"{"ok":true,"data":null,"error":null,"schema":2}"#);
    let r = run::run(&bin, &["version"]);
    assert!(matches!(r, Err(RunError::Envelope(ParseError::Schema(Schema::Number(_))))), "{r:?}");
    let bin = script(
        "run-schema-2-exit-3",
        r#"printf '%s' '{"ok":false,"data":null,"error":{"kind":"not_found","message":"m","exit":3},"schema":2}'; exit 3"#,
    );
    let r = run::run(&bin, &["version"]);
    assert!(matches!(r, Err(RunError::Envelope(ParseError::Schema(_)))), "{r:?}");
}

fn lapis_check(binary: &Path) -> std::process::Output {
    let vault = temp_dir("check-vault");
    std::process::Command::new(env!("CARGO_BIN_EXE_lapis"))
        .args(["--vault"])
        .arg(&vault)
        .args(["desktop", "--check"])
        .env(discover::ENV, binary)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", temp_dir("check-config"))
        .env_remove("LAPIS_VAULT")
        .output()
        .unwrap()
}

/// `desktop --check` exits non-zero on a refusal, through the probe-error
/// path, and the line names only the numbers.
#[test]
fn g5b_desktop_check_exits_non_zero_on_a_wrong_schema() {
    let _g = serial();
    let bin = printing("check-schema-2", r#"{"ok":true,"data":null,"error":null,"schema":2}"#);
    let out = lapis_check(&bin);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stderr.contains("Phantom envelope schema 2 is not supported; Lapis supports schema 1"),
        "{stderr}"
    );
    assert!(!stdout.contains("phantom version: ok"), "{stdout}");
}

// ------------------------------------------------------------------ G5c

/// The probe is `--json version` (unchanged). The real line parses, and `data`
/// has exactly the five `VersionInfo` keys.
#[test]
fn g5c_probe_reads_the_real_version_line() {
    let _g = serial();
    let dir = temp_dir("probe");
    let log = dir.join("calls");
    let bin = script(
        "probe-bin",
        &format!("printf '%s\\n' \"$*\" >> '{}'\nprintf '%s' '{VERSION}'", log.display()),
    );
    let reply = run::run(&bin, &["version"]).unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "--json version\n");
    let data = reply.envelope.data.unwrap();
    let keys: Vec<&str> = data.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["name", "version", "git_sha", "extractor_version", "store_schema"]);
    assert_eq!(data["git_sha"], "486e5f3fa359a0abf1029c4df413aa581896cd7f");
    assert!(reply.envelope.error.is_none());
    assert!(parse(&with_schema(None)).is_err(), "the same line without schema is refused");
}

/// `git_sha: null` means unknown and is accepted.
#[test]
fn g5c_null_git_sha_is_accepted() {
    let reply = parse(VERSION_NO_SHA).unwrap();
    assert!(reply.envelope.ok);
    assert_eq!(reply.envelope.data.unwrap()["git_sha"], Value::Null);
    let refused = VERSION_NO_SHA.replacen(r#""schema":1"#, r#""schema":0"#, 1);
    assert_eq!(parse(&refused), Err(ParseError::Schema(Schema::Number(0.into()))));
}

/// Lapis gates on `schema` alone: another `store_schema` or
/// `extractor_version` is accepted, while another `schema` with the same
/// `data` is refused.
#[test]
fn g5c_only_schema_gates() {
    for (extractor, store) in [(5, 4), (6, 3), (0, 0), (99, 99)] {
        let raw = VERSION
            .replacen(r#""extractor_version":5"#, &format!(r#""extractor_version":{extractor}"#), 1)
            .replacen(r#""store_schema":3"#, &format!(r#""store_schema":{store}"#), 1);
        let data = parse(&raw).unwrap().envelope.data.unwrap();
        assert_eq!(
            (data["extractor_version"].as_i64(), data["store_schema"].as_i64()),
            (Some(extractor), Some(store))
        );
        let bumped = raw.replacen(r#""schema":1"#, r#""schema":2"#, 1);
        assert!(matches!(parse(&bumped), Err(ParseError::Schema(_))), "{bumped}");
    }
}

/// A binary that rejects a flag it does not know, the way a CLI parser does
/// (usage on stderr, exit 2), yields a typed error and no panic.
#[test]
fn g5c_a_binary_that_rejects_unknown_flags_is_a_typed_error() {
    let _g = serial();
    let bin = script(
        "strict",
        "for a in \"$@\"; do case \"$a\" in -*) echo \"error: unexpected argument '$a' found\" >&2; exit 2;; esac; done\n\
         printf '%s' '{\"ok\":true,\"data\":null,\"error\":null,\"schema\":1}'",
    );
    let r = std::panic::catch_unwind(|| run::run(&bin, &["version"])).expect("no panic");
    match r {
        Err(e @ RunError::Failed { status: 2, .. }) => {
            assert_eq!(
                e.to_string(),
                "phantom-fractal exited 2 without an envelope: error: unexpected argument '--json' found"
            );
        }
        other => panic!("expected a typed error, got {other:?}"),
    }
}

// ------------------------------------------------------------------ G5e

/// A panic between spawn and the stop paths (through the test-only hook)
/// still stops the child and its group: the guard's `Drop` runs `stop()`.
#[test]
fn g5e_a_panic_after_spawn_stops_the_child_and_its_group() {
    let _g = serial();
    let dir = temp_dir("panic");
    let pids = dir.join("pids");
    let bin = script(
        "panic-bin",
        &format!("sleep 30 &\necho \"$$ $!\" > '{0}.tmp'\nmv '{0}.tmp' '{0}'\nwait", pids.display()),
    );
    let seen = pids.clone();
    run::AFTER_SPAWN.set(Some(Box::new(move || {
        let t0 = Instant::now();
        while !seen.exists() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("test hook: panic after spawn");
    })));
    let t0 = Instant::now();
    let r = std::panic::catch_unwind(|| run::run_within(&bin, &["version"], Duration::from_secs(20)));
    run::AFTER_SPAWN.set(None);
    assert!(r.is_err(), "the hook panics");
    assert!(t0.elapsed() < Duration::from_secs(15), "stopped by the guard, not the time limit");
    let text = std::fs::read_to_string(&pids).unwrap();
    let mut it = text.split_whitespace().map(|p| p.parse::<libc::pid_t>().unwrap());
    let (child, grandchild) = (it.next().unwrap(), it.next().unwrap());
    assert!(gone(child), "child {child} outlived the panic");
    assert!(gone(grandchild), "grandchild {grandchild} outlived the panic");
}

// ------------------------------------------------------------------ G5g

/// `data` of a `find` at `486e5f3` (`FindResult`): `hits`, then `truncated`.
fn find_reply(truncated: Option<bool>, meta: Option<Value>) -> String {
    let hit = json!({"name": "Envelope", "kind": "struct", "visibility": "pub",
        "path": "crates/phantom-fractal-proto/src/lib.rs", "line": 128});
    let mut data = json!({"hits": [hit]});
    if let Some(t) = truncated {
        data["truncated"] = json!(t);
    }
    let mut raw = json!({"ok": true, "data": data, "error": null, "schema": 1});
    if let Some(m) = meta {
        raw["meta"] = m;
    }
    raw.to_string()
}

/// A real-shaped list reply with `data.truncated: true` and no `meta` is
/// truncated: `find`, and `callers` (`CallResult`, `calls.rs:149-154`).
#[test]
fn g5g_data_truncated_true_without_meta_is_truncated() {
    assert!(parse(&find_reply(Some(true), None)).unwrap().truncated);
    let callers = json!({"ok": true, "data": {
        "symbol": {"name": "parse", "kind": "fn", "path": "src/lib.rs", "line": 12},
        "hits": [{"name": "main", "kind": "fn", "path": "src/main.rs", "line": 3,
            "symbol_path": "src/main.rs", "confidence": 1.0, "reason": "resolved"}],
        "truncated": true}, "error": null, "schema": 1});
    assert!(parse(&callers.to_string()).unwrap().truncated);
}

/// `data.truncated: false`, a missing field, or a non-boolean is not
/// truncated, and Phantom's `meta` is not read even when it says so.
#[test]
fn g5g_data_truncated_false_or_absent_is_not_truncated() {
    let meta = json!({"truncated": true, "next": "cursor-2"});
    for raw in [
        find_reply(Some(false), None),
        find_reply(None, None),
        find_reply(Some(false), Some(meta.clone())),
        find_reply(None, Some(meta)),
        find_reply(None, None).replacen(r#""hits":"#, r#""truncated":"yes","hits":"#, 1),
    ] {
        assert!(!parse(&raw).unwrap().truncated, "{raw}");
    }
}

/// Lapis's own `DATA_CAP` cut still marks a reply truncated, and a full but
/// uncut array is not truncated by `meta`.
#[test]
fn g5g_data_cap_cut_still_marks_truncated() {
    let rows = |n: usize| -> Vec<Value> { (0..n).map(|i| json!(format!("src/{i}.rs"))).collect() };
    let cut = json!({"ok": true, "data": rows(envelope::DATA_CAP + 1), "error": null, "schema": 1});
    let reply = parse(&cut.to_string()).unwrap();
    assert!(reply.truncated);
    assert_eq!(reply.envelope.data.unwrap().as_array().unwrap().len(), envelope::DATA_CAP);
    let full = json!({"ok": true, "data": rows(envelope::DATA_CAP), "error": null, "schema": 1,
        "meta": {"truncated": true, "next": "cursor-2"}});
    assert!(!parse(&full.to_string()).unwrap().truncated);
}
