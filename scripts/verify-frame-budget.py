#!/usr/bin/env python3
"""Read-only Clarion frame-budget capture.

Collects IPC snapshots without sending input or changing a session. Exact run
percentiles require complete CPU sequence coverage. Hardware smoothness requires
fixed-refresh presentation feedback during explicitly continuous activity.
Exit codes: 0 PASS, 1 FAIL, 2 INCOMPLETE, 3 INCONCLUSIVE, 4 UNSUPPORTED.
"""

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import time


COUNTERS = ("redraws", "submitted", "no_damage", "skipped", "presentations")
ACTIVE_COUNTERS = ("continuous_active_intervals", "continuous_active_long_intervals")
OPTIONAL_COUNTERS = ACTIVE_COUNTERS + ("compositor_main_thread_cpu_unavailable_intervals",
                                      "compositor_main_thread_cpu_samples_overwritten",
                                      "continuous_active_long_interval_samples_overwritten",
                                      "gpu_frame_work_samples_overwritten",
                                      "late_presentations", "fixed_refresh_presentations")
MAIN_CPU_SCOPE = "compositor_main_thread_between_output_presentations"
GPU_SCOPE = "native_output_command_buffer"
EXIT_CODES = {"PASS": 0, "FAIL": 1, "INCOMPLETE": 2, "INCONCLUSIVE": 3, "UNSUPPORTED": 4}


def integer(value):
    return isinstance(value, int) and not isinstance(value, bool) and 0 <= value <= 2**64 - 1


def finite(value):
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return False
    try:
        return math.isfinite(value)
    except OverflowError:
        return False


def percentile(values, percent):
    ordered = sorted(values)
    return ordered[math.ceil(len(ordered) * percent / 100) - 1] if ordered else None


def coverage(start, end, observed):
    """Range arithmetic avoids allocating in proportion to an invalid counter jump."""
    expected = max(0, end - start)
    sequences = sorted(sequence for sequence in observed if start < sequence <= end)
    missing, next_sequence = [], start + 1
    for sequence in sequences:
        if sequence > next_sequence:
            missing.append([next_sequence, sequence - 1])
        next_sequence = sequence + 1
    if next_sequence <= end:
        missing.append([next_sequence, end])
    return {"expected": expected, "observed": len(sequences),
            "missing_count": sum(last - first + 1 for first, last in missing),
            "missing_ranges": missing, "complete": len(sequences) == expected}


def validate_outputs(value):
    if not isinstance(value, list):
        raise ValueError("frame-timings is not an output array")
    names = set()
    for output in value:
        if not isinstance(output, dict) or not isinstance(output.get("output"), str) or not output["output"]:
            raise ValueError("invalid output identity")
        if output["output"] in names:
            raise ValueError("duplicate output name")
        names.add(output["output"])
        generation = output.get("output_generation")
        if generation is not None and (not integer(generation) or generation == 0):
            raise ValueError(f"{output['output']}: invalid output generation")
        for key in COUNTERS + tuple(key for key in OPTIONAL_COUNTERS if key in output):
            if not integer(output.get(key)):
                raise ValueError(f"{output['output']}: invalid {key} counter")
        if output["redraws"] != sum(output[key] for key in ("submitted", "no_damage", "skipped")):
            raise ValueError(f"{output['output']}: redraw outcome counters disagree")
        if all(key in output for key in ACTIVE_COUNTERS) and output[ACTIVE_COUNTERS[1]] > output[ACTIVE_COUNTERS[0]]:
            raise ValueError(f"{output['output']}: long interval count exceeds denominator")
        if output.get(ACTIVE_COUNTERS[0], 0) > output["presentations"]:
            raise ValueError(f"{output['output']}: active interval count exceeds hardware presentations")
        for numerator, denominator in (
                ("compositor_main_thread_cpu_unavailable_intervals", ACTIVE_COUNTERS[0]),
                ("compositor_main_thread_cpu_samples_overwritten", ACTIVE_COUNTERS[0]),
                ("continuous_active_long_interval_samples_overwritten", ACTIVE_COUNTERS[1]),
                ("gpu_frame_work_samples_overwritten", "redraws")):
            if numerator in output and denominator in output and output[numerator] > output[denominator]:
                raise ValueError(f"{output['output']}: {numerator} exceeds {denominator}")
        period = output.get("refresh_interval_ms")
        if period is not None and (not finite(period) or period <= 0):
            raise ValueError(f"{output['output']}: invalid refresh period")
        if not isinstance(output.get("variable_refresh_rate"), bool):
            raise ValueError(f"{output['output']}: invalid VRR state")
        for key in ("compositor_main_thread_cpu_scope", "gpu_frame_work_scope"):
            if output.get(key) is not None and not isinstance(output[key], str):
                raise ValueError(f"{output['output']}: invalid {key}")
    return {output["output"]: output for output in value}


def _identity_key(value):
    if not isinstance(value, dict) or value.get("error"):
        return None
    keys = ("pid", "start_ticks", "exe_device", "exe_inode", "boot_id")
    return tuple(value.get(key) for key in keys) if all(value.get(key) is not None for key in keys) else None


def _records(rows, key, counter_key, issues):
    """Keep baseline records too, to detect conflicting reused sequence numbers."""
    records = {}
    for row in rows:
        value = row.get(key)
        if not isinstance(value, list):
            if key in row:
                issues.append(f"invalid {key} array")
            continue
        for record in value:
            sequence_key = "redraw_sequence" if key == "gpu_frame_work_samples" else "sequence"
            sequence = record.get(sequence_key) if isinstance(record, dict) else None
            if not integer(sequence) or sequence == 0 or sequence > row.get(counter_key, 0):
                issues.append(f"invalid {key} sequence")
                continue
            if key == "cpu_frame_work_samples":
                valid = finite(record.get("wall_time_ms")) and record["wall_time_ms"] >= 0
            elif key == "compositor_main_thread_cpu_samples":
                cpu = record.get("cpu_time_ms")
                valid = "cpu_time_ms" in record and (cpu is None or (finite(cpu) and cpu >= 0))
            elif key == "gpu_frame_work_samples":
                valid = finite(record.get("gpu_time_ms")) and record["gpu_time_ms"] >= 0
            else:
                valid = (integer(record.get("presentation_time_ns"))
                         and record["presentation_time_ns"] > 0
                         and finite(record.get("interval_ms"))
                         and finite(record.get("refresh_interval_ms"))
                         and record["refresh_interval_ms"] > 0
                         and record["interval_ms"] > record["refresh_interval_ms"] * 1.5)
            if not valid:
                issues.append(f"invalid {key} value")
                continue
            if sequence in records and records[sequence] != record:
                issues.append(f"conflicting {key} sequence {sequence}; output may have reconnected")
            records[sequence] = record
    return records


def summarize_output(name, rows):
    issues = []
    first, last = rows[0], rows[-1]
    counters = COUNTERS + tuple(key for key in OPTIONAL_COUNTERS if all(key in row for row in rows))
    for before, after in zip(rows, rows[1:]):
        for key in counters:
            if after[key] < before[key]:
                issues.append(f"{key} counter reset; output may have reconnected")
        if all(key in before and key in after for key in ACTIVE_COUNTERS):
            active_delta = after[ACTIVE_COUNTERS[0]] - before[ACTIVE_COUNTERS[0]]
            long_delta = after[ACTIVE_COUNTERS[1]] - before[ACTIVE_COUNTERS[1]]
            if long_delta > active_delta:
                issues.append("long interval delta exceeds active interval delta")
            if active_delta > after["presentations"] - before["presentations"]:
                issues.append("active interval delta exceeds hardware presentation delta")
            for numerator, denominator in (
                    ("compositor_main_thread_cpu_unavailable_intervals", ACTIVE_COUNTERS[0]),
                    ("compositor_main_thread_cpu_samples_overwritten", ACTIVE_COUNTERS[0]),
                    ("continuous_active_long_interval_samples_overwritten", ACTIVE_COUNTERS[1])):
                if numerator in before and numerator in after and after[numerator] - before[numerator] > after[denominator] - before[denominator]:
                    issues.append(f"{numerator} delta exceeds {denominator} delta")
    generations = {row.get("output_generation") for row in rows}
    generation_supported = None not in generations
    if len(generations) != 1:
        issues.append("output generation changed; output reconnected or schema changed")
    periods = {row.get("refresh_interval_ms") for row in rows}
    vrr_states = {row.get("variable_refresh_rate") for row in rows}
    if len(periods) != 1 or len(vrr_states) != 1:
        issues.append("output timing changed during capture")
    fixed = len(periods) == 1 and first.get("refresh_interval_ms") is not None and vrr_states == {False}
    period = first.get("refresh_interval_ms") if fixed else None
    cpu_supported = generation_supported and all(isinstance(row.get("cpu_frame_work_samples"), list) for row in rows)
    # A newer CLI defaults absent old-server fields to empty/null. Existing redraws
    # with no CPU samples or CPU summary distinguish that from a fresh idle output.
    if last["redraws"] > 0 and all(not row.get("cpu_frame_work_samples") and row.get("cpu_frame_work_ms") is None for row in rows):
        cpu_supported = False
    active_supported = generation_supported and all(all(key in row for key in ACTIVE_COUNTERS)
                           and isinstance(row.get("continuous_active_long_interval_samples"), list) for row in rows)
    cpu_records = _records(rows, "cpu_frame_work_samples", "redraws", issues)
    cpu_coverage = coverage(first["redraws"], last["redraws"], cpu_records)
    cpu_values = [cpu_records[sequence]["wall_time_ms"] for sequence in sorted(cpu_records)
                  if first["redraws"] < sequence <= last["redraws"]]
    exact_cpu_p95 = percentile(cpu_values, 95) if cpu_supported and cpu_coverage["complete"] and not issues else None
    if cpu_supported and not cpu_coverage["complete"]:
        issues.append("CPU raw sample coverage is incomplete")
    active = last.get(ACTIVE_COUNTERS[0], 0) - first.get(ACTIVE_COUNTERS[0], 0)
    long = last.get(ACTIVE_COUNTERS[1], 0) - first.get(ACTIVE_COUNTERS[1], 0)
    long_records = _records(rows, "continuous_active_long_interval_samples", ACTIVE_COUNTERS[1], issues)
    long_coverage = coverage(first.get(ACTIVE_COUNTERS[1], 0), last.get(ACTIVE_COUNTERS[1], 0), long_records)
    if active_supported and not long_coverage["complete"]:
        issues.append("long interval raw sample coverage is incomplete")
    new_long_records = [long_records[sequence] for sequence in sorted(long_records)
                        if first.get(ACTIVE_COUNTERS[1], 0) < sequence <= last.get(ACTIVE_COUNTERS[1], 0)]
    hardware = last["presentations"] - first["presentations"]
    eligible = fixed and hardware > 0
    if not cpu_supported:
        cpu_status = "UNSUPPORTED"
    elif not cpu_coverage["complete"] or issues:
        cpu_status = "INCOMPLETE"
    elif not eligible or not cpu_values:
        cpu_status = "INCONCLUSIVE"
    else:
        cpu_status = "PASS" if exact_cpu_p95 <= period * .25 else "FAIL"
    if not active_supported:
        active_status = "UNSUPPORTED"
    elif issues:
        active_status = "INCOMPLETE"
    elif not eligible or active <= 0:
        active_status = "INCONCLUSIVE"
    else:
        active_status = "PASS" if long * 100 < active else "FAIL"
    scopes = {row.get("compositor_main_thread_cpu_scope") for row in rows}
    if len(scopes) != 1:
        issues.append("main-thread CPU scope changed during capture")
    main_supported = generation_supported and all(
        row.get("compositor_main_thread_cpu_scope") == MAIN_CPU_SCOPE
        and isinstance(row.get("compositor_main_thread_cpu_samples"), list)
        and all(key in row for key in ACTIVE_COUNTERS + ("compositor_main_thread_cpu_unavailable_intervals", "compositor_main_thread_cpu_samples_overwritten"))
        for row in rows)
    main_records = _records(rows, "compositor_main_thread_cpu_samples", ACTIVE_COUNTERS[0], issues)
    main_coverage = coverage(first.get(ACTIVE_COUNTERS[0], 0), last.get(ACTIVE_COUNTERS[0], 0), main_records)
    new_main_records = [main_records[sequence] for sequence in sorted(main_records)
                        if first.get(ACTIVE_COUNTERS[0], 0) < sequence <= last.get(ACTIVE_COUNTERS[0], 0)]
    unavailable = last.get("compositor_main_thread_cpu_unavailable_intervals", 0) - first.get("compositor_main_thread_cpu_unavailable_intervals", 0)
    if main_supported and (unavailable < 0 or unavailable > active):
        issues.append("invalid main-thread CPU unavailable interval delta")
    if main_supported and not main_coverage["complete"]:
        issues.append("main-thread CPU raw sample coverage is incomplete")
    observed_unavailable = sum(record["cpu_time_ms"] is None for record in new_main_records)
    if main_supported and main_coverage["complete"] and observed_unavailable != unavailable:
        issues.append("main-thread CPU unavailable counter disagrees with raw samples")
    main_values = [record["cpu_time_ms"] for record in new_main_records if record["cpu_time_ms"] is not None]
    main_p95 = percentile(main_values, 95) if main_supported and main_coverage["complete"] and unavailable == 0 and not issues else None
    if not main_supported:
        main_status = "UNSUPPORTED"
    elif issues or not main_coverage["complete"]:
        main_status = "INCOMPLETE"
    elif not eligible or active <= 0 or unavailable > 0 or main_p95 is None:
        main_status = "INCONCLUSIVE"
    else:
        main_status = "PASS" if main_p95 <= period * .25 else "FAIL"
    # Main-thread CPU, rather than the narrower redraw wall span, is the CPU
    # acceptance gate. Preserve redraw wall measurements for separate diagnosis.
    if issues:
        active_status = "INCOMPLETE" if active_supported else active_status
    main_evictions = last.get("compositor_main_thread_cpu_samples_overwritten", 0) - first.get("compositor_main_thread_cpu_samples_overwritten", 0)
    evictions_first = first.get("continuous_active_long_interval_samples_overwritten")
    evictions_last = last.get("continuous_active_long_interval_samples_overwritten")
    evictions = evictions_last - evictions_first if integer(evictions_first) and integer(evictions_last) and evictions_last >= evictions_first else None
    gpu_issues = []
    gpu_scopes = {row.get("gpu_frame_work_scope") for row in rows}
    gpu_supported = generation_supported and GPU_SCOPE in gpu_scopes and gpu_scopes <= {None, GPU_SCOPE}
    gpu_records = _records(rows, "gpu_frame_work_samples", "redraws", gpu_issues)
    gpu_coverage = coverage(first["redraws"], last["redraws"], gpu_records)
    new_gpu_records = [gpu_records[sequence] for sequence in sorted(gpu_records)
                       if first["redraws"] < sequence <= last["redraws"]]
    gpu_values = [record["gpu_time_ms"] for record in new_gpu_records]
    if not gpu_supported:
        gpu_status = "UNSUPPORTED"
    elif gpu_issues or issues:
        gpu_status = "INCOMPLETE"
    elif not gpu_coverage["complete"] or not gpu_values:
        # Skipped/empty redraws and asynchronous tail completions have no tagged
        # timestamp pair. Their absence cannot be replaced by an inferred zero.
        gpu_status = "INCONCLUSIVE"
    else:
        gpu_status = "COMPLETE"
    return {
        "output": name, "output_generation": first.get("output_generation"),
        "generation_supported": generation_supported,
        "issues": sorted(set(issues)), "fixed_refresh_interval_ms": period,
        "hardware_presentations": hardware,
        "cpu_wall_work": {"status": cpu_status, "coverage": cpu_coverage,
                          "whole_run_p95_ms": exact_cpu_p95,
                          "observed_run_max_ms": max(cpu_values, default=None),
                          "limit_ms": period * .25 if period else None,
                          "samples": [cpu_records[sequence] for sequence in sorted(cpu_records)
                                      if first["redraws"] < sequence <= last["redraws"]],
                          "lifetime_max_ms_at_end": last.get("cpu_frame_work_max_ms")},
        "compositor_main_thread_cpu": {"status": main_status, "scope": first.get("compositor_main_thread_cpu_scope"),
                                       "coverage": main_coverage, "whole_run_p95_ms": main_p95,
                                       "observed_run_max_ms": max(main_values, default=None),
                                       "unavailable_intervals": unavailable,
                                       "limit_ms": period * .25 if period else None,
                                       "samples": new_main_records, "ring_evictions_during_run": main_evictions,
                                       "lifetime_max_ms_at_end": last.get("compositor_main_thread_cpu_max_ms")},
        "continuous_activity": {"status": active_status, "intervals": active,
                                "long_intervals": long, "long_rate": long / active if active > 0 and long >= 0 else None,
                                "required_long_rate_less_than": .01,
                                "raw_coverage": long_coverage,
                                "long_samples": new_long_records,
                                "ring_evictions_during_run": evictions,
                                "observed_long_max_ms": max((row["interval_ms"] for row in new_long_records), default=None),
                                "lifetime_interval_max_ms_at_end": last.get("continuous_active_interval_max_ms")},
        "gpu_execution_time_ms": None,
        "gpu_command_buffer": {"status": gpu_status, "scope": last.get("gpu_frame_work_scope"),
                               "issues": sorted(set(gpu_issues)), "coverage": gpu_coverage,
                               "whole_run_command_buffer_p95_ms": percentile(gpu_values, 95) if gpu_status == "COMPLETE" else None,
                               "observed_run_max_ms": max(gpu_values, default=None),
                               "samples": new_gpu_records,
                               "recent_distribution_at_end": last.get("gpu_frame_work_ms") if gpu_supported else None,
                               "lifetime_max_ms_at_end": last.get("gpu_frame_work_max_ms"),
                               "ring_evictions_during_run": last.get("gpu_frame_work_samples_overwritten", 0) - first.get("gpu_frame_work_samples_overwritten", 0)},
        "prediction_error_recent_at_end": last.get("presentation_error_ms"),
        "prediction_misses_during_run": last.get("late_presentations", 0) - first.get("late_presentations", 0),
    }


def summarize(polls, metadata=None):
    issues, valid = [], []
    for index, poll in enumerate(polls):
        if poll.get("error"):
            issues.append(f"poll {index}: {poll['error']}")
            continue
        try:
            outputs = validate_outputs(poll.get("outputs"))
        except ValueError as error:
            issues.append(f"poll {index}: {error}")
            continue
        valid.append((poll, outputs))
    identities = {_identity_key(poll.get("identity")) for poll, _ in valid}
    if None in identities or not identities:
        issues.append("running compositor process identity could not be verified")
    if len(identities - {None}) > 1:
        issues.append("compositor process changed or PID was reused during capture")
    if len(valid) < 2:
        issues.append("at least two valid snapshots are required")
    names = set().union(*(set(outputs) for _, outputs in valid)) if valid else set()
    if valid and any(set(outputs) != set(valid[0][1]) for _, outputs in valid[1:]):
        issues.append("connected output set changed during capture")
    outputs = []
    for name in sorted(names):
        rows = [by_name[name] for _, by_name in valid if name in by_name]
        outputs.append(summarize_output(name, rows))
    metadata = metadata or {}
    before_version, after_version = metadata.get("version_before"), metadata.get("version_after")
    if isinstance(before_version, dict) and isinstance(after_version, dict):
        if before_version.get("compositor") != after_version.get("compositor"):
            issues.append("compositor IPC version changed during capture")
    service_before, service_after = metadata.get("service_before"), metadata.get("service_after")
    if isinstance(service_before, dict) and isinstance(service_after, dict):
        scoped_before, scoped_after = service_before.get("scoped_to_socket_peer"), service_after.get("scoped_to_socket_peer")
        if scoped_before != scoped_after:
            issues.append("scoped compositor service ownership changed")
        elif scoped_before:
            old, new = service_before.get("properties", {}), service_after.get("properties", {})
            if old.get("InvocationID") != new.get("InvocationID") or old.get("ExecMainStartTimestampMonotonic") != new.get("ExecMainStartTimestampMonotonic"):
                issues.append("scoped compositor service invocation changed")
    statuses = [row[key]["status"] for row in outputs for key in ("compositor_main_thread_cpu", "continuous_activity")]
    if issues or any(row["issues"] for row in outputs) or "INCOMPLETE" in statuses:
        status = "INCOMPLETE"
    elif "FAIL" in statuses:
        status = "FAIL"
    elif "UNSUPPORTED" in statuses:
        status = "UNSUPPORTED"
    elif not outputs or "INCONCLUSIVE" in statuses:
        status = "INCONCLUSIVE"
    else:
        status = "PASS"
    return {"status": status, "issues": sorted(set(issues)), "outputs": outputs,
            "measurement_notes": [
                "CPU wall work includes CPU-side preparation/submission waits; it is not GPU execution time.",
                "Acceptance CPU P95 uses complete active-interval coverage of main-thread CPU busy samples between accepted hardware-feedback callbacks.",
                "Main-thread CPU includes input, IPC, animation, rendering and capture callbacks on that thread; worker CPU and blocked/preempted time are excluded.",
                "Per-output CPU intervals intentionally overlap in multi-output sessions and conservatively include all main-thread work.",
                "Redraw wall P95 measures a narrower preparation/submission span and is reported separately.",
                "GPU command-buffer times come only from genuine native device timestamp pairs; they exclude KMS scanout and earlier offscreen/upload command buffers.",
                "GPU whole-run P95 is reported only with complete unique redraw-sequence coverage; missing/pending samples remain inconclusive and GPU availability does not gate CPU acceptance.",
                "Prediction error is separate from continuous hardware presentation intervals.",
                "Idle, VRR, unknown refresh and absent hardware feedback cannot establish fixed-refresh smoothness.",
                "Lifetime maxima include work before this run; ring eviction alone is not collector data loss.",
                "Acceptance requires a stable process identity and per-output generation; older schemas remain unsupported.",
                "No input was synthesized; scenario describes the operator's workload, not a verified action script.",
            ]}


def _json_float(value):
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("non-finite IPC JSON number")
    return number


def _json_constant(value):
    raise ValueError(f"invalid IPC JSON constant {value}")


def run_json(args, env):
    try:
        result = subprocess.run(args, env=env, capture_output=True, text=True, timeout=3)
        if result.returncode:
            return None, (result.stderr.strip() or f"command exited {result.returncode}")[:1000]
        return json.loads(result.stdout, parse_float=_json_float, parse_constant=_json_constant), None
    except (OSError, UnicodeError, subprocess.TimeoutExpired, ValueError) as error:
        return None, str(error)


def process_identity(socket_path):
    if not socket_path:
        return {"error": "CLARION_SOCKET is unset; select a running Clarion IPC socket"}
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(1)
            stream.connect(socket_path)
            pid, uid, gid = struct.unpack("3i", stream.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
        process = Path("/proc") / str(pid)
        start_ticks = int((process / "stat").read_text().rpartition(")")[2].split()[19])
        executable = process / "exe"
        executable_stat = executable.stat()
        return {"pid": pid, "uid": uid, "gid": gid, "start_ticks": start_ticks,
                "executable": os.readlink(executable),
                "exe_device": executable_stat.st_dev, "exe_inode": executable_stat.st_ino,
                "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip()}
    except (OSError, ValueError, IndexError) as error:
        return {"error": str(error)}


def scoped_service(identity):
    if not identity or identity.get("error"):
        return {"scoped_to_socket_peer": False, "reason": "socket peer identity unavailable"}
    try:
        result = subprocess.run(["systemctl", "--user", "show", "clarion.service",
                                 "-p", "MainPID", "-p", "ActiveState", "-p", "InvocationID",
                                 "-p", "ExecMainStartTimestampMonotonic"],
                                capture_output=True, text=True, timeout=3)
        values = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
        if result.returncode or int(values.get("MainPID", "0")) != identity["pid"]:
            return {"scoped_to_socket_peer": False, "reason": "clarion.service does not own this socket peer"}
        return {"scoped_to_socket_peer": True, "properties": values}
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        return {"scoped_to_socket_peer": False, "reason": str(error)}


def capture(args):
    env = os.environ.copy()
    if args.socket:
        env["CLARION_SOCKET"] = args.socket
    socket_path = env.get("CLARION_SOCKET") or env.get("REALAND_SOCKET")
    if socket_path:
        env["CLARION_SOCKET"] = socket_path
    command = shutil.which(args.clarion) or args.clarion
    version_before, version_before_error = run_json([command, "msg", "--json", "version"], env)
    metadata = {"scenario": args.scenario, "seconds_requested": args.seconds,
                "poll_interval_seconds": args.interval, "cli": command, "socket": socket_path,
                "version_before": version_before, "version_before_error": version_before_error}
    polls = []
    start = time.monotonic()
    deadline, next_poll = start + args.seconds, start
    while True:
        wait = next_poll - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        before = process_identity(socket_path)
        outputs, error = run_json([command, "msg", "--json", "frame-timings"], env)
        after = process_identity(socket_path)
        if _identity_key(before) != _identity_key(after):
            error = "socket peer changed during IPC query"
        polls.append({"monotonic_ns": time.monotonic_ns(), "unix_time_ns": time.time_ns(),
                      "identity": after, "outputs": outputs, "error": error})
        if len(polls) == 1:
            metadata["service_before"] = scoped_service(after)
        if time.monotonic() >= deadline:
            break
        next_poll = min(deadline, max(next_poll + args.interval, time.monotonic()))
    metadata["seconds_elapsed"] = time.monotonic() - start
    metadata["version_after"], metadata["version_after_error"] = run_json([command, "msg", "--json", "version"], env)
    metadata["service_after"] = scoped_service(polls[-1]["identity"])
    report = {"format_version": 1, "metadata": metadata, "summary": summarize(polls, metadata),
              "baseline_snapshot": polls[0], "final_snapshot": polls[-1], "polls": polls}
    return report


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be a positive finite number")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=positive, default=30.)
    parser.add_argument("--interval", type=positive, default=.2)
    parser.add_argument("--scenario", default="unspecified operator workload")
    parser.add_argument("--outputPath", type=Path)
    parser.add_argument("--clarion", "--realand", dest="clarion", default="clarion")
    parser.add_argument("--socket")
    args = parser.parse_args()
    report = capture(args)
    serialized = json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
    if args.outputPath:
        args.outputPath.parent.mkdir(parents=True, exist_ok=True)
        args.outputPath.write_text(serialized)
        print(f"{report['summary']['status']}: {args.outputPath}")
    else:
        print(serialized, end="")
    return EXIT_CODES[report["summary"]["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
