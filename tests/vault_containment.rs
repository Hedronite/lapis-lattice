//! S-3 readers: `check-note` opens the canonical file its containment check
//! returned. A link out of the vault is refused (exit 3); a link inside reads.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Output;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lapis-containment-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn check_note(vault: &Path, config: &Path, rel: &str) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_lapis"))
        .args(["--json", "--vault"])
        .arg(vault)
        .args(["check-note", rel])
        .env("XDG_CONFIG_HOME", config)
        .env_remove("LAPIS_VAULT")
        .output()
        .unwrap()
}

#[test]
fn check_note_refuses_a_link_out_and_reads_a_link_inside() {
    let vault = temp_dir("vault");
    let outside = temp_dir("outside");
    let config = temp_dir("config");
    std::fs::create_dir_all(vault.join("notes")).unwrap();
    std::fs::write(vault.join("notes/Real.md"), "# Real\n").unwrap();
    std::fs::write(outside.join("Secret.md"), "# Secret\n").unwrap();
    std::os::unix::fs::symlink(outside.join("Secret.md"), vault.join("notes/Leak.md")).unwrap();
    std::os::unix::fs::symlink(vault.join("notes/Real.md"), vault.join("notes/Alias.md")).unwrap();

    let out = check_note(&vault, &config, "notes/Leak.md");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(out.stdout.is_empty() || !String::from_utf8_lossy(&out.stdout).contains("Secret"));

    let out = check_note(&vault, &config, "notes/Alias.md");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let review: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(review.is_object(), "{stdout}");

    let _ = std::fs::remove_dir_all(vault);
    let _ = std::fs::remove_dir_all(outside);
    let _ = std::fs::remove_dir_all(config);
}
