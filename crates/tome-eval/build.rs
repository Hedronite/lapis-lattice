//! Bakes the git sha into `tome-eval` so result records carry it even when the
//! binary runs on a machine without the checkout (castle).
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let sha = git(&["rev-parse", "HEAD"]).filter(|s| !s.is_empty()).unwrap_or_else(|| "0000000".into());
    println!("cargo:rustc-env=TOME_EVAL_GIT_SHA={sha}");
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
}
