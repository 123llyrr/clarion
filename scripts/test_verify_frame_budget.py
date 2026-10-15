#!/usr/bin/env python3
"""Acceptance semantics for raw frame-budget captures; no live compositor needed."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    'verify_frame_budget', Path(__file__).with_name('verify-frame-budget.py'))
budget = importlib.util.module_from_spec(spec)
spec.loader.exec_module(budget)

IDENTITY = {"pid": 123, "start_ticks": 700, "exe_device": 8,
            "exe_inode": 12, "boot_id": "fixture"}


def row(count, *, cpu_value=1., active=None, long=0, hardware=None, period=10., vrr=False,
        cpu_start=None, long_start=None, overwritten=0, generation=1, thread_cpu_value=None, thread_start=None):
    cpu_start = max(1, count - 239) if cpu_start is None else cpu_start
    long_start = max(1, long - 239) if long_start is None else long_start
    active = count if active is None else active
    hardware = max(count, active) if hardware is None else hardware
    thread_start = cpu_start if thread_start is None and active == count else max(1, active - 239) if thread_start is None else thread_start
    thread_cpu_value = cpu_value if thread_cpu_value is None else thread_cpu_value
    return {"output": "DP-1", "output_generation": generation, "refresh_interval_ms": period, "variable_refresh_rate": vrr,
            "redraws": count, "submitted": count, "no_damage": 0, "skipped": 0,
            "presentations": hardware,
            "continuous_active_intervals": active, "continuous_active_long_intervals": long,
            "continuous_active_long_interval_samples_overwritten": overwritten,
            "cpu_frame_work_ms": {"p95": 999},
            "cpu_frame_work_max_ms": 999,
            "cpu_frame_work_samples": [{"sequence": n, "wall_time_ms": cpu_value}
                                       for n in range(cpu_start, count + 1)],
            "continuous_active_long_interval_samples": [
                {"sequence": n, "presentation_time_ns": 1_000_000_000 + n * 20_000_000,
                 "interval_ms": 20., "refresh_interval_ms": 10.}
                for n in range(long_start, long + 1)],
            "compositor_main_thread_cpu_scope": budget.MAIN_CPU_SCOPE,
            "compositor_main_thread_cpu_samples": [{"sequence": n, "cpu_time_ms": thread_cpu_value} for n in range(thread_start, active + 1)],
            "compositor_main_thread_cpu_ms": {"p95": 999.},
            "compositor_main_thread_cpu_max_ms": 999.,
            "compositor_main_thread_cpu_unavailable_intervals": 0,
            "compositor_main_thread_cpu_samples_overwritten": max(0, active - 240),
            "continuous_active_interval_max_ms": 999., "gpu_frame_work_ms": None,
            "late_presentations": 0, "fixed_refresh_presentations": hardware,
            "presentation_error_ms": {"p95": 88}}


def poll(*outputs, identity=None, error=None):
    return {"outputs": list(outputs), "identity": copy.deepcopy(identity or IDENTITY), "error": error}


def summary(*rows, metadata=None):
    return budget.summarize([poll(value) for value in rows], metadata)


def gpu(value, sequences, *, elapsed=1., overwritten=0):
    value["gpu_frame_work_scope"] = budget.GPU_SCOPE
    value["gpu_frame_work_samples"] = [{"redraw_sequence": n, "gpu_time_ms": elapsed} for n in sequences]
    value["gpu_frame_work_ms"] = {"p95": 999.}
    value["gpu_frame_work_max_ms"] = 999.
    value["gpu_frame_work_samples_overwritten"] = overwritten
    return value


class FrameBudgetTests(unittest.TestCase):
    def test_exact_whole_run_p95_ignores_rolling_and_lifetime_summaries(self):
        before = row(10)
        after = row(30)
        after["cpu_frame_work_samples"][-1]["wall_time_ms"] = 9.
        result = summary(before, after)
        self.assertEqual(result["status"], "PASS")
        cpu = result["outputs"][0]["cpu_wall_work"]
        self.assertEqual(cpu["coverage"]["observed"], 20)
        self.assertEqual(cpu["whole_run_p95_ms"], 1.)
        self.assertEqual(cpu["observed_run_max_ms"], 9.)
        self.assertEqual(cpu["lifetime_max_ms_at_end"], 999)
        self.assertIsNone(result["outputs"][0]["gpu_execution_time_ms"])

    def test_cpu_quarter_period_equality_passes_but_excess_fails(self):
        self.assertEqual(summary(row(0), row(100, cpu_value=2.5))["status"], "PASS")
        self.assertEqual(summary(row(0), row(100, cpu_value=2.50001))["status"], "FAIL")

    def test_continuous_long_rate_is_strictly_less_than_one_percent(self):
        self.assertEqual(summary(row(0), row(100, long=1))["status"], "FAIL")
        self.assertEqual(summary(row(0), row(101, long=1))["status"], "PASS")

    def test_ring_snapshots_deduplicate_and_baseline_samples_are_excluded(self):
        result = summary(row(200), row(290, cpu_start=51), row(450, cpu_start=211))
        self.assertEqual(result["status"], "PASS")
        cpu = result["outputs"][0]["cpu_wall_work"]
        self.assertEqual(cpu["coverage"]["expected"], 250)
        self.assertEqual(len(cpu["samples"]), 250)
        self.assertEqual(cpu["samples"][0]["sequence"], 201)
        self.assertEqual(cpu["samples"][-1]["sequence"], 450)

    def test_missing_cpu_raw_sequences_are_incomplete_not_a_whole_run_p95(self):
        result = summary(row(200), row(450, cpu_start=211))
        self.assertEqual(result["status"], "INCOMPLETE")
        cpu = result["outputs"][0]["cpu_wall_work"]
        self.assertEqual(cpu["coverage"]["missing_ranges"], [[201, 210]])
        self.assertIsNone(cpu["whole_run_p95_ms"])

    def test_anomaly_ring_eviction_is_distinct_from_collector_loss(self):
        before = row(1000, long=5)
        intermediate = row(1005, long=10)
        after = row(1245, long=245, long_start=6, overwritten=5)
        result = summary(before, intermediate, after)
        activity = result["outputs"][0]["continuous_activity"]
        # A large long-frame rate fails performance, but ring eviction is not
        # collector loss and must not make this an incomplete measurement.
        self.assertEqual(result["status"], "FAIL")
        self.assertEqual(activity["raw_coverage"]["observed"], 240)
        self.assertEqual(activity["ring_evictions_during_run"], 5)
        self.assertEqual(activity["raw_coverage"]["missing_count"], 0)

    def test_missing_raw_anomalies_prevent_acceptance_even_with_exact_counters(self):
        before = row(1000, long=250)
        after = row(1250, long=500, long_start=261, overwritten=260)
        result = summary(before, after)
        self.assertEqual(result["status"], "INCOMPLETE")
        activity = result["outputs"][0]["continuous_activity"]
        self.assertEqual(activity["raw_coverage"]["missing_ranges"], [[251, 260]])

    def test_idle_unknown_refresh_vrr_and_no_hardware_are_inconclusive(self):
        cases = [(row(0, active=0), row(20, active=0)),
                 (row(0, period=None), row(20, period=None)),
                 (row(0, active=0, vrr=True), row(20, active=0, vrr=True)),
                 (row(0, active=0, hardware=0), row(20, active=0, hardware=0))]
        for before, after in cases:
            with self.subTest(before=before):
                self.assertEqual(summary(before, after)["status"], "INCONCLUSIVE")

    def test_older_server_raw_fields_are_unsupported_not_measured_zero(self):
        before, after = row(0), row(20)
        for value in (before, after):
            for key in ("cpu_frame_work_samples", "cpu_frame_work_ms",
                        "continuous_active_intervals", "continuous_active_long_intervals",
                        "continuous_active_long_interval_samples"):
                value.pop(key)
            for key in list(value):
                if key.startswith("compositor_main_thread_cpu_"):
                    value.pop(key)
        self.assertEqual(summary(before, after)["status"], "UNSUPPORTED")

    def test_new_cli_defaulted_old_server_fields_are_unsupported(self):
        before, after = row(10, active=0), row(20, active=0)
        for value in (before, after):
            value["cpu_frame_work_samples"] = []
            value["cpu_frame_work_ms"] = None
            value["compositor_main_thread_cpu_scope"] = None
            value["compositor_main_thread_cpu_samples"] = []
        self.assertEqual(summary(before, after)["status"], "UNSUPPORTED")

    def test_counter_reset_is_detected_even_if_final_count_regains_old_value(self):
        result = summary(row(10), row(2), row(20))
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertTrue(any("reset" in value for value in result["outputs"][0]["issues"]))

    def test_output_reconnect_with_reused_sequence_is_detected(self):
        before, after = row(1), row(2)
        after["cpu_frame_work_samples"][0]["wall_time_ms"] = 2.
        result = summary(before, after)
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertTrue(any("conflicting" in value for value in result["outputs"][0]["issues"]))
        self.assertIsNone(result["outputs"][0]["cpu_wall_work"]["whole_run_p95_ms"])

    def test_invalid_counters_booleans_and_conflicting_outcomes_are_rejected(self):
        for key, value in [("redraws", -1), ("redraws", True), ("redraws", 2**64),
                           ("submitted", 99), ("presentations", 1),
                           ("continuous_active_long_intervals", 100)]:
            bad = row(20)
            bad[key] = value
            with self.subTest(key=key, value=value):
                self.assertEqual(summary(row(0), bad)["status"], "INCOMPLETE")

    def test_impossible_active_and_long_counter_deltas_are_incomplete(self):
        before, after = row(100, active=50, long=0), row(110, active=51, long=2)
        self.assertEqual(summary(before, after)["status"], "INCOMPLETE")
        before, after = row(100, active=50, hardware=100), row(110, active=70, hardware=110)
        self.assertEqual(summary(before, after)["status"], "INCOMPLETE")

    def test_generation_change_catches_reconnect_with_all_identical_counters(self):
        result = summary(row(20, generation=8), row(20, generation=9))
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertTrue(any("generation changed" in issue for issue in result["outputs"][0]["issues"]))

    def test_missing_generation_never_accepts_an_otherwise_complete_old_schema(self):
        result = summary(row(0, generation=None), row(20, generation=None))
        self.assertEqual(result["status"], "UNSUPPORTED")
        self.assertFalse(result["outputs"][0]["generation_supported"])

    def test_invalid_raw_samples_cannot_establish_a_percentile(self):
        for record in [{"sequence": 1, "wall_time_ms": float('nan')},
                       {"sequence": 21, "wall_time_ms": 1.},
                       {"sequence": 1, "wall_time_ms": -1.},
                       {"sequence": 1, "wall_time_ms": 10**1000}]:
            bad = row(20)
            bad["cpu_frame_work_samples"] = [record]
            self.assertEqual(summary(row(0), bad)["status"], "INCOMPLETE")

    def test_missing_ipc_and_changed_output_set_are_incomplete(self):
        rows = [poll(row(0)), poll(error="IPC timeout"), poll(row(20))]
        self.assertEqual(budget.summarize(rows)["status"], "INCOMPLETE")
        self.assertEqual(budget.summarize([poll(row(0)), poll()])["status"], "INCOMPLETE")

    def test_pid_reuse_version_and_scoped_service_invocation_changes_are_incomplete(self):
        after_identity = dict(IDENTITY, start_ticks=701)
        self.assertEqual(budget.summarize([poll(row(0)), poll(row(20), identity=after_identity)])["status"], "INCOMPLETE")
        self.assertEqual(summary(row(0), row(20), metadata={
            "version_before": {"compositor": "before"}, "version_after": {"compositor": "after"}})["status"], "INCOMPLETE")
        metadata = {"service_before": {"scoped_to_socket_peer": True, "properties": {"InvocationID": "a"}},
                    "service_after": {"scoped_to_socket_peer": True, "properties": {"InvocationID": "b"}}}
        self.assertEqual(summary(row(0), row(20), metadata=metadata)["status"], "INCOMPLETE")

    def test_changed_mode_is_incomplete_and_empty_outputs_are_inconclusive(self):
        self.assertEqual(summary(row(0), row(20, period=20.))["status"], "INCOMPLETE")
        self.assertEqual(budget.summarize([poll(), poll()])["status"], "INCONCLUSIVE")

    def test_main_cpu_gate_is_separate_from_redraw_wall_work(self):
        result = summary(row(0), row(100, cpu_value=10., thread_cpu_value=1.))
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["outputs"][0]["cpu_wall_work"]["status"], "FAIL")
        self.assertEqual(result["outputs"][0]["compositor_main_thread_cpu"]["whole_run_p95_ms"], 1.)
        result = summary(row(0), row(100, cpu_value=1., thread_cpu_value=3.))
        self.assertEqual(result["status"], "FAIL")

    def test_missing_main_clock_is_unavailable_not_measured_zero(self):
        before, after = row(0), row(20)
        after["compositor_main_thread_cpu_samples"][5]["cpu_time_ms"] = None
        after["compositor_main_thread_cpu_unavailable_intervals"] = 1
        result = summary(before, after)
        self.assertEqual(result["status"], "INCONCLUSIVE")
        cpu = result["outputs"][0]["compositor_main_thread_cpu"]
        self.assertEqual(cpu["unavailable_intervals"], 1)
        self.assertIsNone(cpu["whole_run_p95_ms"])

    def test_strong_cpu_sequence_loss_and_conflicts_are_incomplete(self):
        result = summary(row(200), row(450, thread_start=211))
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertEqual(result["outputs"][0]["compositor_main_thread_cpu"]["coverage"]["missing_ranges"], [[201, 210]])
        before, after = row(10), row(20)
        after["compositor_main_thread_cpu_samples"][0]["cpu_time_ms"] = 9.
        self.assertEqual(summary(before, after)["status"], "INCOMPLETE")

    def test_main_cpu_counter_mismatch_invalid_clock_and_unknown_scope_cannot_pass(self):
        for key, value in [("compositor_main_thread_cpu_unavailable_intervals", 21),
                           ("compositor_main_thread_cpu_unavailable_intervals", 1)]:
            after = row(20); after[key] = value
            self.assertEqual(summary(row(0), after)["status"], "INCOMPLETE")
        for value in [float('nan'), -1., True]:
            after = row(20); after["compositor_main_thread_cpu_samples"][0]["cpu_time_ms"] = value
            self.assertEqual(summary(row(0), after)["status"], "INCOMPLETE")
        before, after = row(0), row(20)
        before["compositor_main_thread_cpu_scope"] = after["compositor_main_thread_cpu_scope"] = "unknown"
        self.assertEqual(summary(before, after)["status"], "UNSUPPORTED")

    def test_main_cpu_exact_p95_uses_active_sequences_and_reports_eviction(self):
        before, middle, after = row(200), row(290), row(450)
        after["compositor_main_thread_cpu_samples"][-1]["cpu_time_ms"] = 9.
        result = summary(before, middle, after)
        self.assertEqual(result["status"], "PASS")
        cpu = result["outputs"][0]["compositor_main_thread_cpu"]
        self.assertEqual(cpu["coverage"]["expected"], 250)
        self.assertEqual(cpu["whole_run_p95_ms"], 1.)
        self.assertEqual(cpu["observed_run_max_ms"], 9.)
        self.assertEqual(cpu["ring_evictions_during_run"], 210)

    def test_main_cpu_loss_counters_cannot_grow_without_corresponding_intervals(self):
        for key in ("compositor_main_thread_cpu_samples_overwritten",
                    "compositor_main_thread_cpu_unavailable_intervals"):
            before, after = row(100), row(110)
            after[key] = 11
            with self.subTest(key=key):
                self.assertEqual(summary(before, after)["status"], "INCOMPLETE")
        after = row(20)
        after["compositor_main_thread_cpu_samples_overwritten"] = 21
        self.assertEqual(summary(row(0), after)["status"], "INCOMPLETE")

    def test_optional_gpu_absence_does_not_gate_cpu_acceptance(self):
        result = summary(row(0), row(20))
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["outputs"][0]["gpu_command_buffer"]["status"], "UNSUPPORTED")

    def test_gpu_partial_async_tail_is_inconclusive_without_inferred_zero(self):
        result = summary(row(0), gpu(row(20), range(1, 11)))
        self.assertEqual(result["status"], "PASS")
        measurement = result["outputs"][0]["gpu_command_buffer"]
        self.assertEqual(measurement["status"], "INCONCLUSIVE")
        self.assertEqual(measurement["coverage"]["missing_ranges"], [[11, 20]])
        self.assertIsNone(measurement["whole_run_command_buffer_p95_ms"])
        self.assertEqual(measurement["recent_distribution_at_end"], {"p95": 999.})

    def test_gpu_complete_unique_coverage_computes_exact_scope_p95(self):
        after = gpu(row(100), reversed(range(1, 101)))
        after["gpu_frame_work_samples"][0]["gpu_time_ms"] = 9.
        result = summary(row(0), after)
        self.assertEqual(result["status"], "PASS")
        measurement = result["outputs"][0]["gpu_command_buffer"]
        self.assertEqual(measurement["status"], "COMPLETE")
        self.assertEqual(measurement["scope"], budget.GPU_SCOPE)
        self.assertEqual(measurement["whole_run_command_buffer_p95_ms"], 1.)
        self.assertEqual(measurement["observed_run_max_ms"], 9.)
        self.assertEqual(measurement["samples"][0]["redraw_sequence"], 1)
        self.assertEqual(measurement["lifetime_max_ms_at_end"], 999.)

    def test_gpu_evictions_can_arrive_without_new_cpu_redraws(self):
        before = gpu(row(1000), range(1, 241))
        after = gpu(row(1000), range(2, 242), overwritten=1)
        result = summary(before, after)
        self.assertEqual(result["status"], "INCONCLUSIVE")
        self.assertEqual(result["outputs"][0]["gpu_command_buffer"]["ring_evictions_during_run"], 1)
        self.assertEqual(result["outputs"][0]["issues"], [])

    def test_gpu_invalid_and_conflicting_records_never_establish_whole_run_p95(self):
        for record in [{"redraw_sequence": 1, "gpu_time_ms": float('nan')},
                       {"redraw_sequence": 1, "gpu_time_ms": -1.},
                       {"redraw_sequence": 1, "gpu_time_ms": True},
                       {"redraw_sequence": 21, "gpu_time_ms": 1.}]:
            after = gpu(row(20), range(1, 21))
            after["gpu_frame_work_samples"].append(record)
            result = summary(row(0), after)
            self.assertEqual(result["status"], "PASS")
            measurement = result["outputs"][0]["gpu_command_buffer"]
            self.assertEqual(measurement["status"], "INCOMPLETE")
            self.assertIsNone(measurement["whole_run_command_buffer_p95_ms"])
        before, after = gpu(row(10), range(1, 11)), gpu(row(20), range(1, 21))
        after["gpu_frame_work_samples"][0]["gpu_time_ms"] = 9.
        result = summary(before, after)
        self.assertEqual(result["outputs"][0]["gpu_command_buffer"]["status"], "INCOMPLETE")

    def test_gpu_generation_and_unknown_scope_are_explicit(self):
        result = summary(gpu(row(10, generation=8), range(1, 11)),
                         gpu(row(20, generation=9), range(1, 21)))
        self.assertEqual(result["status"], "INCOMPLETE")
        self.assertEqual(result["outputs"][0]["gpu_command_buffer"]["status"], "INCOMPLETE")
        after = gpu(row(20), range(1, 21)); after["gpu_frame_work_scope"] = "unknown"
        result = summary(row(0), after)
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["outputs"][0]["gpu_command_buffer"]["status"], "UNSUPPORTED")
        for key in ("gpu_frame_work_scope", "compositor_main_thread_cpu_scope"):
            after = row(20); after[key] = []
            self.assertEqual(summary(row(0), after)["status"], "INCOMPLETE")
        after = row(20, long=1, overwritten=2)
        self.assertEqual(summary(row(0), after)["status"], "INCOMPLETE")

    def test_coverage_of_extreme_counter_gap_uses_compact_range_arithmetic(self):
        result = budget.coverage(0, 2**64 - 1, {1, 2**64 - 1})
        self.assertEqual(result["missing_ranges"], [[2, 2**64 - 2]])
        self.assertEqual(result["missing_count"], 2**64 - 3)


if __name__ == '__main__':
    unittest.main()
