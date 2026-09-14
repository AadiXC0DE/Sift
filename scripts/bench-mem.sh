#!/bin/bash
# Idle-memory benchmark: launch the app, let it settle, and measure the resident
# footprint of the Rust process plus the WebKit helper processes the app started.
#
# Method (see docs/perf.md for the full write-up):
#   * `footprint -p <pid>` is Apple's own per-process accounting. The value it
#     prints for each process is counted; clean file-backed pages (shared
#     framework code) are excluded, which is the right thing for "memory this
#     app costs".
#   * `ps -o rss=` is reported alongside it because RSS is what most people
#     compare; it double-counts pages shared between processes, so the two
#     totals differ.
#   * WebKit's GPU/WebContent/Networking helpers are spawned by launchd (ppid 1),
#     not by the app, so they are attributed by taking the difference of the
#     WebKit pid set before launch and after settle, then confirming those pids
#     exit when the app exits.
#
# Usage: scripts/bench-mem.sh [options]
#   --settle SEC     wait after the window-shown marker before sampling (default 30)
#   --samples N      measurement rounds (default 3)
#   --interval SEC   pause between rounds (default 5)
#   --budget MIB     total footprint ceiling (default 180, plan P10)
#   --profile P      release | debug (default release)
#   --data-dir DIR   benchmark data dir (default: scripts/bench-fixture.sh)
#   --fresh-fixture  rebuild the benchmark data dir first
#   --no-build       do not build; fail if the binary is missing
#
# Exit status: 0 when the worst sample is within budget, 1 on a breach or on a
# launch that never produced perf:window-shown.
set -euo pipefail
cd "$(dirname "$0")/.."

SETTLE=30
SAMPLES=3
INTERVAL=5
BUDGET_MIB=180
PROFILE=release
DATA_DIR=""
FRESH=0
NO_BUILD=0

while [ $# -gt 0 ]; do
  case "$1" in
    --settle) SETTLE="$2"; shift 2 ;;
    --samples) SAMPLES="$2"; shift 2 ;;
    --interval) INTERVAL="$2"; shift 2 ;;
    --budget) BUDGET_MIB="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --fresh-fixture) FRESH=1; shift ;;
    --no-build) NO_BUILD=1; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "bench-mem: unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$PROFILE" in
  release) BIN=src-tauri/target/release/sift ;;
  debug) BIN=src-tauri/target/debug/sift ;;
  *) echo "bench-mem: --profile must be release or debug" >&2; exit 2 ;;
esac

if [ "$NO_BUILD" = 0 ] && [ ! -x "$BIN" ]; then
  if [ "$PROFILE" = release ] && [ ! -f dist/index.html ]; then
    echo "bench-mem: dist/ missing, running pnpm build"
    pnpm build
  fi
  echo "bench-mem: building $PROFILE binary (this is the slow step)..."
  if [ "$PROFILE" = release ]; then
    cargo build --manifest-path src-tauri/Cargo.toml --release --features custom-protocol --locked
  else
    cargo build --manifest-path src-tauri/Cargo.toml --locked
  fi
fi

if [ ! -x "$BIN" ]; then
  echo "bench-mem: no binary at $BIN and --no-build was given" >&2
  exit 2
fi

if [ -z "$DATA_DIR" ]; then
  if [ "$FRESH" = 1 ]; then
    DATA_DIR=$(bash scripts/bench-fixture.sh --fresh)
  else
    DATA_DIR=$(bash scripts/bench-fixture.sh)
  fi
fi

export BENCH_BIN="$BIN" BENCH_DATA_DIR="$DATA_DIR" BENCH_SETTLE="$SETTLE" \
       BENCH_SAMPLES="$SAMPLES" BENCH_INTERVAL="$INTERVAL" \
       BENCH_BUDGET_MIB="$BUDGET_MIB" BENCH_PROFILE="$PROFILE"

python3 - <<'PY'
import os, sys, time
sys.path.insert(0, "scripts")
import bench_lib as B

binary = os.path.abspath(B.env("BENCH_BIN"))
data_dir = B.env("BENCH_DATA_DIR")
settle = float(B.env("BENCH_SETTLE"))
rounds = int(B.env("BENCH_SAMPLES"))
interval = float(B.env("BENCH_INTERVAL"))
budget = float(B.env("BENCH_BUDGET_MIB"))
profile = B.env("BENCH_PROFILE")

info = B.binary_info(binary)
print(f"bench-mem: {profile} {info['path']} sha256={info['sha256']}")
print(f"bench-mem: method footprint (dirty) + ps RSS, WebKit helpers by pid-set diff")

before_webkit = B.webkit_pids()
launch = B.launch(binary, data_dir, "window-shown", 30)
latency = launch.latency_ms("window-shown")
if latency is None:
    raw = B.keep_raw(time.strftime("mem-%Y%m%dT%H%M%SZ.log", time.gmtime()), launch.log_text())
    launch.terminate()
    payload = {
        "measured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "unit": "MiB",
        "status": "not measured",
        "reason": "the app never emitted perf:window-shown",
        "raw_log": raw,
        "binary": dict(info, profile=profile),
        "data_dir": data_dir,
    }
    B.merge_result("memory", payload)
    B.append_log(f"{payload['measured_at']} memory NOT MEASURED (no perf:window-shown)")
    print("bench-mem: FAILED: no perf:window-shown marker")
    sys.exit(1)

print(f"bench-mem: launch ok ({latency:.0f} ms to window-shown); settling {settle:g}s")
time.sleep(settle)

app_pid = launch.proc.pid
attributed = sorted(B.webkit_pids() - before_webkit)
samples = []
for round_no in range(1, rounds + 1):
    if round_no > 1:
        time.sleep(interval)
    rows = []
    app_fp = B.footprint_mib(app_pid)
    app_rss = B.rss_kib(app_pid)
    rows.append({"role": "app", "pid": app_pid, "footprint_mib": app_fp,
                 "rss_mib": None if app_rss is None else round(app_rss / 1024, 1)})
    for pid in attributed:
        if not B.alive(pid):
            continue
        fp = B.footprint_mib(pid)
        rss = B.rss_kib(pid)
        rows.append({"role": B.webkit_role(pid), "pid": pid, "footprint_mib": fp,
                     "rss_mib": None if rss is None else round(rss / 1024, 1)})
    total_fp = None
    if all(r["footprint_mib"] is not None for r in rows):
        total_fp = round(sum(r["footprint_mib"] for r in rows), 1)
    total_rss = None
    if all(r["rss_mib"] is not None for r in rows):
        total_rss = round(sum(r["rss_mib"] for r in rows), 1)
    sample = {"round": round_no, "at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "processes": rows, "total_footprint_mib": total_fp, "total_rss_mib": total_rss}
    samples.append(sample)
    print(f"  sample {round_no}: total footprint {total_fp} MiB / RSS {total_rss} MiB")
    for r in rows:
        print(f"    {r['role']:16s} pid {r['pid']:6d}  footprint "
              f"{r['footprint_mib'] if r['footprint_mib'] is not None else 'n/a'} MiB  "
              f"rss {r['rss_mib'] if r['rss_mib'] is not None else 'n/a'} MiB")

launch.terminate()
leftover = B.wait_for_exit(set(attributed))
attribution_verified = not leftover

measured = [s["total_footprint_mib"] for s in samples if s["total_footprint_mib"] is not None]
if not measured:
    payload = {
        "measured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "unit": "MiB",
        "status": "not measured",
        "reason": "footprint could not be read for every process in the app set",
        "samples": samples,
        "binary": dict(info, profile=profile),
        "data_dir": data_dir,
    }
    B.merge_result("memory", payload)
    B.append_log(f"{payload['measured_at']} memory NOT MEASURED (footprint unreadable)")
    print("bench-mem: FAILED: footprint unreadable")
    sys.exit(1)

worst = max(measured)
breach = worst > budget
payload = {
    "measured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "unit": "MiB",
    "metric": "total_footprint_mib",
    "method": "footprint -p (dirty physical footprint) summed over the app and the "
              "WebKit helper processes attributed by pid-set difference",
    "limitations": [
        "footprint excludes clean file-backed pages (shared framework code), "
        "so it under-counts any memory genuinely shared with other apps",
        "ps RSS is reported next to it and double-counts pages shared between "
        "the app and its WebKit helpers",
        "WebKit helpers have ppid 1 and carry no owner in argv, so attribution "
        "is by pid-set difference across the app's lifetime",
    ],
    "profile": profile,
    "binary": dict(info, profile=profile),
    "data_dir": data_dir,
    "settle_s": settle,
    "load_average": B.load_average(),
    "samples": samples,
    "worst_footprint_mib": worst,
    "budget_mib": budget,
    "pass": not breach,
    "attribution_verified": attribution_verified,
    "leftover_helpers": sorted(leftover),
}
worst_recorded, gate_pass, history = B.gate_with_history(
    "memory", payload, worst, budget, "MiB",
    "app + WebKit helpers, footprint, settle "
    f"{settle:g}s")
payload["worst_recorded_footprint_mib"] = worst_recorded
payload["recorded_runs"] = len(history)
payload["pass"] = gate_pass
payload["history_detail"] = history
B.merge_result("memory", payload)
B.append_log(
    f"{payload['measured_at']} memory {profile} {info['sha256']} "
    f"worst={worst}MiB worst_recorded={worst_recorded}MiB budget={budget:g}MiB "
    f"-> {'PASS' if payload['pass'] else 'BREACH'}"
)

print()
print(f"bench-mem: worst total footprint {worst} MiB over {len(measured)} sample(s)")
print(f"bench-mem: worst over {len(history)} recorded run(s): {worst_recorded} MiB")
print(f"bench-mem: budget <= {budget:g} MiB -> {'PASS' if gate_pass else 'BREACH'}")
print(f"bench-mem: helpers exited with the app: {attribution_verified}")
if leftover:
    print(f"bench-mem: warning: helpers still alive after exit: {sorted(leftover)}")
print("bench-mem: wrote docs/perf/results.json (key 'memory')")
sys.exit(0 if payload["pass"] else 1)
PY
