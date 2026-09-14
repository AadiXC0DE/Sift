# Performance measurements — Phase 10 budget table

Every number below comes from a script in `scripts/` reading a running Sift
process. Nothing is estimated or simulated. Rows that need a native interaction
harness or a live account are listed as **not measured** with the reason and the
method that would produce them.

Measured on 2026-09-14, machine `MacBookAir10,1` (Apple M1, 8 cores, 16 GiB),
macOS 15.6.1 (24G90), Darwin 24.6.0, against release binary
`src-tauri/target/release/sift` sha256 **643ed89f0737ff5a** (38,399,248 bytes,
built 06:22:53Z from working tree at commit `0f0204f1db56350cb1b7c871e27c0f5b0e5862eb`
plus uncommitted sibling work; the binary contains **no** updater call — the
`src/features/updates` feature landed in the tree after this build).

> **Load caveat.** The measurements were taken on a working laptop while sibling
> agents ran builds and dev servers: load average was 7.8–15 with 1-minute
> spikes to 32.7. The absolute latency numbers are therefore pessimistic; the
> budget verdicts are reported as measured. A quiet-machine re-run is the next
> step for the launch row.

## Verdicts

| Budget row (plan P10) | Target | Measured | Verdict |
|---|---|---|---|
| Warm-cache launch to usable Inbox | p95 ≤ 400 ms, 30 launches, release, no network | 39 warm launches: `first-paint` p50 **587 ms**, p95 **920 ms**, max 1026 ms, min 446 ms; `window-shown` p50 208 ms, p95 354 ms, max 364 ms | **BREACH** on first-paint; within budget on window-shown |
| Total idle memory | ≤ 180 MiB (Rust + WebView) | 6 sessions, 3 samples each: worst sample per session **169.4 / 187.1 / 188.2 / 208.0 / 209.2 / 229.0 MiB**; app 28–29 MiB, WebKit WebContent 115–159 MiB, GPU 17–36 MiB, Networking 7–14 MiB | **BREACH** (worst 229.0 MiB; 5 of 6 sessions ≥ 187 MiB) |
| Idle CPU | < 0.5 % of one core over 60 s after sync settles | outbox empty: **0.05 % / 0.349 %** (two runs) → pass. Outbox holding 10 undeliverable queued ops: **8.33 % / 9.71 % / 10.80 %** (three runs) → breach | **BREACH** as measured; cause identified below |
| List focus change | p95 ≤ 16 ms (keydown→paint) | — | not measured |
| Cached thread body open | p95 ≤ 50 ms | — | not measured |
| Archive response | p95 ≤ 30 ms | — | not measured |
| Local search at 100k messages | p95 ≤ 30 ms query execution | — | not measured |
| List page 100 rows | p95 ≤ 3 ms warm DB | — | not measured |
| Download working memory | ≤ 8 MiB incremental for a 25 MiB attachment | — | not measured |
| IPC payload sizes | list ≤ 60 KiB, body ≤ 512 KiB | — | not measured |
| IMAP connections | ≤ 3 per account during transfer | — | not measured |
| P11-T23 IMAP sync | 200 threads ≤ 10 s, metadata ≤ 90 s, quiet poll ≤ 300 ms | — | not measured |

Gate: `bash scripts/bench-check.sh` → *11 rows: 0 pass, 3 breach, 8 not measured*, exit 1.
Measured rows pass/fail from the worst recorded value for the same budget, so a
lucky run cannot flip a verdict.

## How the app was launched (fixture)

Benchmarks never touch the real mailbox at
`~/Library/Application Support/com.aadixc0de.sift` (161,189,888 bytes,
43,000 messages). `scripts/bench-fixture.sh` clones the pre-migration snapshot
with `cp -c` (APFS copy-on-write, instant, source untouched) into
`$TMPDIR/sift-bench-data` and then makes two edits:

1. `DELETE FROM outbox_ops` — the snapshot's outbox held an `inflight`
   `modify_labels` row; the app retries in-flight work at startup, which would
   have issued a real STORE against the user's Gmail account.
2. account e-mails are rewritten to `bench+<id>@invalid.test`. OAuth
   credentials are keyed by e-mail (`src-tauri/src/secrets.rs`), so no
   credential resolves and `provider_for` fails before any socket is opened —
   the launch row's "no network dependency" holds by construction. Accounts,
   threads and messages stay, so the Inbox still renders the real local state.

The first launch into a fresh fixture also performs the schema 6 → 16 migration
and is reported separately:

| Run | Command | Result |
|---|---|---|
| First launch after migration | `bash scripts/bench-start.sh -n 1 --fresh-fixture` | `window-shown` 3888 ms, `first-paint` 4579 ms |

## Methods

Markers are emitted by the app itself and carry epoch milliseconds:
`perf:window-shown` from Rust setup (`src-tauri/src/lib.rs`),
`perf:first-paint` / `perf:webview-loaded` from the frontend
(`src/App.tsx` → `perf_mark`). Latency = marker epoch-ms − epoch-ms read
immediately before `Popen`, so both ends share one clock and no harness polling
delay is counted. Percentiles are **nearest-rank** (at n = 10 the p95 is the
maximum, which is conservative and stated in every report).

### Launch — `scripts/bench-start.sh`

```bash
cargo build --manifest-path src-tauri/Cargo.toml --release --features custom-protocol --locked
bash scripts/bench-start.sh -n 10 --metric first-paint      # 4 sessions
```

`--features custom-protocol` is what `tauri build` uses; without it the binary
loads `devUrl` and the frontend never emits a marker. The script exits non-zero
on a breach and on any launch that fails to produce the marker; a failed run is
recorded, never substituted. Raw per-launch logs are kept in `docs/perf/raw/`.

Per-session results (all release, sha256 643ed89f0737ff5a):

| Session (UTC) | n | first-paint p50 | first-paint p95 | window-shown max |
|---|---|---|---|---|
| 06:23:46Z | 10 | 608 ms | 920 ms | 354 ms |
| 06:28:29Z | 10 | 533 ms | 840 ms | 226 ms |
| 06:30:43Z | 10 | 516 ms | 1026 ms | 211 ms |
| 06:31:47Z | 9 (+1 failed) | 746 ms | 914 ms | 364 ms |
| **all 39 warm launches** | 39 | **587 ms** | **920 ms** | 364 ms |

One launch in 40 emitted `perf:window-shown` but no `perf:first-paint` inside
the 30 s timeout (raw log: `docs/perf/raw/launch-20260914T063043Z-run1.log`,
contains only the window-shown marker); two further launches during a
same-process diagnostic exited with an empty log, which is the
`tauri-plugin-single-instance` handshake racing a still-closing previous
instance — both are reported as failures, and the scripts now print that hint.

### Memory — `scripts/bench-mem.sh`

```bash
bash scripts/bench-mem.sh            # settle 30 s, 3 samples, 5 s apart
```

* `footprint -p <pid>` (Apple's per-process accounting) for the app and for
  each WebKit helper; the printed footprint is summed. This is the gate metric.
* `ps -o rss=` is reported next to it (342–494 MiB total) and is **not** used
  for the gate: RSS double-counts pages shared between processes.
* WebKit's GPU/WebContent/Networking helpers are spawned by launchd (ppid 1)
  and carry no owner in argv, so they are attributed by pid-set difference
  across the app's lifetime, then confirmed to exit with the app
  (`helpers exited with the app: True` in every run).

Sample from the recorded session (`docs/perf/results.json` → `memory.samples`):

| Process | footprint | RSS |
|---|---|---|
| `sift` (Rust) | 28–29 MiB | 151–258 MiB |
| `webkit.WebContent` | 115–159 MiB | 157–185 MiB |
| `webkit.GPU` | 17–29 MiB | 37–42 MiB |
| `webkit.Networking` | 7.4–14 MiB | 15–26 MiB |
| **total** | **153.2–209.2 MiB** | 343–494 MiB |

Session worsts: 208.0, 229.0, 169.4, 209.2, 188.2, 187.1 MiB. The WebContent
renderer, not the Rust core, is what puts the app over the 180 MiB ceiling.

Limitations: `footprint` excludes clean file-backed pages (shared framework
code), so it under-counts memory genuinely shared with other apps; RSS
over-counts it. Neither number is the whole story, and the spread across
sessions (153–229 MiB) means the row should be re-measured on a quiet machine
before treating 180 MiB as a hard regression gate.

### Idle CPU — `scripts/bench-idle-cpu.sh`

```bash
bash scripts/bench-idle-cpu.sh --outbox empty      # 0.349 %
bash scripts/bench-idle-cpu.sh --outbox pending    # 9.708 %
```

Cumulative CPU (`ps -o time=`, user+sys) is read for the app and its WebKit
helpers at the start and end of a 60 s window after a 30 s settle; percent =
ΔCPU / window × 100. `ps -o %cpu` is deliberately not the gate: it is a decaying
average since process start, not a window measurement (it is recorded for
reference).

| Fixture outbox | Window average | Worst sub-interval |
|---|---|---|
| 10 pending `modify_labels` ops (as the app had accumulated) | **8.33 %** / **9.71 %** / **10.80 %** | 12.66 % |
| emptied (`--outbox empty`) | **0.349 %** / **0.050 %** | 1.60 % |

**Finding (A/B, same session, same binary, consecutive runs):** with 10 pending
undeliverable `modify_labels` ops in the outbox the app idles at ~10 % of one
core; after `DELETE FROM outbox_ops` the same binary idles at 0.05 %. Peak in
the Rust process 1.24 s / WebContent 0.86 s per 20 s window. `sample` on the
Rust process showed no dominant CPU-bound frame (the profile is dominated by
waiting threads), so the mechanism is inferred from the code, not proven:
`drain_loop` (`src-tauri/src/runtime.rs:358-395`) runs every second whenever
`outbox_next_deadline` returns a value — which it does whenever rows are pending
— and on every iteration issues `outbox_pending_count`, `outbox_failed_count`
and `outbox_summary` against the 161 MB database and emits `outbox:state` to the
frontend. The renderer burning in lockstep is consistent with a per-second
`outbox:state` re-render. Nothing here was changed: `src-tauri/**` is outside
this task's ownership — this is a report, not a fix.

## Not measured

Each row needs something this environment does not have. The reason and the
method that would produce the number are recorded in
`scripts/bench-check.sh` (`UNMEASURED`) and repeated here.

| Row | Why not | How it would be measured |
|---|---|---|
| List focus change p95 ≤ 16 ms | No native interaction harness. Playwright drives a browser engine, not the Tauri webview, and the app has no keydown→paint marker. | Add a `perf_mark` after the rAF that follows a list-keydown; drive the real window with Accessibility/`osascript` key codes (needs macOS Accessibility permission); 30 iterations, nearest-rank p95. |
| Cached thread body open p95 ≤ 50 ms | Same harness; no body-open marker exists. | Marker after the body's first text paint; click a thread row through the same driver. |
| Archive response p95 ≤ 30 ms | Same harness. | Click Archive, time the local list update with a marker. |
| Local search at 100k p95 ≤ 30 ms | `fixtures/mailbox-100k/` is empty (`scripts/gen-mailbox.ts` only prints). | Generate the 100k fixture, then time the query (criterion bench `search`, which measures query execution only) or instrument the in-app path. |
| List page 100 rows p95 ≤ 3 ms warm DB | Not run by this task. | `cargo bench --manifest-path src-tauri/Cargo.toml --bench threads_query` against a warm 100k DB. |
| Download working memory ≤ 8 MiB / 25 MiB attachment | Needs a live account and a real 25 MiB attachment; the fixture is credential-free by design. | Start the download, sample `footprint`/`vmmap` at 1 Hz for the app process, report max − baseline; also assert no whole-attachment copy remains. |
| IPC payload sizes | Not attempted. | Instrument the IPC boundary (or the diagnostics channel) and record serialized sizes. |
| IMAP connections ≤ 3 per account | Needs a live signed-in account; no socket is opened against the fixture. | `scripts/check-connections.sh <pid> <accounts>` — it now really counts established `imap.gmail.com` sockets via lsof and exits non-zero above `2 × accounts` (both branches verified against a local listener; unrun against a live account). |
| P11-T23 IMAP sync budgets | `scripts/bench-imap-sync.sh` runs offline wiremock tests that emit no timings; the budgets are defined against the seeded live benchmark account. | Seed `scripts/seed-benchmark-account.ts` (Gmail OAuth, 20k messages) and time first-page/full-metadata/quiet-poll in `sync_log`. |

`tools/sift-bench`'s `prepare`/`run` still only `println!` and
`scripts/smoke-boot.sh` remains the only launch harness that asserts startup
health; neither is owned by this task and neither contributed a number here.

## Artifacts

| Path | Contents |
|---|---|
| `docs/perf/results.json` | machine-readable results: `launch`, `memory`, `idle_cpu`, each with samples, method, `history` (per-session headline + load average), binary sha256 |
| `docs/perf/bench.log` | append-only log of every measurement run, including the ones superseded in `results.json` |
| `docs/perf/raw/*.log` | raw per-launch app output used as evidence (41 files) |
| `scripts/bench_lib.py` | shared measurement helpers (markers, footprint, WebKit attribution, percentiles, history-aware gating) |
| `scripts/bench-fixture.sh` | hermetic mailbox clone |
| `scripts/bench-start.sh` / `bench-mem.sh` / `bench-idle-cpu.sh` | the three measurements |
| `scripts/bench-check.sh` | budget gate over `results.json`; exit 1 on a measured breach, `--require-all` also fails on a not-measured row |

## Reproduce

```bash
bash scripts/bench-fixture.sh --fresh          # isolated mailbox clone
bash scripts/bench-start.sh -n 10 --metric first-paint
bash scripts/bench-mem.sh
bash scripts/bench-idle-cpu.sh --outbox empty
bash scripts/bench-idle-cpu.sh --outbox pending
bash scripts/bench-check.sh                    # exits 1: 3 breaches, 8 not measured
```
