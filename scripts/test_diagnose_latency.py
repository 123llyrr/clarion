#!/usr/bin/env python3
"""Fixtures for read-only compositor latency reports; no live desktop required."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

_spec = importlib.util.spec_from_file_location(
    'diagnose_latency', Path(__file__).with_name('diagnose-latency.py'))
latency = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(latency)


class LatencyReportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        process = self.root / '123'
        process.mkdir()
        fields = ['0'] * 22
        fields[0] = 'S'
        for index, value in {7: 8, 9: 2, 11: 12, 12: 3, 19: 700, 21: 10}.items():
            fields[index] = str(value)
        (process / 'stat').write_text('123 (clarion (compositor)) ' + ' '.join(fields))
        (process / 'status').write_text(
            'VmRSS:\t40 kB\nVmSwap:\t20 kB\nRssAnon:\t30 kB\n'
            'voluntary_ctxt_switches:\t100\nnonvoluntary_ctxt_switches:\t2\n')
        (process / 'schedstat').write_text('1000000 2000000 3\n')
        (process / 'wchan').write_text('do_epoll_wait\n')
        (process / 'io').write_text('read_bytes: 4096\nwrite_bytes: 8192\n')
        (self.root / 'vmstat').write_text('pswpin 2\npswpout 4\npgmajfault 5\n')
        (self.root / 'pressure').mkdir()
        for name in ('cpu', 'memory', 'io'):
            (self.root / 'pressure' / name).write_text(
                'some avg10=0.00 avg60=0.00 avg300=0.00 total=10000\n'
                'full avg10=0.00 avg60=0.00 avg300=0.00 total=5000\n')

    def pair(self):
        before = latency.sample(123, self.root)
        before['monotonic'], before['unix_time'] = 10.0, 100.0
        after = copy.deepcopy(before)
        after['monotonic'], after['unix_time'] = 11.0, 101.0
        return before, after

    def test_sample_preserves_units_and_comm_parentheses(self):
        row = latency.sample(123, self.root)
        self.assertEqual(row['start_ticks'], 700)
        self.assertEqual(row['cpu_ticks'], 15)
        self.assertEqual(row['minor_faults'], 8)
        self.assertEqual(row['major_faults'], 2)
        self.assertEqual(row['memory_status_bytes']['VmSwap'], 20 * 1024)
        self.assertEqual(row['process_io']['read_bytes'], 4096)
        self.assertEqual(row['system_paging']['pswpin'], 2)
        self.assertIsInstance(row['memory_pressure']['some']['total'], int)

    def test_deltas_and_event_correlation_use_same_interval(self):
        before, after = self.pair()
        after['cpu_ticks'] += os.sysconf('SC_CLK_TCK') // 2
        after['major_faults'] += 2
        after['minor_faults'] += 5
        after['main_thread_scheduler']['runqueue_wait_ns'] += 3_000_000
        after['system_paging']['pswpin'] += 2
        after['system_paging']['pswpout'] += 1
        after['process_io']['read_bytes'] += 8192
        after['memory_status_bytes']['VmSwap'] -= 4096
        after['memory_pressure']['some']['total'] += 2500
        events = [{'timestamp_unix': value} for value in (99.0, 100.0, 100.5, 101.0, 101.1)]
        row = latency.summarize([before, after], events)[0]
        self.assertEqual(row['major_faults'], 2)
        self.assertEqual(row['minor_faults'], 5)
        self.assertEqual(row['main_thread_runqueue_wait_ms'], 3)
        self.assertEqual(row['system_swap_in_bytes'], 2 * os.sysconf('SC_PAGE_SIZE'))
        self.assertEqual(row['process_io_bytes']['read_bytes'], 8192)
        self.assertEqual(row['process_swap_change_bytes'], -4096)
        self.assertEqual(row['memory_pressure_stall_ms']['some'], 2.5)
        self.assertEqual(row['latency_event_indices'], [2, 3])

    def test_missing_optional_counters_are_not_measured_zero(self):
        for relative in ('123/io', '123/schedstat', 'vmstat', 'pressure/memory', 'pressure/io'):
            (self.root / relative).unlink()
        before, after = self.pair()
        row = latency.summarize([before, after])[0]
        for key in ('process_io_bytes', 'system_swap_in_bytes',
                    'main_thread_runqueue_wait_ms', 'memory_pressure_stall_ms', 'io_pressure_stall_ms'):
            self.assertNotIn(key, row)
        self.assertIsNone(before['process_io'])
        self.assertEqual(before['memory_pressure'], {})

    def test_partial_or_reset_counters_do_not_invent_negative_activity(self):
        before, after = self.pair()
        after['system_paging']['pswpin'] = 1
        after['cpu_pressure']['some']['total'] = 0
        after['process_io'] = {'write_bytes': 8192}
        row = latency.summarize([before, after])[0]
        self.assertNotIn('system_swap_in_bytes', row)
        self.assertNotIn('some', row['cpu_pressure_stall_ms'])
        self.assertNotIn('read_bytes', row['process_io_bytes'])

    def test_pid_reuse_and_invalid_intervals_are_rejected(self):
        before, after = self.pair()
        after['start_ticks'] += 1
        with self.assertRaisesRegex(RuntimeError, 'PID was reused'):
            latency.summarize([before, after])
        after['start_ticks'] = before['start_ticks']
        after['monotonic'] = before['monotonic']
        with self.assertRaisesRegex(RuntimeError, 'Non-positive'):
            latency.summarize([before, after])

    def test_structured_journal_events_strip_ansi_and_preserve_old_logs(self):
        current = '\x1b[33mslow compositor operation\x1b[0m operation="scene-build" elapsed_ms=5.25 max_ms=8.5 slow_samples=2 threshold_ms=4.0 thread="main"'
        old = 'slow compositor operation operation="output-render" elapsed_ms=20.0 max_ms=20.0 slow_samples=1'
        journal = '\n'.join([json.dumps({'MESSAGE': text, '__REALTIME_TIMESTAMP': str(stamp)})
                             for text, stamp in ((current, 100500000), (old, 101000000), ('noise', 102000000))])
        events = latency.latency_events(journal + '\nnot json\n{}')
        self.assertEqual(len(events), 2)
        self.assertEqual(events[0]['timestamp_unix'], 100.5)
        self.assertEqual(events[0]['operation'], 'scene-build')
        self.assertEqual(events[0]['thread'], 'main')
        self.assertEqual(events[0]['threshold_ms'], 4)
        self.assertNotIn('\x1b', events[0]['message'])
        self.assertNotIn('threshold_ms', events[1])

    def test_real_journal_byte_arrays_keep_client_dispatch_spikes(self):
        # Captured from journalctl --user _PID=1256 --output=json. Only the
        # original MESSAGE and timestamp fields are retained in this fixture.
        fixture = Path(__file__).with_name('fixtures') / 'latency-journal-byte-array.jsonl'
        events = latency.latency_events(fixture.read_text())
        self.assertEqual(len(events), 2)
        self.assertEqual([event['operation'] for event in events],
                         ['client-dispatch', 'client-dispatch'])
        self.assertEqual([event['elapsed_ms'] for event in events],
                         [124.915972, 94.591734])
        self.assertTrue(all(event['thread'] == 'main' for event in events))
        self.assertTrue(all(event['threshold_ms'] == 16 for event in events))
        self.assertTrue(all('\x1b' not in event['message'] for event in events))
        before, after = self.pair()
        before['unix_time'] = events[0]['timestamp_unix'] - 1
        after['unix_time'] = events[1]['timestamp_unix']
        self.assertEqual(latency.summarize([before, after], events)[0]
                         ['latency_event_indices'], [0, 1])

    def test_byte_message_utf8_and_invalid_utf8_do_not_lose_parseable_event(self):
        message = ('slow compositor operation operation="scene-build" '
                   'elapsed_ms=5.0 note="中文"').encode('utf-8')
        rows = [json.dumps({'MESSAGE': list(value), '__REALTIME_TIMESTAMP': '1000000'})
                for value in (message, message + b'\xff')]
        events = latency.latency_events('\n'.join(rows))
        self.assertEqual(len(events), 2)
        self.assertIn('中文', events[0]['message'])
        self.assertTrue(events[1]['message'].endswith('\ufffd'))
        self.assertEqual(events[1]['operation'], 'scene-build')

    def test_malformed_message_arrays_are_ignored_without_coercion(self):
        prefix = list(b'slow compositor operation operation="scene-build" elapsed_ms=5.0')
        invalid = [prefix + [value] for value in (True, False, -1, 256, 1.0, '65', None, [65])]
        invalid.extend([['slow compositor operation'], [], None, {'message': 'slow compositor operation'}])
        for message in invalid:
            with self.subTest(message=message):
                row = json.dumps({'MESSAGE': message, '__REALTIME_TIMESTAMP': '1000000'})
                self.assertEqual(latency.latency_events(row), [])


if __name__ == '__main__':
    unittest.main()
