# Internal history (not linked from README)

Private build hosts and vault paths used before the public scrub. Kept so `git log` is not the only record.

- Operator builds once used a private rsync-to-remote-box helper (removed from `scripts/` in the public-readiness change). GitHub Actions is the public CI.
- The dogfood vault was a personal Obsidian tree. Public clones have no default vault; use `lapis init`.
