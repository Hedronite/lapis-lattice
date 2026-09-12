# Lapis patch to gpui-base 0.6.1

Source: crates.io `gpui-base` 0.6.1, `.crate` sha256
`9d45dcaaeac889bf1e7757db1beb26c9043c8ea3156651facc11c6be56bb6722`
(upstream git `96103905ea0c9c199db206ada7a9b2e3114e6339`, `crates/base`), unpacked
verbatim. Wired through `[patch.crates-io]` in the workspace `Cargo.toml`; `gpui-kit`
stays pinned at `=0.6.1`. Apache-2.0, see `LICENSE-APACHE`.

## Why

A focused `TextareaState` observes its private `BlinkCursor` and notifies every
500 ms. GPUI redraws the whole window for any notify from a rendered view, and the
Live editor must keep the (invisible) source widget rendered for focus, key context
and undo. That idle frame kept native idle CPU at ~1.8% of a core against a 1% gate.
0.6.1 has no public way to turn the blink off.

## Change (only these two files)

- `src/input/base/blink_cursor.rs`: an `enabled` flag. `set_enabled(false)` calls the
  existing `stop()`, drops the pending timer and leaves the caret visible; `start`,
  `pause` and `blink` do nothing while disabled, so focus, window activation and
  keystrokes cannot restart it. Test: `disabled_cursor_stays_visible_and_never_restarts`.
- `src/input/base/state.rs`: public `InputBaseState::set_cursor_blink(bool, cx)`
  (so `TextareaState::set_cursor_blink`). Default stays enabled.

`.rustfmt.toml` here disables formatting so the vendored source stays upstream bytes.
Drop this directory once upstream ships an equivalent switch.
