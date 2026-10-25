#!/usr/bin/env nu
# Deterministic body for the lapis skill. Wraps `lapis --json`.
# Vault: --vault, else $LAPIS_VAULT, else the path `lapis init` recorded.
#
# Flags come before positionals. A `--` ends flags, and string values travel
# as `--flag=value`, so text that starts with `-` (a task line, a body) is
# data. On failure the process exits with the envelope's `exit` code.
#
# CLI each command actually runs:
#   search        lapis search --per-doc --limit=N [--domain=D] -- QUERY
#   read          lapis read -- PATH
#   list          lapis list --limit=N [-- PREFIX]
#   neighbors     lapis neighbors -- PATH
#   resolve       lapis resolve -- LINK
#   recent        lapis analytics recent
#   analytics     lapis analytics -- QUERY
#   tree-retrieve lapis tree-retrieve [--path=P] [--query=Q] [--depth=N] [--max-nodes=N]
#   status        lapis vault info
#   doctor        lapis doctor
#   create        lapis create --title=T [--body=TEXT] [--dry-run] …
#   append        lapis append [--dry-run] [--if-mtime=MS] [--if-hash=HASH] -- PATH TEXT
#   task list     lapis task list [--path=P] [--full] …
#   task toggle   lapis task toggle [--dry-run] [--if-mtime=MS] [--if-hash=HASH] -- ID
#   tome tree     lapis tome tree [--node=ID] [--depth=N] -- DOC
#   tome open     lapis tome open -- DOC NODE…
# `tome` needs a binary built with `--features tome`.

def vault-args [vault: string] {
  let v = if $vault != "" { $vault } else { $env.LAPIS_VAULT? | default "" }
  if $v == "" { [] } else { [$"--vault=($v)"] }
}

# One argv. A separate `$flag $value` lets clap read a leading `-` as a flag.
def with-opt [args: list<string>, flag: string, value: string] {
  if ($value | is-empty) { $args } else { $args ++ [$"($flag)=($value)"] }
}

def with-switch [args: list<string>, flag: string, on: bool] {
  if $on { $args ++ [$flag] } else { $args }
}

def with-repeat [args: list<string>, flag: string, values: list<string>] {
  if ($values | is-empty) { $args } else {
    $values | reduce -f $args {|v, acc| $acc ++ [$"($flag)=($v)"] }
  }
}

def usage [command: string, error: string] {
  {
    ok: false
    command: $command
    error: $error
    stderr: ""
    exit: 1
    kind: "usage"
  }
}

def missing-lapis [command: string, detail: string] {
  {
    ok: false
    command: $command
    error: "lapis is not on PATH"
    stderr: ($detail | str substring 0..<400)
    exit: 127
    kind: "missing_lapis"
  }
}

# Print the envelope and exit with its code (0 when the field is absent).
def finish [e: record] {
  print ($e | to json)
  exit ($e.exit? | default 0)
}

# `vault info` and `doctor` print a report and, when the index is down, a
# second error envelope. Split on a top-level `{` (nested objects are indented).
def json-docs [text: string] {
  let trimmed = ($text | str trim)
  if ($trimmed | is-empty) {
    []
  } else {
    $trimmed | split row "\n{" | enumerate | each {|it|
      let chunk = if $it.index == 0 { $it.item } else { "{" + $it.item }
      try { $chunk | from json } catch { null }
    } | where {|v| $v != null}
  }
}

def lapis-missing [stderr: string, stdout: string, code: int] {
  $code != 0 and ($stdout | is-empty) and (
    ($stderr | str contains "not found")
    or ($stderr | str contains "External command failed")
    or ($stderr | str contains "No such file")
    or ($stderr | str contains "no such file")
  )
}

# Always passes `--json`. On failure the envelope carries the CLI exit code
# and stderr (both trimmed to 400 chars for the message fields).
def envelope [command: string, args: list<string>, vault: string] {
  let r = (try {
    ^lapis --json ...(vault-args $vault) ...$args | complete
  } catch {|err|
    {
      exit_code: 127
      stdout: ""
      stderr: ($err.msg? | default "Command lapis not found")
      missing: true
    }
  })
  let stderr = ($r.stderr? | default "" | str trim)
  let stdout = ($r.stdout? | default "" | str trim)
  let code = ($r.exit_code? | default 127)
  if ($r.missing? | default false) or (lapis-missing $stderr $stdout $code) {
    return (missing-lapis $command $stderr)
  }
  let docs = (json-docs $stdout)
  let good = ($docs | where {|d| ($d.ok? | default false)})
  let bad = ($docs | where {|d| not ($d.ok? | default false)})
  if $code == 0 and not ($good | is-empty) {
    { ok: true, command: $command, data: ($good | first | get data?) }
  } else {
    let from_cli = if ($bad | is-empty) { "" } else {
      $bad | last | get error?.message? | default ""
    }
    let raw = ([$stderr $stdout] | str join " " | str trim)
    let msg = if ($from_cli | is-empty) { $raw } else { $from_cli }
    mut out = {
      ok: false
      command: $command
      error: ($msg | str substring 0..<400)
      stderr: ($stderr | str substring 0..<400)
      exit: $code
    }
    if not ($good | is-empty) {
      $out = ($out | upsert data ($good | first | get data?))
    }
    $out
  }
}

def hit-row [h: record] {
  {
    path: ($h.path? | default null)
    title: ($h.title? | default null)
    score: ($h.score? | default null)
    snippet: ($h.snippet? | default null)
  }
}

def "main search" [
  --limit (-n): string = "5"
  --domain: string = ""
  --vault: string = ""
  ...query: string
] {
  if ($query | is-empty) {
    finish (usage "search" "query required")
  } else {
    let q = ($query | str join " ")
    mut a = ["search" "--per-doc" $"--limit=($limit)"]
    $a = (with-opt $a "--domain" $domain)
    $a = ($a ++ ["--" $q])
    let e = (envelope "search" $a $vault)
    if not $e.ok {
      finish $e
    } else {
      let hits = ($e.data?.hits? | default [])
      let rows = ($hits | each {|h| hit-row $h })
      finish ($e | upsert data $rows)
    }
  }
}

def "main read" [--vault: string = "", ...path: string] {
  if ($path | length) != 1 {
    finish (usage "read" "path required")
  } else {
    finish (envelope "read" ["read" "--" ($path | first)] $vault)
  }
}

def "main list" [
  --prefix: string = ""
  --limit (-n): string = "50"
  --vault: string = ""
] {
  mut a = ["list" $"--limit=($limit)"]
  if $prefix != "" { $a = ($a ++ ["--" $prefix]) }
  finish (envelope "list" $a $vault)
}

def "main neighbors" [--vault: string = "", ...path: string] {
  if ($path | length) != 1 {
    finish (usage "neighbors" "path required")
  } else {
    finish (envelope "neighbors" ["neighbors" "--" ($path | first)] $vault)
  }
}

def "main resolve" [--vault: string = "", ...link: string] {
  if ($link | is-empty) {
    finish (usage "resolve" "link required")
  } else {
    finish (envelope "resolve" ["resolve" "--" ($link | str join " ")] $vault)
  }
}

# `analytics recent` names its cells in `columns`. 0.4.2 is
# path, title, domain, priority, mtime_iso. Older builds are path, title, mtime.
# Position 2 is domain on 0.4.2, so the time has to be chosen by name.
def recent-row [columns: list, row: list] {
  let rec = ($columns | enumerate | reduce -f {} {|c, acc| $acc | upsert $c.item ($row | get $c.index) })
  let iso = ($rec.mtime_iso? | default "")
  let when = if ($iso | is-empty) { $rec.mtime? | default null } else { $iso }
  { path: ($rec.path? | default null), title: ($rec.title? | default null), mtime: $when }
}

def "main recent" [--limit (-n): string = "10", --vault: string = ""] {
  let n = (try { $limit | into int } catch { null })
  if $n == null {
    finish (usage "recent" "limit must be an integer")
  } else {
    let e = (envelope "recent" ["analytics" "--" "recent"] $vault)
    if not $e.ok {
      finish $e
    } else {
      let cols = ($e.data?.columns? | default [])
      let rows = ($e.data?.rows? | default [] | take $n | each {|r| recent-row $cols $r })
      finish ($e | upsert data $rows)
    }
  }
}

def "main analytics" [--vault: string = "", ...query: string] {
  if ($query | is-empty) {
    finish (usage "analytics" "query required")
  } else {
    finish (envelope "analytics" ["analytics" "--" ($query | str join " ")] $vault)
  }
}

def "main tree-retrieve" [
  --path: string = ""
  --query: string = ""
  --depth: string = ""
  --max-nodes: string = ""
  --vault: string = ""
] {
  mut a = ["tree-retrieve"]
  $a = (with-opt $a "--path" $path)
  $a = (with-opt $a "--query" $query)
  $a = (with-opt $a "--depth" $depth)
  $a = (with-opt $a "--max-nodes" $max_nodes)
  finish (envelope "tree-retrieve" $a $vault)
}

def "main status" [--vault: string = ""] {
  finish (envelope "status" ["vault" "info"] $vault)
}

def "main doctor" [--vault: string = ""] {
  finish (envelope "doctor" ["doctor"] $vault)
}

def "main create" [
  --title: string = ""
  --path: string = ""
  --template: string = ""
  --type: string = ""
  --domain: string = ""
  --tag: list<string> = []
  --body: string = ""
  --director: string = ""
  --dry-run
  --no-reindex
  --vault: string = ""
] {
  if ($title | is-empty) {
    finish (usage "create" "title required")
  } else {
    mut a = ["create" $"--title=($title)"]
    $a = (with-opt $a "--path" $path)
    $a = (with-opt $a "--template" $template)
    $a = (with-opt $a "--type" $type)
    $a = (with-opt $a "--domain" $domain)
    $a = (with-repeat $a "--tag" $tag)
    $a = (with-opt $a "--body" $body)
    $a = (with-opt $a "--director" $director)
    $a = (with-switch $a "--dry-run" $dry_run)
    $a = (with-switch $a "--no-reindex" $no_reindex)
    finish (envelope "create" $a $vault)
  }
}

def "main append" [
  --dry-run
  --if-mtime: string = ""
  --if-hash: string = ""
  --no-reindex
  --vault: string = ""
  ...rest: string
] {
  if ($rest | length) < 2 {
    finish (usage "append" "path and text required")
  } else {
    let path = ($rest | first)
    let text = ($rest | skip 1 | str join " ")
    mut a = ["append"]
    $a = (with-switch $a "--dry-run" $dry_run)
    $a = (with-opt $a "--if-mtime" $if_mtime)
    $a = (with-opt $a "--if-hash" $if_hash)
    $a = (with-switch $a "--no-reindex" $no_reindex)
    $a = ($a ++ ["--" $path $text])
    finish (envelope "append" $a $vault)
  }
}

def "main task list" [
  --path: string = ""
  --full
  --limit: string = ""
  --offset: string = ""
  --status: string = ""
  --due: string = ""
  --tag: string = ""
  --vault: string = ""
] {
  mut a = ["task" "list"]
  $a = (with-opt $a "--path" $path)
  $a = (with-switch $a "--full" $full)
  $a = (with-opt $a "--limit" $limit)
  $a = (with-opt $a "--offset" $offset)
  $a = (with-opt $a "--status" $status)
  $a = (with-opt $a "--due" $due)
  $a = (with-opt $a "--tag" $tag)
  finish (envelope "task list" $a $vault)
}

def "main task toggle" [
  --dry-run
  --if-mtime: string = ""
  --if-hash: string = ""
  --no-reindex
  --vault: string = ""
  ...id: string
] {
  if ($id | length) != 1 {
    finish (usage "task toggle" "id required")
  } else {
    mut a = ["task" "toggle"]
    $a = (with-switch $a "--dry-run" $dry_run)
    $a = (with-opt $a "--if-mtime" $if_mtime)
    $a = (with-opt $a "--if-hash" $if_hash)
    $a = (with-switch $a "--no-reindex" $no_reindex)
    $a = ($a ++ ["--" ($id | first)])
    finish (envelope "task toggle" $a $vault)
  }
}

def "main tome tree" [
  --node: string = ""
  --depth: string = ""
  --vault: string = ""
  ...doc: string
] {
  if ($doc | is-empty) {
    finish (usage "tome tree" "doc required")
  } else {
    mut a = ["tome" "tree"]
    $a = (with-opt $a "--node" $node)
    $a = (with-opt $a "--depth" $depth)
    $a = ($a ++ ["--" ($doc | str join " ")])
    finish (envelope "tome tree" $a $vault)
  }
}

def "main tome open" [--vault: string = "", ...rest: string] {
  if ($rest | is-empty) {
    finish (usage "tome open" "doc required")
  } else {
    let doc = ($rest | first)
    let nodes = ($rest | skip 1)
    finish (envelope "tome open" (["tome" "open" "--" $doc] ++ $nodes) $vault)
  }
}

def main [] { help main }
