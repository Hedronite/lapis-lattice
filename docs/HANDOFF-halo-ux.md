# Halo handoff — Lapis GOAL-ux

**Codex is paused. Halo takes over.**

Do not wait for Codex. Do not restart the goal. Do not merge `feat/ux` to
`main`. Continue this branch.

| Field | Value |
| --- | --- |
| Goal | GOAL-ux |
| Repo | [VirtualMachinist/lapis-lattice](https://github.com/VirtualMachinist/lapis-lattice) |
| Branch | `feat/ux` |
| Tip | `3e94191` — `perf(tui): redraw only when workspace state changes` |
| Ahead of `main` | **24 commits** (`39bfa65`… search merge) |
| PR | [#18](https://github.com/VirtualMachinist/lapis-lattice/pull/18) — same branch; **do not merge** until Evan GO |
| Formal checklist | **0 / 32 IDs closed** |
| Ownership | Halo: **Grok PM** · **Claude Code fullstack** · **Composer** |
| Previous owner | Codex (paused for handoff; usage was ~9%) |

This file is the in-repo resume so Grok PM, Claude Code fullstack, and Composer
can continue without rereading the Codex session. The vault foundry remains the
source of truth for IDs, rubric language, and evidence rows. Repo docs on this
branch describe **implementation checkpoints**, not acceptance.

## Resume protocol

Read in this order, then work. Do not invent a new plan.

1. This file (`docs/HANDOFF-halo-ux.md`).
2. Vault foundry (authoritative IDs and evidence; private, not in this clone):
   - `GOAL-ux.md`
   - `SPEC-ux.md`
   - `CHECKLIST-ux.md` (32 IDs; **0 closed**)
   - `RUBRIC-ux.md`
   - `REVIEW-ux.md`
   - `CHECKPOINT.md` (or `CHECKPOINT-ux.md` if that is the living name)
   - `HANDOFF-codex-pause-2026-09-12.md` (Codex pause note)
   - `evidence/ux/` (mostly empty / pending)
3. Repo checkpoints on this tip:
   - [docs/desktop-workspace.md](desktop-workspace.md)
   - [docs/terminal-files.md](terminal-files.md)
   - [docs/pdf-runtime.md](pdf-runtime.md)
   - [docs/ux-performance.md](ux-performance.md)
4. Eli mail_room handoff for Halo (Grok PM · Claude Code fullstack · Composer).
5. Stay on `feat/ux`. Push here. Update PR #18. **No merge. No retarget to `main` work.**

### Halo roles

| Seat | Does | Does not |
| --- | --- | --- |
| **Grok PM** | Gates, evidence honesty, REVIEW/CHECKPOINT updates, refuse invented closures | Close CHECKLIST IDs from unit tests or “looks done” |
| **Claude Code fullstack** | Implementation on `feat/ux` against SPEC/CHECKLIST | Merge, rewrite `main`, reopen settled contracts |
| **Composer** | Scoped grind (one failing ID or one blocker at a time) | Broad refactors, search/G* work, crates.io/release truth |

Codex does not own the next commit. If a Codex transcript and this file
disagree, prefer this file plus the vault CHECKLIST row.

## Tip and what actually landed

Tip `3e94191` (full `3e94191a9f27d9ff7296480aaa9d18c32c2a0f78`). Default CLI
build still has **desktop off**. Desktop: `--features desktop`, then
`lapis --vault <path> desktop`. Headless widget checks:
`cargo test -p lapis-desktop --features gui-tests`.

Code depth is **U1–U5 landings + U6 TUI idle start**. That is not checklist
progress. Native Linux, CUA, GPUI idle CPU, and missing-index TUI remain open.

### Landed on the tip (code, not acceptance)

| Surface | What is in the tree | Honest limit |
| --- | --- | --- |
| **Document panes** | Split right / Split below; up to four panes; session v2 saves order, focus, direction, sizes; v1 sessions still load | No nested layouts, no same-file duplicate views, no native pane-matrix acceptance |
| **Graph** | Sidebar or Ctrl/Cmd+Shift+G; lazy embedded snapshot; Global/Local; filters; Back/Forward; hidden graph stops animation | Not persisted across launches; HTTP graph is an explicit capability limit; dense-graph / frame / memory budgets open |
| **MD live preview** | Markdown opens in Live; caret block shows source; Source / Reading / Split share editor + undo; pointer/IME map to source bytes | Tables, link/task clicks, images, a11y semantics, source-reveal continuity, large-doc layout cache still open |
| **PDF** | Desktop cancellable page worker (`pdfium-render` 0.9.4 / PDFium 7881); TUI text extract stays as before | Runtime is a **separate** packaging step; no download-on-open; no OCR / passwords; CI archives are executable-only |
| **Vim** | Desktop default is normal mode; counted motions; d/c/y; registers + clipboard request; `:w` / `:q` / `:wq` | **Not** Vim parity: no named register banks, text objects, macros, dot-repeat, continuous replace |
| **Session restore** | Normal-exit metadata under the app config dir; visible panes only at startup; hidden tabs lazy; missing files retryable/closable | No note text in metadata; not crash recovery; invalid metadata is reported and **left unchanged** |
| **TUI idle** | Redraw only on input, background messages, active edge scroll, or status expiry; 60 ms poll remains | PTY idle sample is not a native display budget; **GPUI idle CPU is not this commit** |

TUI on this branch also has literal bracketed paste, pointer selection,
clipboard routes, atomic undo, Linux watcher no longer starving input, preview
text copy, dirty/conflict save copies, YAML-literal + HTML reference readers,
and responsive tab overflow. TUI search/health use `ctx.backend()` (embedded
default), not a forced HTTP `:8080` client.

## GOAL-ux — U0–U7 working summary

Authoritative wording is vault `GOAL-ux.md`. This table is the
**landed-vs-gate map** at `3e94191` so Halo does not restart. Status means
*code on the branch*, not CHECKLIST closure.

| ID | Intent (working) | On tip | Evidence |
| --- | --- | --- | --- |
| **U0** | TUI operator contract: paste, pointer, clipboard, undo, watcher, preview copy, dirty/conflict save, layout | Landed as implementation | **pending** — PTY smoke exists; native terminal/clipboard/Linux still required |
| **U1** | GPUI notes workspace: files independent of index, Find, Build index, accessible nav ids | Landed as implementation | **pending** — headless `gui-tests` only |
| **U2** | Native Vim + literal paste + command/search prompt focus isolation | Landed as development slice | **pending** / some **fail** on CUA vs Vim-default |
| **U3** | Source-mapped Markdown Live + YAML/HTML/PDF readers | Landed as checkpoint | **pending** — Live/PDF docs say so explicitly |
| **U4** | Navigation history, resizable chrome, link context, lazy session restore | Landed as checkpoint | **pending** — no native session-restore evidence |
| **U5** | Lazy workspace graph + independent document panes | Landed as checkpoint | **pending** — graph/pane docs list remaining acceptance |
| **U6** | Startup / idle / switch cost | **TUI idle redraw started**; measurement scripts present | **fail / pending** — GPUI idle CPU open; missing-index TUI open; no accepted numbers |
| **U7** | Native Linux + CUA + shipping acceptance | **Not landed** | **fail / pending** — this is the gate, not a polish pass |

Most U* rows stay **pending**. A few already **fail** (see blockers). Do not
relabel them `done` because a commit message says `feat`.

## Evidence honesty

Say this out loud at the start of every Halo session:

- **CHECKLIST-ux is 0/32.** No ID is closed.
- Headless GPUI tests and PTY smokes **supplement** native evidence. They do
  not close Linux, CUA, IME, mouse, clipboard, or performance IDs.
- [docs/desktop-workspace.md](desktop-workspace.md) and
  [docs/ux-performance.md](ux-performance.md) already refuse overclaim. Match
  that tone in REVIEW/CHECKPOINT.
- `scripts/ux-tui-perf.py` timings end when the PTY sees expected output. They
  exclude the terminal renderer and the physical display.
- Preserve measurement failures. Do not retune thresholds to make a run green.
- `evidence/ux/` should stay honest: empty or `pending` is correct; a green
  unit test is not a row.

If a teammate wants to close an ID, Grok PM asks for the vault evidence
artifact first. No artifact → still pending.

## Blockers (do not paper over)

### 1. Native Linux

Desktop is a GPUI checkpoint. There is no accepted compositor run: window
chrome, mouse, clipboard, IME, HiDPI, Wayland vs X11, PDF runtime placement
(`libpdfium.so` beside `lapis`), or Omarchy theme on a real Linux seat.

`cargo test -p lapis-desktop --features gui-tests` is not that run.

Graph, Live Markdown, panes, and PDF all call out native Linux as remaining
acceptance work. U7 does not start from a green Linux desk.

### 2. CUA issues

Common User Access vs this workspace’s Vim-default command layer is unresolved:

- Ctrl/Cmd+C / X / V / Z / A / S / W coexist with `h/j/k/l`, `v/V`, `y/p`, `u`.
- Linux clipboard is `wl-clipboard` or `xclip`/`xsel`; SSH is OSC 52 out and
  local-terminal paste in. Missing tools must stay visible errors, not silent
  “copied”.
- Native mouse selection and IME UTF-16 ranges are mapped in Live view; that
  is not a claim that CUA selection, menu keys, or screen-reader names work.
- Command and search prompts must keep focus and reject multiline paste so
  `:q!` cannot land in the note. That protection is in code; native proof is
  not.

Do not “fix CUA” by deleting Vim. Do not claim CUA by adding more shortcuts
without a native matrix.

### 3. GPUI idle CPU

TUI idle redraw (`3e94191`) is **not** the desktop idle budget.

The graph stops `request_animation_frame` when the sim is at rest and drops
animation plus stale callbacks when hidden. The GPUI workspace process can
still burn CPU while a note sits idle (event loop / vsync / widget invalidation).
There is no accepted desktop idle percentage, RSS, or “quiet window” capture.

U6 stays open until someone measures a **release** desktop binary on named
hardware and keeps the failure if it misses.

### 4. Missing-index TUI

`scripts/ux-tui-perf.py --phase missing-index` is a first-run diagnostic
**before** `lattice.sqlite` exists. It is separate from the indexed 10k
protocol.

That phase is not an accepted passing gate. First-run TUI with no index
(sidebar still lists files; Find / Build index states; no silent empty search)
still needs a preserved measurement and a CHECKLIST row. Do not “help” the
fixture by running `init` or building the index first — `docs/ux-performance.md`
says `init` adds Welcome and changes the corpus.

Indexed TUI idle redraw is a start. Missing-index remains a blocker.

## Do-not-reopen

Already decided or already landed. Do not spend a Halo turn relitigating:

1. **No merge to `main`.** PR #18 stays a checkpoint until Evan GO.
2. **Do not rewrite** the `v0.4.0` GitHub Release or the `0.1.0` crate inside
   that asset. Version truth lives on `main` (0.4.1 line); this branch is UX.
3. **Do not reopen search G1–G4 / sqlite-vec / DiskANN.** That is `main`
   (`#17`). Identifier boost and `vec0` are not GOAL-ux.
4. **Not Vim parity.** No named register banks, text objects, macros,
   dot-repeat, or continuous replace in this slice. The command state must keep
   saying so.
5. **YAML is literal source.** Never parse-and-reserialize on editor save.
6. **PDF never downloads a runtime** on open, never loads from the current
   directory, never mutates a system PDFium. Missing runtime = install/retry.
7. **One document, one mount.** Two panes must not silently compete to save
   the same file. Same-file duplicate views are out of scope.
8. **HTTP graph stays HTTP.** Do not swap in an unrelated embedded index to
   make the canvas look full. Report the capability limit.
9. **Session metadata is not a note and not crash recovery.** Invalid or
   unsupported JSON is retained unchanged; do not “repair” it in place.
10. **Document panes ≠ Source/Reading Split.** Split right / below is a
    separate pane matrix. Do not collapse them.
11. **Graph is not persisted across launches.** Runtime retention only.
    Do not add session-graph persistence as a drive-by.
12. **Desktop stays off** the default CLI/Release asset.
13. **Do not enable `load_extension`** or unpin `sqlite-vec`.
14. **Second `lapis init` does not retarget** an existing config `vault` key.
15. **Do not treat PTY ms as native latency** or unflush OS caches to chase
    a number.
16. **Do not close CHECKLIST IDs** from `gui-tests` or `tests/tui_input.rs`.
17. **TUI HTTP `:8080` as the default search path is not the current
    primary bug.** This branch’s TUI uses `Backend`. Do not reopen the
    v0.3.1 stranger-install hunt unless it regresses on `feat/ux`.
18. **Do not ship `cargo install lapis`, `brew install lapis`, or
    `lapis.sh`.** Names we do not own stay documented as such.
19. **Do not overwrite** a prepared `dist/pdf-runtime` destination; the
    packager refuses on purpose.
20. **Do not adjust performance thresholds** to absorb a miss.

## How to continue (next useful work)

Grok PM picks **one** open gate. Composer or Claude Code fullstack implements
only that gate. Suggested order if the vault CHECKLIST agrees:

1. **Native Linux smoke** for the desktop workspace (files, Live, save, pane
   focus, clipboard, IME) — write evidence, do not close IDs from memory.
2. **CUA matrix** (Linux + whatever seat Evan names): which shortcuts win,
   and what the UI says when they do not.
3. **GPUI idle CPU** on a release `--features desktop` binary; keep the
   trace even if it fails.
4. **Missing-index TUI** `--phase missing-index` with an unmodified
   `scripts/ux-fixture.py` corpus.

Measurement recipe: [docs/ux-performance.md](ux-performance.md). Fixture:
`scripts/ux-fixture.py`. Do not change the manifest to make a run pass.

## Pointers

### Vault foundry (SoT, private)

These names are the GOAL-ux pack. They are not published in this repository
(operator-topology gate). Halo already knows the vault tree.

| Doc | Use |
| --- | --- |
| `GOAL-ux.md` | U0–U7 intent |
| `SPEC-ux.md` | Contracts, non-goals, file/clipboard/session law |
| `CHECKLIST-ux.md` | 32 IDs; still 0 closed |
| `RUBRIC-ux.md` | What “pass” means; native vs headless |
| `REVIEW-ux.md` | Halo updates this; no invented greens |
| `CHECKPOINT.md` | Living slice note (name may be `CHECKPOINT-ux.md`) |
| `HANDOFF-codex-pause-2026-09-12.md` | Codex pause; Halo named |
| `evidence/ux/` | Artifacts or explicit `pending` |

### Repo on `feat/ux` @ `3e94191`

| Path | Use |
| --- | --- |
| [docs/desktop-workspace.md](desktop-workspace.md) | Desktop checkpoint (panes, graph, Live, Vim, session) |
| [docs/terminal-files.md](terminal-files.md) | TUI Markdown/YAML/PDF/HTML |
| [docs/pdf-runtime.md](pdf-runtime.md) | PDFium pin, layout, cache bounds |
| [docs/ux-performance.md](ux-performance.md) | TUI indexed vs missing-index protocol |
| `crates/lapis-desktop/` | GPUI workspace, Live, panes, graph, PDF, session |
| `src/tui/` | Terminal workspace; idle redraw in `mod.rs` |
| `src/desktop_session.rs` | Per-vault session envelope; corrupt JSON retained |
| `scripts/ux-tui-perf.py` | Indexed (30 runs + idle) and missing-index |
| `scripts/ux-input-smoke.py` | PTY input / idle-expiry / edge-scroll |
| `scripts/ux-fixture.py` | 10k synthetic corpus; do not mutate for scoring |
| `scripts/pdfium-runtime.py` | Pinned `chromium/7881` + checksum |

### Git

```
3e94191 perf(tui): redraw only when workspace state changes
4d56598 fix(desktop): report pane navigation status accurately
196cfe7 feat(desktop): add persistent independent document panes
e6a5a0f fix(desktop): keep graph labels and preview layout readable
c3e2ed3 feat(desktop): integrate lazy graph with workspace navigation
86265e4 fix(desktop): clear missing-tab errors after close
0978fdb feat(desktop): restore workspace tabs and positions lazily
a70fd0d fix(desktop): refresh saved metadata and indexed context
a2e56fc feat(desktop): add navigation history resizable panes and link context
bb90e6d fix(tui): clear stale visual anchors on external reload
4b7fd53 fix(tui): preserve inclusive visual ranges and HTML rows
58b6da1 fix(tui): preserve YAML source and make reference readers usable
cfb8fc6 feat(desktop): add cancellable PDF page reader and pinned runtime packaging
9174c55 test(tui): await atomic save completion in PTY smoke
b2dd5b7 feat(desktop): add source-mapped Markdown live preview
f4433ea Add native Vim editing and protect literal paste and command focus
f632573 Expose workspace indexing and accessible navigation controls
f678a21 Start GPUI notes workspace with shared file and search services
1840441 Polish responsive TUI layout, tab overflow and passive themes
5c0f4bc fix(editor): preserve unsaved work and save conflict copies
3005333 feat(tui): select and copy rendered preview text
14c5e91 fix(tui): prevent Linux watcher events from starving input
5647c4f feat(tui): add pointer selection, clipboard routes and atomic undo
4be5053 fix(tui): accept literal bracketed paste and guard note saves
```

`main` is `39bfa65` (GOAL-search / PR #17). Do not merge this list there.

## Stop conditions

- Checklist still 0/32 and someone wants to merge → **stop**.
- A change would retarget `init` vault, enable PDF download-on-open, remount
  one file in two panes, or call HTTP graph “fixed” via embedded fallback →
  **stop**.
- Codex comes back with a conflicting tip → keep `feat/ux` @ newest honest
  SHA and update this file; do not fork a second UX branch unless Evan says so.

Halo owns the next honest increment on `feat/ux`. Codex is paused.
