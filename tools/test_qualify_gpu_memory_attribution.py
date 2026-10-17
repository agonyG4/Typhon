import importlib.util
from importlib.machinery import SourceFileLoader
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "bin" / "qualify-gpu-memory-attribution"
LOADER = SourceFileLoader("gpu_memory_qualifier", str(SCRIPT))
SPEC = importlib.util.spec_from_loader("gpu_memory_qualifier", LOADER)
qualifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qualifier)


class PmonParserTests(unittest.TestCase):
    def test_parses_exact_graphics_pid_and_framebuffer_mib(self):
        output = """# gpu         pid   type     fb   ccpm    command
# Idx           #    C/G     MB     MB    name
    0        1112     G     90      0    other
    0         112     G     34      0    oblivion-one
"""
        self.assertEqual(qualifier.parse_pmon_fb_mib(output, 112), 34.0)

    def test_missing_process_row_is_reported_without_matching_a_prefix_pid(self):
        output = """# gpu pid type fb ccpm command
0 1120 G 90 0 other
"""
        self.assertIsNone(qualifier.parse_pmon_fb_mib(output, 112))

    def test_unsupported_memory_field_is_explicit(self):
        output = """# gpu pid type sm mem command
0 112 G 0 10 oblivion-one
"""
        with self.assertRaises(qualifier.UnsupportedMemoryField):
            qualifier.parse_pmon_fb_mib(output, 112)

    def test_unsupported_process_memory_value_is_explicit(self):
        output = """# gpu pid type fb ccpm command
0 112 G - 0 oblivion-one
"""
        with self.assertRaises(qualifier.UnsupportedMemoryField):
            qualifier.parse_pmon_fb_mib(output, 112)


class ProcessAndSnapshotTests(unittest.TestCase):
    def test_pid_reuse_or_exec_change_invalidates_identity(self):
        first = {"pid": 112, "exe": "/usr/bin/oblivion-one", "binary_sha256": "a", "process_start_time_ticks": 50}
        same = dict(first)
        changed = dict(first, process_start_time_ticks=51)
        self.assertTrue(qualifier.same_process_identity(first, same))
        self.assertFalse(qualifier.same_process_identity(first, changed))

    def test_incoherent_internal_snapshot_retries_then_accepts_coherent_sample(self):
        snapshots = iter(
            [
                {"gpuResidency": {"resource_telemetry": {"coherent": False}}},
                {"gpuResidency": {"resource_telemetry": {"coherent": True, "event_sequence": 8}}},
            ]
        )
        snapshot, residency, retries = qualifier.coherent_internal_sample(lambda: next(snapshots), 3)
        self.assertEqual(retries, 1)
        self.assertTrue(residency["resource_telemetry"]["coherent"])
        self.assertIn("gpuResidency", snapshot)

    def test_persistent_incoherence_is_not_hidden(self):
        with self.assertRaises(qualifier.IncoherentSnapshot):
            qualifier.coherent_internal_sample(
                lambda: {"gpuResidency": {"resource_telemetry": {"coherent": False}}},
                3,
            )


class FdinfoAndAggregationTests(unittest.TestCase):
    def test_drm_fdinfo_memory_fields_are_supported(self):
        result = qualifier.parse_drm_fdinfo_contents(
            ["pos: 0\nflags: 0100002\ndrm-client-id: 7\ndrm-memory-vram: 4096"]
        )
        self.assertTrue(result["supported"])
        self.assertEqual(result["fields"]["drm-memory-vram"], ["4096"])

    def test_basic_fdinfo_is_unsupported(self):
        result = qualifier.parse_drm_fdinfo_contents(["pos: 0\nflags: 0100002\nmnt_id: 22\nino: 3"])
        self.assertFalse(result["supported"])
        self.assertEqual(result["reason"], "no_standard_drm_memory_fields")

    def test_min_median_max_aggregate_all_samples(self):
        result = qualifier.summarize_mib([200.0, 230.0, 210.0, None])
        self.assertEqual((result["min"], result["median"], result["max"]), (200.0, 210.0, 230.0))
        self.assertEqual(result["valid_sample_count"], 3)


class QualificationFlowTests(unittest.TestCase):
    def _identity(self, digest):
        return {
            "pid": 42,
            "exe": "/usr/bin/oblivion-one",
            "binary_sha256": digest,
            "binary_mtime_unix_ns": 1,
            "binary_mtime_utc": "2026-10-16T00:00:00+00:00",
            "process_start_time_ticks": 77,
            "process_start_time_utc": "2026-10-16T00:00:00+00:00",
            "oblivion_one_render_worker": None,
        }

    def _args(self, expected_binary):
        return type("Args", (), {
            "pid": 42,
            "repo": Path("/repo"),
            "samples": 2,
            "interval": 0,
            "coherence_retries": 2,
            "expected_binary": expected_binary,
        })()

    def test_internal_command_failure_still_reports_external_samples_as_invalid(self):
        with tempfile.NamedTemporaryFile() as binary:
            binary.write(b"stage2.3 fixture")
            binary.flush()
            digest = hashlib.sha256(b"stage2.3 fixture").hexdigest()
            identity = self._identity(digest)
            pmon = type("Result", (), {"stdout": "# gpu pid type fb ccpm command\n0 42 G 124 0 oblivion-one\n"})()
            with (
                mock.patch.object(qualifier, "process_identity", return_value=identity),
                mock.patch.object(qualifier, "git_identity", return_value={"head": "abc", "branch": "main"}),
                mock.patch.object(qualifier, "_driver_info", return_value={"driver_version": "test", "gpu_name": "test"}),
                mock.patch.object(
                    qualifier,
                    "coherent_internal_sample",
                    side_effect=qualifier.IncoherentSnapshot("resource telemetry remained incoherent"),
                ),
                mock.patch.object(qualifier.subprocess, "run", return_value=pmon),
                mock.patch.object(qualifier, "drm_fdinfo_probe", return_value={"supported": False}),
            ):
                report = qualifier.qualify(self._args(Path(binary.name)))

        self.assertFalse(report["valid"])
        self.assertIn("internal_sample_failed", report["invalid_reason"])
        self.assertEqual(report["nvidia_process_fb_mib"]["samples"], [124.0, 124.0])
        self.assertEqual(len(report["internal_samples"]), 2)
        self.assertTrue(all(not sample["coherent"] for sample in report["internal_samples"]))

    def test_pid_change_during_sample_invalidates_run(self):
        with tempfile.NamedTemporaryFile() as binary:
            binary.write(b"stage2.3 fixture")
            binary.flush()
            digest = hashlib.sha256(b"stage2.3 fixture").hexdigest()
            identity = self._identity(digest)
            changed = dict(identity, process_start_time_ticks=78)
            pmon = type("Result", (), {"stdout": "# gpu pid type fb ccpm command\n0 42 G 124 0 oblivion-one\n"})()
            with (
                mock.patch.object(
                    qualifier,
                    "process_identity",
                    side_effect=[identity, identity, changed, changed],
                ),
                mock.patch.object(qualifier, "git_identity", return_value={"head": "abc", "branch": "main"}),
                mock.patch.object(qualifier, "_driver_info", return_value={"driver_version": "test", "gpu_name": "test"}),
                mock.patch.object(
                    qualifier,
                    "coherent_internal_sample",
                    return_value=(
                        {},
                        {"resource_telemetry": {"coherent": True, "classes": []}},
                        0,
                    ),
                ),
                mock.patch.object(qualifier.subprocess, "run", return_value=pmon),
                mock.patch.object(qualifier, "drm_fdinfo_probe", return_value={"supported": False}),
            ):
                report = qualifier.qualify(self._args(Path(binary.name)))

        self.assertFalse(report["valid"])
        self.assertEqual(report["invalid_reason"], "target_pid_identity_changed_during_sample")
        self.assertFalse(report["process_identity_verified"])
        self.assertEqual(report["nvidia_process_fb_mib"]["valid_sample_count"], 0)


if __name__ == "__main__":
    unittest.main()
