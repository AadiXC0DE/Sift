"""Shared measurement helpers for scripts/bench-*.sh.

Everything here measures a real, running process. Nothing estimates, simulates
or fills in a default: a metric that cannot be read raises or returns None, and
the calling script reports it as `not measured`.
"""

from __future__ import annotations

import json
import math
import os
import re
import subprocess
import sys
import tempfile
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(REPO, "docs", "perf", "results.json")
RAW_DIR = os.path.join(REPO, "docs", "perf", "raw")

MARKER_RE = re.compile(r"^perf:([a-z0-9-]+) (\d+)$", re.MULTILINE)
# `footprint -p <pid>` header: `sift [9586]: 64-bit    Footprint: 34 MB (...)`
FOOTPRINT_RE = re.compile(r"Footprint:\s*([0-9.]+)\s*(B|KB|MB|GB)\b")
_WEBKIT_MARK = "WebKit.framework/Versions/A/XPCServices"


def now_ms() -> int:
    return int(time.time() * 1000)


def run(cmd: list[str], timeout: float = 30.0) -> str:
    return subprocess.run(
        cmd, capture_output=True, text=True, timeout=timeout, check=False
    ).stdout


def sha256(path: str) -> str:
    import hashlib

    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def git_head() -> str:
    try:
        out = run(["git", "-C", REPO, "rev-parse", "HEAD"]).strip()
        return out or "unknown"
    except Exception:
        return "unknown"


def host_info() -> dict:
    def sysctl(key: str) -> str:
        try:
            return run(["sysctl", "-n", key]).strip()
        except Exception:
            return "unknown"

    try:
        os_name = run(["sw_vers", "-productVersion"]).strip()
        build = run(["sw_vers", "-buildVersion"]).strip()
    except Exception:
        os_name, build = "unknown", "unknown"
    try:
        mem_gib = round(int(sysctl("hw.memsize")) / (1024**3), 1)
    except Exception:
        mem_gib = None
    return {
        "model": sysctl("hw.model"),
        "cpu": sysctl("machdep.cpu.brand_string"),
        "cores": os.cpu_count(),
        "memory_gib": mem_gib,
        "os": f"macOS {os_name} ({build})" if os_name != "unknown" else "unknown",
    }


def load_average() -> dict:
    """1/5/15-minute load average: the measurement's noise context."""
    try:
        one, five, fifteen = os.getloadavg()
    except OSError:
        return {}
    return {"1m": round(one, 2), "5m": round(five, 2), "15m": round(fifteen, 2)}


def binary_info(path: str) -> dict:
    st = os.stat(path)
    return {
        "path": os.path.relpath(path, REPO),
        "sha256": sha256(path)[:16],
        "bytes": st.st_size,
        "mtime": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(st.st_mtime)),
    }


# ---------------------------------------------------------------- process set


def webkit_pids() -> set[int]:
    """PIDs of WebKit's XPC helper processes (GPU/WebContent/Networking).

    These are spawned by launchd (ppid 1), so they are not children of the app;
    the caller attributes them by taking the difference of this set across the
    app's lifetime and confirming they exit with the app.
    """
    pids: set[int] = set()
    for line in run(["ps", "-axo", "pid=,args="]).splitlines():
        pid, _, args = line.strip().partition(" ")
        if _WEBKIT_MARK in args:
            try:
                pids.add(int(pid))
            except ValueError:
                pass
    return pids


def webkit_role(pid: int) -> str:
    """`WebContent` / `GPU` / `Networking` for a WebKit helper pid."""
    out = run(["ps", "-o", "comm=", "-p", str(pid)]).strip()
    name = os.path.basename(out.splitlines()[0]) if out else ""
    prefix = "com.apple.WebKit."
    if name.startswith(prefix):
        name = name[len(prefix) :]
    if name.endswith(".xpc"):
        name = name[: -len(".xpc")]
    return "webkit." + (name or "helper")


def parse_ps_time(value: str) -> float:
    """`ps -o time=` is cumulative CPU (user+sys): `[HH:]MM:SS.ss`."""
    value = value.strip()
    if not value:
        raise ValueError("empty ps time")
    parts = value.split(":")
    seconds = 0.0
    for part in parts:
        seconds = seconds * 60 + float(part)
    return seconds


def rss_kib(pid: int) -> int | None:
    out = run(["ps", "-o", "rss=", "-p", str(pid)]).strip()
    return int(out) if out else None


def cpu_seconds(pid: int) -> float | None:
    out = run(["ps", "-o", "time=", "-p", str(pid)]).strip()
    if not out:
        return None
    try:
        return parse_ps_time(out)
    except ValueError:
        return None


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


_UNIT_MIB = {"B": 1 / 1048576, "KB": 1 / 1024, "MB": 1.0, "GB": 1024.0}


def footprint_mib(pid: int) -> float | None:
    """Physical (dirty) footprint of one process, in MiB.

    `footprint` is Apple's own per-process accounting; the number it prints is
    dirty + compressed-reclaimable memory, excluding clean file-backed pages
    (shared framework code), which are not attributable to one process.
    """
    try:
        out = run(["footprint", "-p", str(pid)], timeout=60)
    except Exception:
        return None
    match = FOOTPRINT_RE.search(out)
    if not match:
        return None
    return float(match.group(1)) * _UNIT_MIB[match.group(2)]


# ------------------------------------------------------------------- launching


class Launch:
    def __init__(self, proc, log_path, spawn_ms):
        self.proc = proc
        self.log_path = log_path
        self.spawn_ms = spawn_ms
        self.markers: dict[str, int] = {}
        self.exit_code: int | None = None

    def log_text(self) -> str:
        try:
            with open(self.log_path, "r", errors="replace") as fh:
                return fh.read()
        except OSError:
            return ""

    def latency_ms(self, marker: str) -> float | None:
        at = self.markers.get(marker)
        return None if at is None else float(at - self.spawn_ms)

    def empty_output(self) -> bool:
        """True when the process wrote nothing at all.

        Observed twice across ~40 automated launches and not reproduced on
        retry: the app is registered with `tauri-plugin-single-instance`, so a
        launch that races a still-closing previous instance connects to the
        existing socket and exits silently with no marker.
        """
        return not self.log_text().strip()

    def terminate(self, grace: float = 8.0) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(grace)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(5)
        self.exit_code = self.proc.returncode


def launch(binary: str, data_dir: str, marker: str, timeout: float) -> Launch:
    """Start the app and wait until `marker` (or the process exits) is seen.

    The latency is taken from the app's own epoch-millisecond marker minus the
    epoch-millisecond reading taken immediately before `Popen`, so both ends are
    on the same clock and no harness polling delay is included.
    """
    spawn_ms = now_ms()
    handle, log_path = tempfile.mkstemp(prefix="sift-bench-", suffix=".log")
    os.close(handle)
    env = dict(os.environ)
    env["SIFT_DATA_DIR"] = data_dir
    # Make launches deterministic: no demo seeding, no locale-dependent paths.
    env.pop("SIFT_DEMO", None)
    with open(log_path, "wb") as sink:
        proc = subprocess.Popen([binary], stdout=sink, stderr=subprocess.STDOUT, env=env)
    launch = Launch(proc, log_path, spawn_ms)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        text = launch.log_text()
        for name, at in MARKER_RE.findall(text):
            launch.markers.setdefault(name, int(at))
        if marker in launch.markers:
            break
        if proc.poll() is not None:
            break
        time.sleep(0.02)
    return launch


def wait_for_exit(pids: set[int], timeout: float = 6.0) -> set[int]:
    """Wait for processes to disappear; returns those still alive."""
    deadline = time.monotonic() + timeout
    remaining = set(pids)
    while time.monotonic() < deadline and remaining:
        remaining = {pid for pid in remaining if alive(pid)}
        if remaining:
            time.sleep(0.2)
    return remaining


# ---------------------------------------------------------------- statistics


def percentile(samples: list[float], pct: float) -> float:
    """Nearest-rank percentile (the value at rank ceil(p/100 * n))."""
    ordered = sorted(samples)
    rank = max(1, math.ceil(pct / 100 * len(ordered)))
    return ordered[rank - 1]


def stats(samples: list[float]) -> dict:
    ordered = sorted(samples)
    return {
        "n": len(ordered),
        "min": round(ordered[0], 3),
        "p50": round(percentile(ordered, 50), 3),
        "p95": round(percentile(ordered, 95), 3),
        "max": round(ordered[-1], 3),
        "mean": round(sum(ordered) / len(ordered), 3),
        "samples": [round(s, 3) for s in samples],
        "percentile_method": "nearest-rank",
    }


# ------------------------------------------------------------------- results


def read_results() -> dict:
    if os.path.exists(RESULTS):
        try:
            with open(RESULTS) as fh:
                return json.load(fh)
        except (OSError, ValueError):
            return {}
    return {}


def gate_with_history(
    key: str, payload: dict, value: float, budget: float, unit: str, detail: str = ""
) -> tuple[float, bool, list]:
    """Append this measurement to the key's history and gate on the worst.

    Several bench runs of the same build can disagree (scheduling, background
    load). Gating on the best run would be cherry-picking, so the pass/fail
    verdict uses the worst value recorded for the same budget, and `history`
    keeps every run's headline number in the artifact. Only entries whose
    `budget` matches the current budget are compared.
    """
    previous = (read_results().get(key) or {}).get("history") or []
    entry = {
        "measured_at": payload.get("measured_at"),
        "value": value,
        "budget": budget,
        "unit": unit,
        "load_average": payload.get("load_average", {}),
        "detail": detail,
    }
    history = previous + [entry]
    payload["history"] = history
    comparable = [
        e["value"]
        for e in history
        if e.get("budget") == budget and e.get("unit") == unit and isinstance(e.get("value"), (int, float))
    ]
    worst = max(comparable) if comparable else value
    return worst, worst <= budget, history


def merge_result(key: str, payload: dict) -> dict:
    """Merge one metric block into docs/perf/results.json and return the file."""
    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    doc: dict = {}
    if os.path.exists(RESULTS):
        try:
            with open(RESULTS) as fh:
                doc = json.load(fh)
        except (OSError, ValueError):
            doc = {}
    doc.setdefault("host", host_info())
    doc.setdefault("git_head", git_head())
    doc["generated_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    doc[key] = payload
    tmp = RESULTS + ".tmp"
    with open(tmp, "w") as fh:
        json.dump(doc, fh, indent=2, sort_keys=True)
        fh.write("\n")
    os.replace(tmp, RESULTS)
    return doc


def append_log(line: str) -> None:
    os.makedirs(os.path.dirname(RESULTS), exist_ok=True)
    with open(os.path.join(os.path.dirname(RESULTS), "bench.log"), "a") as fh:
        fh.write(line.rstrip("\n") + "\n")


def keep_raw(name: str, content: str) -> str:
    os.makedirs(RAW_DIR, exist_ok=True)
    path = os.path.join(RAW_DIR, name)
    with open(path, "w") as fh:
        fh.write(content)
    return os.path.relpath(path, REPO)


def env(name: str, default: str | None = None) -> str:
    value = os.environ.get(name, default)
    if value is None:
        print(f"bench: missing required env {name}", file=sys.stderr)
        raise SystemExit(2)
    return value
