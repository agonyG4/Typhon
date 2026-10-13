import unittest
from pathlib import Path

from scripts.typhon_cpu_saturation_probe import (
    cgroup_path_chain,
    cgroup_cpu_stat_deltas,
    counter_delta,
    parse_cpu_stat,
    parse_meminfo,
    parse_psi,
    parse_proc_stat,
    parse_proc_stat_counters,
    parse_proc_start_time,
    parse_proc_status_context_switches,
    parse_schedstat,
    parse_vmstat,
    percentile,
    percentile_summary,
    sampled_threads,
    thread_identity_key,
    validate_output_path,
)


class ParseSchedstatTests(unittest.TestCase):
    def test_reads_runtime_wait_and_timeslice_counters(self) -> None:
        self.assertEqual(parse_schedstat("1000000 250000 7\n"), (1_000_000, 250_000, 7))

    def test_rejects_missing_or_negative_counters(self) -> None:
        for contents in ("1 2", "1 -2 3", "one 2 3"):
            with self.subTest(contents=contents), self.assertRaises(ValueError):
                parse_schedstat(contents)

    def test_reads_process_start_time_when_comm_contains_parentheses(self) -> None:
        stat_fields = ["R"] + [str(value) for value in range(4, 23)]
        self.assertEqual(
            parse_proc_start_time(f"123 (typhon ) worker) {' '.join(stat_fields)}"),
            22,
        )
        self.assertEqual(
            parse_proc_stat(f"123 (typhon ) worker) {' '.join(stat_fields)}"),
            ("R", 22),
        )
        zombie_fields = ["Z"] + [str(value) for value in range(4, 23)]
        self.assertEqual(
            parse_proc_stat(f"123 (typhon) {' '.join(zombie_fields)}"),
            ("Z", 22),
        )

    def test_rejects_malformed_process_stat(self) -> None:
        for contents in ("123 typhon R", "123 (typhon) R 1 2"):
            with self.subTest(contents=contents), self.assertRaises(ValueError):
                parse_proc_start_time(contents)

    def test_reads_task_fault_counters_from_proc_stat(self) -> None:
        fields = ["R"] + [str(value) for value in range(4, 23)]
        self.assertEqual(
            parse_proc_stat_counters(f"123 (typhon) {' '.join(fields)}"),
            {"start_time_ticks": 22, "minor_faults": 10, "major_faults": 12},
        )

    def test_reads_context_switch_counters_from_proc_status(self) -> None:
        self.assertEqual(
            parse_proc_status_context_switches(
                "Name:\ttyphon\nvoluntary_ctxt_switches:\t19\n"
                "nonvoluntary_ctxt_switches:\t4\n"
            ),
            {
                "voluntary_context_switches": 19,
                "involuntary_context_switches": 4,
            },
        )


class ParsePressureTests(unittest.TestCase):
    def test_parses_present_pressure_fields(self) -> None:
        psi = parse_psi(
            "some avg10=0.12 avg60=0.34 avg300=0.56 total=12345\n"
            "full avg10=0.01 avg60=0.02 avg300=0.03 total=99\n"
        )
        self.assertEqual(psi["some"]["avg10"], 0.12)
        self.assertEqual(psi["some"]["total"], 12345)
        self.assertEqual(psi["full"]["total"], 99)

    def test_does_not_invent_missing_pressure_categories(self) -> None:
        self.assertEqual(
            parse_psi("some avg10=0.5 total=10\n"),
            {"some": {"avg10": 0.5, "total": 10}},
        )

    def test_meminfo_reports_available_memory_and_swap_in_bytes(self) -> None:
        parsed = parse_meminfo(
            "MemTotal: 1000 kB\nMemAvailable: 400 kB\n"
            "SwapTotal: 2000 kB\nSwapFree: 500 kB\n"
        )
        self.assertEqual(parsed["mem_available_bytes"], 409_600)
        self.assertEqual(parsed["swap_used_bytes"], 1_536_000)

    def test_vmstat_reports_active_swap_io_counters_in_pages(self) -> None:
        self.assertEqual(
            parse_vmstat("pswpin 120\npgfault 4000\npswpout 17\n"),
            {"swap_in_pages": 120, "swap_out_pages": 17},
        )


class CounterTests(unittest.TestCase):
    def test_cgroup_path_chain_stops_at_the_mount_root(self) -> None:
        root = Path("/tmp/cgroup-test")
        self.assertEqual(
            cgroup_path_chain(root / "user.slice" / "typhon", root),
            [root / "user.slice" / "typhon", root / "user.slice", root],
        )

    def test_parses_cgroup_cpu_stat_and_deltas(self) -> None:
        earlier = parse_cpu_stat(
            "usage_usec 100\nnr_periods 4\nnr_throttled 1\nthrottled_usec 20\n"
        )
        later = parse_cpu_stat("usage_usec 160\nnr_periods 6\nnr_throttled 2\nthrottled_usec 50\n")
        self.assertEqual(
            counter_delta(later, earlier),
            {"nr_periods": 2, "nr_throttled": 1, "throttled_usec": 30, "usage_usec": 60},
        )

    def test_reports_throttling_deltas_for_visible_cgroup_ancestors(self) -> None:
        earlier = {
            "cpu_weight_hierarchy": [
                {"path": "/user.slice/app.scope", "cpu_stat": {"nr_throttled": 1}},
                {"path": "/user.slice", "cpu_stat": {"nr_throttled": 4}},
            ]
        }
        later = {
            "cpu_weight_hierarchy": [
                {"path": "/user.slice/app.scope", "cpu_stat": {"nr_throttled": 3}},
                {"path": "/user.slice", "cpu_stat": {"nr_throttled": 9}},
            ]
        }
        self.assertEqual(
            cgroup_cpu_stat_deltas(later, earlier),
            {"/user.slice": {"nr_throttled": 5}, "/user.slice/app.scope": {"nr_throttled": 2}},
        )

    def test_counter_delta_preserves_unknown_or_reset_state(self) -> None:
        self.assertIsNone(counter_delta(None, {"usage_usec": 1}))
        self.assertIsNone(counter_delta({"usage_usec": 2}, {"usage_usec": 3}))

    def test_thread_delta_reports_cpu_and_runnable_wait(self) -> None:
        previous = {
            10: {
                "runtime_ns": 100,
                "runnable_wait_ns": 40,
                "timeslices": 3,
                "start_time_ticks": 7,
            }
        }
        current = {
            10: {
                "runtime_ns": 300,
                "runnable_wait_ns": 140,
                "timeslices": 5,
                "name": "reactor",
                "nice": 0,
                "scheduler_policy": "SCHED_OTHER",
                "scheduler_policy_id": 0,
                "affinity_cpus": [0, 1],
                "start_time_ticks": 7,
            },
            11: {
                "runtime_ns": 50,
                "runnable_wait_ns": 0,
                "timeslices": 1,
                "name": "new",
                "nice": 0,
                "scheduler_policy": "SCHED_OTHER",
                "scheduler_policy_id": 0,
                "affinity_cpus": [0, 1],
                "start_time_ticks": 8,
            },
        }
        result = sampled_threads(current, previous, 1_000)
        self.assertEqual([thread["tid"] for thread in result], [10, 11])
        self.assertEqual(result[0]["cpu_percent"], 20.0)
        self.assertEqual(result[0]["runnable_wait_ms_per_second"], 100.0)
        self.assertIsNone(result[1]["runtime_delta_ns"])

    def test_thread_delta_reports_faults_and_context_switch_rates(self) -> None:
        previous = {
            10: {
                "start_time_ticks": 7,
                "runtime_ns": 0,
                "runnable_wait_ns": 0,
                "timeslices": 0,
                "minor_faults": 10,
                "major_faults": 2,
                "voluntary_context_switches": 4,
                "involuntary_context_switches": 1,
            }
        }
        current = {
            10: {
                **previous[10],
                "minor_faults": 16,
                "major_faults": 3,
                "voluntary_context_switches": 10,
                "involuntary_context_switches": 3,
            }
        }
        result = sampled_threads(current, previous, 2_000_000_000)[0]
        self.assertEqual(result["minor_faults_delta"], 6)
        self.assertEqual(result["major_faults_delta"], 1)
        self.assertEqual(result["voluntary_context_switches_delta"], 6)
        self.assertEqual(result["involuntary_context_switches_delta"], 2)
        self.assertEqual(result["major_faults_per_second"], 0.5)
        self.assertEqual(result["involuntary_context_switches_per_second"], 1.0)

    def test_does_not_compute_deltas_across_reused_thread_ids(self) -> None:
        previous = {
            10: {
                "runtime_ns": 100,
                "runnable_wait_ns": 40,
                "timeslices": 3,
                "start_time_ticks": 7,
            }
        }
        current = {
            10: {
                "runtime_ns": 300,
                "runnable_wait_ns": 140,
                "timeslices": 5,
                "start_time_ticks": 8,
            }
        }
        result = sampled_threads(current, previous, 1_000)[0]
        self.assertIsNone(result["runtime_delta_ns"])
        self.assertIsNone(result["runnable_wait_delta_ns"])

    def test_thread_instance_summary_key_includes_start_time(self) -> None:
        self.assertEqual(thread_identity_key(42, 100), "42@100")
        self.assertEqual(thread_identity_key(42, None), "42")


class PercentileTests(unittest.TestCase):
    def test_interpolates_percentiles_and_handles_empty_samples(self) -> None:
        self.assertEqual(percentile([1, 2, 3, 4], 50), 2.5)
        self.assertAlmostEqual(percentile([1, 2, 3, 4], 95) or 0, 3.85)
        self.assertIsNone(percentile([], 99))

    def test_summary_reports_per_thread_percentiles_and_sample_count(self) -> None:
        summary = percentile_summary(
            {
                42: {
                    "cpu_percent": [10.0, 20.0, 30.0],
                    "runnable_wait_ms_per_second": [0.0, 100.0, 300.0],
                }
            }
        )
        self.assertEqual(summary["42"]["cpu_percent_p50"], 20.0)
        self.assertEqual(summary["42"]["runnable_wait_ms_per_second_p95"], 280.0)
        self.assertEqual(summary["42"]["cpu_percent_sample_count"], 3)


class OutputPathTests(unittest.TestCase):
    def test_requires_an_absolute_path_under_the_configured_output_root(self) -> None:
        root = Path("/mnt/Aether/Desktop/GitHub")
        self.assertEqual(
            validate_output_path(root / "trial.jsonl", root),
            root.resolve() / "trial.jsonl",
        )
        for path in (Path("trial.jsonl"), Path("/tmp/trial.jsonl"), root):
            with self.subTest(path=path), self.assertRaises(ValueError):
                validate_output_path(path, root)

    def test_accepts_local_diagnostics_and_legacy_aether_paths(self) -> None:
        local = Path(__file__).resolve().parents[2] / "diagnostics" / "test" / "os.jsonl"
        aether = Path("/mnt/Aether/Desktop/GitHub/Typhon-cpu-saturation/test/os.jsonl")
        self.assertEqual(validate_output_path(local), local.resolve())
        self.assertEqual(validate_output_path(aether), aether.resolve())
        for path in (Path("/tmp/os.jsonl"), Path("diagnostics/os.jsonl")):
            with self.subTest(path=path), self.assertRaises(ValueError):
                validate_output_path(path)


if __name__ == "__main__":
    unittest.main()
