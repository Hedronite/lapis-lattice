# Lapis

Rust-TOPS lives in `rust-tops.yaml` at this workspace root and in `crates/<name>/rust-tops.yaml` for each member crate. The binary is `cargo-tops` (plain `cargo tops` is not a cargo subcommand). It reads `./rust-tops.yaml`. Check a member with `cargo-tops check --path crates/<name>`.

This repo does not vendor `RUST_TOPS.md` or `LAWS.bend`. The coverage job fails when the LCOV or the CRAP JSON is empty. It does not set a line floor. `vendor/gpui-base` is not a workspace member.
