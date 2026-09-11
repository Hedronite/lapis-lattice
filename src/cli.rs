//! clap surface. SPEC.md § CLI: `lapis <command> [--json] [--vault <path>]`.

use clap::{Args, Parser, Subcommand};

use crate::lattice::Mode;

const AFTER_HELP: &str = "\
Exit codes:
  0  ok
  1  usage / user error
  2  lattice / index down (HTTP unreachable, timeout, 5xx, or the embedded sqlite index failed)
  3  path escape or not found

Vault: --vault, $LAPIS_VAULT, or config `vault`. No implicit default; `lapis init ~/Notes` creates a vault, indexes it, and records the path.
Lattice: embedded sqlite at `<vault>/.lapis/lattice.sqlite` by default (no daemon). `--lattice`, `$LAPIS_LATTICE_URL`, or `lattice.mode = \"http\"` opt into HTTP (URL default http://127.0.0.1:8080).
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

    /// Omitted → the TUI, so a bare `lapis` opens the vault.
    #[command(subcommand)]
    pub subcommand: Option<Command>,
}

impl Cli {
    /// The subcommand to run; a bare `lapis` is `lapis tui`.
    pub fn command(self) -> Command {
        self.subcommand.unwrap_or(Command::Tui)
    }
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

    /// Search the vault index (BM25; vectors only if an embedder is configured).
    Search(SearchArgs),

    /// Filename / path inventory from the index path table. Never walks the vault.
    PathsSearch(PathsSearchArgs),

    /// Read one note: body plus parsed HAL frontmatter.
    Read(ReadArgs),

    /// Hop-1 wikilink neighbors of a note from the lattice `edges` table.
    Neighbors(NeighborsArgs),

    /// Resolve a wikilink (`[[Name|alias#anchor]]`) or lattice `dst_raw` to a vault path.
    Resolve(ResolveArgs),

    /// Named read-only analytics over the index: inventory, priority, tags, health, recent, hubs, density, degree, dangling.
    Analytics(AnalyticsArgs),

    /// Hub-routed link walk from a seed note (or the nearest Cross-References hub for --query).
    TreeRetrieve(TreeArgs),

    /// List notes from the lattice `documents` table (metadata only). Never walks the vault.
    List(ListArgs),

    /// Re-index one note in the embedded sqlite index (or the HTTP lattice, if opted in).
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

    /// Open or create today's `Daily/YYYY-MM-DD.md` (HAL create-set, doc_type daily-note).
    Daily(DailyArgs),

    /// Open or create this week's `Weekly/YYYY-Www.md`.
    Weekly(DailyArgs),

    /// Open or create this month's `Monthly/YYYY-MM.md`.
    Monthly(DailyArgs),

    /// Restore a trashed note (path under `.lapis/trash/`) to where it came from.
    Restore(TrashArgs),

    /// Templates: built-ins (`builtin.daily`, `builtin.adr`, …) and `.lapis/templates/`.
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },

    /// Move a note to the trash bucket (`.lapis/trash/`), keeping its path for restore.
    Trash(TrashArgs),

    /// Check this install: vault, index, embedder, search, path sandbox. Exit non-zero if a check fails.
    Doctor,

    /// Terminal UI: sidebar tree, Vim editor, preview, tasks. `?` for keys.
    Tui,

    /// Create a vault, build its embedded index, and record the path in the config.
    Init(InitArgs),

    /// Desktop shell (GPUI). Needs a build with `--features desktop`; `--check` reports what is available.
    Desktop(DesktopArgs),

    /// MCP server over stdio (tools: vault_info, search, read_note, list_notes, neighbors, create_note, append_to_note, list_tasks, toggle_task).
    Mcp,

    /// Serve the operator HTTP API (`/v1`). Default 127.0.0.1:18765. Loopback needs no token.
    Api(ApiArgs),
}

#[derive(Debug, Args)]
pub struct PathsSearchArgs {
    /// Basename, glob, or substring to match against indexed paths.
    #[arg(value_name = "PATTERN")]
    pub pattern: String,

    /// `basename` (default), `glob`, or `substring`.
    #[arg(long = "match", value_enum, default_value_t = PathMatchArg::Basename)]
    pub match_kind: PathMatchArg,

    /// Max rows (1..1000).
    #[arg(long, short = 'n', default_value_t = 50, value_name = "N")]
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum PathMatchArg {
    #[default]
    Basename,
    Glob,
    Substring,
}

#[derive(Debug, Args)]
pub struct ApiArgs {
    /// Bind address. Default config `api.bind` (`127.0.0.1`). Non-loopback needs LAPIS_API_TOKEN.
    #[arg(long, value_name = "ADDR")]
    pub bind: Option<String>,

    /// Port. Default config `api.port` (`18765`).
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,
}

#[derive(Debug, Args)]
pub struct DailyArgs {
    /// Date `YYYY-MM-DD` (default today).
    #[arg(long, value_name = "DATE")]
    pub date: Option<String>,

    /// Skip the lattice reindex kick after creating.
    #[arg(long)]
    pub no_reindex: bool,
}

#[derive(Debug, Args)]
pub struct TrashArgs {
    /// Vault-relative note path.
    #[arg(value_name = "PATH")]
    pub path: String,
}

#[derive(Debug, Subcommand)]
pub enum TemplateCommand {
    /// List available templates.
    List,
    /// Print a template's raw text.
    Show {
        #[arg(value_name = "ID")]
        id: String,
    },
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

    /// Same as the positional PATH.
    #[arg(long = "path", value_name = "PATH", conflicts_with = "path")]
    pub path_flag: Option<String>,

    /// Unscoped runs print a summary (counts by status and folder); `--full` prints every row.
    #[arg(long)]
    pub full: bool,

    /// Max rows per page when listing rows (0 = all).
    #[arg(long, short = 'n', default_value_t = 500, value_name = "N")]
    pub limit: u32,

    /// Row offset for paging.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub offset: u32,

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

impl TaskListArgs {
    /// Scope from the positional or `--path`.
    pub fn scope(&self) -> Option<String> {
        self.path.clone().or_else(|| self.path_flag.clone())
    }
}

#[derive(Debug, Args)]
pub struct TaskToggleArgs {
    /// Task id from `task list`, e.g. `notes/plan.md#0`.
    #[arg(value_name = "ID")]
    pub id: String,

    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,

    #[command(flatten)]
    pub guard: GuardArgs,
}

/// `--dry-run` / stale guards shared by append and task toggle.
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
pub struct CreateArgs {
    /// Note title; becomes HAL `name` and the slugged filename.
    #[arg(long, value_name = "TITLE")]
    pub title: String,

    /// Vault-relative target: a folder (`notes/`) or a file (`notes/x.md`). Default: inbox bucket.
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

    /// Optional template placeholder (default: config operator).
    #[arg(long, value_name = "NAME")]
    pub director: Option<String>,

    /// Read the body from stdin.
    #[arg(long)]
    pub stdin: bool,

    /// Skip the lattice reindex kick after writing.
    #[arg(long)]
    pub no_reindex: bool,

    /// Compute path, HAL and text; write nothing.
    #[arg(long)]
    pub dry_run: bool,
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

    #[command(flatten)]
    pub guard: GuardArgs,
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
    /// Vault-relative markdown path, e.g. `notes/STATUS.md`.
    #[arg(value_name = "PATH")]
    pub path: String,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Only paths under this vault-relative prefix, e.g. `notes/`.
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

    /// Skip this many hits (offset + limit must stay ≤ 50).
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub offset: u32,

    /// Filter by HAL `domain`.
    #[arg(long, value_name = "DOMAIN")]
    pub domain: Option<String>,

    /// Retrieval mode.
    #[arg(long, value_enum, default_value_t = Mode::Hybrid)]
    pub mode: Mode,

    /// Collapse to the best chunk per document.
    #[arg(long)]
    pub per_doc: bool,

    /// Agent defaults: one hit per document (per_doc). Same as the MCP `search` tool.
    #[arg(long)]
    pub agent: bool,

    /// MMR diversity re-ranking.
    #[arg(long)]
    pub mmr: bool,

    /// Include `_archives/` and `*.DRAFT-*` documents.
    #[arg(long)]
    pub include_archives: bool,

    /// Override the vault embedder for this query. `none` = BM25 only: no vector
    /// arm and no connect to Ollama. `auto`/`ollama`/`onnx` use the index as stored
    /// (`auto` is resolved at open, not per search).
    #[arg(long, value_name = "PROVIDER", value_parser = ["none", "auto", "ollama", "onnx"])]
    pub embedder: Option<String>,
}

impl SearchArgs {
    pub fn query_text(&self) -> String {
        self.query.join(" ")
    }
    /// `--per-doc`, or `--agent` with the `[agent].per_doc` profile default.
    pub fn effective_per_doc(&self, agent_default: bool) -> bool {
        self.per_doc || (self.agent && agent_default)
    }
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    /// Vault-relative path, e.g. `notes/SPEC.md` (`.md` may be omitted).
    #[arg(value_name = "PATH")]
    pub path: String,

    /// Print only the parsed frontmatter (JSON), not the body.
    #[arg(long, conflicts_with = "body")]
    pub meta: bool,

    /// Print only the body, without frontmatter.
    #[arg(long)]
    pub body: bool,

    /// Only the section under this heading (case-insensitive, nested sub-sections included).
    #[arg(long, value_name = "H", conflicts_with = "chunk")]
    pub heading: Option<String>,

    /// Only the N-th heading section (0 = preamble / first section), local outline order.
    #[arg(long, value_name = "N")]
    pub chunk: Option<usize>,

    /// Clip the body to this many chars; `meta.truncated` says when it happened.
    #[arg(long, value_name = "N")]
    pub max_chars: Option<usize>,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Directory to create. Default: `~/Notes`.
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
}

#[derive(Debug, Args)]
pub struct DesktopArgs {
    /// Note to mark as active and to seed local mode with. The graph itself is
    /// the whole indexed vault either way.
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,

    /// Report whether the GPUI shell and the GitNexus sidecar are available, then exit.
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// `[[Name]]`, `[[Name|alias#anchor]]`, or a raw link target such as a lattice `dst_raw`.
    #[arg(value_name = "LINK")]
    pub link: String,
}

#[derive(Debug, Args)]
pub struct NeighborsArgs {
    /// Vault-relative path of the note.
    #[arg(value_name = "PATH")]
    pub path: String,

    /// Edge direction: out, in, or both (default from `[agent].neighbors_direction`, else both).
    #[arg(long, value_parser = ["out", "in", "both"])]
    pub direction: Option<String>,

    /// Include dangling (unresolved) links too.
    #[arg(long)]
    pub dangling: bool,

    /// 1 = direct neighbors (edges table); 2 = ego graph with depth / via per row.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=2))]
    pub hop: u32,
}

#[derive(Debug, Args)]
pub struct AnalyticsArgs {
    /// One of: inventory, priority, tags, health, recent, hubs, density, degree, dangling.
    #[arg(value_name = "QUERY")]
    pub query: String,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    /// Seed note (vault-relative). Omit to start at the hub nearest to --query.
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,

    /// Rank children by nomic similarity to this text; also picks the seed hub when --path is omitted.
    #[arg(long, value_name = "TEXT")]
    pub query: Option<String>,

    /// Walk depth (1..3).
    #[arg(long, default_value_t = 2, value_name = "N")]
    pub depth: u32,

    /// Node cap (1..200); `truncated` says when it was hit.
    #[arg(long, default_value_t = 60, value_name = "N")]
    pub max_nodes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_with_globals_anywhere() {
        let c = Cli::try_parse_from(["lapis", "search", "--json", "welcome", "--vault", "/tmp"]).unwrap();
        assert!(c.global.json);
        assert_eq!(c.global.vault.as_deref(), Some("/tmp"));
        match c.command() {
            Command::Search(s) => {
                assert_eq!(s.query_text(), "welcome");
                assert_eq!(s.limit, 10);
                assert_eq!(s.mode, Mode::Hybrid);
            }
            _ => panic!("expected search"),
        }
        let c =
            Cli::try_parse_from(["lapis", "--json", "search", "two", "words", "--mode", "bm25", "-n", "3"])
                .unwrap();
        match c.command() {
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
        assert!(c.global.json);
        assert!(matches!(c.command(), Command::Vault { command: VaultCommand::Info }));
        let c = Cli::try_parse_from(["lapis", "read", "notes/SPEC.md"]).unwrap();
        match c.command() {
            Command::Read(r) => assert_eq!(r.path, "notes/SPEC.md"),
            _ => panic!("expected read"),
        }
    }

    #[test]
    fn parses_list() {
        let c = Cli::try_parse_from(["lapis", "list", "--json", "notes/", "--status", "live", "-n", "5"])
            .unwrap();
        match c.command() {
            Command::List(l) => {
                assert_eq!(l.prefix.as_deref(), Some("notes/"));
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
        match c.command() {
            Command::Create(a) => {
                assert_eq!(a.title, "ship-smoke");
                assert_eq!(a.tags, ["a", "b"]);
                assert!(!a.no_reindex);
            }
            _ => panic!("expected create"),
        }
        assert!(Cli::try_parse_from(["lapis", "create"]).is_err());
        let c = Cli::try_parse_from(["lapis", "append", "a.md", "hello"]).unwrap();
        assert!(matches!(c.command(), Command::Append(a) if a.text.as_deref() == Some("hello")));
        let c = Cli::try_parse_from(["lapis", "capture", "two", "words"]).unwrap();
        assert!(matches!(c.command(), Command::Capture(a) if a.text == ["two", "words"]));
    }

    #[test]
    fn parses_task_and_mcp() {
        let c = Cli::try_parse_from(["lapis", "task", "list", "--status", "open", "--due", "today"]).unwrap();
        assert!(
            matches!(c.command(), Command::Task { command: TaskCommand::List(a) } if a.status.as_deref() == Some("open"))
        );
        let c = Cli::try_parse_from(["lapis", "task", "toggle", "a.md#3"]).unwrap();
        assert!(matches!(c.command(), Command::Task { command: TaskCommand::Toggle(a) } if a.id == "a.md#3"));
        assert!(matches!(Cli::try_parse_from(["lapis", "mcp"]).unwrap().command(), Command::Mcp));
        assert!(matches!(Cli::try_parse_from(["lapis", "tui"]).unwrap().command(), Command::Tui));
        let c = Cli::try_parse_from(["lapis", "paths-search", "AGENTS.md", "--match", "basename"]).unwrap();
        match c.command() {
            Command::PathsSearch(a) => {
                assert_eq!(a.pattern, "AGENTS.md");
                assert!(matches!(a.match_kind, PathMatchArg::Basename));
            }
            _ => panic!("expected paths-search"),
        }
        let c = Cli::try_parse_from(["lapis", "api", "--port", "18765"]).unwrap();
        match c.command() {
            Command::Api(a) => {
                assert_eq!(a.port, Some(18765));
                assert!(a.bind.is_none());
            }
            _ => panic!("expected api"),
        }
        // bare `lapis` (and bare `lapis --vault …`) default to the TUI
        let c = Cli::try_parse_from(["lapis"]).unwrap();
        assert!(c.subcommand.is_none());
        assert!(matches!(c.command(), Command::Tui));
        let c = Cli::try_parse_from(["lapis", "--vault", "/v"]).unwrap();
        assert_eq!(c.global.vault.as_deref(), Some("/v"));
        assert!(matches!(c.command(), Command::Tui));
        assert!(
            matches!(Cli::try_parse_from(["lapis", "daily", "--date", "2026-09-09"]).unwrap().command(), Command::Daily(d) if d.date.as_deref() == Some("2026-09-09"))
        );
        assert!(
            matches!(Cli::try_parse_from(["lapis", "trash", "a/b.md"]).unwrap().command(), Command::Trash(t) if t.path == "a/b.md")
        );
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

    /// N2: hop-1 defaults to both directions; `--direction` still narrows.
    #[test]
    fn neighbors_default_direction_is_both() {
        let c = Cli::try_parse_from(["lapis", "neighbors", "a.md"]).unwrap();
        let Command::Neighbors(n) = c.command() else { panic!("neighbors") };
        assert_eq!(n.direction, None, "unset → config default, which is both");
        assert!(!n.dangling);
        let c =
            Cli::try_parse_from(["lapis", "neighbors", "a.md", "--direction", "in", "--dangling"]).unwrap();
        let Command::Neighbors(n) = c.command() else { panic!("neighbors") };
        assert_eq!(n.direction.as_deref(), Some("in"));
        assert!(n.dangling);
    }

    #[test]
    fn init_subcommand() {
        let Command::Init(i) = Cli::try_parse_from(["lapis", "init"]).unwrap().command() else {
            panic!("init")
        };
        assert!(i.path.is_none());
        let Command::Init(i) = Cli::try_parse_from(["lapis", "init", "~/Notes"]).unwrap().command() else {
            panic!("init path")
        };
        assert_eq!(i.path.as_deref(), Some("~/Notes"));
    }

    /// B6: `lapis doctor` parses with no arguments.
    #[test]
    fn doctor_subcommand() {
        assert!(matches!(Cli::try_parse_from(["lapis", "doctor"]).unwrap().command(), Command::Doctor));
    }

    /// N23: `lapis desktop` parses with and without a build that has GPUI.
    #[test]
    fn desktop_subcommand() {
        let Command::Desktop(d) = Cli::try_parse_from(["lapis", "desktop"]).unwrap().command() else {
            panic!()
        };
        assert!(d.path.is_none() && !d.check);
        let Command::Desktop(d) =
            Cli::try_parse_from(["lapis", "desktop", "--check", "--path", "a.md"]).unwrap().command()
        else {
            panic!()
        };
        assert!(d.check && d.path.as_deref() == Some("a.md"));
    }

    /// N17–N19: hop, analytics and tree-retrieve surfaces.
    #[test]
    fn slice3_flags() {
        let Command::Neighbors(n) = Cli::try_parse_from(["lapis", "neighbors", "a.md"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!(n.hop, 1);
        let Command::Neighbors(n) =
            Cli::try_parse_from(["lapis", "neighbors", "a.md", "--hop", "2"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!(n.hop, 2);
        assert!(Cli::try_parse_from(["lapis", "neighbors", "a.md", "--hop", "3"]).is_err());
        assert!(Cli::try_parse_from(["lapis", "neighbors", "a.md", "--hop", "0"]).is_err());
        let Command::Analytics(a) = Cli::try_parse_from(["lapis", "analytics", "degree"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!(a.query, "degree");
        assert!(Cli::try_parse_from(["lapis", "analytics"]).is_err());
        let Command::TreeRetrieve(t) = Cli::try_parse_from([
            "lapis",
            "tree-retrieve",
            "--json",
            "--path",
            "P.md",
            "--query",
            "Q",
            "--depth",
            "3",
            "--max-nodes",
            "9",
        ])
        .unwrap()
        .command() else {
            panic!()
        };
        assert_eq!(
            (t.path.as_deref(), t.query.as_deref(), t.depth, t.max_nodes),
            (Some("P.md"), Some("Q"), 3, 9)
        );
        let Command::TreeRetrieve(t) = Cli::try_parse_from(["lapis", "tree-retrieve"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!((t.depth, t.max_nodes), (2, 60));
    }

    #[test]
    fn search_embedder_none_parses() {
        let Command::Search(s) =
            Cli::try_parse_from(["lapis", "search", "welcome", "--embedder", "none"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!(s.embedder.as_deref(), Some("none"));
        assert!(Cli::try_parse_from(["lapis", "search", "welcome", "--embedder", "bogus"]).is_err());
        let Command::Search(s) =
            Cli::try_parse_from(["lapis", "--json", "search", "welcome", "--embedder", "none"])
                .unwrap()
                .command()
        else {
            panic!()
        };
        assert_eq!(s.embedder.as_deref(), Some("none"));
    }

    /// N8 / N11 / N12 / N13: new flags parse and defaults hold.
    #[test]
    fn slice2_flags() {
        let Command::Search(s) =
            Cli::try_parse_from(["lapis", "search", "q", "--offset", "10", "-n", "5"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!((s.limit, s.offset), (5, 10));
        let Command::Read(r) =
            Cli::try_parse_from(["lapis", "read", "a.md", "--heading", "Goals", "--max-chars", "300"])
                .unwrap()
                .command()
        else {
            panic!()
        };
        assert_eq!((r.heading.as_deref(), r.chunk, r.max_chars), (Some("Goals"), None, Some(300)));
        assert!(Cli::try_parse_from(["lapis", "read", "a.md", "--heading", "x", "--chunk", "1"]).is_err());
        let Command::Resolve(x) = Cli::try_parse_from(["lapis", "resolve", "[[A|b#c]]"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!(x.link, "[[A|b#c]]");
        let Command::Append(a) = Cli::try_parse_from([
            "lapis",
            "append",
            "a.md",
            "t",
            "--dry-run",
            "--if-hash",
            "fnv1a64:0",
            "--if-mtime",
            "7",
        ])
        .unwrap()
        .command() else {
            panic!()
        };
        let g = a.guard.guard();
        assert!(g.dry_run);
        assert_eq!((g.if_mtime, g.if_hash.as_deref()), (Some(7), Some("fnv1a64:0")));
        let Command::Create(c) =
            Cli::try_parse_from(["lapis", "create", "--title", "T", "--dry-run"]).unwrap().command()
        else {
            panic!()
        };
        assert!(c.dry_run);
        let Command::Task { command: TaskCommand::Toggle(t) } =
            Cli::try_parse_from(["lapis", "task", "toggle", "a.md#0", "--dry-run"]).unwrap().command()
        else {
            panic!()
        };
        assert!(t.guard.dry_run && t.guard.if_mtime.is_none());
        let Command::Task { command: TaskCommand::List(l) } =
            Cli::try_parse_from(["lapis", "task", "list", "x", "--offset", "500"]).unwrap().command()
        else {
            panic!()
        };
        assert_eq!((l.limit, l.offset), (500, 500));
    }

    /// N3: `--agent` implies per_doc; `--per-doc` alone still works.
    #[test]
    fn search_agent_implies_per_doc() {
        let parse = |a: &[&str]| match Cli::try_parse_from(a).unwrap().command() {
            Command::Search(s) => s,
            _ => panic!("search"),
        };
        assert!(!parse(&["lapis", "search", "q"]).effective_per_doc(true));
        assert!(parse(&["lapis", "search", "--per-doc", "q"]).effective_per_doc(false));
        let s = parse(&["lapis", "search", "--agent", "q"]);
        assert!(s.agent && !s.per_doc && s.effective_per_doc(true));
        assert!(!s.effective_per_doc(false), "[agent] per_doc=false turns it off");
    }

    /// N4: scope comes from the positional or `--path`; both plus `--full` parse.
    #[test]
    fn task_list_scope_positional_or_flag() {
        let list = |a: &[&str]| match Cli::try_parse_from(a).unwrap().command() {
            Command::Task { command: TaskCommand::List(l) } => l,
            _ => panic!("task list"),
        };
        let l = list(&["lapis", "task", "list", "--json"]);
        assert_eq!(l.scope(), None);
        assert!(!l.full);
        assert_eq!(list(&["lapis", "task", "list", "notes"]).scope().as_deref(), Some("notes"));
        assert_eq!(
            list(&["lapis", "task", "list", "--json", "--path", "notes"]).scope().as_deref(),
            Some("notes")
        );
        assert!(list(&["lapis", "task", "list", "--full"]).full);
        assert!(Cli::try_parse_from(["lapis", "task", "list", "a", "--path", "b"]).is_err());
    }
}
