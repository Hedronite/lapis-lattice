//! clap surface. SPEC.md § CLI: `lapis <command> [--json] [--vault <path>]`.

use clap::{Args, Parser, Subcommand};

use crate::lattice::Mode;

const AFTER_HELP: &str = "\
Exit codes:
  0  ok
  1  usage / user error
  2  lattice down (HTTP unreachable, timeout, or 5xx)
  3  path escape or not found

Vault precedence: --vault, $LAPIS_VAULT, config `vault`, then ~/Obsidian/Atrium/Atrium.
Lattice precedence: --lattice, $LAPIS_LATTICE_URL, config [lattice].url, then http://127.0.0.1:8080.
Config: ~/.config/lapis/config.toml (or $XDG_CONFIG_HOME/lapis/config.toml).";

#[derive(Debug, Parser)]
#[command(
    name = "lapis",
    version,
    about = "Agent-first notes vault over Markdown files indexed by Lapis Lattice.",
    after_help = AFTER_HELP,
    propagate_version = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args, Clone)]
pub struct Global {
    /// Emit JSON on stdout (the agent contract).
    #[arg(long, global = true)]
    pub json: bool,

    /// Vault root. Notes are addressed by vault-relative POSIX paths.
    #[arg(long, global = true, value_name = "PATH")]
    pub vault: Option<String>,

    /// Lattice base URL.
    #[arg(long, global = true, value_name = "URL")]
    pub lattice: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Vault root, overlay buckets, and lattice health. Exit 2 if lattice is down.
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },

    /// Hybrid lattice search (BM25 + vectors + title, fused). Not a disk scan.
    Search(SearchArgs),

    /// Read one note: body plus parsed HAL frontmatter.
    Read(ReadArgs),

    /// Hop-1 wikilink neighbors of a note from the lattice `edges` table.
    Neighbors(NeighborsArgs),

    /// List notes from the lattice `documents` table (metadata only). Never walks the vault.
    List(ListArgs),

    /// Ask the lattice to re-index one note (runs its own indexer; Lapis never writes lattice.db).
    Reindex(ReindexArgs),

    /// Create a note with a HAL create-set (name, type, domain, status=draft, dates, hal_version).
    Create(CreateArgs),

    /// Append text to a note; bumps `updated:` and keeps every other frontmatter line.
    Append(AppendArgs),

    /// Quick capture: timestamped note under the inbox bucket with doc_type=capture.
    Capture(CaptureArgs),

    /// Checkbox tasks: list and toggle by id (`path#index` or `path#task`).
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },

    /// MCP server over stdio (tools: vault_info, search, read_note, list_notes, neighbors, create_note, append_to_note, list_tasks, toggle_task).
    Mcp,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// Scan .md notes (skipping media, archives, trash) and list tasks.
    List(TaskListArgs),
    /// Flip one task between open and done.
    Toggle(TaskToggleArgs),
}

#[derive(Debug, Args)]
pub struct TaskListArgs {
    /// Restrict the scan to this vault-relative folder or file.
    #[arg(value_name = "PATH")]
    pub path: Option<String>,

    /// open | done | in-progress | cancelled | forwarded | waiting
    #[arg(long, value_name = "STATUS")]
    pub status: Option<String>,

    /// today | overdue | YYYY-MM-DD
    #[arg(long, value_name = "WHEN")]
    pub due: Option<String>,

    /// Require this inline #tag.
    #[arg(long, value_name = "TAG")]
    pub tag: Option<String>,
}

#[derive(Debug, Args)]
pub struct TaskToggleArgs {
    /// Task id from `task list`, e.g. `foundry/lapis/plan.md#0`.
    #[arg(value_name = "ID")]
    pub id: String,

    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Note title; becomes HAL `name` and the slugged filename.
    #[arg(long, value_name = "TITLE")]
    pub title: String,

    /// Vault-relative target: a folder (`foundry/lapis/`) or a file (`foundry/lapis/x.md`). Default: inbox bucket.
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,

    /// Template name from `.lapis/templates/<name>.md` ({{title}}, {{date}}, {{cursor}}).
    #[arg(long, value_name = "NAME")]
    pub template: Option<String>,

    /// HAL `type`/`doc_type` (default: taxonomy for the path, else `note`).
    #[arg(long = "type", value_name = "TYPE")]
    pub doc_type: Option<String>,

    /// HAL `domain` (default: taxonomy for the path, else first folder).
    #[arg(long, value_name = "DOMAIN")]
    pub domain: Option<String>,

    /// Tag (repeatable).
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,

    /// Body text (default: `# <title>`). Use `--stdin` to read it from stdin.
    #[arg(long, value_name = "TEXT", conflicts_with = "stdin")]
    pub body: Option<String>,

    /// Read the body from stdin.
    #[arg(long)]
    pub stdin: bool,

    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(Debug, Args)]
pub struct AppendArgs {
    /// Vault-relative note path.
    #[arg(value_name = "PATH")]
    pub path: String,

    /// Text to append. Omit (or pass `--stdin`) to read from stdin.
    #[arg(value_name = "TEXT", conflicts_with = "stdin")]
    pub text: Option<String>,

    /// Read the text from stdin.
    #[arg(long)]
    pub stdin: bool,

    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(Debug, Args)]
pub struct CaptureArgs {
    /// Captured text. Multiple words are joined; omit to read from stdin.
    #[arg(value_name = "TEXT", num_args = 0..)]
    pub text: Vec<String>,

    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(Debug, Args)]
pub struct ReindexArgs {
    /// Vault-relative markdown path, e.g. `foundry/lapis/STATUS.md`.
    #[arg(value_name = "PATH")]
    pub path: String,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Only paths under this vault-relative prefix, e.g. `foundry/lapis/`.
    #[arg(value_name = "PREFIX")]
    pub prefix: Option<String>,

    /// Filter by HAL `domain` (exact).
    #[arg(long, value_name = "DOMAIN")]
    pub domain: Option<String>,

    /// Filter by canonical `doc_type` (exact).
    #[arg(long, value_name = "TYPE")]
    pub doc_type: Option<String>,

    /// Filter by HAL `status` (exact).
    #[arg(long, value_name = "STATUS")]
    pub status: Option<String>,

    /// Require this tag.
    #[arg(long, value_name = "TAG")]
    pub tag: Option<String>,

    /// Max rows (lattice caps at 1000).
    #[arg(long, short = 'n', default_value_t = 50, value_name = "N")]
    pub limit: u32,

    /// Row offset for paging.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub offset: u32,

    /// Include `_archives/` and `*.DRAFT-*` documents.
    #[arg(long)]
    pub include_archives: bool,
}

#[derive(Debug, Subcommand)]
pub enum VaultCommand {
    /// Show the vault root, overlay, and lattice health.
    Info,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Query text. Multiple words are joined with spaces.
    #[arg(required = true, num_args = 1.., value_name = "QUERY")]
    pub query: Vec<String>,

    /// Final result count (lattice caps at 50).
    #[arg(long, short = 'n', default_value_t = 10, value_name = "N")]
    pub limit: u32,

    /// Filter by HAL `domain`.
    #[arg(long, value_name = "DOMAIN")]
    pub domain: Option<String>,

    /// Retrieval mode.
    #[arg(long, value_enum, default_value_t = Mode::Hybrid)]
    pub mode: Mode,

    /// Collapse to the best chunk per document.
    #[arg(long)]
    pub per_doc: bool,

    /// MMR diversity re-ranking.
    #[arg(long)]
    pub mmr: bool,

    /// Include `_archives/` and `*.DRAFT-*` documents.
    #[arg(long)]
    pub include_archives: bool,
}

impl SearchArgs {
    pub fn query_text(&self) -> String {
        self.query.join(" ")
    }
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Vault-relative path, e.g. `foundry/lapis/SPEC.md` (`.md` may be omitted).
    #[arg(value_name = "PATH")]
    pub path: String,

    /// Print only the parsed frontmatter (JSON), not the body.
    #[arg(long, conflicts_with = "body")]
    pub meta: bool,

    /// Print only the body, without frontmatter.
    #[arg(long)]
    pub body: bool,
}

#[derive(Debug, Args)]
pub struct NeighborsArgs {
    /// Vault-relative path of the note.
    #[arg(value_name = "PATH")]
    pub path: String,

    /// Edge direction: out, in, or both.
    #[arg(long, default_value = "out", value_parser = ["out", "in", "both"])]
    pub direction: String,

    /// Include dangling (unresolved) links too.
    #[arg(long)]
    pub dangling: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_with_globals_anywhere() {
        let c = Cli::try_parse_from(["lapis", "search", "--json", "Hedronite", "--vault", "/tmp"]).unwrap();
        assert!(c.global.json);
        assert_eq!(c.global.vault.as_deref(), Some("/tmp"));
        match c.command {
            Command::Search(s) => {
                assert_eq!(s.query_text(), "Hedronite");
                assert_eq!(s.limit, 10);
                assert_eq!(s.mode, Mode::Hybrid);
            }
            _ => panic!("expected search"),
        }
        let c =
            Cli::try_parse_from(["lapis", "--json", "search", "two", "words", "--mode", "bm25", "-n", "3"])
                .unwrap();
        match c.command {
            Command::Search(s) => {
                assert_eq!(s.query_text(), "two words");
                assert_eq!(s.mode, Mode::Bm25);
                assert_eq!(s.limit, 3);
            }
            _ => panic!("expected search"),
        }
    }

    #[test]
    fn parses_vault_info_and_read() {
        let c = Cli::try_parse_from(["lapis", "vault", "info", "--json"]).unwrap();
        assert!(matches!(c.command, Command::Vault { command: VaultCommand::Info }));
        assert!(c.global.json);
        let c = Cli::try_parse_from(["lapis", "read", "foundry/lapis/SPEC.md"]).unwrap();
        match c.command {
            Command::Read(r) => assert_eq!(r.path, "foundry/lapis/SPEC.md"),
            _ => panic!("expected read"),
        }
    }

    #[test]
    fn parses_list() {
        let c =
            Cli::try_parse_from(["lapis", "list", "--json", "foundry/lapis/", "--status", "live", "-n", "5"])
                .unwrap();
        match c.command {
            Command::List(l) => {
                assert_eq!(l.prefix.as_deref(), Some("foundry/lapis/"));
                assert_eq!(l.status.as_deref(), Some("live"));
                assert_eq!(l.limit, 5);
                assert_eq!(l.offset, 0);
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn parses_write_verbs() {
        let c = Cli::try_parse_from([
            "lapis",
            "create",
            "--title",
            "ship-smoke",
            "--json",
            "--path",
            "inbox/x/",
            "--tag",
            "a",
            "--tag",
            "b",
        ])
        .unwrap();
        match c.command {
            Command::Create(a) => {
                assert_eq!(a.title, "ship-smoke");
                assert_eq!(a.tags, ["a", "b"]);
                assert!(!a.no_reindex);
            }
            _ => panic!("expected create"),
        }
        assert!(Cli::try_parse_from(["lapis", "create"]).is_err());
        let c = Cli::try_parse_from(["lapis", "append", "a.md", "hello"]).unwrap();
        assert!(matches!(c.command, Command::Append(a) if a.text.as_deref() == Some("hello")));
        let c = Cli::try_parse_from(["lapis", "capture", "two", "words"]).unwrap();
        assert!(matches!(c.command, Command::Capture(a) if a.text == ["two", "words"]));
    }

    #[test]
    fn parses_task_and_mcp() {
        let c = Cli::try_parse_from(["lapis", "task", "list", "--status", "open", "--due", "today"]).unwrap();
        assert!(
            matches!(c.command, Command::Task { command: TaskCommand::List(a) } if a.status.as_deref() == Some("open"))
        );
        let c = Cli::try_parse_from(["lapis", "task", "toggle", "a.md#3"]).unwrap();
        assert!(matches!(c.command, Command::Task { command: TaskCommand::Toggle(a) } if a.id == "a.md#3"));
        assert!(matches!(Cli::try_parse_from(["lapis", "mcp"]).unwrap().command, Command::Mcp));
    }

    #[test]
    fn search_requires_query() {
        assert!(Cli::try_parse_from(["lapis", "search"]).is_err());
    }

    #[test]
    fn neighbors_direction_is_validated() {
        assert!(Cli::try_parse_from(["lapis", "neighbors", "a.md", "--direction", "sideways"]).is_err());
        assert!(Cli::try_parse_from(["lapis", "neighbors", "a.md", "--direction", "both"]).is_ok());
    }
}
