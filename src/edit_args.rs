//! Clap args for `replace` and `insert`, plus the shared write guard.
//! Split out of `cli` so that file stays under the source line cap.

use clap::Args;

/// `--dry-run` / stale guards shared by append, replace, insert, and task toggle.
#[derive(Debug, Args, Clone, Default)]
pub struct GuardArgs {
    /// Compute and report the result without writing.
    #[arg(long)]
    pub dry_run: bool,

    /// Refuse if the note's mtime (ms, `updatedAt` from `read`) no longer matches.
    #[arg(long, value_name = "MS")]
    pub if_mtime: Option<u64>,

    /// Refuse if the note's content hash (`hash` from `read`) no longer matches.
    #[arg(long, value_name = "HASH")]
    pub if_hash: Option<String>,
}

impl GuardArgs {
    pub fn guard(&self) -> crate::write::Guard {
        crate::write::Guard { dry_run: self.dry_run, if_mtime: self.if_mtime, if_hash: self.if_hash.clone() }
    }
}

#[derive(Debug, Args)]
pub struct ReplaceArgs {
    /// Vault-relative note path.
    #[arg(value_name = "PATH")]
    pub path: String,
    /// Literal to replace. Raw bytes of the body; must match exactly once. Overlaps count.
    #[arg(long, value_name = "TEXT")]
    pub old: String,
    /// Replacement. Empty deletes the match. Omit with `--stdin` to read it from stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "stdin", required_unless_present = "stdin")]
    pub new: Option<String>,
    /// Read the replacement from stdin.
    #[arg(long)]
    pub stdin: bool,
    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,
    #[command(flatten)]
    pub guard: GuardArgs,
}

#[derive(Debug, Args)]
pub struct InsertArgs {
    /// Vault-relative note path.
    #[arg(value_name = "PATH")]
    pub path: String,
    /// Insert `text` immediately after this anchor. Raw bytes; must match once.
    #[arg(long, value_name = "ANCHOR", conflicts_with = "before", required_unless_present = "before")]
    pub after: Option<String>,
    /// Insert `text` immediately before this anchor. Raw bytes; must match once.
    #[arg(long, value_name = "ANCHOR", required_unless_present = "after")]
    pub before: Option<String>,
    /// Text to insert. Omit with `--stdin` to read it from stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "stdin", required_unless_present = "stdin")]
    pub text: Option<String>,
    /// Read the inserted text from stdin.
    #[arg(long)]
    pub stdin: bool,
    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,
    #[command(flatten)]
    pub guard: GuardArgs,
}
