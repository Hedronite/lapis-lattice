# tome-eval: baseline vs tome-tree A/B harness (spike)

Scores **baseline** (existing lattice chunk retrieval) against **tome** (tome-tree
page-range navigation) on the same 20 hand-verified questions, using the same answer
model and token budget in both arms. Jev only grades; it never answers.

## Files

| Path | What |
|---|---|
| `questions.jsonl` | 20 scored questions: 5 each for the SRE, Database Internals, K8s Security and AFML PDFs. Gold pages were verified against `pdftotext -f N -l N` (1-based physical pages). |
| `failclosed.jsonl` | Non-scored probe(s). `rtg-fc-01` (Red Team Guide, no outline or headings) must fail with `no_structure` (or `parse`) and never with an empty success. |
| `schema/tome-eval-result.schema.json` | Result and summary record schema (`oneOf` result / summary). |
| `config.toml` | Answer model, budgets, baseline indexer provenance, Jev settings. **The model id comes only from here.** |
| `data/chunk-pages.jsonl` | Read-only export of `chunk_id → page_start/page_end` for the 4 docs (baseline page join). |

## Run

```sh
cargo run -p tome-eval -- check                                  # load + validate questions/config
cargo run -p tome-eval -- run --config evals/tome/config.toml \
  --questions evals/tome/questions.jsonl --failclosed evals/tome/failclosed.jsonl \
  --out /tmp/tome-eval --only sre-01                              # smoke (verdict is always "incomplete")
cargo run -p tome-eval -- validate /tmp/tome-eval/results.jsonl /tmp/tome-eval/summary.json
```

The lattice HTTP service listens on 127.0.0.1:8080 on the vault host only. Build the binaries on
the build host, copy them to the vault host, and run from a `zsh -lc` login shell there so
`TYPESAFE_API_KEY` (Jev) is set. Set `answer.opencode_bin` and `baseline.lapis_bin` to
absolute paths in a local copy of the config.

## Baseline arm

`lapis --json --lattice <url> search <q> -n 50 --mode hybrid`: filter to hits from the
question's PDF, take the top 8, then replay lapis' shadow Jev rerank. Lapis search has
no doc filter, which is why the harness filters. The rerank only reorders when every hit
got a Jev judgement. Pages come from joining `chunk_id` against `data/chunk-pages.jsonl`.
A join miss is recorded as a `chunk_join_miss` error and never dropped silently. Every
result records the indexer provenance: `tome_indexer.py` sha256 (atrium-lattice has no git
commits yet), chunk size 2000/4000 chars, no overlap, nomic-embed-text 768d, and the
rerank settings.

Regenerate the chunk map (read-only, never writes the live DB):

```sh
cd "$VAULT/projects/atrium-lattice"
sqlite3 -json "file:data/lattice.db?mode=ro" "SELECT c.chunk_id, d.path AS doc, c.chunk_index, c.page_start, c.page_end, c.chapter, c.section, c.char_count FROM chunks c JOIN documents d ON d.doc_id=c.doc_id WHERE d.doc_id IN (2065,2067,2086,2094) ORDER BY d.doc_id, c.chunk_index" \
  | jq -c '.[]' > /tmp/chunk-pages.jsonl
# then add doc_sha256 (sha256 of the PDF, the same value as in questions.jsonl) to each row
```

The doc ids are the ones indexed on 2026-07-23. `mode=ro` never takes a write lock on the live WAL DB.

## Answer model (OpenCode)

Both arms use `[answer]` from `config.toml` (currently `opencode-go/deepseek-v4.1-flash`,
temperature 0). The harness writes a generated OpenCode agent
(`.opencode/agent/<agent>.md`, temperature from config, tools off) into a scratch git dir
and runs `opencode run --standalone --format json` with stdin set to /dev/null. Token usage
comes from `opencode session export`. OpenCode has no CLI flag for output tokens, so
`max_output_tokens` is stated in the prompt only and enforced by `fit_budget` on the input side.

## Tome arm

Until the R2 interface is frozen (Marci), the harness codes against the provisional
`TomeApi` shim in `crates/tome-eval/src/contract.rs`. The live backend is `StubTome`, so
every tome record currently has `error.kind = "stub"`. `FakeTome` backs the offline tests.
The MCP tools `tome_tree` / `tome_open` exist only with `--features tome` **and**
`LAPIS_TOME=1`, and are off by default.
