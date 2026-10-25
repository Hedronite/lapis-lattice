//! Run `lapis.nu` against the built binary. Nushell is not installed on CI,
//! so the test downloads Nushell 0.108.0 into `target/` (gitignored) and
//! checks the tarball sha256 before it extracts or runs anything.
//! The tarball is verified and unpacked once per process. The binary hash
//! is checked again before every use. A binary that does not match the
//! verified tarball is extracted again from that tarball.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NU_VERSION: &str = "0.108.0";

struct NuPin {
    triple: &'static str,
    sha256: &'static str,
}

fn nu_pin() -> NuPin {
    // Official release checksums. The linux x86_64 value is the one rust-tops pins.
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => NuPin {
            triple: "x86_64-unknown-linux-gnu",
            sha256: "f0daaa0256b0f7dd5f4d9925b4d5522d8de5e947ed9ad64a43526e5cffeb05df",
        },
        ("linux", "aarch64") => NuPin {
            triple: "aarch64-unknown-linux-gnu",
            sha256: "adbe9a5b438eb258d43a40442f87cfac31a88a0c5a5ec2363cb5dcd3d807dbd2",
        },
        ("macos", "x86_64") => NuPin {
            triple: "x86_64-apple-darwin",
            sha256: "7f5fb53a5208b36b8b0276f53ab78ca61e7f5fa4f122d0efd8a94f8db83bccb4",
        },
        ("macos", "aarch64") => NuPin {
            triple: "aarch64-apple-darwin",
            sha256: "21a11b4eb804f4bb7ce67bb18d90d227824d271e19e9f88da15733691888b916",
        },
        (os, arch) => panic!("no pinned nushell sha256 for {os}-{arch}"),
    }
}

fn cache_dir(pin: &NuPin) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/skill-nu")
        .join(format!("nu-{NU_VERSION}-{}", pin.triple))
}

fn member_name(pin: &NuPin) -> String {
    format!("nu-{NU_VERSION}-{}/nu", pin.triple)
}

#[derive(Debug)]
struct ReadyNu {
    bin: PathBuf,
    sha256: String,
}

fn nu_bin() -> PathBuf {
    static READY: OnceLock<ReadyNu> = OnceLock::new();
    let ready = READY.get_or_init(|| {
        let pin = nu_pin();
        load_ready(&cache_dir(&pin), &pin, &tarball_url(&pin))
            .unwrap_or_else(|err| panic!("nushell cache: {err}"))
    });
    // The tarball was hashed once. Re-hash the binary before it runs.
    // A mismatch is re-extracted from the verified tarball before this fails.
    let got = sha256_file(&ready.bin).unwrap_or_else(|err| panic!("nushell cache: {err}"));
    if got != ready.sha256 {
        let pin = nu_pin();
        let repaired = load_ready(&cache_dir(&pin), &pin, &tarball_url(&pin))
            .unwrap_or_else(|err| panic!("nushell cache: {err}"));
        let got = sha256_file(&repaired.bin).unwrap_or_else(|err| panic!("nushell cache: {err}"));
        if got != ready.sha256 {
            panic!("cached nu binary sha256 {got} != {}", ready.sha256);
        }
    }
    ready.bin.clone()
}

fn tarball_url(pin: &NuPin) -> String {
    format!(
        "https://github.com/nushell/nushell/releases/download/{NU_VERSION}/nu-{NU_VERSION}-{}.tar.gz",
        pin.triple
    )
}

/// Verify the pinned tarball, unpack `nu` once, and return its hash.
/// A tarball whose sha256 does not match is deleted and downloaded once.
/// A cached `nu` that does not match that tarball is extracted again.
/// A second mismatch is an error. Nothing unverified is executed.
fn load_ready(dir: &Path, pin: &NuPin, url: &str) -> Result<ReadyNu, String> {
    fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let _guard = lock_dir(dir)?;
    ensure_tarball(dir, pin, url)?;
    let tar_path = dir.join("nu.tar.gz");
    let bin = dir.join("nu");
    if !bin.is_file() {
        match_tar_sha(&tar_path, pin.sha256)?;
        extract_nu(dir, &tar_path)?;
    }
    match_tar_sha(&tar_path, pin.sha256)?;
    if !bin.is_file() {
        return Err(format!("nu missing after extract in {}", dir.display()));
    }
    let inside = sha256_tar_member(&tar_path, &member_name(pin))?;
    let on_disk = sha256_file(&bin)?;
    if on_disk == inside {
        return Ok(ReadyNu { bin, sha256: on_disk });
    }
    // The tarball matched its pin. Replace the bad binary from that archive.
    let _ = fs::remove_file(&bin);
    extract_nu(dir, &tar_path)?;
    if !bin.is_file() {
        return Err(format!("nu missing after re-extract in {}", dir.display()));
    }
    let again = sha256_file(&bin)?;
    if again != inside {
        let _ = fs::remove_file(&bin);
        return Err(format!("cached nu binary sha256 {again} != tarball member {inside}"));
    }
    Ok(ReadyNu { bin, sha256: again })
}

fn ensure_tarball(dir: &Path, pin: &NuPin, url: &str) -> Result<(), String> {
    let path = dir.join("nu.tar.gz");
    if path.is_file() && sha256_file(&path)? == pin.sha256 {
        return Ok(());
    }
    if path.is_file() {
        fs::remove_file(&path).map_err(|err| format!("remove {}: {err}", path.display()))?;
        let _ = fs::remove_file(dir.join("nu"));
    }
    download_locked(dir, pin, url)?;
    if !path.is_file() {
        return Err(format!("sha256 mismatch after re-download of {url}"));
    }
    let got = sha256_file(&path)?;
    if got != pin.sha256 {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(dir.join("nu"));
        return Err(format!("sha256 mismatch after re-download: {got} != {}", pin.sha256));
    }
    Ok(())
}

struct DownloadLock(File);

impl Drop for DownloadLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Exclusive advisory lock. The kernel drops it when the holder is killed,
/// so the next caller does not wait out a stale directory lock. Partials
/// from that holder are removed once the lock is held.
fn lock_dir(dir: &Path) -> Result<DownloadLock, String> {
    let path = dir.join(".download.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    // File::lock needs Rust 1.89+.
    // flock can return EINTR when another test child exits. Retry; a killed
    // holder still drops the lock, so this does not wait out a stale pid.
    loop {
        match file.lock() {
            Ok(()) => break,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(format!("lock {}: {err}", path.display())),
        }
    }
    cleanup_partials(dir)?;
    Ok(DownloadLock(file))
}

fn cleanup_partials(dir: &Path) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("partial-") && name.ends_with(".tar.gz") {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn download_locked(dir: &Path, pin: &NuPin, url: &str) -> Result<(), String> {
    let final_path = dir.join("nu.tar.gz");
    if final_path.is_file() && sha256_file(&final_path)? == pin.sha256 {
        return Ok(());
    }
    let partial = dir.join(format!("partial-{}.tar.gz", std::process::id()));
    if let Err(err) = fetch_to(url, &partial) {
        let _ = fs::remove_file(&partial);
        return Err(err);
    }
    let got = match sha256_file(&partial) {
        Ok(got) => got,
        Err(err) => {
            let _ = fs::remove_file(&partial);
            return Err(err);
        }
    };
    if got != pin.sha256 {
        let _ = fs::remove_file(&partial);
        return Err(format!("sha256 mismatch for {url}: {got} != {}", pin.sha256));
    }
    fs::rename(&partial, &final_path).map_err(|err| format!("rename {}: {err}", final_path.display()))?;
    Ok(())
}

fn fetch_to(url: &str, partial: &Path) -> Result<(), String> {
    if let Some(path) = url.strip_prefix("file://") {
        fs::copy(path, partial).map(|_| ()).map_err(|err| format!("copy {path}: {err}"))
    } else {
        let status = Command::new("curl")
            .args(["--retry", "3", "--retry-delay", "2", "-fsSL", "-o"])
            .arg(partial)
            .arg(url)
            .status()
            .map_err(|err| format!("curl {url}: {err}"))?;
        if status.success() { Ok(()) } else { Err(format!("download {url} failed")) }
    }
}

fn match_tar_sha(tar_path: &Path, expected: &str) -> Result<(), String> {
    if !tar_path.is_file() {
        return Err(format!("missing {}", tar_path.display()));
    }
    let got = sha256_file(tar_path)?;
    if got == expected { Ok(()) } else { Err(format!("tarball sha256 mismatch: {got} != {expected}")) }
}

fn extract_nu(dir: &Path, tar_path: &Path) -> Result<(), String> {
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(tar_path)
        .arg("-C")
        .arg(dir)
        .arg("--strip-components=1")
        .status()
        .map_err(|err| format!("tar: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        let _ = fs::remove_file(dir.join("nu"));
        Err(format!("extract {} failed", tar_path.display()))
    }
}

fn sha256_tar_member(tar_path: &Path, member: &str) -> Result<String, String> {
    let mut tar = Command::new("tar")
        .args(["-xOzf"])
        .arg(tar_path)
        .arg(member)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("tar: {err}"))?;
    let mut sum = hash_command();
    sum.stdin(tar.stdout.take().unwrap());
    let output = sum.output().map_err(|err| format!("sha256: {err}"))?;
    let tar_status = tar.wait().map_err(|err| format!("tar: {err}"))?;
    if !tar_status.success() {
        return Err(format!("tar member {member} failed"));
    }
    digest_from(&output)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let output =
        hash_command().arg(path).output().map_err(|err| format!("sha256 {}: {err}", path.display()))?;
    digest_from(&output)
}

fn hash_command() -> Command {
    // Binary mode. Text mode can rewrite bytes on the way into the digest.
    if cfg!(target_os = "macos") {
        let mut cmd = Command::new("shasum");
        cmd.arg("-a").arg("256").arg("-b");
        cmd
    } else {
        let mut cmd = Command::new("sha256sum");
        cmd.arg("-b");
        cmd
    }
}

fn digest_from(output: &Output) -> Result<String, String> {
    if !output.status.success() {
        return Err(format!("sha256 failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let hex = text.split_whitespace().next().unwrap_or("");
    if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(hex.to_string())
    } else {
        Err(format!("sha256 output was not a digest: {text}"))
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    out
}

fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut hash: [u32; 8] =
        [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let bit_len = (data.len() as u64).saturating_mul(8);
    let mut msg = Vec::with_capacity(data.len() + 72);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = hash[0];
        let mut b = hash[1];
        let mut c = hash[2];
        let mut d = hash[3];
        let mut e = hash[4];
        let mut f = hash[5];
        let mut g = hash[6];
        let mut h = hash[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        hash[0] = hash[0].wrapping_add(a);
        hash[1] = hash[1].wrapping_add(b);
        hash[2] = hash[2].wrapping_add(c);
        hash[3] = hash[3].wrapping_add(d);
        hash[4] = hash[4].wrapping_add(e);
        hash[5] = hash[5].wrapping_add(f);
        hash[6] = hash[6].wrapping_add(g);
        hash[7] = hash[7].wrapping_add(h);
    }
    let mut out = [0u8; 32];
    for (i, word) in hash.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
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

    let appended =
        run_nu(&home, &path_env, &["append", "--vault", vault_s, "--", &created_env.path, "- [ ] x"]);
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
    assert_eq!(bad_env.kind, "usage", "{}", show(&bad_limit));
    assert_eq!(bad_env.exit, Some(1), "{}", show(&bad_limit));
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

#[test]
fn skill_append_rejects_misplaced_flags_without_writing() {
    let home = scratch();
    let vault = home.join("vault");
    init_vault(&home, &vault);
    let path_env = path_with(lapis().parent().unwrap());
    let vault_s = vault.to_str().unwrap();
    let created =
        run_nu(&home, &path_env, &["create", "--title", "Beta", "--body", "hello", "--vault", vault_s]);
    assert!(created.status.success(), "{}", show(&created));
    let path = parse_env(&created.stdout).path;
    let note = vault.join(&path);
    let before = fs::read(&note).unwrap();

    let swallowed = run_nu(
        &home,
        &path_env,
        &["append", &format!("--vault={vault_s}"), &path, "--", "--flagtext", "--dry-run"],
    );
    assert_eq!(swallowed.status.code(), Some(1), "{}", show(&swallowed));
    let swallowed_env = parse_env(&swallowed.stdout);
    assert!(!swallowed_env.ok, "{}", show(&swallowed));
    assert_eq!(swallowed_env.kind, "usage");
    assert_eq!(swallowed_env.exit, Some(1));
    assert_eq!(fs::read(&note).unwrap(), before, "a flag after -- was written");

    let bare = run_nu(&home, &path_env, &["append", &path, "--dry-run is text", "--vault", vault_s]);
    assert_eq!(bare.status.code(), Some(1), "{}", show(&bare));
    let bare_env = parse_env(&bare.stdout);
    assert!(!bare_env.ok, "{}", show(&bare));
    assert_eq!(bare_env.kind, "usage");
    assert_eq!(bare_env.exit, Some(1));
    assert_eq!(bare_env.command, "append");
    assert_eq!(fs::read(&note).unwrap(), before, "dash text without -- was written");

    let swallowed_vault = run_nu(
        &home,
        &path_env,
        &["append", &format!("--vault={vault_s}"), &path, "--", "kept", "--vault", "/tmp/elsewhere"],
    );
    assert_eq!(swallowed_vault.status.code(), Some(1), "{}", show(&swallowed_vault));
    let vault_env = parse_env(&swallowed_vault.stdout);
    assert!(!vault_env.ok, "{}", show(&swallowed_vault));
    assert_eq!(vault_env.kind, "usage");
    assert_eq!(vault_env.exit, Some(1));
    assert_eq!(fs::read(&note).unwrap(), before, "a trailing --vault was swallowed");

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn skill_nu_rejects_tampered_cache() {
    let pin = nu_pin();
    let official = nu_bin();
    assert!(official.is_file());
    let home = scratch();
    let dir = home.join("cache");
    fs::create_dir_all(&dir).unwrap();
    let src = cache_dir(&pin);
    fs::copy(src.join("nu.tar.gz"), dir.join("nu.tar.gz")).unwrap();
    fs::copy(src.join("nu"), dir.join("nu")).unwrap();
    let poison = home.join("poison.tar.gz");
    fs::write(&poison, b"not a nushell tarball").unwrap();
    let poison_url = file_url(&poison);

    let ready = load_ready(&dir, &pin, &poison_url).unwrap();
    assert_eq!(ready.bin, dir.join("nu"));
    assert_eq!(ready.sha256, sha256_file(&src.join("nu")).unwrap());

    let official_nu = fs::read(src.join("nu")).unwrap();
    let mut planted = OpenOptions::new().append(true).open(&ready.bin).unwrap();
    planted.write_all(b"\n").unwrap();
    drop(planted);
    let repaired_bin = load_ready(&dir, &pin, &poison_url).unwrap();
    assert_eq!(repaired_bin.sha256, sha256_file(&src.join("nu")).unwrap());
    assert_eq!(
        fs::read(dir.join("nu")).unwrap(),
        official_nu,
        "tampered nu was not restored from the verified tarball"
    );
    assert_eq!(sha256_file(&dir.join("nu.tar.gz")).unwrap(), pin.sha256);

    let mut tampered = OpenOptions::new().append(true).open(dir.join("nu.tar.gz")).unwrap();
    tampered.write_all(b"x").unwrap();
    drop(tampered);
    let tar_err = load_ready(&dir, &pin, &poison_url).unwrap_err();
    assert!(tar_err.contains("sha256"), "{tar_err}");
    assert!(!dir.join("nu.tar.gz").exists(), "a mismatched re-download was kept");
    assert!(!dir.join("nu").exists(), "a binary from an unverified archive was left in place");

    let repaired = load_ready(&dir, &pin, &file_url(&src.join("nu.tar.gz"))).unwrap();
    assert_eq!(sha256_file(&dir.join("nu.tar.gz")).unwrap(), pin.sha256);
    assert_eq!(repaired.sha256, sha256_file(&src.join("nu")).unwrap());

    let decoy = PathBuf::from(format!("/tmp/nu-{NU_VERSION}-{}", pin.triple));
    let _ = fs::remove_dir_all(&decoy);
    fs::create_dir_all(&decoy).unwrap();
    fs::write(decoy.join("nu"), b"#!/bin/sh\necho PLANTED\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(decoy.join("nu"), fs::Permissions::from_mode(0o755));
    }
    let vault = home.join("vault");
    init_vault(&home, &vault);
    let path_env = path_with(lapis().parent().unwrap());
    let out = run_nu(&home, &path_env, &["read", "nope.md", "--vault", vault.to_str().unwrap()]);
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!text.contains("PLANTED"), "{text}");
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    let _ = fs::remove_dir_all(&decoy);
    let _ = fs::remove_dir_all(&home);
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

#[test]
fn download_lock_releases_when_dropped() {
    let home = scratch();
    let guard = lock_dir(&home).unwrap();
    assert!(home.join(".download.lock").is_file());
    drop(guard);
    let started = Instant::now();
    let again = lock_dir(&home).unwrap();
    assert!(started.elapsed() < Duration::from_secs(2), "relock waited {:?}", started.elapsed());
    drop(again);
    let _ = fs::remove_dir_all(&home);
}

/// Child of `download_lock_takes_over_from_a_dead_holder`. Uses the same
/// `File::lock` as the downloader. A no-op unless that test spawned it.
#[test]
fn download_lock_hold_for_takeover() {
    let Ok(dir) = std::env::var("SKILL_NU_HOLD_DIR") else { return };
    let dir = PathBuf::from(dir);
    let guard = lock_dir(&dir).unwrap_or_else(|err| panic!("holder lock: {err}"));
    fs::write(dir.join("partial-dead.tar.gz"), b"partial").unwrap();
    let mut ready = File::create(dir.join("ready")).unwrap();
    ready.write_all(b"1").unwrap();
    ready.sync_all().unwrap();
    thread::sleep(Duration::from_secs(300));
    drop(guard);
}

#[test]
fn download_lock_takes_over_from_a_dead_holder() {
    // The holder re-execs this binary. Do not start another holder from there.
    if std::env::var_os("SKILL_NU_HOLD_DIR").is_some() {
        return;
    }
    let home = scratch();
    let partial = home.join("partial-dead.tar.gz");
    let mut holder = Command::new(std::env::current_exe().unwrap());
    holder
        .env("SKILL_NU_HOLD_DIR", &home)
        .arg("download_lock_hold_for_takeover")
        .arg("--exact")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut holder = holder.spawn().unwrap_or_else(|err| panic!("holder: {err}"));
    let started = Instant::now();
    while !home.join("ready").is_file() {
        if holder.try_wait().ok().flatten().is_some() || started.elapsed() > Duration::from_secs(30) {
            let _ = holder.kill();
            let out = holder.wait_with_output().ok();
            let text = out
                .as_ref()
                .map(|o| {
                    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
                })
                .unwrap_or_default();
            panic!("holder never locked\n{text}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let probe = OpenOptions::new().read(true).write(true).open(home.join(".download.lock")).unwrap();
    match probe.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => {}
        other => panic!("live holder was not visible to the advisory lock: {other:?}"),
    }
    drop(probe);
    holder.kill().unwrap_or_else(|err| panic!("kill holder: {err}"));
    let _ = holder.wait();

    let takeover = Instant::now();
    let guard = lock_dir(&home).unwrap();
    let waited = takeover.elapsed();
    assert!(waited < Duration::from_secs(10), "dead holder blocked the lock for {waited:?}");
    assert!(!partial.exists(), "partial from the dead holder was kept");
    drop(guard);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn sha256_matches_the_empty_and_abc_vectors() {
    assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
