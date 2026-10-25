#!/usr/bin/env nu
# Deterministic body for the lapis skill. Wraps `lapis --json`.
# Vault: --vault, else $LAPIS_VAULT, else the path `lapis init` recorded.
#
# CLI each command actually runs:
#   search        lapis search --per-doc --limit N [--domain D] -- QUERY
#   read          lapis read -- PATH
#   list          lapis list --limit N [-- PREFIX]
#   neighbors     lapis neighbors -- PATH
#   resolve       lapis resolve -- LINK
#   recent        lapis analytics recent
#   analytics     lapis analytics QUERY
#   tree-retrieve lapis tree-retrieve [--path P] [--query Q] [--depth N] [--max-nodes N]
#   status        lapis vault info
#   doctor        lapis doctor
#   create        lapis create --title T [--dry-run] …
#   append        lapis append PATH TEXT [--dry-run] [--if-mtime MS] [--if-hash HASH]
#   task list     lapis task list [--path P] [--full] …
#   task toggle   lapis task toggle ID [--dry-run] [--if-mtime MS] [--if-hash HASH]
#   tome tree     lapis tome tree DOC [--node ID] [--depth N]
#   tome open     lapis tome open DOC NODE…
# `tome` needs a binary built with `--features tome`.

def vault-args [vault: string] {
  let v = if $vault != "" { $vault } else { $env.LAPIS_VAULT? | default "" }
  if $v == "" { [] } else { ["--vault" $v] }
}

def with-opt [args: list<string>, flag: string, value: string] {
  if ($value | is-empty) { $args } else { $args ++ [$flag $value] }
}

def with-switch [args: list<string>, flag: string, on: bool] {
  if $on { $args ++ [$flag] } else { $args }
}

def with-repeat [args: list<string>, flag: string, values: list<string>] {
  if ($values | is-empty) { $args } else {
    $values | reduce -f $args {|v, acc| $acc ++ [$flag $v] }
  }
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

# Always passes `--json`. On failure the envelope carries the CLI exit code
# and stderr (both trimmed to 400 chars for the message fields).
def envelope [command: string, args: list<string>, vault: string] {
  let r = (^lapis --json ...(vault-args $vault) ...$args | complete)
  let docs = (json-docs $r.stdout)
  let good = ($docs | where {|d| ($d.ok? | default false)})
  let bad = ($docs | where {|d| not ($d.ok? | default false)})
  if $r.exit_code == 0 and not ($good | is-empty) {
    { ok: true, command: $command, data: ($good | first | get data?) }
  } else {
    let from_cli = if ($bad | is-empty) { "" } else {
      $bad | last | get error?.message? | default ""
    }
    let stderr = ($r.stderr | str trim)
    let raw = ([$stderr $r.stdout] | str join " " | str trim)
    let msg = if ($from_cli | is-empty) { $raw } else { $from_cli }
    mut out = {
      ok: false
      command: $command
      error: ($msg | str substring 0..<400)
      stderr: ($stderr | str substring 0..<400)
      exit: $r.exit_code
    }
    if not ($good | is-empty) {
      $out = ($out | upsert data ($good | first | get data?))
    }
    $out
  }
}

def "main search" [
  query: string
  --limit (-n): int = 5
  --domain: string = ""
  --vault: string = ""
] {
  mut a = ["search" "--per-doc" "--limit" ($limit | into string)]
  if $domain != "" { $a = ($a ++ ["--domain" $domain]) }
  $a = ($a ++ ["--" $query])
  let e = (envelope "search" $a $vault)
  if not $e.ok { return ($e | to json) }
  let hits = ($e.data?.hits? | default [])
  let rows = if ($hits | is-empty) { [] } else { $hits | select path title }
  $e | upsert data $rows | to json
}

def "main read" [path: string, --vault: string = ""] {
  envelope "read" ["read" "--" $path] $vault | to json
}

def "main list" [
  --prefix: string = ""
  --limit (-n): int = 50
  --vault: string = ""
] {
  mut a = ["list" "--limit" ($limit | into string)]
  if $prefix != "" { $a = ($a ++ ["--" $prefix]) }
  envelope "list" $a $vault | to json
}

def "main neighbors" [path: string, --vault: string = ""] {
  envelope "neighbors" ["neighbors" "--" $path] $vault | to json
}

def "main resolve" [link: string, --vault: string = ""] {
  envelope "resolve" ["resolve" "--" $link] $vault | to json
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

def "main recent" [--limit (-n): int = 10, --vault: string = ""] {
  let e = (envelope "recent" ["analytics" "recent"] $vault)
  if not $e.ok { return ($e | to json) }
  let cols = ($e.data?.columns? | default [])
  let rows = ($e.data?.rows? | default [] | take $limit | each {|r| recent-row $cols $r })
  $e | upsert data $rows | to json
}

def "main analytics" [query: string, --vault: string = ""] {
  envelope "analytics" ["analytics" $query] $vault | to json
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
  envelope "tree-retrieve" $a $vault | to json
}

def "main status" [--vault: string = ""] {
  envelope "status" ["vault" "info"] $vault | to json
}

def "main doctor" [--vault: string = ""] {
  envelope "doctor" ["doctor"] $vault | to json
}

def "main create" [
  --title: string
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
  mut a = ["create" "--title" $title]
  $a = (with-opt $a "--path" $path)
  $a = (with-opt $a "--template" $template)
  $a = (with-opt $a "--type" $type)
  $a = (with-opt $a "--domain" $domain)
  $a = (with-repeat $a "--tag" $tag)
  $a = (with-opt $a "--body" $body)
  $a = (with-opt $a "--director" $director)
  $a = (with-switch $a "--dry-run" $dry_run)
  $a = (with-switch $a "--no-reindex" $no_reindex)
  envelope "create" $a $vault | to json
}

def "main append" [
  path: string
  text: string
  --dry-run
  --if-mtime: string = ""
  --if-hash: string = ""
  --no-reindex
  --vault: string = ""
] {
  mut a = ["append" $path $text]
  $a = (with-switch $a "--dry-run" $dry_run)
  $a = (with-opt $a "--if-mtime" $if_mtime)
  $a = (with-opt $a "--if-hash" $if_hash)
  $a = (with-switch $a "--no-reindex" $no_reindex)
  envelope "append" $a $vault | to json
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
  envelope "task list" $a $vault | to json
}

def "main task toggle" [
  id: string
  --dry-run
  --if-mtime: string = ""
  --if-hash: string = ""
  --no-reindex
  --vault: string = ""
] {
  mut a = ["task" "toggle" $id]
  $a = (with-switch $a "--dry-run" $dry_run)
  $a = (with-opt $a "--if-mtime" $if_mtime)
  $a = (with-opt $a "--if-hash" $if_hash)
  $a = (with-switch $a "--no-reindex" $no_reindex)
  envelope "task toggle" $a $vault | to json
}

def "main tome tree" [
  doc: string
  --node: string = ""
  --depth: string = ""
  --vault: string = ""
] {
  mut a = ["tome" "tree" $doc]
  $a = (with-opt $a "--node" $node)
  $a = (with-opt $a "--depth" $depth)
  envelope "tome tree" $a $vault | to json
}

def "main tome open" [doc: string, ...nodes: string, --vault: string = ""] {
  envelope "tome open" (["tome" "open" $doc] ++ $nodes) $vault | to json
}

def main [] { help main }
