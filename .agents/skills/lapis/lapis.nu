#!/usr/bin/env nu
# Deterministic body for the lapis skill. Wraps read-only `lapis --json`.
# Vault: --vault, else $LAPIS_VAULT, else the path `lapis init` recorded.
#
# CLI each command actually runs:
#   search     lapis search --per-doc --limit N [--domain D] -- QUERY
#   read       lapis read -- PATH
#   list       lapis list --limit N [-- PREFIX]
#   neighbors  lapis neighbors -- PATH          (hop defaults to 1)
#   resolve    lapis resolve -- LINK
#   recent     lapis analytics recent           (columns by name; LIMIT 50)
#   status     lapis vault info
#   doctor     lapis doctor

def vault-args [vault: string] {
  let v = if $vault != "" { $vault } else { $env.LAPIS_VAULT? | default "" }
  if $v == "" { [] } else { ["--vault" $v] }
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
    let raw = ([$r.stderr $r.stdout] | str join " " | str trim)
    let msg = if ($from_cli | is-empty) { $raw } else { $from_cli }
    mut out = {
      ok: false
      command: $command
      error: ($msg | str substring 0..<400)
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

def "main status" [--vault: string = ""] {
  envelope "status" ["vault" "info"] $vault | to json
}

def "main doctor" [--vault: string = ""] {
  envelope "doctor" ["doctor"] $vault | to json
}

def main [] { help main }
