//! `lapis tome` arguments. Always parsed, so `--help` works without `--features tome`.

use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub enum TomeCommand {
    /// Build a section tree for one or more PDFs into `<vault>/.lapis/tomes/`.
    Build(TomeBuildArgs),
    /// Print a stored tree, or the subtree under `--node`.
    Tree(TomeTreeArgs),
    /// Walk the tree for a query and open the chosen pages.
    Search(TomeSearchArgs),
    /// Open page text for one or more node ids.
    Open(TomeOpenArgs),
}

#[derive(Debug, Args)]
pub struct TomeBuildArgs {
    /// PDF paths. Vault-relative or absolute.
    #[arg(value_name = "PDF", required = true)]
    pub pdfs: Vec<String>,

    /// Rebuild even when the stored hash and builder version match.
    #[arg(long)]
    pub force: bool,

    /// When the PDF has no outline and no headings, build an explicit page-window tree.
    #[arg(long)]
    pub allow_windows: bool,

    /// Try the llm-struct builder. In this spike that path fails closed.
    #[arg(long)]
    pub llm_struct: bool,
}

#[derive(Debug, Args)]
pub struct TomeTreeArgs {
    /// Doc id (PDF sha256) or a path to the PDF.
    #[arg(value_name = "DOC")]
    pub doc: String,

    /// Node id, for example `0003.0002`. Omit for the roots.
    #[arg(long, value_name = "ID")]
    pub node: Option<String>,

    /// How many child levels to include. Omit for the full tree.
    #[arg(long, value_name = "N")]
    pub depth: Option<u8>,
}

#[derive(Debug, Args)]
pub struct TomeSearchArgs {
    /// Doc id (PDF sha256) or a path to the PDF.
    #[arg(value_name = "DOC")]
    pub doc: String,

    /// Question the walk judges sections against.
    #[arg(value_name = "QUERY")]
    pub query: String,

    /// Maximum Jev calls for this walk.
    #[arg(long, default_value_t = 24)]
    pub max_judge_calls: u32,

    /// Maximum pages to open. The hard cap is 12.
    #[arg(long, default_value_t = 12)]
    pub max_pages: u32,
}

#[derive(Debug, Args)]
pub struct TomeOpenArgs {
    /// Doc id (PDF sha256) or a path to the PDF.
    #[arg(value_name = "DOC")]
    pub doc: String,

    /// Node ids to open.
    #[arg(value_name = "NODE", num_args = 1..)]
    pub nodes: Vec<String>,
}
