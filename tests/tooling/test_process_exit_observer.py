import importlib.util
import json
import os
import pty
import select
import signal
import subprocess
import sys
import tempfile
import termios
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

    def start_observer(self, command, profile="release", **popen_options):
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
            **popen_options,
        )

    def wait_for_compositor_pid(self, observer):
        record_dir = self.state_dir / "diagnostics" / "process-exits"
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            records = list(record_dir.glob("*.json")) if record_dir.exists() else []
            if records:
                record = json.loads(records[0].read_text())
                if record.get("compositor_pid"):
                    return record["compositor_pid"]
            time.sleep(0.01)
        self.fail(f"observer {observer.pid} did not record the compositor PID")

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

        self.wait_for_compositor_pid(observer)
        os.kill(observer.pid, signal.SIGTERM)
        _, stderr = observer.communicate(timeout=10)
        self.assertEqual(observer.returncode, 128 + signal.SIGTERM, stderr)
        record = self.read_record()
        self.assertEqual(record["classification"], "signal_termination")
        self.assertIsNone(record["exit_code"])
        self.assertEqual(record["terminating_signal"]["name"], "SIGTERM")
        self.assertEqual(record["signals_received"], [signal.SIGTERM])

    def test_direct_compositor_signal_is_observed_without_observer_forwarding(self):
        observer = self.start_observer(
            [sys.executable, "-c", "import time; time.sleep(60)"]
        )
        self.addCleanup(self.cleanup_observer, observer)

        compositor_pid = self.wait_for_compositor_pid(observer)
        os.kill(compositor_pid, signal.SIGTERM)
        _, stderr = observer.communicate(timeout=10)

        self.assertEqual(observer.returncode, 128 + signal.SIGTERM, stderr)
        record = self.read_record()
        self.assertEqual(record["classification"], "signal_termination")
        self.assertEqual(record["terminating_signal"]["name"], "SIGTERM")
        self.assertEqual(record["signals_received"], [])

    def test_shared_process_group_signal_is_forwarded_twice_after_graceful_handler(self):
        events_path = Path(self.temp_dir.name) / "group-signal-events.json"
        ready_path = Path(self.temp_dir.name) / "group-signal-ready"
        graceful_path = Path(self.temp_dir.name) / "group-signal-graceful"
        child = (
            "import json, signal, sys, time\n"
            "from pathlib import Path\n"
            "events_path, ready_path, graceful_path = map(Path, sys.argv[1:])\n"
            "events = []\n"
            "def on_term(signum, frame):\n"
            "    events.append(signum)\n"
            "    events_path.write_text(json.dumps(events))\n"
            "    if len(events) == 1:\n"
            "        time.sleep(0.1)\n"
            "        graceful_path.write_text('first TERM handled')\n"
            "    else:\n"
            "        raise SystemExit(92)\n"
            "signal.signal(signal.SIGTERM, on_term)\n"
            "ready_path.write_text('ready')\n"
            "while True: time.sleep(1)\n"
        )
        observer = self.start_observer(
            [sys.executable, "-c", child, str(events_path), str(ready_path), str(graceful_path)],
            start_new_session=True,
        )
        self.addCleanup(self.cleanup_observer, observer)
        compositor_pid = self.wait_for_compositor_pid(observer)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and not ready_path.exists():
            time.sleep(0.01)
        self.assertTrue(ready_path.exists(), "compositor signal handler was not ready")
        self.assertEqual(os.getpgid(compositor_pid), observer.pid)

        # Hold the observer until the compositor has completed one graceful
        # TERM handler. Resuming it releases the group-delivered TERM to the
        # observer, whose forwarder sends a second TERM to the compositor.
        os.kill(observer.pid, signal.SIGSTOP)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                state = next(
                    line.split()[1]
                    for line in Path(f"/proc/{observer.pid}/status").read_text().splitlines()
                    if line.startswith("State:")
                )
            except (OSError, StopIteration):
                state = None
            if state == "T":
                break
            time.sleep(0.005)
        self.assertEqual(state, "T", "observer did not stop before the group signal")
        os.killpg(observer.pid, signal.SIGTERM)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not graceful_path.exists():
            time.sleep(0.01)
        self.assertTrue(graceful_path.exists(), "compositor did not finish its first TERM handler")
        os.kill(observer.pid, signal.SIGCONT)

        _, stderr = observer.communicate(timeout=10)
        self.assertEqual(observer.returncode, 92, stderr)
        self.assertEqual(json.loads(events_path.read_text()), [signal.SIGTERM, signal.SIGTERM])
        record = self.read_record()
        self.assertEqual(record["signals_received"], [signal.SIGTERM])
        self.assertEqual(record["classification"], "nonzero_exit")
        self.assertEqual(record["exit_code"], 92)

    @unittest.skipUnless(hasattr(termios, "TIOCSCTTY"), "requires Linux PTY session ioctls")
    def test_tty_launcher_has_controlling_terminal_and_foreground_process_group(self):
        facts_path = Path(self.temp_dir.name) / "pty-child-facts.json"
        fake_compositor = Path(self.temp_dir.name) / "fake-pty-compositor"
        fake_compositor.write_text(
            f"#!{sys.executable}\n"
            "import json, os\n"
            "from pathlib import Path\n"
            "Path(os.environ['OBSERVER_TEST_FACTS']).write_text(json.dumps({"
            "'pid': os.getpid(), 'session_id': os.getsid(0), 'process_group': os.getpgrp(), "
            "'foreground_process_group': os.tcgetpgrp(0), 'tty_name': os.ttyname(0), "
            "'stdin_is_tty': os.isatty(0)}))\n"
        )
        fake_compositor.chmod(0o755)
        master_fd, slave_fd = pty.openpty()
        tty_name = os.ttyname(slave_fd)
        environment = os.environ.copy()
        environment.update(
            {
                "OBLIVION_ONE_BINARY": str(fake_compositor),
                "OBLIVION_ONE_PROFILE": "release",
                "OBLIVION_ONE_STATE_DIR": str(self.state_dir),
                "OBLIVION_ONE_LOG_FILE": str(Path(self.temp_dir.name) / "pty-session.log"),
                "OBLIVION_ONE_SKIP_INPUT_GROUP_CHECK": "1",
                "OBSERVER_TEST_FACTS": str(facts_path),
                "XDG_SESSION_ID": "isolated-pty-session",
            }
        )
        bootstrap = (
            "import fcntl, os, sys, termios\n"
            "fcntl.ioctl(0, termios.TIOCSCTTY, 0)\n"
            "os.tcsetpgrp(0, os.getpgrp())\n"
            "os.execv(sys.argv[1], [sys.argv[1]])\n"
        )
        launcher = REPO_ROOT / "bin" / "start-oblivion-one-tty"
        process = subprocess.Popen(
            [sys.executable, "-c", bootstrap, str(launcher)],
            env=environment,
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            start_new_session=True,
            close_fds=True,
        )
        os.close(slave_fd)
        try:
            deadline = time.monotonic() + 10
            while process.poll() is None and time.monotonic() < deadline:
                readable, _, _ = select.select([master_fd], [], [], 0.05)
                if readable:
                    try:
                        os.read(master_fd, 4096)
                    except OSError:
                        break
            self.assertIsNotNone(process.poll(), "PTY launcher did not exit")
            self.assertEqual(process.wait(timeout=2), 0)
        finally:
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=2)
            os.close(master_fd)

        facts = json.loads(facts_path.read_text())
        record = self.read_record()
        self.assertEqual(facts["session_id"], process.pid)
        self.assertEqual(record["observer_pid"], process.pid)
        self.assertEqual(facts["process_group"], process.pid)
        self.assertEqual(facts["foreground_process_group"], process.pid)
        self.assertEqual(facts["tty_name"], tty_name)
        self.assertTrue(facts["stdin_is_tty"])
        self.assertEqual(record["session_identity"]["desktop_session_id"], "isolated-pty-session")

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
