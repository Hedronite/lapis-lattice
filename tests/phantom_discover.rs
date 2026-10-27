//! G2, G0a and G4.2: discovery finds `phantom-fractal` only, stores it absolute,
//! refuses a relative configured path,
//! absence is quiet, and no Phantom crate is linked. No Phantom install is
//! needed; binaries here are stub files.

#[allow(dead_code)]
#[path = "../src/phantom/mod.rs"]
mod phantom;

use phantom::discover;
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
    let d = std::env::temp_dir().join(format!("lapis-phantom-{name}-{}-{seq}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A stub executable. Its body exits 1, so running it would be visible.
fn stub(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&p, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    p
}

/// Where discovery reports `name` in `dir`: the canonical directory.
fn found(dir: &Path, name: &str) -> PathBuf {
    dir.canonicalize().unwrap().join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
    std::env::join_paths(dirs).unwrap()
}

#[test]
fn finds_phantom_fractal_on_path() {
    let _g = serial();
    let empty = temp_dir("path-empty");
    let bin = temp_dir("path-bin");
    stub(&bin, discover::BINARY);
    let path = path_of(&[&empty, &bin]);
    assert_eq!(discover::discover(Some(&path), None), Some(found(&bin, discover::BINARY)));
}

#[test]
fn finds_configured_path_named_phantom_fractal() {
    let _g = serial();
    let empty = temp_dir("cfg-empty");
    let bin = temp_dir("cfg-bin");
    let configured = stub(&bin, discover::BINARY);
    let want = found(&bin, discover::BINARY);
    let path = path_of(&[&empty]);
    assert_eq!(discover::discover(Some(&path), Some(&configured)), Some(want.clone()));
    assert_eq!(discover::discover(None, Some(&configured)), Some(want));
}

/// #59: a configured binary wins over one on `PATH`, and a configured path that
/// fails its checks does not fall back to `PATH`.
#[test]
fn configured_path_takes_precedence_over_path() {
    let _g = serial();
    let early = temp_dir("prec-path");
    let chosen = temp_dir("prec-cfg");
    stub(&early, discover::BINARY);
    let configured = stub(&chosen, discover::BINARY);
    let path = path_of(&[&early]);
    assert_eq!(discover::discover(Some(&path), Some(&configured)), Some(found(&chosen, discover::BINARY)));
    let missing = chosen.join("gone").join(discover::BINARY);
    assert_eq!(discover::discover(Some(&path), Some(&missing)), None, "no fallback to PATH");
    let wrong_name = stub(&chosen, "fract");
    assert_eq!(discover::discover(Some(&path), Some(&wrong_name)), None, "no fallback to PATH");
}

#[test]
fn configured_path_with_another_name_is_ignored() {
    let _g = serial();
    let bin = temp_dir("cfg-other");
    let fract = stub(&bin, "fract");
    let other = stub(&bin, "phantom");
    assert_eq!(discover::discover(None, Some(&fract)), None);
    assert_eq!(discover::discover(None, Some(&other)), None);
}

#[cfg(unix)]
#[test]
fn non_executable_file_is_not_found() {
    use std::os::unix::fs::PermissionsExt;
    let _g = serial();
    let bin = temp_dir("noexec");
    let p = stub(&bin, discover::BINARY);
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(discover::discover(Some(&path_of(&[&bin])), Some(&p)), None);
}

#[test]
fn no_binary_is_none() {
    let empty = temp_dir("none");
    assert_eq!(discover::discover(Some(&path_of(&[&empty])), None), None);
    assert_eq!(discover::discover(None, None), None);
}

#[test]
fn fract_on_path_is_never_found() {
    let _g = serial();
    let bin = temp_dir("fract-only");
    stub(&bin, "fract");
    assert_eq!(discover::discover(Some(&path_of(&[&bin])), None), None);
}

#[test]
fn earlier_fract_does_not_win() {
    let _g = serial();
    let early = temp_dir("fract-early");
    let late = temp_dir("fract-late");
    stub(&early, "fract");
    stub(&late, discover::BINARY);
    assert_eq!(
        discover::discover(Some(&path_of(&[&early, &late])), None),
        Some(found(&late, discover::BINARY))
    );
}

/// G0a: a relative `PATH` entry is joined to the current directory and stored
/// absolute, with `..` gone.
#[test]
fn relative_path_entry_is_stored_absolute() {
    let _g = serial();
    let cwd = temp_dir("rel-path");
    std::fs::create_dir_all(cwd.join("bin")).unwrap();
    std::fs::create_dir_all(cwd.join("other")).unwrap();
    stub(&cwd.join("bin"), discover::BINARY);
    let want = found(&cwd.join("bin"), discover::BINARY);
    for entry in ["bin", "./bin", "other/../bin"] {
        let got = discover::discover_from(Some(&cwd), Some(entry.as_ref()), None).unwrap();
        assert!(got.is_absolute(), "{entry}: {}", got.display());
        assert!(!got.components().any(|c| c == std::path::Component::ParentDir), "{}", got.display());
        assert_eq!(got, want, "{entry}");
    }
    // Without a current directory a relative entry cannot be made absolute.
    assert_eq!(discover::discover_from(None, Some("bin".as_ref()), None), None);
}

/// G0a: an empty entry is still skipped, even when the current directory holds
/// a `phantom-fractal`.
#[test]
fn empty_path_entry_is_still_skipped() {
    let _g = serial();
    let cwd = temp_dir("empty-entry");
    stub(&cwd, discover::BINARY);
    let sep = if cfg!(windows) { ";" } else { ":" };
    for var in ["", sep] {
        assert_eq!(discover::discover_from(Some(&cwd), Some(var.as_ref()), None), None, "{var:?}");
    }
}

/// G4.2: a relative `$LAPIS_PHANTOM_BINARY` is refused. A valid stub sits at
/// that path under the current directory and another is on `PATH`; neither is
/// returned.
#[test]
fn relative_configured_path_is_refused() {
    let _g = serial();
    let cwd = temp_dir("rel-cfg");
    let other = temp_dir("rel-cfg-path");
    std::fs::create_dir_all(cwd.join("bin")).unwrap();
    stub(&cwd.join("bin"), discover::BINARY);
    stub(&other, discover::BINARY);
    let path = path_of(&[&other]);
    for rel in ["bin", "./bin", "bin/../bin"] {
        let rel = PathBuf::from(rel).join(discover::BINARY);
        assert!(cwd.join(&rel).is_file(), "the stub is really there");
        assert_eq!(discover::discover_from(Some(&cwd), Some(&path), Some(&rel)), None, "{}", rel.display());
        assert_eq!(discover::discover_from(None, Some(&path), Some(&rel)), None, "{}", rel.display());
    }
    // The same stub named absolutely is found, so only the relative form is refused.
    let abs = cwd.join("bin").join(discover::BINARY);
    let want = found(&cwd.join("bin"), discover::BINARY);
    assert_eq!(discover::discover_from(Some(&cwd), Some(&path), Some(&abs)), Some(want));
}

/// `lapis desktop --check` with nothing on `PATH` but `fract`: exit 0 and a
/// `Phantom not found` line, no dialog, no panic.
#[test]
fn desktop_check_without_phantom_exits_zero() {
    let _g = serial();
    let vault = temp_dir("check-vault");
    let config = temp_dir("check-config");
    let bin = temp_dir("check-path");
    stub(&bin, "fract");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lapis"))
        .args(["--vault"])
        .arg(&vault)
        .args(["desktop", "--check"])
        .env("PATH", path_of(&[&bin]))
        .env("XDG_CONFIG_HOME", &config)
        .env_remove(discover::ENV)
        .env_remove("LAPIS_VAULT")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "exit {:?}\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.lines().any(|l| l == "Phantom not found"), "{stdout}");
}

/// G2c: no Phantom crate in the dependency graph, under any feature.
#[test]
fn cargo_tree_has_no_phantom_crate() {
    let _g = serial();
    let out = std::process::Command::new(env!("CARGO"))
        .args(["tree", "-p", "lapis", "--all-features", "--prefix", "none", "--manifest-path"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(tree.lines().any(|l| l.starts_with("lapis v")), "{tree}");
    let phantom: Vec<_> = tree.lines().filter(|l| l.to_ascii_lowercase().starts_with("phantom")).collect();
    assert!(phantom.is_empty(), "Phantom crate(s) in `cargo tree -p lapis`: {phantom:?}");
}
