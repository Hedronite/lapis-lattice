//! Run `lapis.nu` against the built binary. Nushell is not installed on CI,
//! so the test downloads a pinned build into `target/` (gitignored).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NU_VERSION: &str = "0.106.1";

fn nu_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        (os, arch) => panic!("no pinned nushell binary for {os}-{arch}"),
    }
}

fn nu_bin() -> &'static Path {
    static NU: OnceLock<PathBuf> = OnceLock::new();
    NU.get_or_init(ensure_nu)
}

fn nu_ok(bin: &Path) -> bool {
    bin.is_file()
        && Command::new(bin)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
}

/// nextest runs each test in its own process, and the ci profile kills a
/// silent test after 20s. One process downloads; the others wait on the
/// directory lock instead of writing the same tarball.
fn ensure_nu() -> PathBuf {
    let name = format!("nu-{NU_VERSION}-{}", nu_triple());
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/skill-nu").join(&name);
    let cached = dir.join("nu");
    if let Some(found) = ready_nu(&cached, &name) {
        return found;
    }
    fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("{}: {err}", dir.display()));
    let lock = dir.join(".download-lock");
    let started = Instant::now();
    loop {
        if let Some(found) = ready_nu(&cached, &name) {
            return found;
        }
        match fs::create_dir(&lock) {
            Ok(()) => {
                let bin = download_nu(&dir, &name);
                let _ = fs::remove_dir(&lock);
                return bin;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if started.elapsed() > Duration::from_secs(90) {
                    let _ = fs::remove_dir(&lock);
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(err) => panic!("{}: {err}", lock.display()),
        }
    }
}

fn ready_nu(cached: &Path, name: &str) -> Option<PathBuf> {
    if nu_ok(cached) {
        return Some(cached.to_path_buf());
    }
    let unpacked = PathBuf::from(format!("/tmp/{name}/nu"));
    nu_ok(&unpacked).then_some(unpacked)
}

fn download_nu(dir: &Path, name: &str) -> PathBuf {
    let bin = dir.join("nu");
    if nu_ok(&bin) {
        return bin;
    }
    let url = format!("https://github.com/nushell/nushell/releases/download/{NU_VERSION}/{name}.tar.gz");
    let tar_path = dir.join(format!("nu-{}.tar.gz", std::process::id()));
    let curl = Command::new("curl")
        .args(["--retry", "3", "--retry-delay", "2", "-fsSL", "-o"])
        .arg(&tar_path)
        .arg(&url)
        .status()
        .unwrap_or_else(|err| panic!("curl {url}: {err}"));
    assert!(curl.success(), "download {url} failed");
    let extract = dir.join(format!("extract-{}", std::process::id()));
    fs::create_dir_all(&extract).unwrap_or_else(|err| panic!("{}: {err}", extract.display()));
    let tar = Command::new("tar")
        .args(["-xzf"])
        .arg(&tar_path)
        .arg("-C")
        .arg(&extract)
        .arg("--strip-components=1")
        .status()
        .unwrap_or_else(|err| panic!("tar: {err}"));
    assert!(tar.success(), "extract {url} failed");
    let staged = extract.join("nu");
    assert!(nu_ok(&staged), "extracted nu at {} does not run", staged.display());
    fs::rename(&staged, &bin).unwrap_or_else(|err| panic!("rename {}: {err}", bin.display()));
    let _ = fs::remove_dir_all(&extract);
    let _ = fs::remove_file(&tar_path);
    bin
}

fn scratch() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("lapis-skill-nu-{}-{nanos}", std::process::id()));
    fs::create_dir_all(dir.join("config")).unwrap();
    fs::create_dir_all(dir.join("vault")).unwrap();
    dir
}

fn lapis() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lapis"))
}

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".agents/skills/lapis/lapis.nu")
}

fn path_with(dir: &Path) -> std::ffi::OsString {
    let mut path = std::ffi::OsString::from(dir);
    if let Some(rest) = std::env::var_os("PATH") {
        path.push(":");
        path.push(rest);
    }
    path
}

fn apply_isolation(cmd: &mut Command, home: &Path, path: &std::ffi::OsString) {
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("PATH", path)
        .env_remove("LAPIS_VAULT")
        .env_remove("LAPIS_LATTICE_URL");
}

fn run_nu(home: &Path, path: &std::ffi::OsString, args: &[&str]) -> Output {
    let mut cmd = Command::new(nu_bin());
    cmd.arg(script()).args(args);
    apply_isolation(&mut cmd, home, path);
    cmd.output().unwrap_or_else(|err| panic!("nu: {err}"))
}

fn init_vault(home: &Path, vault: &Path) {
    let mut cmd = Command::new(lapis());
    cmd.args(["--json", "init"]).arg(vault);
    apply_isolation(&mut cmd, home, &path_with(lapis().parent().unwrap()));
    let out = cmd.output().unwrap_or_else(|err| panic!("init: {err}"));
    assert!(out.status.success(), "init failed\n{}", String::from_utf8_lossy(&out.stderr));
}

struct Env {
    ok: bool,
    exit: Option<i64>,
    kind: String,
    command: String,
    path: String,
    score: String,
    snippet: String,
    has_score: bool,
    has_snippet: bool,
}

fn parse_env(stdout: &[u8]) -> Env {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import json, sys
doc = json.loads(sys.stdin.read())
def emit(k, v):
    print(f"{k}\t{v}")
emit("ok", "true" if doc.get("ok") is True else "false")
exit_code = doc.get("exit", None)
emit("exit", "" if exit_code is None else str(exit_code))
emit("kind", doc.get("kind") or "")
emit("command", doc.get("command") or "")
data = doc.get("data")
if isinstance(data, dict):
    emit("path", data.get("path") or "")
    text = data.get("text") or ""
    emit("text", text if isinstance(text, str) else "")
if isinstance(data, list) and data and isinstance(data[0], dict):
    row = data[0]
    emit("has_score", "true" if "score" in row else "false")
    emit("has_snippet", "true" if "snippet" in row else "false")
    emit("score", "" if row.get("score") is None else str(row.get("score")))
    snip = row.get("snippet")
    emit("snippet", "" if snip is None else str(snip))
"#,
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child.stdin.take().unwrap().write_all(stdout).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stdout is not one JSON value\n{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let mut fields = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some((k, v)) = line.split_once('\t') {
            fields.insert(k.to_string(), v.to_string());
        }
    }
    Env {
        ok: fields.get("ok").map(String::as_str) == Some("true"),
        exit: fields.get("exit").filter(|s| !s.is_empty()).and_then(|s| s.parse().ok()),
        kind: fields.get("kind").cloned().unwrap_or_default(),
        command: fields.get("command").cloned().unwrap_or_default(),
        path: fields.get("path").cloned().unwrap_or_default(),
        score: fields.get("score").cloned().unwrap_or_default(),
        snippet: fields.get("snippet").cloned().unwrap_or_default(),
        has_score: fields.get("has_score").map(String::as_str) == Some("true"),
        has_snippet: fields.get("has_snippet").map(String::as_str) == Some("true"),
    }
}

fn show(out: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

#[test]
fn skill_append_keeps_a_leading_dash_task() {
    let home = scratch();
    let vault = home.join("vault");
    init_vault(&home, &vault);
    let path_env = path_with(lapis().parent().unwrap());
    let vault_s = vault.to_str().unwrap();

    let created =
        run_nu(&home, &path_env, &["create", "--title", "Beta", "--body", "hello", "--vault", vault_s]);
    assert!(created.status.success(), "{}", show(&created));
    let created_env = parse_env(&created.stdout);
    assert!(created_env.ok, "{}", show(&created));
    assert!(!created_env.path.is_empty(), "{}", show(&created));

    let appended = run_nu(&home, &path_env, &["append", &created_env.path, "- [ ] x", "--vault", vault_s]);
    assert!(appended.status.success(), "{}", show(&appended));
    let appended_env = parse_env(&appended.stdout);
    assert!(appended_env.ok, "{}", show(&appended));
    let note = fs::read_to_string(vault.join(&created_env.path)).unwrap();
    assert!(note.contains("- [ ] x"), "note missing the task line:\n{note}");

    let dashed =
        run_nu(&home, &path_env, &["create", "--title", "Dash", "--body", "- [ ] x", "--vault", vault_s]);
    assert!(dashed.status.success(), "{}", show(&dashed));
    let dashed_env = parse_env(&dashed.stdout);
    assert!(dashed_env.ok, "{}", show(&dashed));
    let dash_note = fs::read_to_string(vault.join(&dashed_env.path)).unwrap();
    assert!(dash_note.contains("- [ ] x"), "create --body dropped the task line:\n{dash_note}");

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn skill_exits_nonzero_for_not_found_and_sandbox() {
    let home = scratch();
    let vault = home.join("vault");
    init_vault(&home, &vault);
    let path_env = path_with(lapis().parent().unwrap());
    let vault_s = vault.to_str().unwrap();

    let missing = run_nu(&home, &path_env, &["read", "nope.md", "--vault", vault_s]);
    assert_eq!(missing.status.code(), Some(3), "{}", show(&missing));
    let missing_env = parse_env(&missing.stdout);
    assert!(!missing_env.ok, "{}", show(&missing));
    assert_eq!(missing_env.exit, Some(3), "{}", show(&missing));
    assert_eq!(missing_env.command, "read");

    let escaped = run_nu(&home, &path_env, &["read", "../outside.md", "--vault", vault_s]);
    assert_eq!(escaped.status.code(), Some(3), "{}", show(&escaped));
    let escaped_env = parse_env(&escaped.stdout);
    assert!(!escaped_env.ok, "{}", show(&escaped));
    assert_eq!(escaped_env.exit, Some(3), "{}", show(&escaped));
    assert_eq!(escaped_env.command, "read");

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn skill_usage_and_missing_lapis_are_envelopes() {
    let home = scratch();
    let vault = home.join("vault");
    init_vault(&home, &vault);
    let path_env = path_with(lapis().parent().unwrap());
    let vault_s = vault.to_str().unwrap();

    let bare = run_nu(&home, &path_env, &["read", "--vault", vault_s]);
    assert_eq!(bare.status.code(), Some(1), "{}", show(&bare));
    let bare_env = parse_env(&bare.stdout);
    assert!(!bare_env.ok, "{}", show(&bare));
    assert_eq!(bare_env.kind, "usage");
    assert_eq!(bare_env.exit, Some(1));
    assert_eq!(bare_env.command, "read");

    let bad_limit = run_nu(&home, &path_env, &["search", "--limit", "notanint", "hello", "--vault", vault_s]);
    assert!(!bad_limit.status.success(), "{}", show(&bad_limit));
    let bad_env = parse_env(&bad_limit.stdout);
    assert!(!bad_env.ok, "{}", show(&bad_limit));
    assert!(bad_env.exit.is_some_and(|code| code != 0), "{}", show(&bad_limit));
    let stdout = String::from_utf8_lossy(&bad_limit.stdout);
    assert!(!stdout.contains("can't convert"), "{stdout}");
    assert!(!stdout.contains("Parse error"), "{stdout}");

    let empty = home.join("empty-path");
    fs::create_dir_all(&empty).unwrap();
    let mut gone_cmd = Command::new(nu_bin());
    gone_cmd.arg(script()).args(["read", "x.md", "--vault", vault_s]);
    apply_isolation(&mut gone_cmd, &home, &std::ffi::OsString::from(&empty));
    let gone = gone_cmd.output().unwrap_or_else(|err| panic!("nu: {err}"));
    assert_eq!(gone.status.code(), Some(127), "{}", show(&gone));
    let gone_env = parse_env(&gone.stdout);
    assert!(!gone_env.ok, "{}", show(&gone));
    assert_eq!(gone_env.kind, "missing_lapis");
    assert_eq!(gone_env.exit, Some(127));
    assert_eq!(gone_env.command, "read");

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn skill_search_passes_score_and_snippet() {
    let home = scratch();
    let vault = home.join("vault");
    init_vault(&home, &vault);
    let path_env = path_with(lapis().parent().unwrap());
    let vault_s = vault.to_str().unwrap();
    let token = "zebraskilltoken";

    let created =
        run_nu(&home, &path_env, &["create", "--title", "Zebra", "--body", token, "--vault", vault_s]);
    assert!(created.status.success(), "{}", show(&created));

    let found = run_nu(&home, &path_env, &["search", token, "--vault", vault_s]);
    assert!(found.status.success(), "{}", show(&found));
    let env = parse_env(&found.stdout);
    assert!(env.ok, "{}", show(&found));
    assert!(env.has_score, "score was dropped\n{}", show(&found));
    assert!(env.has_snippet, "snippet was dropped\n{}", show(&found));
    assert!(!env.score.is_empty(), "{}", show(&found));
    assert!(env.snippet.contains(token), "snippet {:?}\n{}", env.snippet, show(&found));

    let _ = fs::remove_dir_all(&home);
}
