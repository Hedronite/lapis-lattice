//! G0c, G3a-G3c, T1-T2: the re-check before spawn, the inbound envelope, exits,
//! and what Lapis refuses to do with Phantom's JSON. Envelope fixtures follow
//! Phantom SPEC §9 and Phantom's G1 proto at `2063a55`
//! (`crates/phantom-fractal-proto/src/lib.rs`). Phantom runs as stub scripts.

#![cfg(unix)]

#[allow(dead_code)]
#[path = "../src/phantom/mod.rs"]
mod phantom;

use phantom::discover;
use phantom::envelope::{self, DATA_CAP, Envelope, EnvelopeError, ParseError};
use phantom::run::{self, RunError};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Writing a stub while another thread forks can leave it busy (ETXTBSY) on
/// Linux. Tests that write stubs or spawn hold this lock.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn temp_dir(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lapis-phantom-env-{name}-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// A stub `name` in `dir` that logs its argv to `calls`, prints `stdout`, and
/// exits `code`. Shell builtins only, so it runs with any `PATH`.
fn stub(dir: &Path, name: &str, stdout: &str, code: i32) -> (PathBuf, PathBuf) {
    assert!(!stdout.contains('\''), "fixture must not contain a single quote");
    let p = dir.join(name);
    let calls = dir.join(format!("{name}.calls"));
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nprintf '%s' '{stdout}'\nexit {code}\n",
        calls.display()
    );
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    (p, calls)
}

fn calls(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).map(|s| s.lines().map(str::to_owned).collect()).unwrap_or_default()
}

fn phantom_stub(stdout: &str, code: i32) -> (PathBuf, PathBuf) {
    stub(&temp_dir("stub"), discover::BINARY, stdout, code)
}

const SUCCESS: &str = r#"{"ok":true,"data":{"version":"0.0.1","schema":1},"error":null}"#;
/// `Envelope::<String>::failure(ErrorKind::NotFound, "missing")` at `2063a55`.
const NOT_FOUND: &str =
    r#"{"ok":false,"data":null,"error":{"kind":"not_found","message":"missing","exit":3}}"#;

fn parse(raw: &str) -> Result<envelope::Reply, ParseError> {
    envelope::parse(raw.as_bytes())
}

// ------------------------------------------------------------------ G3a

#[test]
fn g3a_success_envelope_parses() {
    let reply = parse(SUCCESS).unwrap();
    assert_eq!(
        reply.envelope,
        Envelope { ok: true, data: Some(json!({"version": "0.0.1", "schema": 1})), error: None, meta: None }
    );
    assert!(!reply.truncated);
}

#[test]
fn g3a_error_envelope_parses() {
    let reply = parse(NOT_FOUND).unwrap();
    assert_eq!(
        reply.envelope,
        Envelope {
            ok: false,
            data: None,
            error: Some(EnvelopeError { kind: "not_found".into(), message: "missing".into(), exit: 3 }),
            meta: None,
        }
    );
}

#[test]
fn g3a_section_9_meta_parses() {
    let raw = r#"{"ok":true,"data":[],"error":null,"meta":{"latency_ms":4,"truncated":true,"next":"c2","count":0,
        "limit":50,"offset":0,"stale":false,"index_head":"abc","worktree_dirty":false,"rank_epoch":1,"used_tokens":9}}"#;
    let reply = parse(raw).unwrap();
    assert!(reply.envelope.meta.as_ref().unwrap().truncated);
    assert!(reply.truncated, "Phantom's own meta.truncated is kept");
}

// ------------------------------------------------------------------ G3b

/// The raw JSON of `unknown_json_fields_are_ignored` at `2063a55`.
#[test]
fn g3b_unknown_fields_parse_equal_to_the_stripped_envelope() {
    let raw = r#"{
        "ok": false,
        "data": null,
        "error": { "kind": "ambiguous", "message": "two hits", "exit": 6, "hint": "pick one" },
        "meta": { "latency_ms": 4 },
        "trace": []
    }"#;
    let stripped = r#"{"ok":false,"data":null,"error":{"kind":"ambiguous","message":"two hits","exit":6},"meta":{"latency_ms":4}}"#;
    assert_eq!(parse(raw).unwrap(), parse(stripped).unwrap());

    let raw =
        r#"{"api_version":"1.0","ok":true,"data":{"version":"0.0.1","schema":1},"error":null,"x":{"y":[1]}}"#;
    assert_eq!(parse(raw).unwrap(), parse(SUCCESS).unwrap());
}

// ------------------------------------------------------------------ G3c

#[test]
fn g3c_exits_zero_through_six_are_kept() {
    let kinds = ["ok", "usage", "no_index", "not_found", "lock_held", "schema_newer", "ambiguous"];
    for (exit, kind) in kinds.iter().enumerate() {
        let raw = json!({"ok": false, "data": null, "error": {"kind": kind, "message": "m", "exit": exit}});
        let reply = parse(&raw.to_string()).unwrap();
        let e = reply.envelope.error.unwrap();
        assert_eq!((e.exit, e.kind.as_str()), (exit as i64, *kind));
    }
}

#[test]
fn g3c_exit_outside_zero_through_six_is_a_typed_error() {
    for exit in [-1i64, 7, 42, 255] {
        let raw =
            json!({"ok": false, "data": null, "error": {"kind": "usage", "message": "m", "exit": exit}});
        assert_eq!(parse(&raw.to_string()), Err(ParseError::UnknownExit(exit)));
    }
}

#[test]
fn g3c_malformed_envelopes_are_typed_errors() {
    for raw in ["", "not json", "[]", r#"{"data":1}"#, r#"{"ok":"yes"}"#] {
        assert!(matches!(parse(raw), Err(ParseError::NotEnvelope(_))), "{raw:?}");
    }
    assert_eq!(parse(r#"{"ok":false,"data":null,"error":null}"#), Err(ParseError::MissingError));
}

#[test]
fn g3c_run_keeps_a_non_zero_status_that_carries_the_envelope() {
    let _g = serial();
    let (bin, log) = phantom_stub(NOT_FOUND, 3);
    let reply = run::run(&bin, &["version"]).unwrap();
    assert_eq!(reply.envelope.error.unwrap().exit, 3);
    assert_eq!(calls(&log), ["--json version"]);
}

#[test]
fn g3c_run_status_outside_zero_through_six_is_a_typed_error() {
    let _g = serial();
    let (bin, _) = phantom_stub(NOT_FOUND, 9);
    assert!(matches!(run::run(&bin, &["version"]), Err(RunError::UnknownStatus { status: Some(9), .. })));
}

#[test]
fn g3c_run_non_zero_status_without_an_envelope_is_a_typed_error() {
    let _g = serial();
    let (bin, _) = phantom_stub("thread main panicked", 1);
    assert!(matches!(run::run(&bin, &["version"]), Err(RunError::Failed { status: 1, .. })));
}

#[test]
fn g3c_run_zero_status_with_bad_stdout_is_a_typed_error() {
    let _g = serial();
    let (bin, _) = phantom_stub("hello", 0);
    assert!(matches!(run::run(&bin, &["version"]), Err(RunError::Envelope(ParseError::NotEnvelope(_)))));
}

#[test]
fn g3c_run_killed_by_a_signal_is_a_typed_error() {
    let _g = serial();
    let dir = temp_dir("signal");
    let bin = dir.join(discover::BINARY);
    std::fs::write(&bin, "#!/bin/sh\nkill -9 $$\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(run::run(&bin, &["version"]), Err(RunError::UnknownStatus { status: None, .. })));
}

// ------------------------------------------------------------------ G0c

#[test]
fn g0c_recheck_passes_and_run_spawns_once() {
    let _g = serial();
    let (bin, log) = phantom_stub(SUCCESS, 0);
    assert!(run::recheck(&bin).is_ok());
    assert!(run::run(&bin, &["version"]).unwrap().envelope.ok);
    assert_eq!(calls(&log), ["--json version"]);
}

#[test]
fn g0c_failed_recheck_does_not_spawn() {
    let _g = serial();
    let reason = |r: Result<envelope::Reply, RunError>| match r {
        Err(RunError::Recheck { reason, .. }) => reason,
        other => panic!("expected a re-check error, got {other:?}"),
    };

    // Removed after discovery.
    let (bin, log) = phantom_stub(SUCCESS, 0);
    let found = discover::discover(Some(bin.parent().unwrap().as_os_str()), None).unwrap();
    std::fs::remove_file(&bin).unwrap();
    assert_eq!(reason(run::run(&found, &["version"])), "no such file");

    // Made non-executable after discovery.
    let (bin, log2) = phantom_stub(SUCCESS, 0);
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(reason(run::run(&bin, &["version"])), "not executable");

    // Replaced by a directory.
    let (bin, log3) = phantom_stub(SUCCESS, 0);
    std::fs::remove_file(&bin).unwrap();
    std::fs::create_dir(&bin).unwrap();
    assert_eq!(reason(run::run(&bin, &["version"])), "not a file");

    // Relative.
    let rel = PathBuf::from("bin").join(discover::BINARY);
    assert_eq!(reason(run::run(&rel, &["version"])), "not an absolute path");

    // Another name, `fract` included: never run.
    let dir = temp_dir("recheck-fract");
    let (fract, fract_log) = stub(&dir, "fract", SUCCESS, 0);
    assert_eq!(reason(run::run(&fract, &["version"])), "file name is not phantom-fractal");

    for log in [log, log2, log3, fract_log] {
        assert!(calls(&log).is_empty(), "{} was spawned", log.display());
    }
}

// ------------------------------------------------------- bounded child

/// A stub `phantom-fractal` with `body` after the shebang.
fn script(name: &str, body: &str) -> PathBuf {
    let bin = temp_dir(name).join(discover::BINARY);
    std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
fn a_hung_binary_is_stopped_at_the_time_limit() {
    let _g = serial();
    let bin = script("hang", "exec sleep 30");
    let t0 = std::time::Instant::now();
    let r = run::run_within(&bin, &["version"], std::time::Duration::from_millis(300));
    assert!(matches!(r, Err(RunError::Timeout { .. })), "{r:?}");
    assert!(t0.elapsed() < std::time::Duration::from_secs(5), "stopped near the limit, not at sleep's end");
}

#[test]
fn endless_output_is_cut_at_the_cap() {
    let _g = serial();
    let bin = script("flood", "while :; do printf '%s' 0123456789abcdef0123456789abcdef; done");
    let r = run::run_within(&bin, &["version"], std::time::Duration::from_secs(20));
    assert!(matches!(r, Err(RunError::Overflow { stream: "stdout", cap: run::STDOUT_CAP })), "{r:?}");
    let bin = script("flood-err", "while :; do printf '%s' 0123456789abcdef0123456789abcdef >&2; done");
    let r = run::run_within(&bin, &["version"], std::time::Duration::from_secs(20));
    assert!(matches!(r, Err(RunError::Overflow { stream: "stderr", cap: run::STDERR_CAP })), "{r:?}");
}

#[test]
fn the_child_sees_only_allowlisted_environment() {
    let _g = serial();
    let dir = temp_dir("env");
    let seen = dir.join("seen");
    let bin = script(
        "env-stub",
        &format!(
            "printf '%s|%s|%s' \"$LAPIS_TEST_SECRET\" \"$PHANTOM_TEST_SETTING\" \"$PATH\" > '{}'\nprintf '%s' '{SUCCESS}'",
            seen.display()
        ),
    );
    // SAFETY: the test holds `serial()`, so no other test thread reads the
    // environment while it changes.
    unsafe {
        std::env::set_var("LAPIS_TEST_SECRET", "token-should-not-pass");
        std::env::set_var("PHANTOM_TEST_SETTING", "kept");
    }
    let r = run::run(&bin, &["version"]);
    unsafe {
        std::env::remove_var("LAPIS_TEST_SECRET");
        std::env::remove_var("PHANTOM_TEST_SETTING");
    }
    assert!(r.unwrap().envelope.ok);
    let seen = std::fs::read_to_string(seen).unwrap();
    let parts: Vec<&str> = seen.split('|').collect();
    assert_eq!(parts[0], "", "a secret in Lapis's environment does not reach the child");
    assert_eq!(parts[1], "kept", "PHANTOM_* settings do");
    assert!(!parts[2].is_empty(), "and so does PATH");
}

// ------------------------------------------------------------------ T1

/// An absolute path in the JSON is a FIFO with no writer: opening it to read
/// would block, so finishing at all shows nothing opened it.
#[test]
fn t1_absolute_path_in_json_is_not_opened() {
    let _g = serial();
    let dir = temp_dir("t1");
    let fifo = dir.join("phantom.db");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let db = fifo.display().to_string();
    let raw = json!({"ok": true, "data": {"db": db, "rows": [db, {"path": db}]}, "error": null,
        "meta": {"index_head": db}})
    .to_string();
    let (bin, _) = phantom_stub(&raw, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(run::run(&bin, &["version"]).map(|r| r.envelope.data)).unwrap());
    let data =
        rx.recv_timeout(std::time::Duration::from_secs(20)).expect("run blocked: a JSON path was opened");
    assert_eq!(data.unwrap().unwrap()["db"], Value::String(db));
}

/// No file or database open anywhere in the Phantom client.
#[test]
fn t1_client_source_opens_nothing() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/phantom");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let src = std::fs::read_to_string(&path).unwrap();
        for banned in [
            "File::open",
            "OpenOptions",
            "Connection::open",
            "rusqlite",
            "fs::read(",
            "read_to_string",
            "read_dir",
        ] {
            assert!(!src.contains(banned), "{} contains {banned}", path.display());
        }
    }
}

// ------------------------------------------------------------------ T2

#[test]
fn t2_data_array_is_capped_at_256_with_one_spawn() {
    let _g = serial();
    let rows: Vec<Value> = (0..=DATA_CAP).map(|i| json!({"id": i})).collect();
    assert_eq!(rows.len(), 257);
    let raw =
        json!({"ok": true, "data": rows, "error": null, "meta": {"next": "cursor-2", "truncated": false}});
    let (bin, log) = phantom_stub(&raw.to_string(), 0);
    let reply = run::run(&bin, &["find", "x"]).unwrap();
    let kept = reply.envelope.data.as_ref().unwrap().as_array().unwrap();
    assert_eq!(kept.len(), 256);
    assert_eq!(kept.first(), Some(&json!({"id": 0})));
    assert_eq!(kept.last(), Some(&json!({"id": 255})));
    assert!(reply.truncated);
    assert_eq!(calls(&log), ["--json find x"], "meta.next is not followed");
}

#[test]
fn t2_exactly_256_is_not_truncated() {
    let rows: Vec<Value> = (0..DATA_CAP).map(|i| json!(i)).collect();
    let reply = parse(&json!({"ok": true, "data": rows, "error": null}).to_string()).unwrap();
    assert_eq!(reply.envelope.data.unwrap().as_array().unwrap().len(), 256);
    assert!(!reply.truncated);
}

// ------------------------------------------------------------------ binary

fn lapis_check(path_var: &std::ffi::OsStr, cwd: &Path) -> std::process::Output {
    let vault = temp_dir("check-vault");
    let config = temp_dir("check-config");
    std::process::Command::new(env!("CARGO_BIN_EXE_lapis"))
        .args(["--vault"])
        .arg(&vault)
        .args(["desktop", "--check"])
        .current_dir(cwd)
        .env("PATH", path_var)
        .env("XDG_CONFIG_HOME", &config)
        .env_remove(discover::ENV)
        .env_remove("LAPIS_VAULT")
        .output()
        .unwrap()
}

/// G0a end to end: a relative `PATH` entry, an absolute path printed, one spawn.
#[test]
fn desktop_check_runs_phantom_once_from_a_relative_path_entry() {
    let _g = serial();
    let cwd = temp_dir("check-rel");
    std::fs::create_dir(cwd.join("bin")).unwrap();
    let (_, log) = stub(&cwd.join("bin"), discover::BINARY, SUCCESS, 0);
    let out = lapis_check("bin".as_ref(), &cwd);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let abs = cwd.join("bin").join(discover::BINARY);
    assert!(stdout.lines().any(|l| l == format!("phantom: {}", abs.display())), "{stdout}");
    assert!(stdout.lines().any(|l| l == "phantom version: ok"), "{stdout}");
    assert_eq!(calls(&log), ["--json version"]);
}

/// G3c end to end: a broken Phantom is an error, not the empty overlay line.
#[test]
fn desktop_check_reports_a_typed_error_for_a_broken_phantom() {
    let _g = serial();
    let dir = temp_dir("check-broken");
    stub(&dir, discover::BINARY, "segfault", 139);
    let out = lapis_check(dir.as_os_str(), &dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(stderr.contains("phantom-fractal exited 139, outside 0-6"), "{stderr}");
    assert!(!stdout.contains("Phantom not found"), "{stdout}");
}

// ------------------------------------------------------------------ rows

/// The overlay's rows come from the capped reply, as text. A path stays a string.
#[test]
fn rows_are_text_from_the_capped_reply() {
    let rows: Vec<Value> = (0..=DATA_CAP).map(|i| json!(format!("/abs/src/{i}.rs"))).collect();
    let reply = parse(&json!({"ok": true, "data": rows, "error": null}).to_string()).unwrap();
    let text = reply.rows();
    assert_eq!(text.len(), 256);
    assert_eq!(text[0], "/abs/src/0.rs");
    assert!(reply.truncated);

    let reply = parse(r#"{"ok":true,"data":[{"id":1},2,null],"error":null}"#).unwrap();
    assert_eq!(reply.rows(), [r#"{"id":1}"#, "2", "null"]);

    assert_eq!(parse(SUCCESS).unwrap().rows(), [r#"{"version":"0.0.1","schema":1}"#]);
    assert_eq!(parse(NOT_FOUND).unwrap().rows(), ["exit 3 not_found: missing"]);
}
