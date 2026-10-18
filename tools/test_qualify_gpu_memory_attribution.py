import importlib.util
from importlib.machinery import SourceFileLoader
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "bin" / "qualify-gpu-memory-attribution"
LOADER = SourceFileLoader("gpu_memory_qualifier", str(SCRIPT))
SPEC = importlib.util.spec_from_loader("gpu_memory_qualifier", LOADER)
qualifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qualifier)


def valid_gpu_residency(coherent=True, event_sequence=12):
    return {
        "resource_telemetry": {
            "coherent": coherent,
            "event_sequence": event_sequence,
            "snapshot_retries": 0,
            "classes": [{"class": "drm_framebuffer", "accounting_error_count": 0}],
        },
        "attribution_summary": {
            "owned_storage_estimated_bytes": 100,
            "external_mapping_logical_bytes": 200,
            "unique_external_backing_bytes": None,
            "physical_vram_claim": False,
            "live_unresolved_class_count": 0,
            "live_unresolved_domain_count": 0,
        },
        "renderer_import_cache": {"active_mapping_count": 1},
        "renderer_import_cache_available": True,
        "direct_import_cache": {"entries": 0},
        "domains": [{"domain": "client_import"}],
    }


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
                {"gpuResidency": valid_gpu_residency(coherent=False)},
                {"gpuResidency": valid_gpu_residency(coherent=True, event_sequence=8)},
            ]
        )
        snapshot, residency, retries = qualifier.coherent_internal_sample(lambda: next(snapshots), 3)
        self.assertEqual(retries, 1)
        self.assertTrue(residency["resource_telemetry"]["coherent"])
        self.assertIn("gpuResidency", snapshot)

    def test_persistent_incoherence_is_not_hidden(self):
        with self.assertRaises(qualifier.IncoherentSnapshot):
            qualifier.coherent_internal_sample(
                lambda: {"gpuResidency": valid_gpu_residency(coherent=False)},
                3,
            )

    def test_old_client_response_without_stage_2_3_residency_is_rejected(self):
        with self.assertRaises(qualifier.InternalSampleFailure) as context:
            qualifier.coherent_internal_sample(lambda: {"performance": {}}, 1)

        self.assertEqual(context.exception.reason, "astreactl_response_missing_gpu_residency")


class ControlClientTests(unittest.TestCase):
    def _client(self, path, executable=True):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"matching astreactl")
        path.chmod(0o755 if executable else 0o644)
        return path

    def test_explicit_client_wins_and_identity_records_path_hash_mtime_and_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            explicit = self._client(root / "explicit" / "astreactl")
            expected = root / "expected" / "oblivion-one"
            running = root / "running" / "oblivion-one"
            self._client(expected.parent / "astreactl")
            self._client(running.parent / "astreactl")

            client = qualifier.resolve_control_client(explicit, expected, running)

        self.assertEqual(client.identity["path"], str(explicit))
        self.assertEqual(client.identity["sha256"], hashlib.sha256(b"matching astreactl").hexdigest())
        self.assertIsNotNone(client.identity["mtime_utc"])
        self.assertEqual(client.identity["resolution_source"], "explicit")

    def test_expected_binary_sibling_is_selected_before_running_sibling(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = root / "expected" / "oblivion-one"
            running = root / "running" / "oblivion-one"
            expected_client = self._client(expected.parent / "astreactl")
            self._client(running.parent / "astreactl")

            client = qualifier.resolve_control_client(None, expected, running)

        self.assertEqual(client.path, expected_client)
        self.assertEqual(client.identity["resolution_source"], "expected_binary_sibling")

    def test_running_binary_sibling_is_used_when_expected_sibling_is_absent(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = root / "expected" / "oblivion-one"
            running = root / "running" / "oblivion-one"
            running_client = self._client(running.parent / "astreactl")

            client = qualifier.resolve_control_client(None, expected, running)

        self.assertEqual(client.path, running_client)
        self.assertEqual(client.identity["resolution_source"], "running_binary_sibling")

    def test_unrelated_path_client_is_ignored(self):
        with tempfile.TemporaryDirectory() as directory:
            path_client = self._client(Path(directory) / "path-bin" / "astreactl")
            expected = Path(directory) / "expected" / "oblivion-one"
            running = Path(directory) / "running" / "oblivion-one"
            with mock.patch.dict(os.environ, {"PATH": str(path_client.parent)}):
                with self.assertRaises(qualifier.ControlClientResolutionError) as context:
                    qualifier.resolve_control_client(None, expected, running)

        self.assertEqual(context.exception.reason, "astreactl_not_found")

    def test_missing_explicit_client_fails_without_falling_back(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self._client(root / "running" / "astreactl")
            with self.assertRaises(qualifier.ControlClientResolutionError) as context:
                qualifier.resolve_control_client(root / "missing" / "astreactl", None, root / "running" / "oblivion-one")

        self.assertEqual(context.exception.reason, "astreactl_not_found")

    def test_non_executable_exact_client_fails_clearly(self):
        with tempfile.TemporaryDirectory() as directory:
            candidate = self._client(Path(directory) / "astreactl", executable=False)
            with self.assertRaises(qualifier.ControlClientResolutionError) as context:
                qualifier.resolve_control_client(candidate, None, Path(directory) / "oblivion-one")

        self.assertEqual(context.exception.reason, "astreactl_not_executable")

    def test_internal_fetch_executes_the_resolved_absolute_path(self):
        exact_path = Path("/opt/typhon-release/astreactl")
        with mock.patch.object(qualifier, "_run_json", return_value={}) as run_json:
            qualifier._internal_fetch(exact_path)

        run_json.assert_called_once_with([str(exact_path), "--json", "performance"])

    def test_cli_accepts_explicit_astreactl_path(self):
        args = qualifier.build_parser().parse_args(["--astreactl", "/exact/astreactl"])

        self.assertEqual(args.astreactl, Path("/exact/astreactl"))


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
            "astreactl": None,
        })()

    def _valid_internal_response(self):
        return {"gpuResidency": valid_gpu_residency()}

    def _pmon_result(self):
        return type("Result", (), {"stdout": "# gpu pid type fb ccpm command\n0 42 G 124 0 oblivion-one\n"})()

    def _qualify_with_response(self, root, response, accounting_errors=0):
        expected_binary = root / "release" / "oblivion-one"
        expected_binary.parent.mkdir(parents=True, exist_ok=True)
        expected_binary.write_bytes(b"stage2.3 fixture")
        expected_binary.chmod(0o755)
        client_path = root / "release" / "astreactl"
        client_path.write_bytes(b"compatible astreactl")
        client_path.chmod(0o755)
        digest = hashlib.sha256(expected_binary.read_bytes()).hexdigest()
        identity = self._identity(digest)
        if accounting_errors:
            response["gpuResidency"]["resource_telemetry"]["classes"][0]["accounting_error_count"] = accounting_errors
        with (
            mock.patch.object(qualifier, "process_identity", return_value=identity),
            mock.patch.object(qualifier, "git_identity", return_value={"head": "abc", "branch": "main"}),
            mock.patch.object(qualifier, "_driver_info", return_value={"driver_version": "test", "gpu_name": "test"}),
            mock.patch.object(qualifier, "_internal_fetch", return_value=response) as internal_fetch,
            mock.patch.object(qualifier.subprocess, "run", return_value=self._pmon_result()),
            mock.patch.object(qualifier, "drm_fdinfo_probe", return_value={"supported": False, "reason": "no_standard_drm_memory_fields", "fields": {}}),
        ):
            report = qualifier.qualify(self._args(expected_binary))

        internal_fetch.assert_called_with(client_path)
        return report, client_path

    def test_internal_command_failure_still_reports_external_samples_as_invalid(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = root / "oblivion-one"
            expected.write_bytes(b"stage2.3 fixture")
            expected.chmod(0o755)
            digest = hashlib.sha256(expected.read_bytes()).hexdigest()
            identity = self._identity(digest)
            client_path = root / "astreactl"
            client_path.write_bytes(b"compatible client")
            client_path.chmod(0o755)
            with (
                mock.patch.object(qualifier, "process_identity", return_value=identity),
                mock.patch.object(qualifier, "git_identity", return_value={"head": "abc", "branch": "main"}),
                mock.patch.object(qualifier, "_driver_info", return_value={"driver_version": "test", "gpu_name": "test"}),
                mock.patch.object(qualifier, "_internal_fetch", side_effect=qualifier.subprocess.CalledProcessError(1, [str(client_path)])),
                mock.patch.object(qualifier.subprocess, "run", return_value=self._pmon_result()),
                mock.patch.object(qualifier, "drm_fdinfo_probe", return_value={"supported": False, "reason": "no_standard_drm_memory_fields", "fields": {}}),
            ):
                report = qualifier.qualify(self._args(expected))

        self.assertFalse(report["valid"])
        self.assertEqual(report["invalid_reason"], "astreactl_internal_command_failed")
        self.assertEqual(report["nvidia_process_fb_mib"]["samples"], [124.0, 124.0])
        self.assertEqual(len(report["internal_samples"]), 2)
        self.assertTrue(all(not sample["coherent"] for sample in report["internal_samples"]))

    def test_missing_exact_client_keeps_external_samples_and_invalidates_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = root / "release" / "oblivion-one"
            expected.parent.mkdir(parents=True)
            expected.write_bytes(b"stage2.3 fixture")
            identity = self._identity(hashlib.sha256(expected.read_bytes()).hexdigest())
            with (
                mock.patch.object(qualifier, "process_identity", return_value=identity),
                mock.patch.object(qualifier, "git_identity", return_value={"head": "abc", "branch": "main"}),
                mock.patch.object(qualifier, "_driver_info", return_value={"driver_version": "test", "gpu_name": "test"}),
                mock.patch.object(qualifier.subprocess, "run", return_value=self._pmon_result()),
                mock.patch.object(qualifier, "drm_fdinfo_probe", return_value={"supported": False, "reason": "no_standard_drm_memory_fields", "fields": {}}),
            ):
                report = qualifier.qualify(self._args(expected))

        self.assertFalse(report["valid"])
        self.assertEqual(report["invalid_reason"], "astreactl_not_found")
        self.assertEqual(report["nvidia_process_fb_mib"]["samples"], [124.0, 124.0])

    def test_old_incompatible_client_response_remains_invalid(self):
        with tempfile.TemporaryDirectory() as directory:
            report, _ = self._qualify_with_response(Path(directory), {"performance": {}})

        self.assertFalse(report["valid"])
        self.assertEqual(report["invalid_reason"], "astreactl_response_missing_gpu_residency")
        self.assertEqual(report["nvidia_process_fb_mib"]["valid_sample_count"], 2)
        self.assertFalse(report["physical_vram_claim"])
        self.assertIsNone(report["coverage_reconciliation_status"]["accounting_error_count"])

    def test_coherent_stage_2_3_response_is_accepted_and_reports_client_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            report, client_path = self._qualify_with_response(Path(directory), self._valid_internal_response())

        self.assertTrue(report["valid"])
        self.assertEqual(report["control_client"]["path"], str(client_path))
        self.assertEqual(report["control_client"]["sha256"], hashlib.sha256(b"compatible astreactl").hexdigest())
        self.assertIsNotNone(report["control_client"]["mtime_utc"])
        self.assertEqual(report["control_client"]["resolution_source"], "expected_binary_sibling")
        self.assertFalse(report["physical_vram_claim"])
        self.assertIn("not arithmetically additive", report["accounting_note"])
        self.assertFalse(report["drm_fdinfo"]["supported"])
        self.assertEqual(report["owned_storage_estimated_bytes"], 100)
        self.assertEqual(report["external_mapping_logical_bytes"], 200)

    def test_resource_accounting_errors_invalidate_a_coherent_run(self):
        with tempfile.TemporaryDirectory() as directory:
            report, _ = self._qualify_with_response(Path(directory), self._valid_internal_response(), accounting_errors=1)

        self.assertFalse(report["valid"])
        self.assertEqual(report["invalid_reason"], "resource_telemetry_accounting_errors_nonzero")
        self.assertEqual(report["coverage_reconciliation_status"]["accounting_error_count"], 1)

    def test_pid_change_during_sample_invalidates_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = root / "oblivion-one"
            expected.write_bytes(b"stage2.3 fixture")
            expected.chmod(0o755)
            client_path = root / "astreactl"
            client_path.write_bytes(b"compatible client")
            client_path.chmod(0o755)
            digest = hashlib.sha256(expected.read_bytes()).hexdigest()
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
                        self._valid_internal_response()["gpuResidency"],
                        0,
                    ),
                ),
                mock.patch.object(qualifier.subprocess, "run", return_value=pmon),
                mock.patch.object(qualifier, "drm_fdinfo_probe", return_value={"supported": False}),
            ):
                report = qualifier.qualify(self._args(expected))

        self.assertFalse(report["valid"])
        self.assertEqual(report["invalid_reason"], "target_pid_identity_changed")
        self.assertFalse(report["process_identity_verified"])
        self.assertEqual(report["nvidia_process_fb_mib"]["valid_sample_count"], 0)


if __name__ == "__main__":
    unittest.main()
