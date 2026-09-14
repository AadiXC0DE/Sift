#!/bin/bash
# Warm-cache launch benchmark: launch the built Sift binary N times and report
# p50/p95 from the app's own perf markers. Nothing here is hardcoded: every
# number comes from a marker the running process wrote, and a run that never
# emits the requested marker is reported as failed, never substituted.
#
# Markers (src-tauri/src/lib.rs, src-tauri/src/commands/system.rs):
#   perf:window-shown <epoch_ms>  Rust setup: the webview window exists and the
#                                 native background colour is applied.
#   perf:first-paint <epoch_ms>   Frontend rAF after App mounts: the UI shell is
#                                 painted. This is the "usable Inbox" proxy --
#                                 the app has no marker for "first inbox row
#                                 painted", see docs/perf.md.
# Latency = marker epoch-ms - epoch-ms read immediately before spawn, so both
# ends share one clock and no harness polling delay is counted.
#
# The app runs against an isolated benchmark data directory
# (scripts/bench-fixture.sh), never against the real mailbox.
#
# Usage: scripts/bench-start.sh [options]
#   -n, --runs N       launches to perform (default 10)
#   --metric NAME      perf marker to gate on: first-paint | window-shown
#                      (default first-paint)
#   --profile P        release | debug (default release)
#   --data-dir DIR     benchmark data dir (default: scripts/bench-fixture.sh)
#   --fresh-fixture    rebuild the benchmark data dir before measuring
#   --timeout SEC      per-launch wait for the marker (default 30)
#   --gap SEC          pause between launches (default 3)
#   --budget MS        p95 ceiling in milliseconds (default 400, plan P10)
#   --no-build         do not build; fail if the binary is missing
#
# Exit status: 0 when every launch produced the marker and p95 is within budget,
# 1 on a breach or a failed launch.
set -euo pipefail
cd "$(dirname "$0")/.."

RUNS=10
METRIC=first-paint
PROFILE=release
DATA_DIR=""
FRESH=0
TIMEOUT=30
GAP=3
BUDGET_MS=400
NO_BUILD=0

while [ $# -gt 0 ]; do
  case "$1" in
    -n|--runs) RUNS="$2"; shift 2 ;;
    --metric) METRIC="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --fresh-fixture) FRESH=1; shift ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    --gap) GAP="$2"; shift 2 ;;
    --budget) BUDGET_MS="$2"; shift 2 ;;
    --no-build) NO_BUILD=1; shift ;;
    -h|--help) sed -n '2,40p' "$0"; exit 0 ;;
    *) echo "bench-start: unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$PROFILE" in
  release) BIN=src-tauri/target/release/sift ;;
  debug) BIN=src-tauri/target/debug/sift ;;
  *) echo "bench-start: --profile must be release or debug" >&2; exit 2 ;;
esac

if [ "$NO_BUILD" = 0 ]; then
  if [ "$PROFILE" = release ] && [ ! -f dist/index.html ]; then
    echo "bench-start: dist/ missing, running pnpm build"
    pnpm build
  fi
  if [ ! -x "$BIN" ]; then
    echo "bench-start: building $PROFILE binary (this is the slow step)..."
    if [ "$PROFILE" = release ]; then
      # `tauri build` compiles with custom-protocol so the frontend is embedded;
      # without it the binary loads devUrl and no frontend marker is emitted.
      cargo build --manifest-path src-tauri/Cargo.toml --release --features custom-protocol --locked
    else
      cargo build --manifest-path src-tauri/Cargo.toml --locked
    fi
  fi
fi

if [ ! -x "$BIN" ]; then
  echo "bench-start: no binary at $BIN and --no-build was given" >&2
  exit 2
fi

if [ -z "$DATA_DIR" ]; then
  if [ "$FRESH" = 1 ]; then
    DATA_DIR=$(bash scripts/bench-fixture.sh --fresh)
  else
    DATA_DIR=$(bash scripts/bench-fixture.sh)
  fi
fi
if [ ! -d "$DATA_DIR" ]; then
  echo "bench-start: data dir does not exist: $DATA_DIR" >&2
  exit 2
fi

if [ "$PROFILE" = debug ] && [ "$METRIC" = first-paint ]; then
  echo "bench-start: note: a debug build loads devUrl (http://localhost:1420);" >&2
  echo "             run 'pnpm dev:vite' first or first-paint will never appear." >&2
fi

export BENCH_BIN="$BIN" BENCH_DATA_DIR="$DATA_DIR" BENCH_RUNS="$RUNS" \
       BENCH_METRIC="$METRIC" BENCH_TIMEOUT="$TIMEOUT" BENCH_BUDGET_MS="$BUDGET_MS" \
       BENCH_PROFILE="$PROFILE" BENCH_GAP="$GAP"

python3 - <<'PY'
import os, sys, time
sys.path.insert(0, "scripts")
import bench_lib as B

binary = os.path.abspath(B.env("BENCH_BIN"))
data_dir = B.env("BENCH_DATA_DIR")
runs_n = int(B.env("BENCH_RUNS"))
metric = B.env("BENCH_METRIC")
timeout = float(B.env("BENCH_TIMEOUT"))
budget = float(B.env("BENCH_BUDGET_MS"))
profile = B.env("BENCH_PROFILE")
gap = float(B.env("BENCH_GAP"))

info = B.binary_info(binary)
stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
print(f"bench-start: {profile} {info['path']} sha256={info['sha256']} bytes={info['bytes']}")
print(f"bench-start: data dir {data_dir}")
print(f"bench-start: metric={metric} runs={runs_n} budget(p95)={budget:g} ms")

samples, failed, rows = [], [], []
for i in range(1, runs_n + 1):
    if i > 1:
        time.sleep(gap)
    launch = B.launch(binary, data_dir, metric, timeout)
    latency = launch.latency_ms(metric)
    raw = B.keep_raw(f"launch-{stamp}-run{i}.log", launch.log_text())
    row = {
        "run": i,
        "latency_ms": None if latency is None else round(latency, 3),
        "markers": launch.markers,
        "exit_code": launch.proc.poll(),
        "raw_log": raw,
        "empty_output": launch.empty_output(),
    }
    if latency is None:
        launch.terminate()
        row["exit_code"] = launch.exit_code
        failed.append(row)
        rows.append(row)
        hint = " (empty log: single-instance lock held by a closing instance?)" \
            if row["empty_output"] else ""
        print(f"  run {i:2d}: FAILED (no perf:{metric}; markers={sorted(launch.markers)}){hint}")
        continue
    samples.append(latency)
    launch.terminate()
    row["exit_code"] = launch.exit_code
    rows.append(row)
    print(f"  run {i:2d}: {latency:8.1f} ms   window-shown={launch.latency_ms('window-shown')}")

if not samples:
    payload = {
        "measured_at": B.time.strftime("%Y-%m-%dT%H:%M:%SZ", B.time.gmtime()),
        "metric": metric,
        "unit": "ms",
        "status": "not measured",
        "reason": f"no launch emitted perf:{metric}",
        "runs": rows,
        "binary": dict(info, profile=profile),
        "data_dir": data_dir,
    }
    B.merge_result("launch", payload)
    B.append_log(f"{payload['measured_at']} launch NOT MEASURED (no perf:{metric})")
    print("bench-start: FAILED: not one launch produced the marker")
    sys.exit(1)

summary = B.stats(samples)
breach = summary["p95"] > budget
payload = {
    "measured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "metric": metric,
    "unit": "ms",
    "method": "app perf marker epoch-ms minus pre-spawn epoch-ms; nearest-rank percentile",
    "profile": profile,
    "binary": dict(info, profile=profile),
    "data_dir": data_dir,
    "load_average": B.load_average(),
    "budget_p95": budget,
    "pass": (not breach) and not failed,
    "failed_launches": failed,
    "runs": rows,
    **summary,
}
worst_p95, gate_pass, history = B.gate_with_history(
    "launch", payload, summary["p95"], budget, "ms",
    f"n={summary['n']} p50={summary['p50']}ms max={summary['max']}ms metric={metric}")
payload["worst_recorded_p95"] = worst_p95
payload["recorded_runs"] = len(history)
payload["pass"] = gate_pass and not failed
B.merge_result("launch", payload)
B.append_log(
    f"{payload['measured_at']} launch {profile} {info['sha256']} "
    f"n={summary['n']} p50={summary['p50']}ms p95={summary['p95']}ms "
    f"max={summary['max']}ms metric={metric} budget={budget:g}ms "
    f"worst_recorded_p95={worst_p95}ms -> {'PASS' if payload['pass'] else 'BREACH'}"
)

print()
print(f"bench-start: n={summary['n']} min={summary['min']} p50={summary['p50']} "
      f"p95={summary['p95']} max={summary['max']} mean={summary['mean']} ms")
print(f"bench-start: percentile method nearest-rank (p95 = max at n={summary['n']})")
print(f"bench-start: budget p95 <= {budget:g} ms -> "
      f"{'PASS' if not breach else 'BREACH'}")
if worst_p95 != summary["p95"]:
    print(f"bench-start: worst over {len(history)} recorded run(s): p95={worst_p95} ms")
if failed:
    print(f"bench-start: {len(failed)} launch(es) failed to emit perf:{metric}")
print(f"bench-start: wrote docs/perf/results.json (key 'launch') and docs/perf/raw/*")
sys.exit(0 if payload["pass"] else 1)
PY
