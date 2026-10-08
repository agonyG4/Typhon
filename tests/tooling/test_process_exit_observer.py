import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[2]
OBSERVER_PATH = REPO_ROOT / "bin" / "observe-compositor-exit.py"
SPEC = importlib.util.spec_from_file_location("observe_compositor_exit", OBSERVER_PATH)
assert SPEC is not None and SPEC.loader is not None
OBSERVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OBSERVER)


class ExitClassificationTests(unittest.TestCase):
    def test_zero_exit_is_successful_shutdown(self):
        result = OBSERVER.classify_wait_result(0, compositor_was_direct_child=True)

        self.assertEqual(result["classification"], "successful_shutdown")
        self.assertEqual(result["exit_code"], 0)
        self.assertIsNone(result["terminating_signal"])

    def test_nonzero_exit_is_not_mistaken_for_a_signal(self):
        result = OBSERVER.classify_wait_result(143, compositor_was_direct_child=True)

        self.assertEqual(result["classification"], "nonzero_exit")
        self.assertEqual(result["exit_code"], 143)
        self.assertIsNone(result["terminating_signal"])

    def test_negative_wait_status_records_the_terminating_signal(self):
        result = OBSERVER.classify_wait_result(
            -signal.SIGTERM,
            compositor_was_direct_child=True,
        )

        self.assertEqual(result["classification"], "signal_termination")
        self.assertIsNone(result["exit_code"])
        self.assertEqual(result["terminating_signal"]["number"], signal.SIGTERM)
        self.assertEqual(result["terminating_signal"]["name"], "SIGTERM")
        self.assertEqual(result["launcher_exit_code"], 128 + signal.SIGTERM)

    def test_missing_wait_status_is_unknown_observation(self):
        result = OBSERVER.classify_wait_result(None, compositor_was_direct_child=True)

        self.assertEqual(result["classification"], "unknown")
        self.assertIsNone(result["exit_code"])
        self.assertIsNone(result["terminating_signal"])

    def test_cargo_wrapper_status_does_not_claim_compositor_exit_status(self):
        result = OBSERVER.classify_wait_result(101, compositor_was_direct_child=False)

        self.assertEqual(result["classification"], "unknown")
        self.assertIsNone(result["exit_code"])
        self.assertIsNone(result["terminating_signal"])
        self.assertEqual(result["supervisor_exit_code"], 101)


class ObserverIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory(prefix="typhon-exit-observer-")
        self.addCleanup(self.temp_dir.cleanup)
        self.state_dir = Path(self.temp_dir.name) / "state"

    def start_observer(self, command, profile="release"):
        return subprocess.Popen(
            [
                sys.executable,
                str(OBSERVER_PATH),
                "--state-dir",
                str(self.state_dir),
                "--socket-name",
                "observer-test",
                "--profile",
                profile,
                "--",
                *command,
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )

    def read_record(self):
        records = list((self.state_dir / "diagnostics" / "process-exits").glob("*.json"))
        self.assertEqual(len(records), 1)
        return json.loads(records[0].read_text())

    def test_records_successful_and_nonzero_child_exit(self):
        for exit_code, expected in ((0, "successful_shutdown"), (7, "nonzero_exit")):
            with self.subTest(exit_code=exit_code):
                observer = self.start_observer(
                    [sys.executable, "-c", f"raise SystemExit({exit_code})"]
                )
                _, stderr = observer.communicate(timeout=10)
                self.assertEqual(observer.returncode, exit_code, stderr)
                record = self.read_record()
                self.assertEqual(record["classification"], expected)
                self.assertEqual(record["exit_code"], exit_code)
                self.assertIsNone(record["terminating_signal"])
                self.assertLessEqual(record["started_at_unix_ms"], record["ended_at_unix_ms"])
                self.assertGreater(record["compositor_pid"], 0)
                self.assertEqual(record["session_identity"]["socket_name"], "observer-test")
                # Each subtest uses a separate observation record.
                for path in (self.state_dir / "diagnostics" / "process-exits").glob("*.json"):
                    path.unlink()

    def test_debug_wrapper_status_is_not_claimed_as_compositor_status(self):
        observer = self.start_observer(
            [sys.executable, "-c", "raise SystemExit(0)"],
            profile="debug",
        )
        _, stderr = observer.communicate(timeout=10)

        self.assertEqual(observer.returncode, 0, stderr)
        record = self.read_record()
        self.assertEqual(record["classification"], "unknown")
        self.assertEqual(record["observation_status"], "completed_with_unknown_compositor_status")
        self.assertIsNone(record["exit_code"])
        self.assertEqual(record["supervisor_exit_code"], 0)
        self.assertIsNone(record["compositor_pid"])

    def cleanup_observer(self, observer):
        if observer.poll() is None:
            records = list((self.state_dir / "diagnostics" / "process-exits").glob("*.json"))
            for path in records:
                record = json.loads(path.read_text())
                compositor_pid = record.get("compositor_pid")
                if isinstance(compositor_pid, int):
                    try:
                        os.kill(compositor_pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            observer.kill()
        observer.communicate()

    def test_forwards_signal_and_records_child_signal_classification(self):
        observer = self.start_observer(
            [sys.executable, "-c", "import time; time.sleep(60)"]
        )
        self.addCleanup(self.cleanup_observer, observer)

        record_path = self.state_dir / "diagnostics" / "process-exits"
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            records = list(record_path.glob("*.json")) if record_path.exists() else []
            if records and json.loads(records[0].read_text()).get("compositor_pid"):
                break
            time.sleep(0.01)
        else:
            self.fail("observer did not record the compositor PID before signal delivery")

        os.kill(observer.pid, signal.SIGTERM)
        _, stderr = observer.communicate(timeout=10)
        self.assertEqual(observer.returncode, 128 + signal.SIGTERM, stderr)
        record = self.read_record()
        self.assertEqual(record["classification"], "signal_termination")
        self.assertIsNone(record["exit_code"])
        self.assertEqual(record["terminating_signal"]["name"], "SIGTERM")
        self.assertEqual(record["signals_received"], [signal.SIGTERM])

    def test_diagnostic_storage_failure_does_not_prevent_child_launch(self):
        self.state_dir.parent.mkdir(parents=True, exist_ok=True)
        self.state_dir.write_text("block diagnostic directory creation")
        observer = self.start_observer(
            [sys.executable, "-c", "raise SystemExit(7)"]
        )
        stderr = observer.communicate(timeout=10)[1]

        self.assertEqual(observer.returncode, 7)
        self.assertIn("diagnostic", stderr.lower())

    def test_launcher_routes_release_child_through_observer(self):
        fake_compositor = Path(self.temp_dir.name) / "fake-oblivion-one"
        fake_compositor.write_text("#!/bin/sh\nexit 7\n")
        fake_compositor.chmod(0o755)
        environment = os.environ.copy()
        environment.update(
            {
                "OBLIVION_ONE_BINARY": str(fake_compositor),
                "OBLIVION_ONE_PROFILE": "release",
                "OBLIVION_ONE_STATE_DIR": str(self.state_dir),
                "OBLIVION_ONE_SKIP_INPUT_GROUP_CHECK": "1",
            }
        )
        environment.pop("OBLIVION_ONE_DRY_RUN", None)

        result = subprocess.run(
            [str(REPO_ROOT / "bin" / "start-oblivion-one")],
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
        )

        self.assertEqual(result.returncode, 7, result.stderr)
        record = self.read_record()
        self.assertEqual(record["classification"], "nonzero_exit")
        self.assertEqual(record["exit_code"], 7)
        self.assertTrue(record["session_identity"]["socket_name"].startswith("oblivion-one"))
        self.assertTrue((self.state_dir / "session.log").exists())

    def test_tty_launcher_preserves_child_process_group(self):
        facts_path = Path(self.temp_dir.name) / "child-facts.json"
        fake_compositor = Path(self.temp_dir.name) / "fake-oblivion-one"
        fake_compositor.write_text(
            f"#!{sys.executable}\n"
            "import json, os\n"
            "from pathlib import Path\n"
            "Path(os.environ['OBSERVER_TEST_FACTS']).write_text(json.dumps({"
            "'pid': os.getpid(), 'process_group': os.getpgrp()}))\n"
        )
        fake_compositor.chmod(0o755)
        environment = os.environ.copy()
        environment.update(
            {
                "OBLIVION_ONE_BINARY": str(fake_compositor),
                "OBLIVION_ONE_PROFILE": "release",
                "OBLIVION_ONE_STATE_DIR": str(self.state_dir),
                "OBLIVION_ONE_SKIP_INPUT_GROUP_CHECK": "1",
                "OBSERVER_TEST_FACTS": str(facts_path),
            }
        )

        result = subprocess.run(
            [str(REPO_ROOT / "bin" / "start-oblivion-one-tty")],
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        facts = json.loads(facts_path.read_text())
        record = self.read_record()
        self.assertEqual(facts["pid"], record["compositor_pid"])
        self.assertEqual(facts["process_group"], os.getpgrp())

    def test_correlates_only_matching_pid_and_observation_window(self):
        diagnostics = self.state_dir / "diagnostics"
        incomplete = diagnostics / "incomplete-runs"
        incomplete.mkdir(parents=True)
        process_id = os.getpid()
        boot_id = OBSERVER.read_boot_id()
        stale_record = {
            "status": "completed",
            "metadata": {
                "process_id": process_id,
                "boot_id": boot_id,
                "desktop_session_id": "test-session",
                "started_at_unix_ms": 99,
                "session_id": "stale-run",
            },
            "cause": {"category": "normal"},
            "runtime_phase": "complete",
        }
        (diagnostics / "last-termination.json").write_text(json.dumps(stale_record))
        current_record = json.loads(json.dumps(stale_record))
        current_record["metadata"]["started_at_unix_ms"] = 150
        current_record["metadata"]["session_id"] = "current-run"
        current_record["cause"]["category"] = "error"
        (incomplete / "current-run.json").write_text(json.dumps(current_record))

        match = OBSERVER.correlate_internal_record(
            diagnostics,
            compositor_pid=process_id,
            boot_id=boot_id,
            desktop_session_id="test-session",
            observation_started_at_ms=100,
            observation_ended_at_ms=200,
        )

        self.assertTrue(match["matched"])
        self.assertEqual(match["session_id"], "current-run")
        self.assertEqual(match["cause_category"], "error")

    def test_reconciles_dead_observer_as_interrupted_unknown(self):
        records = self.state_dir / "diagnostics" / "process-exits"
        records.mkdir(parents=True)
        record = {
            "observation_status": "observing",
            "observer_pid": 2147483647,
            "observer_start_ticks": 100,
            "boot_id": OBSERVER.read_boot_id(),
            "started_at_unix_ms": 100,
            "ended_at_unix_ms": None,
        }
        path = records / "interrupted.json"
        path.write_text(json.dumps(record))

        with mock.patch.object(OBSERVER, "proc_start_ticks", return_value=None):
            OBSERVER.reconcile_interrupted_records(records, OBSERVER.read_boot_id())

        reconciled = json.loads(path.read_text())
        self.assertEqual(reconciled["observation_status"], "interrupted")
        self.assertEqual(reconciled["classification"], "unknown")
        self.assertIsNone(reconciled["exit_code"])
        self.assertIsNone(reconciled["ended_at_unix_ms"])
        self.assertIn("reconciled_at_unix_ms", reconciled)


if __name__ == "__main__":
    unittest.main()
