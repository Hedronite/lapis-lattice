//! Task-line grammar (SPEC.md § Tasks), ported from zn's behavior, not its Go.
//!
//! IDs are `<path>#<taskIndex>` (zero-based among task lines) or `<path>#task`
//! for a note that is itself the task. Fenced code blocks are skipped. All
//! writes here are line-wise: only the checkbox character changes.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{LapisError, Result};
use crate::hal;
use crate::notes;

static TASK_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*(?:>\s*)*(?:[-+*]|\d+[.)])\s+\[)( |x|X|>|-|/)(\].*)$").unwrap());
static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(```|~~~)").unwrap());
static DUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)due:\s*(\S+)").unwrap());
static PRIORITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)!(high|med|medium|low|h|m|l)\b").unwrap());
static WAITING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)@waiting\b").unwrap());
static FIELD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)@([a-z][a-z0-9_-]*):([\p{L}\d][\p{L}\d/_-]*)").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)#([\p{L}\d][\p{L}\d/_-]*)").unwrap());
static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());
static MULTI_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}").unwrap());

/// Directories never descended into (indexer.py EXCLUDE_DIRS plus dot-dirs).
const EXCLUDE_DIRS: &[&str] = &[
    ".obsidian",
    ".git",
    ".lattice",
    ".lapis",
    ".venv",
    "venv",
    "node_modules",
    "_Operators",
    "External-Refs",
];
/// Vault-relative prefixes never scanned (indexer.py EXCLUDE_PREFIXES + SPEC scan set).
const EXCLUDE_PREFIXES: &[&str] = &[
    "Aeon/notes/media/",
    "Aeon/working/",
    "_archives/",
    ".lapis/trash/",
    "projects/atrium-lattice/data/",
    "projects/karakeep-deployment/data/",
    "projects/karakeep-deployment/meilisearch/",
    "Archmagus-Stack/09-Tomes/",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Line,
    File,
}

/// Task object per `schema/task.schema.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub source_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_title: Option<String>,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_number: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    pub content: String,
    pub checked: bool,
    pub cancelled: bool,
    pub in_progress: bool,
    pub forwarded: bool,
    pub waiting: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    pub status: String,
    pub tags: Vec<String>,
    pub fields: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hal: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    All,
    Note,
    Off,
}

pub fn mode_of(hal: &Map<String, Value>) -> Mode {
    match hal.get("tasks") {
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "note" => Mode::Note,
            "false" | "off" | "no" | "none" => Mode::Off,
            _ => Mode::All,
        },
        Some(Value::Bool(false)) => Mode::Off,
        _ => {
            if hal::tags_from_hal(hal).iter().any(|t| t == "task") {
                Mode::Note
            } else {
                Mode::All
            }
        }
    }
}

fn normalize_priority(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "high" | "h" => "high",
        "med" | "medium" | "m" => "med",
        _ => "low",
    }
}

struct Tokens {
    due: Option<String>,
    priority: Option<String>,
    waiting: bool,
    fields: Map<String, Value>,
    tags: Vec<String>,
    content: String,
}

fn tokens(tail: &str) -> Tokens {
    let mut t = Tokens {
        due: None,
        priority: None,
        waiting: false,
        fields: Map::new(),
        tags: vec![],
        content: String::new(),
    };
    let mut stripped = tail.to_string();
    if let Some(c) = DUE.captures(tail) {
        let v = c[1].to_string();
        if ISO_DATE.is_match(&v) {
            t.due = Some(v);
        }
        stripped = DUE.replace(&stripped, " ").to_string();
    }
    if let Some(c) = PRIORITY.captures(tail) {
        t.priority = Some(normalize_priority(&c[1]).to_string());
        stripped = PRIORITY.replace(&stripped, " ").to_string();
    }
    if WAITING.is_match(tail) {
        t.waiting = true;
        stripped = WAITING.replace_all(&stripped, " ").to_string();
    }
    for c in FIELD.captures_iter(tail) {
        t.fields.insert(c[1].to_ascii_lowercase(), Value::String(c[2].to_string()));
    }
    stripped = FIELD.replace_all(&stripped, " ").to_string();
    for c in TAG.captures_iter(tail) {
        t.tags.push(c[1].to_string());
    }
    stripped = TAG.replace_all(&stripped, " ").to_string();
    t.content = MULTI_SPACE.replace_all(stripped.trim(), " ").to_string();
    t
}

fn status_of(mark: &str, waiting: bool) -> (String, bool, bool, bool, bool) {
    // (status, checked, cancelled, in_progress, forwarded)
    match mark {
        "x" | "X" => ("done".into(), true, false, false, false),
        "-" => ("cancelled".into(), false, true, false, false),
        "/" => ("in-progress".into(), false, false, true, false),
        ">" => ("forwarded".into(), false, false, false, true),
        _ => ((if waiting { "waiting" } else { "open" }).into(), false, false, false, false),
    }
}

/// Parse tasks out of one note. `text` is the full file (frontmatter included).
pub fn parse(rel: &str, text: &str) -> Vec<Task> {
    let p = hal::parse(text);
    let title = hal::title_from_hal(&p.hal).or_else(|| notes::first_heading(&p.body));
    match mode_of(&p.hal) {
        Mode::Off => vec![],
        Mode::Note => {
            let status = p.hal.get("status").and_then(|v| v.as_str()).unwrap_or("open").to_string();
            let checked = matches!(status.as_str(), "done" | "closed" | "complete" | "completed" | "shipped");
            vec![Task {
                id: format!("{rel}#task"),
                source_path: rel.to_string(),
                note_title: title.clone(),
                kind: Kind::File,
                line_number: None,
                task_index: None,
                raw_text: None,
                content: title.unwrap_or_else(|| notes::stem_of(rel)),
                checked,
                cancelled: status == "cancelled",
                in_progress: matches!(status.as_str(), "in-progress" | "active" | "wip"),
                forwarded: false,
                waiting: status == "waiting",
                due: p.hal.get("due").and_then(|v| v.as_str()).map(str::to_string),
                priority: p
                    .hal
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .map(|s| normalize_priority(s).to_string()),
                status,
                tags: hal::tags_from_hal(&p.hal),
                fields: Map::new(),
                hal: Some(p.hal),
            }]
        }
        Mode::All => {
            let mut out = Vec::new();
            for (idx, (line_number, line)) in task_lines(text).into_iter().enumerate() {
                let c = TASK_LINE.captures(line).unwrap();
                let mark = &c[2];
                let tail = c[3].strip_prefix(']').unwrap_or(&c[3]).trim().to_string();
                let tk = tokens(&tail);
                let (status, checked, cancelled, in_progress, forwarded) = status_of(mark, tk.waiting);
                out.push(Task {
                    id: format!("{rel}#{idx}"),
                    source_path: rel.to_string(),
                    note_title: title.clone(),
                    kind: Kind::Line,
                    line_number: Some(line_number),
                    task_index: Some(idx),
                    raw_text: Some(line.to_string()),
                    content: tk.content,
                    checked,
                    cancelled,
                    in_progress,
                    forwarded,
                    waiting: tk.waiting,
                    due: tk.due,
                    priority: tk.priority,
                    status,
                    tags: tk.tags,
                    fields: tk.fields,
                    hal: None,
                });
            }
            out
        }
    }
}

/// (zero-based line number, line) for every checkbox line outside code fences.
fn task_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (n, line) in text.lines().enumerate() {
        if FENCE.is_match(line) {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && TASK_LINE.is_match(line) {
            out.push((n, line));
        }
    }
    out
}

/// Flip one checkbox: open/other → `x`, done → open. Returns the new text and
/// the new `checked` state. `None` when the index does not exist.
pub fn toggle_in_text(text: &str, index: usize) -> Option<(String, bool)> {
    let (line_number, _) = *task_lines(text).get(index)?;
    let mut lines: Vec<&str> = text.split('\n').collect();
    let line = lines[line_number];
    let c = TASK_LINE.captures(line)?;
    let now_checked = !matches!(&c[2], "x" | "X");
    let new_line = format!("{}{}{}", &c[1], if now_checked { "x" } else { " " }, &c[3]);
    lines[line_number] = &new_line;
    Some((lines.join("\n"), now_checked))
}

pub fn split_id(id: &str) -> Result<(String, Option<usize>)> {
    let (rel, idx) = id
        .rsplit_once('#')
        .ok_or_else(|| LapisError::Usage(format!("task id must be path#index or path#task: {id}")))?;
    let rel = notes::clean_rel(rel)?;
    if idx == "task" {
        return Ok((rel, None));
    }
    let n: usize = idx.parse().map_err(|_| LapisError::Usage(format!("bad task index in {id}")))?;
    Ok((rel, Some(n)))
}

/// Toggle a task by id on disk. File tasks (`#task`) flip HAL `status`
/// between `done` and `open` (an allowlisted key), line-wise.
pub fn toggle(root: &Path, id: &str) -> Result<Task> {
    toggle_with(root, id, &crate::write::Guard::default())
}

/// [`toggle`] with a stale guard and dry-run switch (N13).
pub fn toggle_with(root: &Path, id: &str, guard: &crate::write::Guard) -> Result<Task> {
    let (rel, idx) = split_id(id)?;
    let (rel, abs) = notes::resolve(root, &rel)?;
    let raw = std::fs::read(&abs)?;
    guard.check(&rel, &abs, &raw)?;
    let text = String::from_utf8(raw).map_err(|_| LapisError::Usage(format!("{rel}: not UTF-8 text")))?;
    let (next, want_index) = match idx {
        Some(i) => {
            let (next, _) =
                toggle_in_text(&text, i).ok_or_else(|| LapisError::Path(format!("no task #{i} in {rel}")))?;
            (next, i)
        }
        None => {
            let p = hal::parse(&text);
            if mode_of(&p.hal) != Mode::Note {
                return Err(LapisError::Usage(format!("{rel} is not a file task (tasks: note)")));
            }
            let done = matches!(p.hal.get("status").and_then(|v| v.as_str()), Some("done"));
            (crate::write::set_frontmatter_key(&text, "status", if done { "open" } else { "done" }), 0)
        }
    };
    let next = crate::write::set_frontmatter_key(&next, "updated", &crate::write::today());
    if !guard.dry_run {
        std::fs::write(&abs, &next)?;
    }
    let mut tasks = parse(&rel, &next);
    let pos = if idx.is_some() { want_index } else { 0 };
    if pos < tasks.len() {
        Ok(tasks.swap_remove(pos))
    } else {
        Err(LapisError::Internal("toggle lost the task".into()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub status: Option<String>,
    /// `today`, `overdue`, or `YYYY-MM-DD`.
    pub due: Option<String>,
    pub tag: Option<String>,
    pub prefix: Option<String>,
    /// Vault-relative prefixes to skip (from `[agent].task_exclude`), e.g. `assets/`.
    pub exclude: Vec<String>,
}

impl Filter {
    /// True when `rel` sits under one of the configured exclude prefixes
    /// (`assets/` and `assets` both mean the folder).
    pub fn excluded(&self, rel: &str) -> bool {
        self.exclude.iter().any(|p| {
            let p = p.trim_start_matches("./").trim_end_matches('/');
            !p.is_empty() && rel.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))
        })
    }

    pub fn matches(&self, t: &Task, today: &str) -> bool {
        if let Some(s) = &self.status
            && t.status != *s
        {
            return false;
        }
        if let Some(tag) = &self.tag
            && !t.tags.iter().any(|x| x == tag)
        {
            return false;
        }
        if let Some(d) = &self.due {
            let Some(due) = &t.due else { return false };
            let ok = match d.as_str() {
                "today" => due == today,
                "overdue" => due.as_str() < today && !t.checked && !t.cancelled,
                other => due == other,
            };
            if !ok {
                return false;
            }
        }
        true
    }
}

/// True when the scan set skips this entry (also used by the TUI sidebar and watcher).
pub fn excluded(rel: &str, is_dir: bool, name: &str) -> bool {
    if name.starts_with('.') || EXCLUDE_DIRS.contains(&name) {
        return true;
    }
    let probe = if is_dir { format!("{rel}/") } else { rel.to_string() };
    EXCLUDE_PREFIXES.iter().any(|p| probe.starts_with(p))
}

/// Walk `.md` files under `root/prefix` honoring the scan set. Never descends
/// into excluded dirs, so media trees are never listed.
pub fn scan_files(root: &Path, prefix: Option<&str>) -> Result<Vec<String>> {
    let start_rel = match prefix {
        Some(p) => notes::clean_rel(p)?,
        None => String::new(),
    };
    let start = if start_rel.is_empty() { root.to_path_buf() } else { root.join(&start_rel) };
    if start.is_file() {
        return Ok(vec![start_rel]);
    }
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(start, start_rel)];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child_rel = if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") };
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if !excluded(&child_rel, true, &name) {
                    stack.push((entry.path(), child_rel));
                }
            } else if ft.is_file() && name.ends_with(".md") && !excluded(&child_rel, false, &name) {
                out.push(child_rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Counts instead of rows: what an unscoped `task list` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub n: usize,
    /// `open`, `done`, `in-progress`, … → count.
    pub by_status: std::collections::BTreeMap<String, usize>,
    /// First path segment (`foundry`, `agents`, …; `.` for root notes) → count.
    pub by_folder: std::collections::BTreeMap<String, usize>,
}

/// Top-level folder of a vault-relative path; `.` for a root-level note.
pub fn folder_of(rel: &str) -> &str {
    match rel.split_once('/') {
        Some((head, _)) => head,
        None => ".",
    }
}

pub fn summarize(tasks: &[Task]) -> Summary {
    let mut s = Summary { n: tasks.len(), ..Default::default() };
    for t in tasks {
        *s.by_status.entry(t.status.clone()).or_default() += 1;
        *s.by_folder.entry(folder_of(&t.source_path).to_string()).or_default() += 1;
    }
    s
}

/// Unscoped and not `--full` → summary. A PATH scope always means rows.
pub fn wants_summary(scope: Option<&str>, full: bool) -> bool {
    scope.is_none_or(|s| s.trim().is_empty()) && !full
}

pub fn list(root: &Path, filter: &Filter) -> Result<Vec<Task>> {
    let today = crate::write::today();
    let mut out = Vec::new();
    for rel in scan_files(root, filter.prefix.as_deref())? {
        if filter.excluded(&rel) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(&rel)) else { continue };
        out.extend(parse(&rel, &text).into_iter().filter(|t| filter.matches(t, &today)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The due dates here are deliberately far from any real "today". A fixture
    /// dated the day it was written passes until the clock rolls over: this one
    /// failed on a UTC runner the first midnight after it was added, because a
    /// task due "today" became overdue while the same suite passed locally.
    const NOTE: &str = "---\nname: Plan\ntags: [x]\n---\n# Plan\n\n- [ ] Write spec due:2099-12-31 !high #lapis @owner:fable\n- [x] Old one\n> - [/] quoted in progress @waiting\n1. [-] cancelled numbered\n```\n- [ ] not a task (fenced)\n```\n- [>] forwarded @waiting\n* plain bullet\n";

    #[test]
    fn parses_lines_with_tokens_and_skips_fences() {
        let t = parse("a/plan.md", NOTE);
        assert_eq!(t.len(), 5);
        let first = &t[0];
        assert_eq!(first.id, "a/plan.md#0");
        assert_eq!(first.content, "Write spec");
        assert_eq!(first.due.as_deref(), Some("2099-12-31"));
        assert_eq!(first.priority.as_deref(), Some("high"));
        assert_eq!(first.tags, ["lapis"]);
        assert_eq!(first.fields["owner"], "fable");
        assert_eq!(first.status, "open");
        assert_eq!(first.line_number, Some(6));
        assert_eq!(first.note_title.as_deref(), Some("Plan"));
        assert!(t[1].checked && t[1].status == "done");
        assert!(t[2].in_progress && t[2].waiting);
        assert!(t[3].cancelled);
        assert!(t[4].forwarded);
        assert_eq!(t[4].id, "a/plan.md#4");
    }

    #[test]
    fn toggle_flips_only_the_checkbox() {
        let (next, checked) = toggle_in_text(NOTE, 0).unwrap();
        assert!(checked);
        assert!(next.contains("- [x] Write spec due:2099-12-31 !high #lapis @owner:fable"));
        let (back, checked) = toggle_in_text(&next, 0).unwrap();
        assert!(!checked);
        assert_eq!(back, NOTE);
        let (n, c) = toggle_in_text(NOTE, 1).unwrap();
        assert!(!c && n.contains("- [ ] Old one"));
        let (_, c) = toggle_in_text(NOTE, 2).unwrap();
        assert!(c, "in-progress toggles to done");
        assert!(toggle_in_text(NOTE, 5).is_none());
    }

    #[test]
    fn modes_note_and_off() {
        let t =
            parse("t.md", "---\nname: Ship it\ntasks: note\nstatus: done\npriority: h\n---\n- [ ] ignored\n");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].id, "t.md#task");
        assert_eq!(t[0].kind, Kind::File);
        assert!(t[0].checked);
        assert_eq!(t[0].priority.as_deref(), Some("high"));
        assert!(t[0].hal.is_some());
        assert!(parse("t.md", "---\ntasks: off\n---\n- [ ] ignored\n").is_empty());
        assert!(parse("t.md", "---\ntasks: false\n---\n- [ ] ignored\n").is_empty());
        assert_eq!(parse("t.md", "---\ntags: [task]\n---\n- [ ] ignored\n")[0].kind, Kind::File);
    }

    #[test]
    fn ids_split() {
        assert_eq!(split_id("a/b.md#3").unwrap(), ("a/b.md".into(), Some(3)));
        assert_eq!(split_id("a/b.md#task").unwrap(), ("a/b.md".into(), None));
        assert!(split_id("a/b.md").is_err());
        assert_eq!(split_id("../b.md#1").unwrap_err().exit_code(), 3);
    }

    #[test]
    fn summary_counts_by_status_and_folder() {
        let mut ts = parse("notes/plan.md", NOTE);
        ts.extend(parse("agents/FLEET.md", "- [ ] a\n- [x] b\n- [x] c\n"));
        ts.extend(parse("ROOT.md", "- [ ] root task\n"));
        let s = summarize(&ts);
        assert_eq!(s.n, ts.len());
        // NOTE: open, done, in-progress, cancelled, forwarded (fenced one skipped)
        assert_eq!(s.by_folder["notes"], 5);
        assert_eq!(s.by_folder["agents"], 3);
        assert_eq!(s.by_folder["."], 1);
        assert_eq!(s.by_status["done"], 1 + 2);
        assert_eq!(s.by_status["open"], 1 + 1 + 1);
        assert_eq!(s.by_status["in-progress"], 1);
        assert_eq!(folder_of("a/b/c.md"), "a");
        assert_eq!(folder_of("c.md"), ".");
        let j = serde_json::to_value(&s).unwrap();
        assert!(j.get("byStatus").is_some() && j.get("byFolder").is_some() && j["n"] == ts.len());
    }

    #[test]
    fn toggle_on_disk_and_scan_respects_exclusions() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        // Unique per call: pid alone repeats across parallel tests in one
        // binary, and the clock is coarser than a nanosecond.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let v = std::env::temp_dir().join(format!("lapis-tasks-{}-{n}-{seq}", std::process::id()));
        for d in ["notes", "Aeon/notes/media", "_archives", ".lapis/trash", ".obsidian", "inbox"] {
            std::fs::create_dir_all(v.join(d)).unwrap();
        }
        std::fs::write(v.join("notes/plan.md"), NOTE).unwrap();
        std::fs::write(v.join("inbox/q.md"), "- [ ] quick one due:2020-01-01\n").unwrap();
        std::fs::write(v.join("Aeon/notes/media/x.md"), "- [ ] hidden\n").unwrap();
        std::fs::write(v.join("_archives/x.md"), "- [ ] hidden\n").unwrap();
        std::fs::write(v.join(".lapis/trash/x.md"), "- [ ] hidden\n").unwrap();
        std::fs::write(v.join(".obsidian/x.md"), "- [ ] hidden\n").unwrap();
        std::fs::write(v.join("notes/file.md"), "---\nname: F\ntasks: note\n---\nbody\n").unwrap();

        assert_eq!(scan_files(&v, None).unwrap(), ["inbox/q.md", "notes/file.md", "notes/plan.md"]);
        assert_eq!(scan_files(&v, Some("inbox")).unwrap(), ["inbox/q.md"]);
        let all = list(&v, &Filter::default()).unwrap();
        assert_eq!(all.len(), 7);
        // N4: unscoped → summary; scoped or --full → rows
        assert!(wants_summary(None, false));
        assert!(wants_summary(Some(""), false));
        assert!(!wants_summary(Some("notes"), false));
        assert!(!wants_summary(None, true));
        let s = summarize(&all);
        assert_eq!(s.n, 7);
        assert_eq!(s.by_folder.get("notes"), Some(&6));
        assert_eq!(s.by_folder.get("inbox"), Some(&1));
        assert_eq!(s.by_status.values().sum::<usize>(), 7);
        let scoped = list(&v, &Filter { prefix: Some("notes".into()), ..Default::default() }).unwrap();
        assert_eq!(scoped.len(), 6);
        assert!(scoped.iter().all(|t| t.source_path.starts_with("notes/")));
        // N22: configured exclude prefixes are honoured (with or without a trailing slash)
        let ex = list(&v, &Filter { exclude: vec!["notes/".into()], ..Default::default() }).unwrap();
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].source_path, "inbox/q.md");
        let f = Filter { exclude: vec!["inbox".into()], ..Default::default() };
        assert!(f.excluded("inbox/q.md") && !f.excluded("inboxes/q.md") && !f.excluded("notes/x.md"));
        assert_eq!(list(&v, &f).unwrap().len(), 6);
        let open = list(&v, &Filter { status: Some("open".into()), ..Default::default() }).unwrap();
        assert_eq!(open.len(), 3);
        // Only the 2020 task is past due, whatever day this runs on.
        let overdue = list(&v, &Filter { due: Some("overdue".into()), ..Default::default() }).unwrap();
        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].id, "inbox/q.md#0");
        // And "today" is computed, never a date baked into a fixture.
        let today_due = list(&v, &Filter { due: Some("today".into()), ..Default::default() }).unwrap();
        assert!(today_due.is_empty(), "no fixture task is due on the day the suite happens to run");

        let t = toggle(&v, "notes/plan.md#0").unwrap();
        assert!(t.checked);
        let on_disk = std::fs::read_to_string(v.join("notes/plan.md")).unwrap();
        assert!(on_disk.contains("- [x] Write spec"));
        assert!(on_disk.contains(&format!("updated: {}", crate::write::today())));
        assert!(on_disk.contains("- [ ] not a task (fenced)"));
        let f = toggle(&v, "notes/file.md#task").unwrap();
        assert!(f.checked && f.status == "done");
        let f = toggle(&v, "notes/file.md#task").unwrap();
        assert!(!f.checked);
        assert_eq!(toggle(&v, "notes/plan.md#99").unwrap_err().exit_code(), 3);
        assert_eq!(toggle(&v, "notes/plan.md#task").unwrap_err().exit_code(), 1);
        // N13: dry run reports the flipped task without touching the file
        let before = std::fs::read_to_string(v.join("notes/plan.md")).unwrap();
        let dry =
            toggle_with(&v, "notes/plan.md#0", &crate::write::Guard { dry_run: true, ..Default::default() })
                .unwrap();
        assert!(!dry.checked, "was toggled to done above; dry run flips back in the report only");
        assert_eq!(std::fs::read_to_string(v.join("notes/plan.md")).unwrap(), before);
        let stale = crate::write::Guard { if_hash: Some("fnv1a64:0".into()), ..Default::default() };
        assert_eq!(toggle_with(&v, "notes/plan.md#0", &stale).unwrap_err().exit_code(), 1);
        let _ = std::fs::remove_dir_all(&v);
    }
}
