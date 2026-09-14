# Sift acceptance dossier (P10.5)

Reviewer-facing record of what the `fix/reliability-hardening` branch proves, what it
merely claims, and what has not been measured. It is deliberately conservative: a row is
only `verified offline` when a committed test in this tree asserts the required behaviour,
and no row is ever filled in with a checkmark that the repository cannot produce.

## 1. Header

| Field | Value |
| --- | --- |
| Branch | `fix/reliability-hardening` |
| HEAD commit | `5a5859fe7151b3fb0afcea181b7f5cd000bfc871` ("Make the browser suite exercise real behavior instead of root presence", committed 2026-09-14T00:56:58+05:30) |
| Dossier written | 2026-09-14 (+05:30), 2026-09-13T19:27Z |
| Build version | `1.0.0` — `package.json:3`, `src-tauri/Cargo.toml:3`, `src-tauri/tauri.conf.json:4`, `CHANGELOG.md` all agree; `pnpm exec tsx scripts/check-versions.ts` printed `version consistency ok` (exit 0) |
| Minimum OS declared | macOS 13.0 (`src-tauri/tauri.conf.json:32`) |
| Environment for every number quoted below | Apple M1, arm64 (`uname -a`: Darwin 24.6.0, `RELEASE_ARM64_T8103`), 8 logical CPUs |
| Measurement status | Only two commands were executed to produce this dossier: `node scripts/eager-js.mjs --dist dist --max-js-kib 250` and `pnpm exec tsx scripts/check-versions.ts`. Every other figure is either a static count (`grep`/`wc` over committed files), a size/mtime read from an on-disk artifact, or quoted from a committed file. Anything else is marked "not measured in this environment". |

### Status vocabulary (the only four values used in the matrices)

| Status | Meaning here |
| --- | --- |
| `verified offline` | A committed test in this tree asserts the required behaviour, and I read that test. It does not mean the suite was re-executed for this dossier, and it never means live Gmail. |
| `pending native validation` | The behaviour exists in source but can only be observed on a real macOS build (WKWebView, Finder, Preview, sleep/wake, installer). |
| `pending live Gmail` | The behaviour can only be observed against a real Gmail account. |
| `not implemented` | The required behaviour, its regression test, or the required artifact is absent from this tree. |

Partially completed rows are split into `a`/`b` rows rather than given a compound status.

**Scope.** Every statement below describes commit `5a5859f`. Other work is in flight in
the same working tree (`src-tauri/src/**`, `src/app/ipc/**`, `e2e/**` show as modified),
so line numbers and any row that those edits touch can drift; re-check the cited file
before quoting a row after that work lands.

## 2. Verification-artifact inventory

Every row names a reproducible command, what it establishes, and explicitly what it does
**not** establish. Nothing here is a substitute for the live acceptance protocol in
section 7.

### 2.1 `cargo test --manifest-path src-tauri/Cargo.toml`

- **Reproduce:** `cargo test --manifest-path src-tauri/Cargo.toml` (the spec's baseline form adds `--locked`); `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` run alongside it in `.github/workflows/ci.yml:30-32`.
- **Surface:** 28 integration targets in `src-tauri/tests/`, plus in-module unit tests. Static declaration counts I verified: `imap_attachment_protocol.rs` 31 tests, `attachment_lifecycle.rs` 22, `account_scoping.rs` 3, `imap_bodies.rs` 4. The shared oracle is the strict fake Gmail server in `src-tauri/tests/support/fake_imap.rs`, seeded from `fixtures/mailbox-imap-2k.json`.
- **Establishes:** the attachment wire format (parenthesised `(UID BODY.PEEK[<section>])`, dotted sections, root section `1`), strict FETCH/selected-state handling, reconnect/reselect ordering, UIDVALIDITY epochs, the attachment cache state machine and filename policy, atomic writes and disk-full/corrupt-payload recovery, account-scoped keys and the 0007/0008 upgrade paths, and account-removal cancellation.
- **Does NOT establish:** anything against live Gmail; any native surface; packaging, signing, memory, CPU or latency budgets. No cargo suite was executed for this dossier and no stored run output exists anywhere in the tree (`**/*.log`, `coverage/`, `cargo-test*` are all absent), so the current pass/fail state of this suite is **not measured in this environment**.

### 2.2 `pnpm test` (vitest)

- **Reproduce:** `pnpm test` (`vitest run`; `vitest.config.ts` — jsdom, `src/test/setup.ts`, include `src/**/*.test.{ts,tsx}` and `scripts/**/*.test.ts`).
- **Surface:** 44 matching files (42 in `src/`, 2 in `scripts/lib/`), 184 `it(`/`test(` declarations counted statically (~162 in `src/`, ~22 in `scripts/`; `it.each` counts once while expanding at runtime).
- **Establishes:** component/store/parser behaviour in jsdom — thread windowing and the 1000-row cap, density token mapping, list command resolution, account-qualified reader and view store, the eager-JS budget unit, composer recipients/signature/attachment-drop behaviour, storage panel against a mocked API, and the updater contract tests.
- **Does NOT establish:** any real Tauri IPC (the `api` modules are `vi.mock`-ed), any native behaviour, any live mailbox behaviour. Static counts only — **not executed for this dossier**; no stored vitest output exists.

### 2.3 `pnpm e2e` (Playwright, both engines)

- **Reproduce:** `pnpm e2e`; `playwright.config.ts` runs `pnpm exec vite --config e2e/vite.e2e.config.ts` on port 4399 and serves the app through a test-only entry (`e2e/fixture/index.e2e.html`) that aliases `@tauri-apps/api/core`, `@tauri-apps/api/event` and `@tauri-apps/plugin-dialog` to the fixture in `e2e/fixture/`.
- **Surface:** 14 spec files, 43 `test(` declarations, run against **chromium and webkit** (`playwright.config.ts:43-46`) → 86 executions. `e2e/fixture/test.ts:28-62` fails a test on unexpected `console.error`, page errors/unhandled rejections, and any fixture command the app calls that the mock does not implement.
- **Establishes:** real frontend behaviour against a deterministic stateful fixture backend — 100-row first page and cursor paging, archive/trash actions with persisted fixture state across `page.reload()`, undo restoring a row, account-marker geometry, unified scope merge, reversed account responses, density and theme persistence, attachment Save As/Open plumbing, axe-core a11y scanning, and the no-`#root`-only rule (`e2e/fixture/test.ts:11-13`).
- **Does NOT establish:** anything about the Rust backend or the native shell. The fixture backend implements behaviours the Rust backend does not have (for example it snapshots and restores label state on undo, `e2e/fixture/backend.ts:600-660`, and skips `deleteForever`), so a browser pass must never be read as an offline pass of the equivalent Rust path. Also note `pnpm e2e` is listed in CI while `ci.yml:16` installs only chromium.
- **Stored artifact:** `test-results/.last-run.json` = `{"status":"passed","failedTests":[]}` plus `playwright-report/index.html` and 8 `error-context.md` directories (a11y / search / setup / window × 2 engines), which match the four inline `test.fail()` sites. The artifact predates the HEAD commit by minutes, so it records a local run of the work in progress, not a post-commit run: **the 86-passing figure is a commit-message claim that was not independently re-measured here.**

### 2.4 Bundle / eager-JS gate

- **Reproduce:** `pnpm build` (runs `tsc --noEmit && vite build && node scripts/eager-js.mjs --dist dist`), or the gate alone: `node scripts/eager-js.mjs --dist dist --max-js-kib 250`.
- **Measured in this environment:** `assets/index-BrxcyGcf.js` raw 603,974 B, **gzip 192,165 B (187.7 KiB)**; total eager gzip 192,165 B against a 250 KiB (256,000 B) ceiling → `PASS` (exit 0). Measured against the working-tree `dist/` built 2026-09-14 00:56 (+05:30), which contains the split dynamic imports (`ComposerSheet`, `SettingsDialog`, `Palette`, `ShortcutHelp`, `Dialog`, `x`) and no `attachHelper` chunk.
- **Establishes:** the gzip weight of the chunks a visitor must download before first paint, walked through `dist/.vite/manifest.json` static `imports` only (dynamic imports excluded by construction).
- **Does NOT establish:** memory, launch latency, CPU, or the DMG size of a real signed build. A stale `dist/` passes the gate against old code; the measurement above is of a working-tree bundle, not of a bundle produced by `pnpm build` in this session and not of the released artifact.
- **Related:** `bash scripts/check-bundle.sh [--mode release|debug|auto] [--require-artifacts] [--max-js-kib N] [--max-dmg-mib N]` adds the DMG ceiling (12 MiB) and the presence check for release artifacts. CI's debug job runs it without `--require-artifacts`, so on pull requests the DMG half is skipped and only the JS half is enforced. **The only DMG on disk is a debug build** (`src-tauri/target/debug/bundle/dmg/Sift_1.0.0_aarch64.dmg`, 24.2 MB, ~1 week old); no universal/release tree, no `.app.tar.gz` and no `.sig` exist locally.

### 2.5 Updater dry-run

- **Reproduce:** `pnpm exec tsx scripts/updater-dryrun.ts` (also via `bash scripts/verify-release.sh --selftest` and in CI).
- **Establishes:** over a loopback fixture endpoint with an in-memory minisign key, 13 scenarios — valid signed update, legacy Ed signature, bad signature, unknown key, placeholder-pubkey refusal, wrong platform, truncated asset with and without checksums, equal/older version, unreachable metadata, unreachable asset, malformed metadata — each expected decision is asserted and every rejection must leave the installed version untouched. Companion unit coverage lives in `scripts/lib/updater.test.ts` and `scripts/lib/release-metadata.test.ts`.
- **Does NOT establish:** a real `tauri-plugin-updater` install, a real signature from the production key, or a real endpoint. The production public key is still the placeholder (`src-tauri/tauri.conf.json:37`), so `scripts/release-metadata.ts` refuses to generate metadata on purpose. **Not re-executed for this dossier; no stored output exists.**

### 2.6 Version-consistency check

- **Reproduce:** `pnpm exec tsx scripts/check-versions.ts [--tag vX.Y.Z] [--metadata latest.json]`.
- **Measured in this environment:** exits 0 and prints `version consistency ok` with `1.0.0` in `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` and `v1.0.0` in `CHANGELOG.md`.
- **Establishes:** writer/validator agreement — `scripts/bump-version.ts` writes the same `VERSION_SOURCES` list that this check reads, and the changelog must carry a `## vX.Y.Z` heading matching the other three.
- **Does NOT establish:** that build metadata exists (`release-assets/` is absent), or that the site's pinned constants agree (`docs/release.md` still asks the operator to update `sift-site/src/lib` by hand).

### 2.7 Release gate

- **Reproduce:** `bash scripts/verify-release.sh --selftest` (offline: version consistency + updater dry-run), `--mode strict --bundle-dir <dir>` (codesign/`spctl`/`stapler`/`lipo`/bundle version), `--mode local` (banner, skips signature checks), `--rollback-check --repo owner/name --tag vX.Y.Z` (uses `gh`).
- **Establishes:** on a signed bundle, that the DMG/app/updater archive and `.sig` exist, that `codesign --verify --deep --strict` and `spctl` accept the app, that the app and DMG are stapled, that the binary is universal, and that the bundle version matches `package.json`. There is no `|| true` in the script or the release workflow.
- **Does NOT establish:** anything without signed artifacts. None exist in this tree (`src-tauri/target/universal-apple-darwin` is absent), so the strict path, the metadata generation path and `--rollback-check` were **not run and cannot be run here**. `--rollback-check` silently skips when `gh` is unavailable.

### 2.8 Smoke boot

- **Reproduce:** `./scripts/smoke-boot.sh` — builds the debug binary, launches it for 20 s, and fails on a panic or a missing `perf:window-shown` marker.
- **Establishes:** that the debug binary starts and reaches the first-paint marker on this machine.
- **Does NOT establish:** anything about inbox correctness, attachments, performance or the release artifact. **Not run for this dossier** (it requires `cargo`), and it is not wired into CI.

### 2.9 Not measurements

`scripts/screenshots.ts` prints 11 scene names and a determinism hash without writing a PNG; `scripts/bench-start.sh` prints hardcoded `p50 0.31s` / `p95 0.34s` and appends the same constants to `docs/perf/bench.log`; `scripts/bench-mem.sh` is a reminder stub; `tools/sift-bench`'s `prepare`/`run` only `println!`; `docs/perf.md` and `docs/perf/` do not exist; `fixtures/mailbox-100k/` is empty. Any launch-time, memory or large-mailbox figure from those paths is fabricated, not measured.

## 3. P10.5 required-evidence matrix

| Requirement | Status | Basis |
| --- | --- | --- |
| Local test results (stored, current) | `not implemented` | No stored output for cargo, vitest or the rewritten Playwright suite exists; `test-results/.last-run.json` predates HEAD and the 86-passing figure is a commit-message claim. |
| Local test surface (unit + integration + browser) | `verified offline` | 28 Rust integration targets, 44 vitest files, 14 Playwright specs over chromium **and** webkit (43 declarations counted statically, 86 executions) — read, not re-executed. |
| Native screenshots | `not implemented` | `scripts/screenshots.ts` writes no PNG; the pixel-diff gate is a comment. |
| Checksum table | `pending native validation` | Only `scripts/release-metadata.ts` produces `SHA256SUMS`, from signed staged bytes; `release-assets/` does not exist. |
| Benchmark report | `not implemented` | No `docs/perf.md`, no `docs/perf/`, stubs only (see 2.9). |
| App-password Gmail (personal) | `pending live Gmail` | Section 7 protocol; implementation verified offline; live attachment acceptance pending. |
| Optional OAuth sign-in | `pending live Gmail` | The button is inert unless a real client id is compiled in; no offline end-to-end flow test. |
| Workspace account | `pending live Gmail` | No fixture, no run. |
| Light/dark rendering | `verified offline` | `e2e/settings.spec.ts:28-38` applies and persists `data-theme`; `MailFrame` dark variants covered by unit tests. |
| Light/dark native capture | `pending native validation` | No WKWebView screenshot exists (and no screenshot pipeline). |
| Densities: token map + live row height | `verified offline` | `rowHeight.test.ts` (32/40/48 tokens) and `e2e/settings.spec.ts:14-25` (row height 40 → 32, persisted). |
| Densities: native capture at all three | `pending native validation` | P3.1 requires before/after native captures; none exist. |
| Pane right/bottom/off | `verified offline` | `viewStore` cycles the layout; `e2e/reading.spec.ts:57-69` asserts switching threads replaces the displayed conversation immediately. |
| Pane layout native capture | `pending native validation` | No native capture of the three layouts. |
| One vs multiple accounts (browser) | `verified offline` | `e2e/unified.spec.ts` (2 seeded accounts, merge/narrowing, marker geometry) and `e2e/accounts.spec.ts`. |
| Multiple accounts (native, real accounts) | `pending live Gmail` | Needs two real mailboxes. |
| Offline / reconnect (protocol) | `verified offline` | `p1_t03_disconnect_after_select_restores_order`, `p1_t03_mutation_disconnect_is_not_replayed`, `p1_t03_epoch_change_on_reconnect_fetches_nothing`. |
| Offline / reconnect (product) | `pending live Gmail` | "Second save works offline" is the P2.8 step in section 7; unrun. |
| Sleep / wake | `pending native validation` | No test and no artifact covers sleep across a due wake. |
| Fresh install / upgrade (offline migration) | `verified offline` | `v6_upgrade_preserves_bodies_bytes_fts_and_reports_orphans` (account_scoping.rs), `migration_0007_dedupes_and_preserves_richest_row` (attachment_lifecycle.rs). |
| Older app refuses a newer database | `verified offline` | `db::tests::p10_t03_refuses_a_database_from_a_newer_app`: a fixture at `SCHEMA_VERSION + 1` fails to open, the message names the newer version, and the version row and account rows are unchanged. |
| Fresh install / upgrade (real installer) | `pending native validation` | No installer has been run end to end; migration correctness is covered offline by `p4_t01_fresh_and_v1_fixtures_upgrade_cleanly`. |
| Small mailbox (2k) | `verified offline` | `fixtures/mailbox-imap-2k.json` seeds the fake Gmail server used by the IMAP suites. |
| Large mailbox (100k) | `not implemented` | `fixtures/mailbox-100k/` is empty and gitignored; the generator only prints a message. |

## 4. Release-blocker list

States are `pending`, `pass` or `fail` as the specification requires. `pass` is used only
where a committed offline test proves the condition; it never means live.

| Blocker | Owner / task ID | State | Basis |
| --- | --- | --- | --- |
| Wrong-account content or action (reader/list) | P3.2 | `pass` | `ThreadView.test.tsx` (reversed responses, repeated thread id), `viewStore.test.ts`, `listCommands.test.ts`; browser `e2e/unified.spec.ts:104`. |
| Wrong-account content or action (storage/IPC) | P4.2 | `pass` | Migration `0008_account_scoping`, `MessageRef` throughout, `account_scoping.rs:identical_provider_ids_stay_isolated_per_account`, account-qualified `attachments_open`/`save_as`/`save_all` (`src/app/ipc/commands.ts:99-107`). |
| Draft file ownership for staged attachments | P2.5 | `pending` | `attachments_add_from_paths` still takes `paths` only (`src-tauri/src/commands/compose.rs:101-110`, `src/app/ipc/commands.ts:105`), so a staged file is not yet bound to `{accountId,draftId}`. |
| Silent attachment loss | P2.2/P2.4 | `pass` | metadata-then-body, corrupt payload, deleted cache, disk full, read-only destination, no-half-file tests in `attachment_lifecycle.rs`. |
| Silent draft loss | P5.1 | `pending` | Unmount flush exists (`ComposerSheet.tsx`), but drafts have no revision/durable close path and a hard kill inside the 300 ms debounce is uncovered. |
| Malformed fetch | P1.1 | `pass` | `imap_attachment_protocol.rs` asserts the corrected `(UID BODY.PEEK[...])` bytes and that the strict fake rejects the old form. |
| Malformed send | P5.3 | `fail` | `build_raw` still emits `From: me` and unwrapped base64, and omits `In-Reply-To`/`References` (`src-tauri/src/commands/compose.rs:174,181,195`). Bcc is now emitted. |
| Ambiguous send automatically retried | P6.1 | `fail` | `recover_outbox` unconditionally sets every `inflight` op back to `pending` (`src-tauri/src/db/mod.rs:94-99`); there is no `uncertain` operation state. |
| Irreversible delete against unintended IDs | P6.4 | `fail` | `threads_action` deletes local rows before enqueueing (`commands/actions.rs:118-146`) and `apply_delete_threads` derives its message list from those same rows (`provider/imap/ops.rs:460-491`), so the remote expunge has nothing to target; `action_undo` has no `delete` branch. |
| Broken upgrade | P10.3/P4.1 | `fail` | No `db_schema_too_new` guard anywhere in `src-tauri/src`; migration `0006` still deletes every cached body on upgrade. |
| External-content block bypass | P9.1 | `pending` | CSP gating is asserted as a string, but no test blocks a remote image and asserts zero outbound requests; the default is `remote_images = "always"` (`src-tauri/src/dto.rs:539`). |
| Signature verification failure | P10.3 | `pass` | 13 dry-run scenarios plus `updater.test.ts`/`release-metadata.test.ts`. No real release can be produced while the updater pubkey is the placeholder. |
| Missing real attachment acceptance | P2.8 | `pending` | Required wording: **implementation verified offline; live attachment acceptance pending**. |
| Hard-budget breach | P10.2 | `pending` | Eager JS measured 187.7 KiB against the 250 KiB gate; no DMG, memory, CPU or latency measurement exists. |

## 5. Appendix B fixture matrix

Statuses follow section 1. Rows split into `a`/`b` were partly satisfiable.

| ID | Fixture / trigger | Status | Evidence / gap |
| --- | --- | --- | --- |
| ATT-01 | Flat mixed text + PDF, exact bytes | `verified offline` | `p1_t01_exact_wire_bytes_for_sections`, `p1_t02_decoded_bytes_and_length_are_exact`, `imap_bodies` p11_t08 byte equality. |
| ATT-02 | Nested mixed/alternative dotted section | `verified offline` | `p1_t01_builder_renders_partial_section_items` (`(UID BODY.PEEK[1.2])`); strict parser accepts `1 BODY.PEEK[1.2]` and `1 (UID BODY.PEEK[2]<0.2048>)`. |
| ATT-03 | Single-part binary attachment, root section 1 | `verified offline` | `p27_single_part_root_is_section_one`, `p27_single_part_attachment_is_addressable_section_one`, `p27_single_part_root_rows_and_snippet`, `p27_single_part_root_downloads_exact_bytes`, `p27_single_part_attachment_row_is_ingested`. |
| ATT-04 | CID `logo@example.test`, metadata-only cache | `verified offline` | `same_cid_on_two_messages_resolves_to_each_rows_section`, `cid_lookup_returns_stored_part_id`. |
| ATT-05 | Metadata first, full body second | `verified offline` | `metadata_then_body_then_reopen_keeps_one_attachment_with_bytes`, `repeated_ingest_preserves_id_and_local_path`, `metadata_refresh_alone_does_not_redownload`. |
| ATT-06 | Wrapped base64 / QP chunk boundaries | `verified offline` | `p24_streaming_decoder_base64_is_chunk_boundary_safe`, `..._quoted_printable_is_chunk_boundary_safe`, `..._encodings_and_unsupported`, `p1_t02_fragmented_literals_and_delayed_completion_still_decode`. |
| ATT-07 | Zero-byte attachment | `verified offline` | `p1_t02_zero_byte_attachment_is_not_missing`, `empty_attachment_is_ready_with_zero_bytes`. |
| ATT-08 | Message moved All → Trash/Junk | `verified offline` | `p1_t04_stale_cached_uid_is_rediscovered_by_gmmsgid`, `p1_t04_deleted_message_reports_message_missing`. |
| ATT-09 | UIDVALIDITY reset with reused UID | `verified offline` | `p1_t04_uidvalidity_reset_invalidates_only_uids`, `p1_t03_epoch_change_on_reconnect_fetches_nothing`. |
| ATT-10 | Unsolicited FETCH before the requested row | `verified offline` | `p1_t02_unsolicited_wrong_uid_returns_no_bytes`; the fake advertises a deliberately wrong UID. |
| ATT-11a | Disconnect after SELECT | `verified offline` | `p1_t03_disconnect_after_select_restores_order`, `p1_t03_mutation_disconnect_is_not_replayed`. |
| ATT-11b | Disconnect during a literal | `not implemented` | The fake can fragment and delay literals, but no case truncates a literal and then reuses the connection. |
| ATT-12 | Same filename, traversal, Unicode continuation | `verified offline` | `filename_matrix_covers_unicode_quotes_traversal_and_missing_names`, `duplicate_names_are_distinct_per_attachment`, `p27_rfc2231_and_quoted_filenames_decode`. |
| ATT-13 | Disk full, stale `local_path`, corrupt zstd | `verified offline` | `disk_full_fails_without_ready_or_half_file` (macOS-gated), `deleted_cache_file_is_repaired`, `corrupt_payload_refetches_and_never_yields_empty_file`, `read_only_destination_fails_without_partial_file`, `failed_write_leaves_no_ready_state_and_no_half_file`. |
| ATT-14a | 25 MiB attachment, progress and cancel | `verified offline` | `p1_t05_repeated_click_creates_one_transfer`, `p1_t05_consumer_cancel_does_not_cancel_shared_download`, `progress_events_are_throttled_and_never_carry_bytes`, `save_cancel_aborts_immediately_and_cleans_up`. |
| ATT-14b | Connection cap and memory under 20 concurrent clicks + sync | `pending native validation` | No test asserts ≤3 connections under load; the ≤8 MiB incremental memory budget is unmeasured. |
| ATT-15a | Staging failures are visible; picker and drop share one path | `verified offline` | `attachDrop.test.ts`, `attachDrop.hook.test.tsx`, `Composer.test.tsx`; the fake `attachHelper` module is deleted. |
| ATT-15b | Real Finder drop, restart, source deleted, then send | `pending native validation` | Requires the native webview drag/drop listener and a real SMTP send. |
| UI-01a | 30 adjacent same-account rows, marker geometry | `verified offline` | `e2e/unified.spec.ts:9-70` asserts marker size/inset, ≥4 px gaps, no adjacent touching, per-account colours; `ThreadRow.test.tsx` pins the 3×12 dash. |
| UI-01b | Before/after native captures | `pending native validation` | P3.1 asks for captures; none exist. |
| UI-02 | A/B delayed responses reversed | `verified offline` | `ThreadView.test.tsx` (abandoned response discarded, action targets the clicked account), `viewStore.test.ts`. |
| UI-03a | New head appears, density change keeps the anchor | `verified offline` | `e2e/window.spec.ts:10-42` and `:72`; `useThreadsWindow.test.ts`, `threadWindow.test.ts`. |
| UI-03b | Anchor retained across a head refresh | `not implemented` | Pinned as an expected failure with a stated reason (`e2e/window.spec.ts:45-70`, `test.fail()`). |
| UI-04 | One selected A, focused B | `verified offline` | `listCommands.test.ts`; `e2e/unified.spec.ts:77`. |
| DB-01 | Every pooled connection enforces cascades | `verified offline` | `Db::open` now applies PRAGMAs in the pool's per-connection hook; `p1_t03_every_pooled_connection_is_initialized` (`src-tauri/src/db/mod.rs:460-481`) checks all four pooled connections, not just the first. |
| DB-02 | Same IDs in two accounts | `verified offline` | `identical_provider_ids_stay_isolated_per_account`, migration `0008_account_scoping`. |
| SYNC-01 | SELECT/FETCH interleave at a deterministic barrier | `not implemented` | The worker mutex serializes commands and call sites hold the guard across SELECT+FETCH, but no deterministic-barrier test proves it; the legacy `Conn::uid_fetch` string entry point survives (`conn.rs:1127-1134`) with one test caller. |
| SYNC-02a | Reconcile on a 404 | `verified offline` | `p3_t13_reconcile_on_404`. |
| SYNC-02b | Failed upsert retried, no skipped mail | `not implemented` | No test proves a failed row is retried without skipping mail. |
| SYNC-03 | Remove/re-add/reauth while tasks are active | `verified offline` | `p44_t05_removal_stops_sync_loop_without_late_writes`, `p44_t07_removal_records_uncertain_sends_and_counts`, `removal_cleans_every_scoped_table`. |
| SEND-01a | Subject-only edit, immediate close | `verified offline` | Client-side flush on unmount plus "one durable local row" (`drafts.rs` p7_t05, `Composer.test.tsx`). |
| SEND-01b | Revision/conflict-free durable draft lifecycle | `not implemented` | No `revision`/`expectedRevision` in the schema, IPC or store; `dirty` is written and never read. |
| SEND-02 | Bcc-only / quoted address / Unicode / missing file | `not implemented` | `build_raw` emits `From: me`, unwrapped base64, no RFC2231 `filename*`; no fixture test. |
| SEND-03 | Reply-To, reply-all, own message, forwarded file | `not implemented` | No server-side reply construction test; only a pure reply-all helper is covered. |
| SEND-04 | Queue → Undo → reopen; restart before deadline | `not implemented` | Sends are enqueued without an undo group (`compose.rs:52`); no draft restore after undo. |
| SEND-05 | SMTP accepted DATA then connection lost | `not implemented` | No `uncertain` op state; `recover_outbox` requeues `inflight` as `pending`. |
| ACT-01 | Mixed previous labels, cross-account Undo | `not implemented` | Undo inverts the op payload (swap add/remove) rather than restoring a persisted before-state; no previous-state table. |
| ACT-02 | Permanent delete with local rows removed first | `not implemented` | See the delete blocker in section 4. |
| TIME-01 | Sleep/DST/timezone/offline past due | `not implemented` | `check_snoozes` exists but neither scheduled send nor reminders do; the Snoozed view's ordering (`snoozed_until`) still disagrees with the list keyset cursor (`last_message_at`), and no test covers it. |
| SEARCH-01 | `is:read in:sent label:"Client Work"` | `not implemented` | `in:` parses into `Query.in_` (`search/query.rs:84-86`) and is never applied by `search/local.rs`; there are no local-search tests at all. |
| SEARCH-02 | Phrase + 400 hits in one conversation | `not implemented` | No test. |
| SEARCH-03 | 251 equal-timestamp results across accounts | `not implemented` | No cursor/limit test for search paging. |
| SEARCH-04 | Clear/Escape while a debounced/server call is pending | `not implemented` | Pinned as an expected failure (`e2e/search.spec.ts:88-89`). |
| PRIV-01 | Blocked mode: zero external network requests | `pending native validation` | CSP gating is asserted as a string (`MailFrame.test.tsx`); nothing counts requests, and the default is `always` (`src-tauri/src/dto.rs:539`). |
| PRIV-02 | Unsubscribe redirect / private IP / missing auth | `not implemented` | Only a one-click method picker is tested. |
| NATIVE-01a | Save As / Save All | `verified offline` | `save_all_writes_every_non_inline_attachment_and_avoids_collisions`, `save_all_needs_two_non_inline_attachments`; registered in `lib.rs`. |
| NATIVE-01b | Startup mailto, attachment preview, print | `pending native validation` | No Quick Look/preview adapter and no print path exist; mailto handling is unverified. |
| REL-01a | Bad update signature | `verified offline` | `updater-dryrun.ts` 13 scenarios and the updater unit tests. |
| REL-01b | Older app refuses a newer database | `verified offline` | `p10_t03_refuses_a_database_from_a_newer_app` asserts refusal, the named versions and untouched rows. |

### Appendix B–adjacent tasks that are not single rows

| Task | Status | Evidence / gap |
| --- | --- | --- |
| P1.2 remove the legacy permissive fake-IMAP mode | `not implemented` | `src-tauri/tests/support/fake_imap.rs:50-91` keeps `Behavior::legacy`/`legacy_unselected_defaults`; strict is the default and no attachment test depends on it. |
| P1.5 connection-cap evidence | `pending native validation` | Source documents ≤3 per account (worker + IDLE + foreground lease); `scripts/check-connections.sh` still advertises ≤2 and only prints an `lsof` count without asserting anything. |
| P2.3 executable-open policy | `verified offline` | Policy test only (`open_requires_confirmation_for_downloaded_executables`); the native opener itself is unverified. |
| P2.6 preview | `not implemented` | Save All is covered; no preview path exists. |
| P3.6 All Mail view | `not implemented` | No `View::AllMail` in the DTO or view store. |
| P10.4 Storage panel and clear action | `verified offline` | `commands/storage.rs` measures four cache classes, enforces the 512 MiB default cap and clears the attachment cache (unit tests in that file); the Settings panel is covered against mocked calls to those two commands in `StoragePanel.test.tsx`. |
| P10.4 LRU eviction and retention | `not implemented` | The 512 MiB cap is a reported setting; no eviction code exists (`last_accessed_at` is written, never read for eviction). |
| P10.2 measurement set (launch, focus, body open, archive, search, list page, memory, CPU, IPC, connections) | `not implemented` | No measurement harness or artifact exists; only the eager-JS and DMG gates are executable. |

## 6. What is not verified

Stated plainly, with no hedging:

- **Live Gmail attachment download.** No real mailbox has been touched. No fixture, fake
  server, file size or toast substitutes for it. The required wording applies exactly:
  *implementation verified offline; live attachment acceptance pending.*
- **Live Gmail send round trip.** Outgoing staging, MIME construction and SMTP acceptance
  have not been exercised against a real account, and `build_raw` is known-bad
  (`From: me`, unwrapped base64, no `In-Reply-To`), so the send path is not merely
  unverified — a real send is expected to mislabel or fail.
- **Native screenshots.** None exist; the screenshot pipeline writes no PNG.
- **Checksum table.** `SHA256SUMS` has never been produced here, because it can only be
  generated from signed staged artifacts and `release-assets/` does not exist.
- **Benchmark report.** No benchmark has been run. Idle memory, launch time, CPU, list
  focus, body open, archive latency, search latency, IPC volume and connection counts are
  all unmeasured, and `scripts/bench-start.sh` prints fabricated constants.
- **Bundle size of a released build.** The eager-JS figure is real and measured
  (187.7 KiB gzip against a 250 KiB gate) but the DMG download size is not: no universal
  or release build exists in this tree, and the only DMG is a 24.2 MB debug artifact.
- **The P10.2 budget rows.** Launch p95 ≤400 ms, list focus p95 ≤16 ms, cached body open
  p95 ≤50 ms, archive p95 ≤30 ms, search p95, list page p95, idle memory, CPU under load,
  download working memory, IPC payload sizes and per-account connection counts have no
  measurement artifact and no executable gate beyond the two size gates.
- **Playwright's 86-passing claim.** The rewritten suite is real, but the stored last-run
  artifact predates the commit; the pass count has not been reproduced since.
- **CI parity for the browser suite.** `.github/workflows/ci.yml:16` installs only
  chromium while `playwright.config.ts:43-46` declares chromium and webkit. Whether CI
  currently runs webkit successfully is unverified here; on a clean runner the webkit
  project has no installed binary.

## 7. P2.8 live-acceptance protocol (runnable checklist)

Run against a disposable Gmail mailbox with app-password IMAP, plus a separate OAuth
account. Fill the Results column only with observed output; a failure is a result.
Do not close the attachment bug from a fake-server pass, a visual toast, a nonempty file
or a prefix comparison.

| # | Step | Expected result | Results |
| --- | --- | --- | --- |
| 1 | Create the disposable mailbox and send fixtures into it: known PDF, PNG, ZIP, `.eml`, a Unicode filename, an unnamed part, and a zero-byte file | All messages arrive and appear in the list | |
| 2 | Record the sender-side SHA-256 of each fixture before sending | One checksum per fixture, stored alongside this table | |
| 3 | In Sift, open a fixture message in the Inbox; the attachment must be uncached (fresh install or after Settings → Storage → Clear) | First save performs a real FETCH; `sift-att://` resolves; no error toast | |
| 4 | Save As the PDF to disk | File opens in Preview; `shasum -a 256` equals the sender-side value; filename and extension preserved | |
| 5 | Save the ZIP and open it with the system handler | Archive opens; contents match | |
| 6 | Verify the saved file belongs to the account that owns the message (repeat with two accounts signed in) | Correct account, no cross-account file | |
| 7 | Check the message's IMAP flags after download | No change from download alone (no `\Seen` added) | |
| 8 | Inspect the download directory and cache directory | No leaked `.part` file; no half-written file | |
| 9 | Disconnect the network and save the same attachment again | Second save succeeds from cache; no network attempt | |
| 10 | Repeat steps 3-9 for messages in Inbox, archived, Trash, Junk, and with unified account switching | Same result in every location and scope | |
| 11 | Opening (not saving) each attachment type | Correct system opener, correct confirmation for executables | |
| 12 | Repeat steps 3-8 while initial sync of a large mailbox is running | Downloads still exact; no interleaving corruption | |
| 13 | Put the Mac to sleep mid-download, wake it, then retry the save | Connection recovers (reselect/poison as designed); file still exact | |
| 14 | Attach the same fixtures in the composer (picker **and** a real Finder drag onto the composer) | Per-file chips, real size/MIME, failures name the file and reason, successful files kept | |
| 15 | Quit and relaunch Sift, delete the source files, then reopen the draft | Staged attachments survive and are still sendable | |
| 16 | Send the message to the same mailbox and open the received copy | Recipients, subject, body and attachments correct; attachment checksum equals the original; no duplicate send | |
| 17 | (Optional OAuth) repeat steps 3, 4 and 16 with the OAuth account | Same results through the REST path | |

## 8. Follow-ups that require source changes

These are code/config defects found while writing this dossier. They are **not** fixed
here because this change is documentation-only and must not touch source, tests, `e2e/**`,
`scripts/**` or `.github/**`.

| Item | Location | Problem |
| --- | --- | --- |
| Stale provider comment | `src-tauri/src/provider/imap/provider.rs:5-7` | Claims `apply`/`send`/`watch` are unwired and that "nothing constructs this provider in production paths yet"; production constructs it and the operations are implemented and tested. |
| Stale connection comment | `src-tauri/src/provider/imap/conn.rs:3` | Says "exactly two connections per account"; `provider.rs:158-160` documents three (worker, IDLE, foreground lease). |
| Stale budget script | `scripts/check-connections.sh:2,11-16` | Asserts "≤2 established IMAP connections per account" in a comment, compares against no threshold, and is not wired into CI. |
| Legacy IMAP entry point | `src-tauri/src/provider/imap/conn.rs:1127-1134` | `Conn::uid_fetch` (string form) is documented as "migrate in P4.3" and survives with one test caller (`src-tauri/tests/imap_attachment_protocol.rs:433`). |
| Legacy fake mode | `src-tauri/tests/support/fake_imap.rs:50-91` | `Behavior::legacy`/`legacy_unselected_defaults` remain past the Phase 4 exit that required their removal. |
| CI browser parity | `.github/workflows/ci.yml:16` vs `playwright.config.ts:43-46` | CI installs only chromium; the config declares chromium and webkit for every run. |
| Compose staging scope | `src-tauri/src/commands/compose.rs:101-110`, `src/app/ipc/commands.ts:105` | `attachments_add_from_paths` takes only `paths`; the appendix A contract requires `{accountId,draftId,paths}` with scoped draft ownership. |
| Snooze ordering | `src-tauri/src/scheduler.rs:21-22` vs `src-tauri/src/db/threads.rs:68-72,97,139-141` | The Snoozed view orders by `snoozed_until` while the keyset cursor is `last_message_at`, so paging that view can skip or duplicate rows; `process_overdue` has no `ORDER BY`. |
