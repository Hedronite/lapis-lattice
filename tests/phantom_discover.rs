//! G2: discovery finds `phantom-fractal` only, absence is quiet, and no Phantom
//! crate is linked. No Phantom install is needed; binaries here are stub files.

#[allow(dead_code)]
#[path = "../src/phantom/discover.rs"]
mod discover;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

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

fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
    std::env::join_paths(dirs).unwrap()
}

#[test]
fn finds_phantom_fractal_on_path() {
    let empty = temp_dir("path-empty");
    let bin = temp_dir("path-bin");
    let want = stub(&bin, discover::BINARY);
    let path = path_of(&[&empty, &bin]);
    assert_eq!(discover::discover(Some(&path), None), Some(want));
}

#[test]
fn finds_configured_path_named_phantom_fractal() {
    let empty = temp_dir("cfg-empty");
    let bin = temp_dir("cfg-bin");
    let want = stub(&bin, discover::BINARY);
    let path = path_of(&[&empty]);
    assert_eq!(discover::discover(Some(&path), Some(&want)), Some(want.clone()));
    assert_eq!(discover::discover(None, Some(&want)), Some(want));
}

#[test]
fn configured_path_with_another_name_is_ignored() {
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
    let bin = temp_dir("fract-only");
    stub(&bin, "fract");
    assert_eq!(discover::discover(Some(&path_of(&[&bin])), None), None);
}

#[test]
fn earlier_fract_does_not_win() {
    let early = temp_dir("fract-early");
    let late = temp_dir("fract-late");
    stub(&early, "fract");
    let want = stub(&late, discover::BINARY);
    assert_eq!(discover::discover(Some(&path_of(&[&early, &late])), None), Some(want));
}

/// `lapis desktop --check` with nothing on `PATH` but `fract`: exit 0 and a
/// `Phantom not found` line, no dialog, no panic.
#[test]
fn desktop_check_without_phantom_exits_zero() {
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
