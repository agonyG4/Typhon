#!/usr/bin/env python3
"""Sample Linux scheduler and pressure counters for a running Typhon process."""

from __future__ import annotations

import argparse
import json
import os
import platform
import sys
import time
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


OUTPUT_ROOT = Path("/mnt/Aether/Desktop/GitHub")
PROC_ROOT = Path("/proc")
SYS_CPU_ROOT = Path("/sys/devices/system/cpu")
THERMAL_ROOT = Path("/sys/class/thermal")
THREAD_METRICS = ("cpu_percent", "runnable_wait_ms_per_second")


def parse_schedstat(contents: str) -> tuple[int, int, int]:
    """Return cumulative runtime ns, runnable-wait ns, and timeslices."""
    fields = contents.split()
    if len(fields) < 3:
        raise ValueError("schedstat must contain at least three counters")
    try:
        counters = [int(value) for value in fields[:3]]
    except ValueError as error:
        raise ValueError("schedstat counters must be integers") from error
    if any(value < 0 for value in counters):
        raise ValueError("schedstat counters cannot be negative")
    return counters[0], counters[1], counters[2]


def parse_proc_stat(contents: str) -> tuple[str, int]:
    """Read process state and field 22, tolerating ')' in comm."""
    closing_parenthesis = contents.rfind(")")
    if closing_parenthesis < 0:
        raise ValueError("process stat is missing the comm terminator")
    fields = contents[closing_parenthesis + 1 :].split()
    if len(fields) <= 19:
        raise ValueError("process stat is missing the state or start-time field")
    try:
        start_time = int(fields[19])
    except ValueError as error:
        raise ValueError("process start time must be an integer") from error
    if len(fields[0]) != 1:
        raise ValueError("process state must be a single character")
    return fields[0], start_time


def parse_proc_start_time(contents: str) -> int:
    """Read field 22 from /proc/<pid>/stat."""
    return parse_proc_stat(contents)[1]


def parse_psi(contents: str) -> dict[str, dict[str, float | int]]:
    """Parse Linux pressure-stall information without filling missing fields."""
    result: dict[str, dict[str, float | int]] = {}
    for line in contents.splitlines():
        fields = line.split()
        if not fields:
            continue
        category = fields[0]
        if category not in {"some", "full"}:
            continue
        values: dict[str, float | int] = {}
        for field in fields[1:]:
            key, separator, raw_value = field.partition("=")
            if not separator:
                continue
            values[key] = float(raw_value) if key.startswith("avg") else int(raw_value)
        result[category] = values
    return result


def parse_cpu_stat(contents: str) -> dict[str, int]:
    """Parse cgroup v2 cpu.stat counters."""
    result: dict[str, int] = {}
    for line in contents.splitlines():
        fields = line.split()
        if len(fields) == 2:
            result[fields[0]] = int(fields[1])
    return result


def parse_meminfo(contents: str) -> dict[str, int]:
    """Return selected /proc/meminfo values in bytes, omitting absent values."""
    values: dict[str, int] = {}
    for line in contents.splitlines():
        key, separator, remainder = line.partition(":")
        if not separator:
            continue
        fields = remainder.split()
        if not fields:
            continue
        amount = int(fields[0])
        values[key] = amount * 1024 if len(fields) > 1 and fields[1] == "kB" else amount
    result = {}
    for source_key, output_key in (
        ("MemAvailable", "mem_available_bytes"),
        ("SwapTotal", "swap_total_bytes"),
        ("SwapFree", "swap_free_bytes"),
    ):
        if source_key in values:
            result[output_key] = values[source_key]
    if "SwapTotal" in values and "SwapFree" in values:
        result["swap_used_bytes"] = max(0, values["SwapTotal"] - values["SwapFree"])
    return result


def counter_delta(
    current: dict[str, int] | None, previous: dict[str, int] | None
) -> dict[str, int] | None:
    """Return counter deltas, or None when values are absent or reset."""
    if current is None or previous is None:
        return None
    common = current.keys() & previous.keys()
    if not common or any(current[key] < previous[key] for key in common):
        return None
    return {key: current[key] - previous[key] for key in sorted(common)}


def percentile(values: list[float], percentile_value: float) -> float | None:
    """Return a linearly interpolated percentile, or None for no samples."""
    if not 0 <= percentile_value <= 100:
        raise ValueError("percentile must be between 0 and 100")
    if not values:
        return None
    ordered = sorted(values)
    position = (len(ordered) - 1) * percentile_value / 100
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] + (ordered[upper] - ordered[lower]) * fraction


def validate_output_path(path: Path, root: Path = OUTPUT_ROOT) -> Path:
    """Require output beneath the designated external results filesystem."""
    if not path.is_absolute():
        raise ValueError("--output must be an absolute path")
    resolved_root = root.resolve()
    resolved_path = path.resolve()
    try:
        resolved_path.relative_to(resolved_root)
    except ValueError as error:
        raise ValueError(f"--output must be inside {resolved_root}") from error
    if resolved_path == resolved_root:
        raise ValueError("--output must name a file beneath the output root")
    return resolved_path


def read_text(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8").strip()
    except (OSError, UnicodeError):
        return None


def process_identity(pid: int) -> tuple[str, int] | None:
    contents = read_text(PROC_ROOT / str(pid) / "stat")
    if contents is None:
        return None
    try:
        return parse_proc_stat(contents)
    except ValueError:
        return None


def target_is_alive(pid: int, expected_start_time: int) -> bool:
    identity = process_identity(pid)
    return (
        identity is not None
        and identity[1] == expected_start_time
        and identity[0] not in {"Z", "X", "x"}
    )


def pressure_snapshot(path: Path) -> dict[str, dict[str, float | int]] | None:
    contents = read_text(path)
    if contents is None:
        return None
    try:
        return parse_psi(contents)
    except ValueError:
        return None


def cgroup_for_pid(pid: int) -> tuple[str | None, Path | None]:
    contents = read_text(PROC_ROOT / str(pid) / "cgroup")
    if contents is None:
        return None, None
    for line in contents.splitlines():
        hierarchy, separator, relative_path = line.partition("::")
        if hierarchy == "0" and separator:
            relative_path = relative_path.lstrip("/")
            return relative_path, Path("/sys/fs/cgroup") / relative_path
    return None, None


def cgroup_path_chain(path: Path, root: Path) -> list[Path]:
    """Return a cgroup and its visible parents through the cgroup mount root."""
    resolved_root = root.resolve()
    current = path.resolve()
    try:
        current.relative_to(resolved_root)
    except ValueError:
        return []
    chain = []
    while True:
        chain.append(current)
        if current == resolved_root:
            return chain
        if current.parent == current:
            return []
        current = current.parent


def read_cpu_stat(path: Path) -> dict[str, int] | None:
    contents = read_text(path / "cpu.stat")
    if contents is None:
        return None
    try:
        return parse_cpu_stat(contents)
    except ValueError:
        return None


def cgroup_cpu_stat_deltas(
    current: dict[str, Any] | None, previous: dict[str, Any] | None
) -> dict[str, dict[str, int] | None] | None:
    """Return cpu.stat deltas keyed by visible cgroup path."""
    if current is None or previous is None:
        return None
    previous_by_path = {
        entry["path"]: entry.get("cpu_stat")
        for entry in previous.get("cpu_weight_hierarchy", [])
    }
    result = {}
    for entry in current.get("cpu_weight_hierarchy", []):
        path = entry.get("path")
        if isinstance(path, str):
            result[path] = counter_delta(
                entry.get("cpu_stat"), previous_by_path.get(path)
            )
    return result or None


def cgroup_snapshot(path: Path | None) -> dict[str, Any] | None:
    if path is None:
        return None
    cpu_stat = read_cpu_stat(path)
    cgroup_root = Path("/sys/fs/cgroup")
    hierarchy = [
        {
            "path": "/" + str(cgroup.relative_to(cgroup_root))
            if cgroup != cgroup_root
            else "/",
            "cpu_weight": read_text(cgroup / "cpu.weight"),
            "cpu_max": read_text(cgroup / "cpu.max"),
            "cpu_stat": read_cpu_stat(cgroup),
        }
        for cgroup in cgroup_path_chain(path, cgroup_root)
    ]
    return {
        "cpu_max": read_text(path / "cpu.max"),
        "cpu_weight_hierarchy": hierarchy,
        "cpu_stat": cpu_stat,
        "pressure": {
            name: pressure_snapshot(path / f"{name}.pressure")
            for name in ("cpu", "memory", "io")
        },
    }


def system_snapshot() -> dict[str, Any]:
    pressures = {
        name: pressure_snapshot(Path("/proc/pressure") / name)
        for name in ("cpu", "memory", "io")
    }
    frequencies: list[int] = []
    for path in SYS_CPU_ROOT.glob("cpu[0-9]*/cpufreq/scaling_cur_freq"):
        value = read_text(path)
        if value is not None:
            try:
                frequencies.append(int(value))
            except ValueError:
                continue
    thermal_zones: dict[str, dict[str, Any]] = {}
    for zone in sorted(THERMAL_ROOT.glob("thermal_zone*")):
        temperature = read_text(zone / "temp")
        if temperature is None:
            continue
        try:
            temperature_millidegrees = int(temperature)
        except ValueError:
            continue
        thermal_zones[zone.name] = {
            "type": read_text(zone / "type"),
            "temperature_millidegrees_c": temperature_millidegrees,
        }
    mean_frequency = (
        round(sum(frequencies) / len(frequencies)) if frequencies else None
    )
    meminfo_contents = read_text(Path("/proc/meminfo"))
    try:
        memory = parse_meminfo(meminfo_contents) if meminfo_contents is not None else {}
    except ValueError:
        memory = {}
    return {
        "pressure": pressures,
        "memory": memory,
        "cpu_frequency": {"readings": len(frequencies), "mean_khz": mean_frequency},
        "kernel_schedstats_enabled": read_text(
            Path("/proc/sys/kernel/sched_schedstats")
        ),
        "thermal_zones": thermal_zones,
    }


def scheduler_name(policy: int | None) -> str | None:
    if policy is None:
        return None
    known_policies = (
        "SCHED_OTHER",
        "SCHED_BATCH",
        "SCHED_IDLE",
        "SCHED_FIFO",
        "SCHED_RR",
        "SCHED_DEADLINE",
    )
    for name in known_policies:
        if getattr(os, name, None) == policy:
            return name
    return f"UNKNOWN_{policy}"


def thread_identity_key(tid: int, start_time_ticks: int | None) -> str:
    """Keep percentile observations separate when Linux reuses a TID."""
    return f"{tid}@{start_time_ticks}" if start_time_ticks is not None else str(tid)


def thread_snapshot(pid: int) -> dict[int, dict[str, Any]]:
    task_root = PROC_ROOT / str(pid) / "task"
    result: dict[int, dict[str, Any]] = {}
    try:
        task_paths = sorted(task_root.iterdir(), key=lambda path: int(path.name))
    except (FileNotFoundError, PermissionError, ProcessLookupError):
        return result
    for task_path in task_paths:
        try:
            tid = int(task_path.name)
        except ValueError:
            continue
        schedstat_contents = read_text(task_path / "schedstat")
        task_stat_contents = read_text(task_path / "stat")
        counters = None
        if schedstat_contents is not None:
            try:
                counters = parse_schedstat(schedstat_contents)
            except ValueError:
                pass
        start_time_ticks = None
        if task_stat_contents is not None:
            try:
                start_time_ticks = parse_proc_stat(task_stat_contents)[1]
            except ValueError:
                pass
        try:
            nice = os.getpriority(os.PRIO_PROCESS, tid)
        except (AttributeError, OSError, ProcessLookupError):
            nice = None
        try:
            policy = os.sched_getscheduler(tid)
        except (AttributeError, OSError, ProcessLookupError):
            policy = None
        try:
            affinity = sorted(os.sched_getaffinity(tid))
        except (AttributeError, OSError, ProcessLookupError):
            affinity = None
        result[tid] = {
            "name": read_text(task_path / "comm"),
            "start_time_ticks": start_time_ticks,
            "runtime_ns": counters[0] if counters else None,
            "runnable_wait_ns": counters[1] if counters else None,
            "timeslices": counters[2] if counters else None,
            "nice": nice,
            "scheduler_policy": scheduler_name(policy),
            "scheduler_policy_id": policy,
            "affinity_cpus": affinity,
        }
    return result


def sampled_threads(
    current: dict[int, dict[str, Any]],
    previous: dict[int, dict[str, Any]],
    elapsed_ns: int,
) -> list[dict[str, Any]]:
    result = []
    for tid, thread in sorted(current.items()):
        row = {"tid": tid, **thread}
        prior = previous.get(tid)
        if prior is not None and (
            thread.get("start_time_ticks") is None
            or prior.get("start_time_ticks") != thread.get("start_time_ticks")
        ):
            prior = None
        for counter, delta_name in (
            ("runtime_ns", "runtime_delta_ns"),
            ("runnable_wait_ns", "runnable_wait_delta_ns"),
            ("timeslices", "timeslices_delta"),
        ):
            current_value = thread[counter]
            prior_value = prior[counter] if prior is not None else None
            delta = None
            if (
                current_value is not None
                and prior_value is not None
                and current_value >= prior_value
            ):
                delta = current_value - prior_value
            row[delta_name] = delta
        runtime_delta = row["runtime_delta_ns"]
        wait_delta = row["runnable_wait_delta_ns"]
        row["cpu_percent"] = (
            runtime_delta / elapsed_ns * 100
            if runtime_delta is not None and elapsed_ns > 0
            else None
        )
        row["runnable_wait_ms_per_second"] = (
            wait_delta / elapsed_ns * 1000
            if wait_delta is not None and elapsed_ns > 0
            else None
        )
        result.append(row)
    return result


def percentile_summary(
    observations: dict[str | int, dict[str, list[float]]],
) -> dict[str, dict[str, float | int | None]]:
    summary: dict[str, dict[str, float | int | None]] = {}
    for thread_identity, metrics in sorted(
        observations.items(), key=lambda item: str(item[0])
    ):
        summary[str(thread_identity)] = {}
        for metric, values in metrics.items():
            for quantile in (50, 95, 99):
                summary[str(thread_identity)][f"{metric}_p{quantile}"] = percentile(
                    values, quantile
                )
            summary[str(thread_identity)][f"{metric}_sample_count"] = len(values)
    return summary


def json_line(stream: Any, value: dict[str, Any]) -> None:
    stream.write(json.dumps(value, sort_keys=True, allow_nan=False) + "\n")
    stream.flush()


def make_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Sample per-thread Linux scheduler counters and system pressure for "
            "a running Typhon process."
        )
    )
    parser.add_argument("--pid", type=int, required=True, help="native Typhon PID")
    parser.add_argument("--duration", type=float, default=120, help="sample duration in seconds")
    parser.add_argument("--interval", type=float, default=1, help="sample interval in seconds")
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="new JSONL file under /mnt/Aether/Desktop/GitHub",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = make_parser()
    args = parser.parse_args(argv)
    if not sys.platform.startswith("linux") or not PROC_ROOT.is_dir():
        parser.error("this probe requires Linux procfs")
    if args.pid <= 0:
        parser.error("--pid must be a positive process ID")
    if not 0.5 <= args.interval <= 60:
        parser.error("--interval must be between 0.5 and 60 seconds")
    if not args.interval <= args.duration <= 1_800:
        parser.error("--duration must be at least --interval and at most 1800 seconds")
    aether_mount = Path("/mnt/Aether")
    try:
        output_root_on_aether = (
            OUTPUT_ROOT.resolve().is_relative_to(aether_mount.resolve())
            and os.stat(OUTPUT_ROOT).st_dev == os.stat(aether_mount).st_dev
        )
    except OSError:
        output_root_on_aether = False
    if (
        not aether_mount.is_mount()
        or not OUTPUT_ROOT.is_dir()
        or not output_root_on_aether
    ):
        parser.error("/mnt/Aether/Desktop/GitHub must be present on the Aether mount")
    try:
        output_path = validate_output_path(args.output)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    process_root = PROC_ROOT / str(args.pid)
    if not process_root.is_dir():
        parser.error(f"PID {args.pid} does not exist in procfs")
    target_identity = process_identity(args.pid)
    if target_identity is None:
        parser.error(f"cannot read process identity for PID {args.pid}")
    target_start_time = target_identity[1]
    relative_cgroup, cgroup_path = cgroup_for_pid(args.pid)
    try:
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output = output_path.open("x", encoding="utf-8", buffering=1)
    except OSError as error:
        parser.error(f"cannot create output file {output_path}: {error}")

    metadata = {
        "type": "metadata",
        "schema_version": 1,
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "pid": args.pid,
        "process_name": read_text(process_root / "comm"),
        "cgroup_path": relative_cgroup,
        "cpu_count": os.cpu_count(),
        "platform": platform.platform(),
        "duration_seconds": args.duration,
        "interval_seconds": args.interval,
        "output_path": str(output_path),
    }
    json_line(output, metadata)
    metric_observations: dict[str, dict[str, list[float]]] = defaultdict(
        lambda: {metric: [] for metric in THREAD_METRICS}
    )
    sample_count = 0
    previous_threads = thread_snapshot(args.pid)
    previous_cgroup = cgroup_snapshot(cgroup_path)
    previous_time_ns = time.monotonic_ns()
    deadline_ns = previous_time_ns + int(args.duration * 1_000_000_000)
    interrupted = False
    try:
        while time.monotonic_ns() < deadline_ns:
            if not target_is_alive(args.pid, target_start_time):
                break
            remaining_ns = deadline_ns - time.monotonic_ns()
            if remaining_ns <= 0:
                break
            time.sleep(min(args.interval, remaining_ns / 1_000_000_000))
            if not target_is_alive(args.pid, target_start_time):
                break
            current_time_ns = time.monotonic_ns()
            current_threads = thread_snapshot(args.pid)
            current_cgroup = cgroup_snapshot(cgroup_path)
            elapsed_ns = current_time_ns - previous_time_ns
            threads = sampled_threads(current_threads, previous_threads, elapsed_ns)
            sample = {
                "type": "sample",
                "sequence": sample_count + 1,
                "monotonic_ns": current_time_ns,
                "elapsed_ns": elapsed_ns,
                "system": system_snapshot(),
                "cgroup": {
                    **(current_cgroup or {}),
                    "cpu_stat_delta": counter_delta(
                        (current_cgroup or {}).get("cpu_stat"),
                        (previous_cgroup or {}).get("cpu_stat"),
                    ),
                    "cpu_stat_delta_by_path": cgroup_cpu_stat_deltas(
                        current_cgroup, previous_cgroup
                    ),
                },
                "threads": threads,
            }
            json_line(output, sample)
            sample_count += 1
            for thread in threads:
                for metric in THREAD_METRICS:
                    value = thread.get(metric)
                    if isinstance(value, (int, float)):
                        thread_key = thread_identity_key(
                            thread["tid"], thread.get("start_time_ticks")
                        )
                        metric_observations[thread_key][metric].append(float(value))
            previous_threads = current_threads
            previous_cgroup = current_cgroup
            previous_time_ns = current_time_ns
    except KeyboardInterrupt:
        interrupted = True
    finally:
        final_identity = process_identity(args.pid)
        identity_present = (
            final_identity is not None and final_identity[1] == target_start_time
        )
        summary = {
            "type": "summary",
            "interrupted": interrupted,
            "target_identity_present_at_end": identity_present,
            "target_alive_at_end": (
                identity_present and final_identity[0] not in {"Z", "X", "x"}
            ),
            "sample_count": sample_count,
            "thread_percentiles": percentile_summary(metric_observations),
        }
        json_line(output, summary)
        output.close()
    print(json.dumps(summary, sort_keys=True, allow_nan=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
