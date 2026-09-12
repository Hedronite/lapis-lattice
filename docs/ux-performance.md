# Measuring TUI startup and idle cost

Use a release binary built with the same features as the comparison binary and
a disposable corpus from `scripts/ux-fixture.py`. Keep the manifest and source
files unchanged. Prepare the index through Lapis's workspace **Build workspace
index** action; `init` adds a Welcome note and changes this benchmark corpus.

Run `scripts/ux-tui-perf.py --bin /absolute/path/lapis --vault /absolute/path/fixture
--out /absolute/path/new-output --phase indexed` (as one shell command). The
script validates all 10,000 note hashes and the index's document count, graph
state and no-embedder configuration before running. Use `--phase missing-index`
before creating an index to record a separate first-run diagnostic.

The indexed protocol records ten new-process launches followed by twenty
immediate reopen launches. Every run opens the first folder and note through
actual terminal input. Outputs include the complete terminal byte streams,
reconstructed editor screens, raw measurements, nearest-rank p95, and a 60-second
idle sample after a five-second settle. RSS and cumulative CPU time include the
UI and its current descendants; the default embedded engine lives in the UI
process. Record hardware, OS, display, terminal and background applications
alongside the output when collecting acceptance evidence.

These timings end when the PTY receives the expected output. They exclude the
terminal renderer and physical display, so they supplement native latency
measurements. Fixture hash validation warms file data; the script does not flush
OS caches. A warm reopen here is another process launch, not switching an
already-open tab. The standard note is 10,486 bytes; this does not measure the
separate 100 KiB note-switch workload. No-embedder measurements do not represent
semantic model cost. Compare only matching feature sets, fixtures and cache
conditions, and preserve failures rather than adjusting the thresholds.

The TUI redraws after input, background messages, active edge scrolling, and
status expiry. An idle workspace still polls terminal input/background results
and checks backend health periodically, but avoids rebuilding the frame on
every poll. Regression smoke covers expired status, continuing drag scrolling,
release stopping the scroll, and ordinary clipboard/save/undo/external updates.
