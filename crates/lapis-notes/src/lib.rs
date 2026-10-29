//! Note creation for library callers: the HAL create-set, slugging, the inbox
//! bucket from the overlay, containment and the write — exactly what the CLI
//! writes, byte for byte, without the CLI.
//!
//! Files on disk stay the write source of truth. Every function here writes at
//! most one file under the vault root and returns its vault-relative path; as
//! in the CLI, the caller kicks any reindex it wants afterwards.
//!
//! What stayed out of the crate on purpose: `--template` resolution and the
//! in-scope RAG rewrite gate are CLI-side policies around the composed text
//! (see [`compose_text`]/[`write_composed`]): the CLI applies them between
//! composing and writing so its bytes cannot drift from the library's.
//!
//! Dates: [`CreateRequest::created`] carries the ISO date stamped into
//! `created:` and `updated:` so an app can pin time deterministically.
//! Classification is caller-supplied: a `classify` hook returning
//! `Option<[`Classified`]>` per vault-relative path, or none at all — the
//! crate never spawns a process; without a hook it falls back to the first
//! path segment.
use std::path::Path;

pub mod contain;
pub mod create;
pub mod error;
pub mod fsutil;
pub mod overlay;
pub mod slug;
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
struct RootReadmeDoctests;
pub mod taxonomy;

pub use contain::{clean_rel, contained_new};
pub use create::{
    AUTHORITATIVE_MARKER, CANON_TYPES, Classify, CreateRequest, HAL_VERSION, WrittenNote, compose_text,
    create_new, create_set, target_path, to_yaml, write_composed,
};
pub use error::{Error, Result};
pub use overlay::{Buckets, Corpus, OVERLAY_REL, Overlay, OverlaySource, load_overlay};
pub use slug::slug;
pub use taxonomy::{Classified, fallback};

/// The inbox bucket of a vault: `.lapis/vault.json`'s `buckets.inbox`.
/// Missing overlay means `inbox`; a malformed overlay is a usage error.
pub fn inbox_bucket(vault: &Path) -> Result<String> {
    Ok(load_overlay(vault)?.0.buckets.inbox)
}
