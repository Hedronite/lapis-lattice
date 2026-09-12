//! Exercise the actual binary through a PTY on both supported OS families.
//! In particular, Linux directory-access notifications must not starve input.

#[test]
fn terminal_paste_selection_undo_and_external_save_guards() {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let out = std::env::temp_dir().join(format!("lapis-tui-input-{}-{stamp}", std::process::id()));
    let result = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/ux-input-smoke.py"))
        .args(["--bin", env!("CARGO_BIN_EXE_lapis"), "--out"])
        .arg(&out)
        .arg("--check")
        .output()
        .expect("the supported development/CI environments require Python 3 for PTY smoke");
    assert!(
        result.status.success(),
        "TUI regression failed; artifacts retained at {}\n{}\n{}",
        out.display(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn terminal_missing_index_offers_background_build_and_leaves_built_health() {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let out = std::env::temp_dir().join(format!("lapis-tui-index-{}-{stamp}", std::process::id()));
    let result = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/ux-index-smoke.py"))
        .args(["--bin", env!("CARGO_BIN_EXE_lapis"), "--out"])
        .arg(&out)
        .arg("--check")
        .output()
        .expect("the supported development/CI environments require Python 3 for PTY smoke");
    assert!(
        result.status.success(),
        "missing-index TUI regression failed; artifacts retained at {}\n{}\n{}",
        out.display(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    std::fs::remove_dir_all(out).unwrap();
}

#[test]
fn terminal_screen_decoder_handles_incremental_status_repaints() {
    let result = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/test_ux_terminal.py"))
        .output()
        .expect("Python 3 is required for terminal regression checks");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}
