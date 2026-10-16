#!/usr/bin/env python3
"""Read-only sampling of Clarion scheduling, memory/IO pressure and slow render phases."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time


PROC = Path('/proc')
ANSI = re.compile(r'\x1b\[[0-?]*[ -/]*[@-~]')


def pressure(resource='cpu', proc_root=PROC):
    values = {}
    try:
        for line in (proc_root / 'pressure' / resource).read_text().splitlines():
            fields = line.split()
            values[fields[0]] = {
                key: int(value) if key == 'total' else float(value)
                for key, value in (field.split('=') for field in fields[1:])
            }
    except (OSError, ValueError, IndexError):
        return {}
    return values


def integer_fields(path):
    try:
        return {parts[0].rstrip(':'): int(parts[1])
                for line in path.read_text().splitlines()
                if len(parts := line.split()) >= 2}
    except (OSError, ValueError):
        return None


def sample(pid, proc_root=PROC):
    base = proc_root / str(pid)
    # comm may contain spaces and parentheses; numeric fields start after the final ')'.
    fields = (base / 'stat').read_text().rpartition(')')[2].split()
    switches, memory = {}, {}
    for line in (base / 'status').read_text().splitlines():
        key, _, value = line.partition(':')
        if key in ('voluntary_ctxt_switches', 'nonvoluntary_ctxt_switches'):
            switches[key] = int(value.strip())
        elif key in ('VmRSS', 'VmSwap', 'RssAnon', 'RssFile', 'RssShmem'):
            parts = value.split()
            if len(parts) == 2 and parts[1] == 'kB':
                memory[key] = int(parts[0]) * 1024
    sched = None
    try:
        values = [int(value) for value in (base / 'schedstat').read_text().split()]
        if len(values) >= 3:
            sched = dict(zip(('runtime_ns', 'runqueue_wait_ns', 'slices'), values))
    except (OSError, ValueError):
        pass
    try:
        wait_channel = (base / 'wchan').read_text().strip()
    except OSError:
        wait_channel = None
    vmstat = integer_fields(proc_root / 'vmstat')
    return {
        'main_thread_wait_channel': wait_channel,
        'monotonic': time.monotonic(), 'unix_time': time.time(),
        'cpu_ticks': int(fields[11]) + int(fields[12]),
        'start_ticks': int(fields[19]), 'rss_bytes': int(fields[21]) * os.sysconf('SC_PAGE_SIZE'),
        'minor_faults': int(fields[7]), 'major_faults': int(fields[9]),
        'main_thread_switches': switches, 'main_thread_scheduler': sched,
        'memory_status_bytes': memory, 'process_io': integer_fields(base / 'io'),
        'system_paging': None if vmstat is None else {
            key: vmstat[key] for key in ('pswpin', 'pswpout', 'pgmajfault') if key in vmstat
        },
        'cpu_pressure': pressure('cpu', proc_root),
        'memory_pressure': pressure('memory', proc_root),
        'io_pressure': pressure('io', proc_root),
        'loadavg': os.getloadavg(),
    }


def counter_delta(before, after, key):
    if before is None or after is None or key not in before or key not in after:
        return None
    delta = after[key] - before[key]
    return delta if delta >= 0 else None


def summarize(samples, events=()):
    ticks = os.sysconf('SC_CLK_TCK')
    intervals = []
    for before, after in zip(samples, samples[1:]):
        if before['start_ticks'] != after['start_ticks']:
            raise RuntimeError('PID was reused during sampling; report discarded')
        duration = after['monotonic'] - before['monotonic']
        if duration <= 0:
            raise RuntimeError('Non-positive sampling interval; report discarded')
        interval = {
            'seconds': duration,
            'started_unix': before['unix_time'], 'ended_unix': after['unix_time'],
            'cpu_percent_of_one_core': 100 * (after['cpu_ticks'] - before['cpu_ticks']) / ticks / duration,
            'minor_faults': after['minor_faults'] - before['minor_faults'],
            'major_faults': after['major_faults'] - before['major_faults'],
            'latency_event_indices': [index for index, event in enumerate(events)
                                      if before['unix_time'] < event['timestamp_unix'] <= after['unix_time']],
        }
        old, new = before['main_thread_scheduler'], after['main_thread_scheduler']
        if old is not None and new is not None:
            interval['main_thread_runqueue_wait_ms'] = (new['runqueue_wait_ns'] - old['runqueue_wait_ns']) / 1e6
            interval['main_thread_runtime_ms'] = (new['runtime_ns'] - old['runtime_ns']) / 1e6
        for resource in ('cpu', 'memory', 'io'):
            field = f'{resource}_pressure'
            durations = {}
            for kind in ('some', 'full'):
                delta = counter_delta(before[field].get(kind), after[field].get(kind), 'total')
                if delta is not None:
                    durations[kind] = delta / 1000
            if durations:
                interval[f'{field}_stall_ms'] = durations
        for key, name, unit in (
            ('pswpin', 'system_swap_in_bytes', os.sysconf('SC_PAGE_SIZE')),
            ('pswpout', 'system_swap_out_bytes', os.sysconf('SC_PAGE_SIZE')),
            ('pgmajfault', 'system_major_faults', 1),
        ):
            delta = counter_delta(before['system_paging'], after['system_paging'], key)
            if delta is not None:
                interval[name] = delta * unit
        io_delta = {}
        for key in ('read_bytes', 'write_bytes'):
            delta = counter_delta(before['process_io'], after['process_io'], key)
            if delta is not None:
                io_delta[key] = delta
        if io_delta:
            interval['process_io_bytes'] = io_delta
        if 'VmSwap' in before['memory_status_bytes'] and 'VmSwap' in after['memory_status_bytes']:
            interval['process_swap_bytes'] = after['memory_status_bytes']['VmSwap']
            interval['process_swap_change_bytes'] = (
                after['memory_status_bytes']['VmSwap'] - before['memory_status_bytes']['VmSwap'])
        intervals.append(interval)
    return intervals


def latency_events(journal_text):
    events = []
    for line in journal_text.splitlines():
        try:
            row = json.loads(line)
            message = row.get('MESSAGE', '')
            # journalctl --output=json encodes non-printable fields (including
            # ANSI-coloured tracing output) as an array of unsigned bytes.
            # Lists of strings may instead represent repeated journal fields;
            # do not silently coerce those or malformed arrays into messages.
            if isinstance(message, list):
                if not all(type(value) is int and 0 <= value <= 255 for value in message):
                    continue
                message = bytes(message).decode('utf-8', errors='replace')
            if not isinstance(message, str):
                continue
            message = ANSI.sub('', message)
            if 'slow compositor operation' not in message and 'lagging behind' not in message:
                continue
            event = {'timestamp_unix': int(row['__REALTIME_TIMESTAMP']) / 1e6,
                     'message': message}
            for key in ('operation', 'thread'):
                match = re.search(rf'\b{key}="([^"]+)"', message)
                if match:
                    event[key] = match.group(1)
            for key in ('elapsed_ms', 'max_ms', 'threshold_ms', 'slow_samples'):
                match = re.search(rf'\b{key}=([0-9.eE+-]+)', message)
                if match:
                    event[key] = int(match.group(1)) if key == 'slow_samples' else float(match.group(1))
            events.append(event)
        except (ValueError, KeyError, TypeError, AttributeError):
            continue
    return events[-200:]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, help='Compositor PID; default: clarion.service MainPID')
    parser.add_argument('--seconds', type=int, default=15, choices=range(1, 61), metavar='1..60')
    parser.add_argument('--output', type=Path, required=True, help='Write a new JSON report; existing files are not overwritten')
    args = parser.parse_args()
    pid = args.pid
    if pid is None:
        pid = int(subprocess.check_output(['systemctl', '--user', 'show', 'clarion.service',
                                         '-p', 'MainPID', '--value'], text=True, timeout=3).strip())
    if pid <= 0:
        parser.error('Clarion is not running; specify --pid')
    if args.output.exists():
        parser.error('Output already exists; choose a new report path')
    started = time.time()
    samples = [sample(pid)]
    for _ in range(args.seconds):
        time.sleep(1)
        samples.append(sample(pid))
    finished = time.time()
    events = []
    journal_error = None
    try:
        result = subprocess.run(['journalctl', '--user', f'_PID={pid}',
                                 '--since', f'@{started:.6f}', '--until', f'@{finished:.6f}',
                                 '--no-pager', '-o', 'json'],
                                capture_output=True, text=True, timeout=5, check=True)
        events = latency_events(result.stdout)
    except (OSError, subprocess.SubprocessError) as error:
        journal_error = str(error)
    report = {
        'pid': pid, 'started_unix': started, 'duration_seconds': samples[-1]['monotonic']-samples[0]['monotonic'],
        'intervals': summarize(samples, events), 'samples': samples,
        'latency_events': events, 'latency_logs': [event['message'] for event in events],
        'journal_error': journal_error,
        'notes': [
            'CPU percentage aggregates process threads; scheduler counters refer only to the main thread.',
            'Zero scheduler wait may mean kernel scheduler statistics are disabled, not absence of contention.',
            'Wall-time warnings include scheduling delays; nested render phases overlap and must not be added.',
            'PSI and swap-in/out counters describe the entire system, not only this compositor.',
            'A nonzero VmSwap is retained swapped memory, not proof of swap IO during this interval.',
            'Missing optional counters are unavailable, never a measured zero; CPU time is not GPU time.',
            'Latency events correlate by journal receipt time; slow_samples/max_ms can summarize preceding seconds.',
            'New timing instrumentation needs a compositor session running the updated binary.',
            'No journal match during this sample is not proof that input or rendering cannot stall.',
        ],
    }
    with args.output.open('x') as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
    cpu = [row['cpu_percent_of_one_core'] for row in report['intervals']]
    print(f"Saved {args.output}: {len(cpu)} intervals, max CPU {max(cpu):.1f}% of one core, {len(events)} latency logs")


if __name__ == '__main__':
    main()
