"""Run paired, offline Python and Rust workloads from the two checkouts."""

import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
from statistics import median
import subprocess
import sys

from workloads import cases, checked, fixture_server

ROOT = Path(__file__).resolve().parents[1]
PYTHON_WORKER = ROOT / "benchmarks/python_worker.py"


def command(*args, **kwargs):
    return subprocess.run(
        args, text=True, check=True, stdout=subprocess.PIPE, **kwargs
    ).stdout.strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_hash():
    """Identify the compiled inputs even when they are uncommitted."""
    files = [ROOT / "Cargo.toml", ROOT / "instructor/examples/benchmark.rs"]
    files.extend((ROOT / "benchmarks").glob("*.py"))
    for package in ("instructor", "instruct-macros", "instruct-macros-types"):
        files.append(ROOT / package / "Cargo.toml")
        files.extend((ROOT / package / "src").rglob("*.rs"))
    hashes = [f"{p.relative_to(ROOT)} {digest(p)}" for p in sorted(files)]
    return hashlib.sha256("\n".join(hashes).encode()).hexdigest()


def cpu():
    path = Path("/proc/cpuinfo")
    if path.exists():
        for line in path.read_text().splitlines():
            if line.startswith("model name"):
                return line.partition(":")[2].strip()
    return platform.processor()


def throttle_us():
    path = Path("/sys/fs/cgroup/cpu.stat")
    if path.exists():
        for line in path.read_text().splitlines():
            if line.startswith("throttled_usec "):
                return int(line.split()[1])
    return None


def metadata(reference):
    quota = Path("/sys/fs/cgroup/cpu.max")
    distributions = [
        "instructor",
        "pydantic",
        "pydantic-core",
        "jiter",
        "httpx",
        "openai",
        "anthropic",
        "google-genai",
        "cohere",
        "mistralai",
        "jinja2",
    ]
    return {
        "run_started_utc": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(),
        "cpu": cpu(),
        "logical_cpus": os.cpu_count(),
        "cgroup_cpu_max": quota.read_text().strip() if quota.exists() else None,
        "python": sys.version,
        "rustc": command("rustc", "--version"),
        "cargo": command("cargo", "--version"),
        "rustflags": os.environ.get("RUSTFLAGS", ""),
        "python_revision": command("git", "rev-parse", "HEAD", cwd=reference),
        "python_tracked_changes": bool(
            command("git", "status", "--porcelain", "-uno", cwd=reference)
        ),
        "rust_revision": command("git", "rev-parse", "HEAD", cwd=ROOT),
        "rust_and_benchmark_source_sha256": source_hash(),
        "cargo_lock_sha256": digest(ROOT / "Cargo.lock"),
        "uv_lock_sha256": digest(reference / "uv.lock"),
        "distributions": {d: importlib.metadata.version(d) for d in distributions},
    }


def build():
    lock_arg = ("--locked",) if (ROOT / "Cargo.lock").exists() else ()
    output = command(
        "cargo",
        "build",
        "--release",
        "--package",
        "instructor-ai",
        "--example",
        "benchmark",
        "--message-format=json-render-diagnostics",
        *lock_arg,
        cwd=ROOT,
    )
    for line in output.splitlines():
        event = json.loads(line)
        if event.get("executable") and event["target"]["name"] == "benchmark":
            return event["executable"]
    raise RuntimeError("Missing benchmark executable in Cargo output")


def as_row(name, trials):
    """Render times per operation in microseconds and the ratio of the medians."""

    def us(language):
        return [t["elapsed_ns"] / t["iterations"] / 1000 for t in trials[language]]

    py, rs = us("python"), us("rust")
    ratios = [p / r for p, r in zip(py, rs)]
    return (
        f"| {name} | {median(py):.3f} | {median(rs):.3f} | {median(py) / median(rs):.2f}× "
        f"| {min(ratios):.2f}–{max(ratios):.2f} |"
    )


def difference(a, b, path="result"):
    """Locate the first mismatch, including paths into nested stream snapshots."""
    if isinstance(a, list) and isinstance(b, list) and len(a) == len(b):
        for i, (left, right) in enumerate(zip(a, b)):
            if left != right:
                return difference(left, right, f"{path}[{i}]")
    if isinstance(a, dict) and isinstance(b, dict) and a.keys() == b.keys():
        for k in a:
            if a[k] != b[k]:
                return difference(a[k], b[k], f"{path}.{k}")
    return f"{path}: {repr(a)[:200]} vs {repr(b)[:200]}"


def expected_calls(report):
    counts = Counter()
    for name, result in report.items():
        if name == "cache/hit":
            counts["total"] += 1
        if name.startswith("http"):
            # One proof, doubling calibration batches totaling 2n-1, then n timed.
            invocations = 3 * result["iterations"]
            counts["total"] += invocations
            if name == "http/retry":
                counts["total"] += invocations
                counts["invalid"] += invocations
            if name.startswith("http-stream/"):
                counts["stream"] += invocations
    return counts


def collect(workers, request, rounds, counts):
    specimens = request["cases"]
    env = {
        **os.environ,
        "PYTHONPATH": request["python_repo"],
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONHASHSEED": "0",
    }
    samples = {c["name"]: {"python": [], "rust": []} for c in specimens}
    witnesses = {}
    for i in range(rounds):
        order = ["python", "rust"] if i % 2 == 0 else ["rust", "python"]
        for language in order:
            print(f"Pair {i + 1}/{rounds}, {language}", file=sys.stderr)
            before = counts.copy()
            raw = command(*workers[language], input=json.dumps(request), env=env)
            report = json.loads(raw)
            calls = counts - before
            if calls != expected_calls(report):
                raise ValueError(
                    f"Unexpected {language} fixture calls {calls}, expected {expected_calls(report)}"
                )
            if report.keys() != samples.keys():
                raise ValueError(f"{language} worker returned different workloads")
            for case in specimens:
                name = case["name"]
                result = report[name]
                try:
                    outcome = checked(case, result.pop("witness"))
                except AssertionError as exc:
                    raise ValueError(f"Unexpected {language} {name} result") from exc
                if name in witnesses and outcome != witnesses[name]:
                    raise ValueError(
                        f"{language} {name} {difference(witnesses[name], outcome)}"
                    )
                witnesses[name] = outcome
                samples[name][language].append(result)
    return samples


def positive(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python-repo", type=Path, required=True)
    parser.add_argument("--rounds", type=positive, default=6)
    parser.add_argument(
        "--target-ms",
        type=positive,
        default=10,
        help="minimum calibration batch duration",
    )
    parser.add_argument(
        "--output", type=Path, help="write raw samples and environment to JSON"
    )
    parser.add_argument(
        "--smoke", action="store_true", help="verify outputs with a single iteration"
    )
    args = parser.parse_args()
    if not __debug__:
        parser.error("Use normal Python so result assertions are active")
    reference = args.python_repo.resolve()
    if not (reference / "instructor/decisions").is_dir():
        parser.error(
            "--python-repo must point to a Python Instructor checkout with Decisions"
        )
    rounds = 1 if args.smoke else args.rounds
    target_ms = 0 if args.smoke else args.target_ms
    initial = source_hash()
    workers = {"rust": [build()], "python": [sys.executable, PYTHON_WORKER]}
    environment = metadata(reference)
    if initial != environment["rust_and_benchmark_source_sha256"]:
        raise RuntimeError(
            "Sources changed during compilation. Re-run to identify the build."
        )
    with fixture_server() as (endpoint, counts):
        request = {
            "cases": list(cases()),
            "target_ns": target_ms * 1_000_000,
            "python_repo": str(reference),
            "endpoint": endpoint,
        }
        before = throttle_us()
        samples = collect(workers, request, rounds, counts)
        after = throttle_us()
        if before is not None and after is not None:
            environment["cgroup_throttled_us_during_rounds"] = after - before
    if args.output:
        report = {
            "environment": environment,
            "rounds": rounds,
            "target_ms": target_ms,
            "samples": samples,
        }
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    if args.smoke:
        print(f"{len(samples)} paired workloads verified")
    else:
        print(
            "| Workload | Python µs/op | Rust µs/op | Python / Rust | Round ratio range |"
        )
        print("| --- | ---: | ---: | ---: | ---: |")
        for name, trials in samples.items():
            print(as_row(name, trials))


if __name__ == "__main__":
    main()
