#!/bin/bash
# Assert the P10 budget rows that have a real measurement in
# docs/perf/results.json, and name every row that does not.
#
# Exit status:
#   0  every measured row is within budget
#   1  at least one measured row breached its budget
#   2  docs/perf/results.json is missing (run the bench scripts first)
#   3  --require-all was given and a row has no measurement
#
# Usage: scripts/bench-check.sh [--require-all] [--json]
set -euo pipefail
cd "$(dirname "$0")/.."

REQUIRE_ALL=0
AS_JSON=0
for arg in "$@"; do
  case "$arg" in
    --require-all) REQUIRE_ALL=1 ;;
    --json) AS_JSON=1 ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "bench-check: unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if [ ! -f docs/perf/results.json ]; then
  echo "bench-check: docs/perf/results.json not found; run scripts/bench-start.sh," >&2
  echo "             scripts/bench-mem.sh and scripts/bench-idle-cpu.sh first." >&2
  exit 2
fi

export BENCH_REQUIRE_ALL="$REQUIRE_ALL" BENCH_AS_JSON="$AS_JSON"

python3 - <<'PY'
import json, os, sys
sys.path.insert(0, "scripts")
import bench_lib as B

doc = json.load(open("docs/perf/results.json"))
require_all = os.environ["BENCH_REQUIRE_ALL"] == "1"
as_json = os.environ["BENCH_AS_JSON"] == "1"

UNMEASURED = [
    ("list focus change", "p95 <= 16 ms",
     "needs a native interaction harness: a keydown injected into the real webview "
     "with keydown->paint timing captured; no such harness exists (Playwright drives "
     "a browser, not the Tauri webview)"),
    ("cached thread body open", "p95 <= 50 ms",
     "needs the same native harness plus a per-open paint timeline; the app exposes "
     "no body-open marker"),
    ("archive response", "p95 <= 30 ms",
     "needs the native harness to click Archive and time the local list update"),
    ("local search at 100k messages", "p95 <= 30 ms",
     "needs a 100k-message fixture (fixtures/mailbox-100k is empty) and in-app timing "
     "of the query path"),
    ("list page 100 rows", "p95 <= 3 ms warm DB",
     "criterion benches exist (src-tauri/benches/{threads_query,search}.rs) but were "
     "not run by this task; they time the query, not the rendered page"),
    ("download working memory, 25 MiB attachment", "<= 8 MiB incremental",
     "needs a real 25 MiB attachment download driven through the app and sampled "
     "during the transfer; the hermetic fixture has no live account"),
    ("IPC payload sizes", "list <= 60 KiB, body <= 512 KiB",
     "needs instrumentation of the IPC boundary; not attempted here"),
    ("IMAP connections", "<= 3 per account during transfer",
     "needs a live account and network; the benchmark fixture is deliberately "
     "credential-free so no benchmark touches a real mailbox"),
]

rows = []
breaches = []
missing = []

def add(name, budget, measured, status, detail=""):
    rows.append({"budget": name, "target": budget, "measured": measured, "status": status,
                 "detail": detail})
    if status == "BREACH":
        breaches.append(rows[-1])
    if status == "not measured":
        missing.append(rows[-1])

launch = doc.get("launch")
if launch and launch.get("status") != "not measured":
    target = f"p95 <= {launch.get('budget_p95')} ms"
    worst = launch.get("worst_recorded_p95", launch["p95"])
    measured = (f"p95={launch['p95']} ms, p50={launch['p50']} ms, max={launch['max']} ms, "
                f"n={launch['n']}, {launch['profile']}, metric={launch['metric']}")
    if worst != launch["p95"]:
        measured += f"; worst recorded p95={worst} ms over {launch.get('recorded_runs', 1)} runs"
    status = "PASS" if launch.get("pass") else "BREACH"
    if launch.get("failed_launches"):
        status = "BREACH"
    add("warm-cache launch to usable Inbox", target, measured, status,
        f"sha256={launch.get('binary', {}).get('sha256')}")
else:
    add("warm-cache launch to usable Inbox", "p95 <= 400 ms",
        "not measured", "not measured", "run scripts/bench-start.sh")

memory = doc.get("memory")
if memory and memory.get("status") != "not measured":
    target = f"<= {memory.get('budget_mib')} MiB"
    worst = memory.get("worst_recorded_footprint_mib", memory["worst_footprint_mib"])
    measured = (f"worst={memory['worst_footprint_mib']} MiB over "
                f"{len(memory['samples'])} sample(s)")
    if worst != memory["worst_footprint_mib"]:
        measured += (f"; worst recorded={worst} MiB over "
                     f"{memory.get('recorded_runs', 1)} runs")
    add("total idle memory", target, measured, "PASS" if memory.get("pass") else "BREACH",
        f"method={memory.get('method', '')[:60]}...")
else:
    add("total idle memory", "<= 180 MiB", "not measured", "not measured",
        "run scripts/bench-mem.sh")

idle = doc.get("idle_cpu")
if idle and idle.get("status") != "not measured":
    target = f"< {idle.get('budget_percent')}% of one core"
    worst = idle.get("worst_recorded_percent", idle["window_average_percent"])
    measured = (f"{idle['window_average_percent']}% over {idle['window_s']} s "
                f"(worst sub-interval {idle['max_interval_percent']}%)")
    if worst != idle["window_average_percent"]:
        measured += f"; worst recorded={worst}%"
    add("idle CPU after sync settles", target, measured,
        "PASS" if idle.get("pass") else "BREACH", "")
else:
    add("idle CPU after sync settles", "< 0.5% over 60 s", "not measured", "not measured",
        "run scripts/bench-idle-cpu.sh")

for name, target, reason in UNMEASURED:
    add(name, target, "not measured", "not measured", reason)

if as_json:
    print(json.dumps({"rows": rows, "breaches": len(breaches), "missing": len(missing)},
                     indent=2, sort_keys=True))
else:
    print(f"{'budget row':52s} {'target':26s} measured")
    print("-" * 130)
    for r in rows:
        printed = r["measured"]
        if r["status"] == "not measured":
            printed = r["measured"]
        print(f"{r['budget']:52s} {r['target']:26s} {printed}")
    print("-" * 130)
    print(f"{len(rows)} rows: {sum(1 for r in rows if r['status'] == 'PASS')} pass, "
          f"{len(breaches)} breach, {len(missing)} not measured")
    if missing:
        print("\nnot measured:")
        for r in missing:
            print(f"  - {r['budget']}\n      {r['detail']}")
    if breaches:
        print("\nBREACHED:")
        for r in breaches:
            print(f"  - {r['budget']}: {r['measured']} (target {r['target']})")

if breaches:
    sys.exit(1)
if require_all and missing:
    sys.exit(3)
sys.exit(0)
PY
