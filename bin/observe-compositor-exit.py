#!/usr/bin/env python3
"""Wait for the native compositor and save an external process-exit record."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import uuid


MAX_RECORD_BYTES = 16 * 1024
FORWARDED_SIGNALS = tuple(
    value
    for value in (signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTERM)
    if value is not None
)


def timestamp() -> tuple[int, str]:
    epoch_ms = time.time_ns() // 1_000_000
    iso = dt.datetime.fromtimestamp(epoch_ms / 1000, tz=dt.timezone.utc)
    return epoch_ms, iso.isoformat(timespec="milliseconds").replace("+00:00", "Z")


def read_boot_id() -> str | None:
    try:
        value = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        return value or None
    except OSError:
        return None


def proc_start_ticks(pid: int) -> int | None:
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        close_paren = stat.rfind(")")
        fields_after_comm = stat[close_paren + 1 :].split()
        # The first field after comm is field 3; starttime is field 22.
        return int(fields_after_comm[19])
    except (OSError, ValueError, IndexError):
        return None


def atomic_json_write(path: Path, value: dict) -> None:
    encoded = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(encoded) > MAX_RECORD_BYTES:
        raise ValueError("process-exit record exceeded its fixed size limit")
    diagnostics_dir = path.parent.parent
    diagnostics_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(diagnostics_dir, 0o700)
    path.parent.mkdir(mode=0o700, exist_ok=True)
    os.chmod(path.parent, 0o700)
    fd, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(encoded)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def classify_wait_result(returncode: int | None, *, compositor_was_direct_child: bool) -> dict:
    """Classify subprocess wait status without confusing exit 143 with SIGTERM."""
    if returncode is None:
        return {
            "classification": "unknown",
            "exit_code": None,
            "terminating_signal": None,
            "supervisor_exit_code": None,
            "launcher_exit_code": 125,
        }

    if not compositor_was_direct_child:
        return {
            "classification": "unknown",
            "exit_code": None,
            "terminating_signal": None,
            "supervisor_exit_code": returncode,
            "launcher_exit_code": returncode if returncode >= 0 else 128 + -returncode,
        }

    if returncode < 0:
        number = -returncode
        try:
            name = signal.Signals(number).name
        except ValueError:
            name = f"SIG{number}"
        return {
            "classification": "signal_termination",
            "exit_code": None,
            "terminating_signal": {"number": number, "name": name},
            "supervisor_exit_code": None,
            "launcher_exit_code": 128 + number,
        }

    if returncode == 0:
        classification = "successful_shutdown"
    else:
        classification = "nonzero_exit"
    return {
        "classification": classification,
        "exit_code": returncode,
        "terminating_signal": None,
        "supervisor_exit_code": None,
        "launcher_exit_code": returncode,
    }


def _read_json(path: Path) -> dict | None:
    try:
        if path.stat().st_size > MAX_RECORD_BYTES:
            return None
        value = json.loads(path.read_text())
        return value if isinstance(value, dict) else None
    except (OSError, json.JSONDecodeError):
        return None


def reconcile_interrupted_records(directory: Path, current_boot_id: str | None) -> None:
    """Mark dead observers' start records interrupted on the next launch."""
    try:
        paths = tuple(directory.glob("*.json"))
    except OSError as error:
        print(f"oblivion-one: could not inspect process-exit records: {error}", file=sys.stderr)
        return

    for path in paths:
        record = _read_json(path)
        if not record or record.get("observation_status") not in {"starting", "observing"}:
            continue
        previous_boot_id = record.get("boot_id")
        if previous_boot_id and current_boot_id and previous_boot_id != current_boot_id:
            reason = "host_rebooted_before_observation_completed"
        else:
            observer_pid = record.get("observer_pid")
            previous_start_ticks = record.get("observer_start_ticks")
            if not isinstance(observer_pid, int) or observer_pid <= 0:
                continue
            current_start_ticks = proc_start_ticks(observer_pid)
            if current_start_ticks is None:
                if Path(f"/proc/{observer_pid}").exists():
                    continue
                reason = "observer_process_no_longer_exists"
            elif previous_start_ticks is None or current_start_ticks == previous_start_ticks:
                continue
            else:
                reason = "observer_pid_was_reused_before_reconciliation"

        ended_at_ms, ended_at = timestamp()
        record.update(
            {
                "observation_status": "interrupted",
                "classification": "unknown",
                "exit_code": None,
                "terminating_signal": None,
                "ended_at_unix_ms": None,
                "ended_at_utc": None,
                "reconciled_at_unix_ms": ended_at_ms,
                "reconciled_at_utc": ended_at,
                "interruption_reason": reason,
            }
        )
        try:
            atomic_json_write(path, record)
        except (OSError, ValueError) as error:
            print(f"oblivion-one: could not mark interrupted process-exit record: {error}", file=sys.stderr)


def find_compositor_descendant(root_pid: int) -> int | None:
    """Find the compositor below cargo run without claiming cargo's status."""
    parents: dict[int, int] = {}
    command_lines: dict[int, list[str]] = {}
    try:
        entries = tuple(Path("/proc").iterdir())
    except OSError:
        return None
    for entry in entries:
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        try:
            stat = (entry / "stat").read_text()
            tail = stat[stat.rfind(")") + 1 :].split()
            parents[pid] = int(tail[1])
            command_lines[pid] = (entry / "cmdline").read_bytes().decode(errors="replace").split("\0")
        except (OSError, ValueError, IndexError):
            continue

    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for pid, parent in parents.items():
            if parent in descendants and pid not in descendants:
                descendants.add(pid)
                changed = True
    for pid in descendants - {root_pid}:
        argv = command_lines.get(pid, [])
        if argv and Path(argv[0]).name in {"oblivion-one", "oblivion-one.exe"} and "compositor" in argv[1:]:
            return pid
    return None


def correlate_internal_record(
    diagnostics_dir: Path,
    *,
    compositor_pid: int | None,
    boot_id: str | None,
    desktop_session_id: str | None,
    observation_started_at_ms: int,
    observation_ended_at_ms: int,
) -> dict:
    if compositor_pid is None:
        return {"matched": False, "reason": "compositor_pid_unavailable"}

    candidates = [diagnostics_dir / "last-termination.json"]
    candidates.extend((diagnostics_dir / "incomplete-runs").glob("*.json"))
    matches: list[tuple[int, dict]] = []
    for path in candidates:
        record = _read_json(path)
        metadata = record.get("metadata") if record else None
        if not isinstance(metadata, dict) or metadata.get("process_id") != compositor_pid:
            continue
        record_boot_id = metadata.get("boot_id")
        if boot_id and record_boot_id and boot_id != record_boot_id:
            continue
        record_session = metadata.get("desktop_session_id")
        if desktop_session_id and record_session and desktop_session_id != record_session:
            continue
        started_at = metadata.get("started_at_unix_ms")
        if not isinstance(started_at, int) or not observation_started_at_ms <= started_at <= observation_ended_at_ms:
            continue
        try:
            relative_path = str(path.relative_to(diagnostics_dir))
        except ValueError:
            relative_path = path.name
        status = record.get("status")
        cause = record.get("cause")
        if not isinstance(cause, dict):
            cause = {}
        matches.append(
            (
                1 if status == "completed" and path.name == "last-termination.json" else 0,
                {
                    "matched": True,
                    "record_path": relative_path,
                    "session_id": metadata.get("session_id"),
                    "status": status,
                    "cause_category": cause.get("category"),
                    "runtime_phase": record.get("runtime_phase"),
                    "cause_observed_at_unix_ms": record.get("cause_observed_at_unix_ms"),
                },
            )
        )
    if not matches:
        return {"matched": False, "reason": "no_internal_record_for_observed_process"}
    return max(matches, key=lambda item: item[0])[1]


def make_initial_record(args: argparse.Namespace, observation_id: str, start_ms: int, start_utc: str) -> dict:
    return {
        "schema_version": 1,
        "observation_id": observation_id,
        "observation_status": "starting",
        "classification": "unknown",
        "observer_pid": os.getpid(),
        "observer_start_ticks": proc_start_ticks(os.getpid()),
        "compositor_pid": None,
        "supervisor_pid": None,
        "boot_id": read_boot_id(),
        "session_identity": {
            "desktop_session_id": os.environ.get("XDG_SESSION_ID") or None,
            "session_type": os.environ.get("XDG_SESSION_TYPE") or None,
            "desktop": os.environ.get("XDG_CURRENT_DESKTOP") or None,
            "sddm_session": os.environ.get("OBLIVION_ONE_SDDM_SESSION") == "1",
            "socket_name": args.socket_name,
            "profile": args.profile,
        },
        "started_at_unix_ms": start_ms,
        "started_at_utc": start_utc,
        "ended_at_unix_ms": None,
        "ended_at_utc": None,
        "exit_code": None,
        "terminating_signal": None,
        "signals_received": [],
        "internal_correlation": {"matched": False, "reason": "observation_incomplete"},
    }


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", required=True)
    parser.add_argument("--socket-name", default="unknown")
    parser.add_argument("--profile", default="unknown")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.command and args.command[0] == "--":
        args.command = args.command[1:]
    if not args.command:
        parser.error("a compositor command is required after --")
    args.state_dir = Path(args.state_dir)
    return args


def run(args: argparse.Namespace) -> int:
    diagnostics_dir = args.state_dir / "diagnostics"
    records_dir = diagnostics_dir / "process-exits"
    boot_id = read_boot_id()
    reconcile_interrupted_records(records_dir, boot_id)

    start_ms, start_utc = timestamp()
    observation_id = str(uuid.uuid4())
    record_path = records_dir / f"{observation_id}.json"
    record = make_initial_record(args, observation_id, start_ms, start_utc)
    try:
        atomic_json_write(record_path, record)
    except (OSError, ValueError) as error:
        print(f"oblivion-one: process-exit diagnostic write failed; continuing launch: {error}", file=sys.stderr)

    received_signals: list[int] = []
    forwarded_signals: set[int] = set()
    child: subprocess.Popen | None = None

    def forward_signal(signum: int, _frame) -> None:
        if signum not in received_signals:
            received_signals.append(signum)
        if child is not None and signum not in forwarded_signals and child.poll() is None:
            forwarded_signals.add(signum)
            try:
                child.send_signal(signum)
            except ProcessLookupError:
                pass

    try:
        for signum in FORWARDED_SIGNALS:
            signal.signal(signum, forward_signal)
    except (OSError, ValueError) as error:
        record["observation_status"] = "observer_unavailable"
        try:
            atomic_json_write(record_path, record)
        except (OSError, ValueError):
            pass
        print(f"oblivion-one: observer signal setup failed; falling back to direct launch: {error}", file=sys.stderr)
        os.execvpe(args.command[0], args.command, os.environ)

    try:
        child = subprocess.Popen(args.command)
    except OSError as error:
        record.update(
            {
                "observation_status": "observer_unavailable",
                "launch_error": str(error)[:1024],
            }
        )
        try:
            atomic_json_write(record_path, record)
        except (OSError, ValueError):
            pass
        print(f"oblivion-one: observer could not start child; falling back to direct launch: {error}", file=sys.stderr)
        os.execvpe(args.command[0], args.command, os.environ)
        return 127

    record["observation_status"] = "observing"
    record["compositor_pid"] = child.pid if args.profile != "debug" else None
    if args.profile == "debug":
        record["supervisor_pid"] = child.pid
    else:
        record["supervisor_pid"] = None
    try:
        atomic_json_write(record_path, record)
    except (OSError, ValueError) as error:
        print(f"oblivion-one: process-exit diagnostic write failed; child is running: {error}", file=sys.stderr)

    for signum in received_signals:
        forward_signal(signum, None)

    compositor_pid = record["compositor_pid"]
    if args.profile == "debug":
        next_scan = 0.0
        while child.poll() is None:
            now = time.monotonic()
            if compositor_pid is None and now >= next_scan:
                compositor_pid = find_compositor_descendant(child.pid) or compositor_pid
                if compositor_pid is not None:
                    record["compositor_pid"] = compositor_pid
                    try:
                        atomic_json_write(record_path, record)
                    except (OSError, ValueError):
                        pass
                next_scan = now + 1.0
            try:
                child.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                pass
    returncode = child.wait()
    ended_ms, ended_utc = timestamp()
    direct_child = args.profile != "debug"
    result = classify_wait_result(returncode, compositor_was_direct_child=direct_child)

    record.update(
        {
            "observation_status": "completed",
            "classification": result["classification"],
            "exit_code": result["exit_code"],
            "terminating_signal": result["terminating_signal"],
            "supervisor_exit_code": result["supervisor_exit_code"],
            "launcher_exit_code": result["launcher_exit_code"],
            "signals_received": received_signals,
            "signal_names_received": [
                signal.Signals(value).name if value in signal.Signals._value2member_map_ else f"SIG{value}"
                for value in received_signals
            ],
            "ended_at_unix_ms": ended_ms,
            "ended_at_utc": ended_utc,
        }
    )
    try:
        record["internal_correlation"] = correlate_internal_record(
            diagnostics_dir,
            compositor_pid=compositor_pid,
            boot_id=record.get("boot_id"),
            desktop_session_id=record["session_identity"]["desktop_session_id"],
            observation_started_at_ms=start_ms,
            observation_ended_at_ms=ended_ms,
        )
    except (OSError, ValueError, TypeError) as error:
        record["internal_correlation"] = {"matched": False, "reason": "correlation_failed"}
        print(f"oblivion-one: internal termination correlation failed: {error}", file=sys.stderr)
    if not direct_child:
        record["observation_status"] = "completed_with_unknown_compositor_status"
    try:
        atomic_json_write(record_path, record)
    except (OSError, ValueError) as error:
        print(f"oblivion-one: process-exit diagnostic write failed after child exit: {error}", file=sys.stderr)

    return result["launcher_exit_code"]


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())
