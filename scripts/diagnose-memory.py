#!/usr/bin/env python3
"""Sample compositor memory and its service cgroup without changing the session."""

import argparse
import json
from pathlib import Path
import subprocess
import time


ROLLUP_FIELDS = ("Rss", "Pss", "Anonymous", "Swap")
CGROUP_FIELDS = ("anon", "file", "shmem", "kernel")


def read_kib_fields(path, fields):
    wanted = set(fields)
    values = {}
    for line in path.read_text().splitlines():
        name, separator, remainder = line.partition(":")
        if separator and name in wanted:
            values[name] = int(remainder.split()[0]) * 1024
    return values


def cgroup_dir(pid):
    for line in (Path("/proc") / str(pid) / "cgroup").read_text().splitlines():
        hierarchy, controllers, path = line.split(":", 2)
        if hierarchy == "0" and not controllers:
            return Path("/sys/fs/cgroup") / path.lstrip("/")
    return None


def sample(pid, group):
    proc = Path("/proc") / str(pid)
    # The numeric fields start after the final ')' because comm can contain parentheses.
    stat = (proc / "stat").read_text().rpartition(")")[2].split()
    rollup = read_kib_fields(proc / "smaps_rollup", ROLLUP_FIELDS)
    fd_count = sum(1 for _ in (proc / "fd").iterdir())
    cgroup = None
    if group is not None:
        memory_stat = {}
        for line in (group / "memory.stat").read_text().splitlines():
            name, _, value = line.partition(" ")
            if name in CGROUP_FIELDS:
                memory_stat[name] = int(value)
        cgroup = {
            "current_bytes": int((group / "memory.current").read_text()),
            "stat_bytes": memory_stat,
        }
    return {
        "elapsed_monotonic": time.monotonic(),
        "pid_start_ticks": int(stat[19]),
        "process_bytes": rollup,
        "fd_count": fd_count,
        "service_cgroup": cgroup,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, help="Compositor PID; default: realand.service MainPID")
    parser.add_argument("--seconds", type=int, default=60, help="Sampling duration (1–3600 seconds)")
    parser.add_argument("--interval", type=float, default=5, help="Sampling interval in seconds")
    parser.add_argument("--output", type=Path, required=True, help="New JSON report path")
    args = parser.parse_args()
    if not 1 <= args.seconds <= 3600 or args.interval <= 0:
        parser.error("--seconds must be 1–3600 and --interval must be positive")
    if args.output.exists():
        parser.error("Output already exists")

    pid = args.pid
    if pid is None:
        pid = int(subprocess.check_output(
            ["systemctl", "--user", "show", "realand.service", "-p", "MainPID", "--value"],
            text=True, timeout=3,
        ).strip())
    if pid <= 0:
        parser.error("Realand is not running; specify --pid")

    group = cgroup_dir(pid)
    start = time.monotonic()
    samples = [sample(pid, group)]
    while True:
        elapsed = time.monotonic() - start
        if elapsed >= args.seconds:
            break
        time.sleep(min(args.interval, args.seconds - elapsed))
        current = sample(pid, group)
        if current["pid_start_ticks"] != samples[0]["pid_start_ticks"]:
            raise RuntimeError("Compositor restarted or PID was reused; report discarded")
        samples.append(current)

    first, last = samples[0], samples[-1]
    report = {
        "pid": pid,
        "duration_seconds": last["elapsed_monotonic"] - first["elapsed_monotonic"],
        "cgroup_path": str(group) if group is not None else None,
        "samples": samples,
        "notes": [
            "Process RSS/PSS/Anonymous come from smaps_rollup; the service cgroup also includes child processes and shared buffers.",
            "A short-term increase alone does not prove a leak; repeat with the same workload and after resources are released.",
            "GPU memory may not be fully visible in process RSS or these cgroup counters.",
        ],
    }
    with args.output.open("x") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")

    process_delta = (last["process_bytes"]["Rss"] - first["process_bytes"]["Rss"]) / 1048576
    group_delta = None
    if first["service_cgroup"] is not None and last["service_cgroup"] is not None:
        group_delta = (last["service_cgroup"]["current_bytes"] - first["service_cgroup"]["current_bytes"]) / 1048576
    group_text = f", service cgroup {group_delta:+.1f} MiB" if group_delta is not None else ""
    print(f"Saved {args.output}: compositor RSS {process_delta:+.1f} MiB{group_text}, FD {first['fd_count']}→{last['fd_count']}")


if __name__ == "__main__":
    main()
