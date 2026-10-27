//! Phantom code-graph client: find `phantom-fractal`, run it once with
//! `--json`, and parse the envelope it prints. No Phantom crate, no Phantom
//! database, and no path taken from the JSON is ever opened.
//!
//! Std, serde, and serde_json only (plus libc on unix, to stop a process
//! group), so `tests/phantom_*.rs` include this module and call the same
//! functions the binary ships.

pub mod discover;
pub mod envelope;
pub mod run;
