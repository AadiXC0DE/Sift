#!/bin/bash
# Idle-CPU benchmark: launch the app, let it settle, then measure CPU consumed
# over a fixed window.
#
# Method: cumulative CPU time (`ps -o time=`, user+system) is read for the app
# and for the WebKit helper processes it started, at the start and at the end of
# the window. Percent = (delta CPU seconds) / (window seconds) * 100, summed
# over the process set: 100% means one core saturated. `ps -o %cpu` is *not*
# used for the gate because it is a decaying average since process start, not a
# measurement of the window.
#
# The app runs against an isolated, network-free benchmark data directory
# (scripts/bench-fixture.sh), so nothing here is a sync burst hiding in "idle".
#
# Usage: scripts/bench-idle-cpu.sh [options]
#   --settle SEC   wait after the window-shown marker before the window (default 30)
#   --window SEC   measurement window (default 60, plan P10)
#   --interval SEC sub-sample interval inside the window (default 10)
#   --budget PCT   window ceiling in percent of one core (default 0.5)
#   --outbox MODE  normalise the fixture's outbox before launching (default: empty)
#                    empty   - delete queued operations (a settled app)
#                    pending - insert 10 undispatchable modify_labels operations,
#                              reproducing the backlog the app itself accumulated
#                              during measurement; see docs/perf.md
#   --profile P    release | debug (default release)
#   --data-dir DIR benchmark data dir (default: scripts/bench-fixture.sh)
#   --fresh-fixture rebuild the benchmark data dir first
#   --no-build     do not build; fail if the binary is missing
#
# Exit status: 0 when the window average is within budget, 1 on a breach or on a
# launch that never produced perf:window-shown.
set -euo pipefail
cd "$(dirname "$0")/.."

SETTLE=30
WINDOW=60
INTERVAL=10
BUDGET_PCT=0.5
PROFILE=release
DATA_DIR=""
FRESH=0
NO_BUILD=0
OUTBOX=empty

while [ $# -gt 0 ]; do
  case "$1" in
    --settle) SETTLE="$2"; shift 2 ;;
    --window) WINDOW="$2"; shift 2 ;;
    --interval) INTERVAL="$2"; shift 2 ;;
    --budget) BUDGET_PCT="$2"; shift 2 ;;
    --outbox) OUTBOX="$2"; shift 2 ;;
    --profile) PROFILE="$2"; shift 2 ;;
    --data-dir) DATA_DIR="$2"; shift 2 ;;
    --fresh-fixture) FRESH=1; shift ;;
    --no-build) NO_BUILD=1; shift ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) echo "bench-idle-cpu: unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$OUTBOX" in
  empty|pending) ;;
  *) echo "bench-idle-cpu: --outbox must be empty or pending" >&2; exit 2 ;;
esac

case "$PROFILE" in
  release) BIN=src-tauri/target/release/sift ;;
  debug) BIN=src-tauri/target/debug/sift ;;
  *) echo "bench-idle-cpu: --profile must be release or debug" >&2; exit 2 ;;
esac

if [ "$NO_BUILD" = 0 ] && [ ! -x "$BIN" ]; then
  if [ "$PROFILE" = release ] && [ ! -f dist/index.html ]; then
    echo "bench-idle-cpu: dist/ missing, running pnpm build"
    pnpm build
  fi
  echo "bench-idle-cpu: building $PROFILE binary (this is the slow step)..."
  if [ "$PROFILE" = release ]; then
    cargo build --manifest-path src-tauri/Cargo.toml --release --features custom-protocol --locked
  else
    cargo build --manifest-path src-tauri/Cargo.toml --locked
  fi
fi

if [ ! -x "$BIN" ]; then
  echo "bench-idle-cpu: no binary at $BIN and --no-build was given" >&2
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
       BENCH_WINDOW="$WINDOW" BENCH_INTERVAL="$INTERVAL" \
       BENCH_BUDGET_PCT="$BUDGET_PCT" BENCH_PROFILE="$PROFILE" BENCH_OUTBOX="$OUTBOX"

python3 - <<'PY'
import sys, time
import os
sys.path.insert(0, "scripts")
import bench_lib as B

binary = os.path.abspath(B.env("BENCH_BIN"))
data_dir = B.env("BENCH_DATA_DIR")
settle = float(B.env("BENCH_SETTLE"))
window = float(B.env("BENCH_WINDOW"))
interval = min(float(B.env("BENCH_INTERVAL")), window)
budget = float(B.env("BENCH_BUDGET_PCT"))
profile = B.env("BENCH_PROFILE")
outbox_mode = B.env("BENCH_OUTBOX")

fixture_db = os.path.join(data_dir, "sift.db")
outbox_rows = None
if os.path.exists(fixture_db):
    if outbox_mode == "empty":
        B.run(["sqlite3", fixture_db, "delete from outbox_ops;"])
    else:
        ids = [
            line.strip()
            for line in B.run(
                ["sqlite3", fixture_db,
                 "select id from messages order by internal_date desc limit 10"]
            ).splitlines()
            if line.strip()
        ]
        if len(ids) != 10:
            print(f"bench-idle-cpu: fixture has {len(ids)} message ids, need 10", file=sys.stderr)
            raise SystemExit(2)
        now = int(time.time() * 1000)
        stmts = "\n".join(
            "insert into outbox_ops (account_id,kind,payload,state,attempts,not_before,created_at) "
            "select account_id,'modify_labels',"
            f"'{{\"add\":[],\"ids\":[\"{mid}\"],\"remove\":[\"UNREAD\"]}}',"
            f"'pending',0,0,{now} from messages where id='{mid}';"
            for mid in ids
        )
        B.run(["sqlite3", fixture_db, stmts])
    outbox_rows = int(B.run(["sqlite3", fixture_db, "select count(*) from outbox_ops"]).strip() or 0)
    print(f"bench-idle-cpu: fixture outbox normalised to '{outbox_mode}' "
          f"({outbox_rows} row(s))")

info = B.binary_info(binary)
print(f"bench-idle-cpu: {profile} {info['path']} sha256={info['sha256']}")

before_webkit = B.webkit_pids()
launch = B.launch(binary, data_dir, "window-shown", 30)
if launch.latency_ms("window-shown") is None:
    raw = B.keep_raw(time.strftime("idle-cpu-%Y%m%dT%H%M%SZ.log", time.gmtime()), launch.log_text())
    launch.terminate()
    payload = {
        "measured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "unit": "percent_of_one_core",
        "status": "not measured",
        "reason": "the app never emitted perf:window-shown",
        "raw_log": raw,
        "binary": dict(info, profile=profile),
        "data_dir": data_dir,
    }
    B.merge_result("idle_cpu", payload)
    B.append_log(f"{payload['measured_at']} idle-cpu NOT MEASURED (no perf:window-shown)")
    print("bench-idle-cpu: FAILED: no perf:window-shown marker")
    sys.exit(1)

print(f"bench-idle-cpu: launch ok; settling {settle:g}s, then a {window:g}s window")
time.sleep(settle)

app_pid = launch.proc.pid
helpers = sorted(B.webkit_pids() - before_webkit)
pids = [app_pid] + helpers
print(f"bench-idle-cpu: process set app={app_pid} webkit={helpers}")

def snapshot() -> dict:
    return {pid: B.cpu_seconds(pid) for pid in pids if B.alive(pid)}

t0 = time.monotonic()
base = snapshot()
if not base:
    launch.terminate()
    print("bench-idle-cpu: FAILED: could not read CPU time")
    sys.exit(1)

intervals = []
prev, prev_t = base, t0
elapsed = 0.0
while elapsed < window:
    time.sleep(min(interval, window - elapsed))
    now_t = time.monotonic()
    cur = snapshot()
    dt = now_t - prev_t
    dcpu = 0.0
    for pid, seconds in cur.items():
        if pid in prev and prev[pid] is not None:
            dcpu += max(0.0, seconds - prev[pid])
    pct = 0.0 if dt <= 0 else dcpu / dt * 100
    intervals.append({"at": round(now_t - t0, 2), "cpu_seconds": round(dcpu, 4),
                      "percent": round(pct, 3)})
    print(f"  t+{now_t - t0:5.1f}s  cpu={dcpu:7.3f}s  {pct:6.3f}%  "
          f"({len(cur)} process(es) alive)")
    prev, prev_t = cur, now_t
    elapsed = now_t - t0

final_t = time.monotonic()
final = snapshot()
total_cpu = 0.0
for pid, seconds in final.items():
    if pid in base and base[pid] is not None:
        total_cpu += max(0.0, seconds - base[pid])
window_s = final_t - t0
window_pct = total_cpu / window_s * 100

# Reference only: `%cpu` is a decaying average since process start.
ps_cpu = {}
for pid in pids:
    out = B.run(["ps", "-o", "%cpu=", "-p", str(pid)]).strip()
    if out:
        try:
            ps_cpu[str(pid)] = float(out)
        except ValueError:
            pass

launch.terminate()
leftover = B.wait_for_exit(set(helpers))
breach = window_pct >= budget
payload = {
    "measured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "unit": "percent_of_one_core",
    "metric": "window_average_percent",
    "method": "delta of cumulative CPU time (ps -o time=, user+sys) over the window, "
              "summed across the app and its WebKit helpers",
    "profile": profile,
    "binary": dict(info, profile=profile),
    "data_dir": data_dir,
    "outbox": {"mode": outbox_mode, "pending_rows": outbox_rows},
    "settle_s": settle,
    "window_s": round(window_s, 3),
    "load_average": B.load_average(),
    "processes": {"app": app_pid, "webkit": helpers},
    "cpu_seconds": round(total_cpu, 4),
    "window_average_percent": round(window_pct, 4),
    "max_interval_percent": round(max(i["percent"] for i in intervals), 4),
    "intervals": intervals,
    "ps_percent_cpu_reference": ps_cpu,
    "budget_percent": budget,
    "pass": not breach,
    "attribution_verified": not leftover,
    "leftover_helpers": sorted(leftover),
}
worst_pct, gate_pass, history = B.gate_with_history(
    "idle_cpu", payload, round(window_pct, 4), budget, "percent_of_one_core",
    f"outbox={outbox_mode}({outbox_rows}) window={window_s:.1f}s "
    f"max_interval={payload['max_interval_percent']}%")
payload["worst_recorded_percent"] = worst_pct
payload["recorded_runs"] = len(history)
payload["pass"] = gate_pass
B.merge_result("idle_cpu", payload)
B.append_log(
    f"{payload['measured_at']} idle-cpu {profile} {info['sha256']} "
    f"window={window_s:.1f}s avg={round(window_pct, 4)}% "
    f"max_interval={payload['max_interval_percent']}% budget={budget:g}% "
    f"worst_recorded={worst_pct}% -> {'PASS' if gate_pass else 'BREACH'}"
)

print()
print(f"bench-idle-cpu: window {window_s:.1f}s total CPU {total_cpu:.3f}s "
      f"-> {window_pct:.4f}% of one core")
print(f"bench-idle-cpu: worst sub-interval {payload['max_interval_percent']:.4f}%")
print(f"bench-idle-cpu: worst over {len(history)} recorded run(s): {worst_pct}%")
print(f"bench-idle-cpu: budget < {budget:g}% -> {'PASS' if gate_pass else 'BREACH'}")
print("bench-idle-cpu: wrote docs/perf/results.json (key 'idle_cpu')")
sys.exit(0 if gate_pass else 1)
PY
