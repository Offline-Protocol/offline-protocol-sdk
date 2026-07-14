#!/usr/bin/env python3
"""Turn Criterion's machine-readable output into a CI-friendly dashboard."""

from __future__ import annotations

import argparse
import fnmatch
import json
import math
import os
import platform
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


@dataclass
class Budget:
    name: str
    max_time_ns: float


@dataclass
class Benchmark:
    full_id: str
    group_id: str
    parameter: str | None
    estimate_ns: float
    lower_ns: float
    upper_ns: float
    ops_per_second: float
    throughput: str | None
    noise_cv_percent: float
    outlier_percent: float
    change_percent: float | None
    change_lower_percent: float | None
    change_upper_percent: float | None
    change_status: str
    budget: Budget | None
    budget_passed: bool | None


def read_json(path: Path) -> Any:
    with path.open(encoding="utf-8") as handle:
        return json.load(handle)


def estimate_block(estimates: dict[str, Any]) -> dict[str, Any]:
    # Median is robust to noisy shared CI runners and has its own bootstrapped CI.
    return estimates.get("median") or estimates.get("slope") or estimates["mean"]


def format_duration(nanoseconds: float) -> str:
    units = ((1.0, "ns"), (1_000.0, "µs"), (1_000_000.0, "ms"), (1_000_000_000.0, "s"))
    divisor, unit = units[0]
    for candidate_divisor, candidate_unit in units:
        if nanoseconds < candidate_divisor * 1_000 or candidate_unit == "s":
            divisor, unit = candidate_divisor, candidate_unit
            break
    value = nanoseconds / divisor
    precision = 2 if value < 10 else 1 if value < 100 else 0
    return f"{value:.{precision}f} {unit}"


def format_rate(value: float, suffix: str = "ops/s") -> str:
    for divisor, prefix in ((1e12, "T"), (1e9, "G"), (1e6, "M"), (1e3, "k")):
        if value >= divisor:
            return f"{value / divisor:.2f} {prefix}{suffix}"
    return f"{value:.2f} {suffix}"


def throughput_label(throughput: Any, estimate_ns: float) -> str | None:
    if not throughput or estimate_ns <= 0:
        return None
    per_second = 1_000_000_000.0 / estimate_ns
    if "Bytes" in throughput:
        bytes_per_second = float(throughput["Bytes"]) * per_second
        for divisor, unit in ((1024**3, "GiB/s"), (1024**2, "MiB/s"), (1024, "KiB/s")):
            if bytes_per_second >= divisor:
                return f"{bytes_per_second / divisor:.2f} {unit}"
        return f"{bytes_per_second:.0f} B/s"
    if "Elements" in throughput:
        return format_rate(float(throughput["Elements"]) * per_second, "items/s")
    return None


def outlier_percentage(directory: Path) -> float:
    sample_path = directory / "sample.json"
    tukey_path = directory / "tukey.json"
    if not sample_path.exists() or not tukey_path.exists():
        return 0.0
    sample = read_json(sample_path)
    fences = read_json(tukey_path)
    values = [time / iterations for time, iterations in zip(sample["times"], sample["iters"])]
    if not values or len(fences) < 4:
        return 0.0
    outliers = sum(value < fences[1] or value > fences[2] for value in values)
    return 100.0 * outliers / len(values)


def matching_budget(full_id: str, metadata: dict[str, Any]) -> Budget | None:
    for item in metadata.get("budgets", []):
        if fnmatch.fnmatchcase(full_id, item["pattern"]):
            return Budget(item["name"], float(item["max_time_ns"]))
    return None


def classify_change(
    point_percent: float | None,
    lower_percent: float | None,
    upper_percent: float | None,
    threshold_percent: float,
) -> str:
    if point_percent is None or lower_percent is None or upper_percent is None:
        return "not compared"
    if lower_percent > threshold_percent:
        return "regression"
    if upper_percent < -threshold_percent:
        return "improvement"
    return "stable"


def discover_benchmarks(
    criterion_dir: Path, metadata: dict[str, Any], after: Path | None = None
) -> list[Benchmark]:
    cutoff = after.stat().st_mtime if after else None
    threshold = float(metadata.get("regression_threshold_percent", 5.0))
    benchmarks: list[Benchmark] = []

    for estimates_path in criterion_dir.glob("**/new/estimates.json"):
        if cutoff is not None and estimates_path.stat().st_mtime < cutoff:
            continue
        new_dir = estimates_path.parent
        benchmark_path = new_dir / "benchmark.json"
        if not benchmark_path.exists():
            continue

        descriptor = read_json(benchmark_path)
        estimates = read_json(estimates_path)
        estimate = estimate_block(estimates)
        confidence = estimate["confidence_interval"]
        mean = estimates["mean"]["point_estimate"]
        std_dev = estimates.get("std_dev", {}).get("point_estimate", 0.0)
        estimate_ns = float(estimate["point_estimate"])

        change_path = new_dir.parent / "change" / "estimates.json"
        change_point = change_lower = change_upper = None
        if change_path.exists() and (cutoff is None or change_path.stat().st_mtime >= cutoff):
            change = read_json(change_path)["mean"]
            change_point = 100.0 * float(change["point_estimate"])
            change_lower = 100.0 * float(change["confidence_interval"]["lower_bound"])
            change_upper = 100.0 * float(change["confidence_interval"]["upper_bound"])

        full_id = descriptor["full_id"]
        budget = matching_budget(full_id, metadata)
        benchmarks.append(
            Benchmark(
                full_id=full_id,
                group_id=descriptor["group_id"],
                parameter=descriptor.get("value_str") or descriptor.get("function_id"),
                estimate_ns=estimate_ns,
                lower_ns=float(confidence["lower_bound"]),
                upper_ns=float(confidence["upper_bound"]),
                ops_per_second=1_000_000_000.0 / estimate_ns,
                throughput=throughput_label(descriptor.get("throughput"), estimate_ns),
                noise_cv_percent=100.0 * float(std_dev) / float(mean) if mean else 0.0,
                outlier_percent=outlier_percentage(new_dir),
                change_percent=change_point,
                change_lower_percent=change_lower,
                change_upper_percent=change_upper,
                change_status=classify_change(change_point, change_lower, change_upper, threshold),
                budget=budget,
                budget_passed=confidence["upper_bound"] <= budget.max_time_ns if budget else None,
            )
        )

    return sorted(benchmarks, key=lambda item: natural_key(item.full_id))


def natural_key(value: str) -> tuple[tuple[int, Any], ...]:
    return tuple(
        (0, int(part)) if part.isdigit() else (1, part.lower())
        for part in re.split(r"(\d+)", value)
    )


def relative_bar(value: float, minimum: float, maximum: float) -> str:
    if maximum <= minimum:
        return "█████"
    width = max(1, math.ceil(10 * value / maximum))
    return "█" * width


def markdown_escape(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ")


def environment_rows() -> list[tuple[str, str]]:
    try:
        rustc = subprocess.run(
            ["rustc", "--version"], check=False, capture_output=True, text=True
        ).stdout.strip()
    except OSError:
        rustc = "unavailable"
    cpu = platform.processor()
    cpuinfo = Path("/proc/cpuinfo")
    if not cpu and cpuinfo.exists():
        for line in cpuinfo.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.lower().startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    rows = [
        (
            "Commit",
            os.environ["GITHUB_SHA"][:12]
            if os.environ.get("GITHUB_SHA")
            else "local working tree",
        ),
        ("Platform", f"{platform.system()} {platform.machine()}"),
        ("CPU", cpu or "unknown"),
        ("Toolchain", rustc or "unavailable"),
    ]
    if os.environ.get("BENCHMARK_SAMPLE_SIZE"):
        rows.append(
            (
                "Sampling",
                f"{os.environ['BENCHMARK_SAMPLE_SIZE']} samples, "
                f"{os.environ.get('BENCHMARK_WARM_UP_SECONDS', '?')} s warm-up, "
                f"{os.environ.get('BENCHMARK_MEASUREMENT_SECONDS', '?')} s measurement",
            )
        )
    return rows


def change_label(benchmark: Benchmark) -> str:
    if benchmark.change_percent is None:
        return "—"
    icon = {"regression": "🔴", "improvement": "🟢", "stable": "⚪"}[benchmark.change_status]
    return f"{icon} {benchmark.change_percent:+.1f}%"


def budget_label(benchmark: Benchmark) -> str:
    if not benchmark.budget:
        return "—"
    icon = "✅" if benchmark.budget_passed else "❌"
    return f"{icon} ≤ {format_duration(benchmark.budget.max_time_ns)}"


def render_markdown(benchmarks: list[Benchmark], metadata: dict[str, Any]) -> str:
    if not benchmarks:
        raise ValueError("no fresh Criterion measurements were found")

    groups = {benchmark.group_id for benchmark in benchmarks}
    compared = [benchmark for benchmark in benchmarks if benchmark.change_percent is not None]
    regressions = [benchmark for benchmark in compared if benchmark.change_status == "regression"]
    improvements = [benchmark for benchmark in compared if benchmark.change_status == "improvement"]
    budgeted = [benchmark for benchmark in benchmarks if benchmark.budget]
    failed_budgets = [benchmark for benchmark in budgeted if not benchmark.budget_passed]
    noise_threshold = float(metadata.get("noise_cv_threshold_percent", 10.0))
    noisy = [benchmark for benchmark in benchmarks if benchmark.noise_cv_percent > noise_threshold]

    lines = [
        "# Performance dashboard",
        "",
        "> Lower latency is better. Changes use Criterion's configured baseline;",
        (
            "> CI measures the PR base on the same runner; confidence intervals wholly beyond "
            f"±{metadata.get('regression_threshold_percent', 5):g}% are flagged."
        ),
        "",
        "## At a glance",
        "",
        "| Signal | Result |",
        "|---|---:|",
        f"| Coverage | {len(benchmarks)} benchmarks across {len(groups)} groups |",
    ]
    if compared:
        stable_count = len(compared) - len(regressions) - len(improvements)
        lines.append(
            f"| Base comparison | 🔴 {len(regressions)} regressions · "
            f"🟢 {len(improvements)} improvements · ⚪ {stable_count} stable |"
        )
    else:
        lines.append("| Base comparison | Not available for this run |")
    if budgeted:
        passing = len(budgeted) - len(failed_budgets)
        lines.append(f"| Performance budgets | {passing}/{len(budgeted)} passing |")
    else:
        lines.append("| Performance budgets | No matching budgeted benchmarks |")
    lines.append(f"| Measurement quality | {len(noisy)} noisy (CV > {noise_threshold:g}%) |")

    attention: list[tuple[str, Benchmark, str]] = []
    attention.extend(("Budget exceeded", item, budget_label(item)) for item in failed_budgets)
    attention.extend(("Regression", item, change_label(item)) for item in regressions)
    attention.extend(
        ("High variance", item, f"CV {item.noise_cv_percent:.1f}%")
        for item in noisy
        if item not in failed_budgets and item not in regressions
    )
    lines.extend(["", "## Attention", ""])
    if attention:
        lines.extend(["| Signal | Benchmark | Detail |", "|---|---|---:|"])
        for signal, benchmark, detail in attention:
            lines.append(f"| {signal} | `{markdown_escape(benchmark.full_id)}` | {detail} |")
    else:
        lines.append(
            "No budget violations, material regressions, or high-variance measurements detected."
        )

    lines.extend(["", "## Results", ""])
    group_metadata = metadata.get("groups", {})
    for group_id in sorted(groups):
        members = [item for item in benchmarks if item.group_id == group_id]
        group_info = group_metadata.get(group_id, {})
        configured_order = group_info.get("order", [])
        members.sort(
            key=lambda item: (
                configured_order.index(item.parameter)
                if item.parameter in configured_order
                else len(configured_order),
                natural_key(item.full_id),
            )
        )
        lines.extend([f"### `{markdown_escape(group_id)}`", ""])
        if group_info.get("description"):
            lines.extend([group_info["description"], ""])
        if group_info.get("parameter"):
            lines.extend([f"Parameter: {group_info['parameter']}.", ""])
        lines.extend(
            [
                "| Benchmark | Median (95% CI) | Throughput | Change | "
                "Noise / outliers | Budget | Latency scale¹ |",
                "|---|---:|---:|---:|---:|---:|:---|",
            ]
        )
        minimum = min(item.estimate_ns for item in members)
        maximum = max(item.estimate_ns for item in members)
        for benchmark in members:
            name = benchmark.full_id[len(group_id) :].lstrip("/") or group_id
            confidence = (
                f"{format_duration(benchmark.estimate_ns)} "
                f"({format_duration(benchmark.lower_ns)}–{format_duration(benchmark.upper_ns)})"
            )
            throughput = benchmark.throughput or format_rate(benchmark.ops_per_second)
            quality = f"CV {benchmark.noise_cv_percent:.1f}% / {benchmark.outlier_percent:.0f}%"
            lines.append(
                f"| `{markdown_escape(name)}` | {confidence} | {throughput} | "
                f"{change_label(benchmark)} | {quality} | {budget_label(benchmark)} | "
                f"{relative_bar(benchmark.estimate_ns, minimum, maximum)} |"
            )
        lines.append("")

    lines.extend(
        [
            "¹ Group-local linear scale relative to the slowest result; a longer bar means "
            "higher latency. Compare bars only within a group.",
            "",
            "## Environment",
            "",
            "| Field | Value |",
            "|---|---|",
        ]
    )
    lines.extend(f"| {name} | {markdown_escape(value)} |" for name, value in environment_rows())
    lines.extend(
        [
            "",
            "The downloadable benchmark artifact contains Criterion's interactive HTML charts "
            "and raw sample data.",
            "Hosted-runner measurements are directional; reproduce important findings on "
            "controlled hardware before making release decisions.",
            "",
        ]
    )
    return "\n".join(lines)


def json_document(benchmarks: list[Benchmark], metadata: dict[str, Any]) -> dict[str, Any]:
    noise_threshold = float(metadata.get("noise_cv_threshold_percent", 10.0))
    return {
        "schema_version": 1,
        "environment": dict(environment_rows()),
        "summary": {
            "benchmarks": len(benchmarks),
            "groups": len({item.group_id for item in benchmarks}),
            "regressions": sum(item.change_status == "regression" for item in benchmarks),
            "improvements": sum(item.change_status == "improvement" for item in benchmarks),
            "budget_failures": sum(item.budget_passed is False for item in benchmarks),
            "noisy_measurements": sum(
                item.noise_cv_percent > noise_threshold for item in benchmarks
            ),
        },
        "benchmarks": [
            {**asdict(item), "budget": asdict(item.budget) if item.budget else None}
            for item in benchmarks
        ],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--criterion-dir", type=Path, default=Path("target/criterion"))
    parser.add_argument("--metadata", type=Path, default=Path("benches/benchmark-metadata.json"))
    parser.add_argument(
        "--after", type=Path, help="include only measurements newer than this marker"
    )
    parser.add_argument("--output", type=Path, help="write Markdown here instead of stdout")
    parser.add_argument(
        "--json-output", type=Path, help="also write normalized machine-readable results"
    )
    parser.add_argument("--fail-on-budget", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    metadata = read_json(args.metadata)
    benchmarks = discover_benchmarks(args.criterion_dir, metadata, args.after)
    try:
        markdown = render_markdown(benchmarks, metadata)
    except ValueError as error:
        print(f"benchmark report: {error}", file=sys.stderr)
        return 2

    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(markdown, encoding="utf-8")
    else:
        print(markdown)
    if args.json_output:
        args.json_output.parent.mkdir(parents=True, exist_ok=True)
        args.json_output.write_text(
            json.dumps(json_document(benchmarks, metadata), indent=2) + "\n", encoding="utf-8"
        )

    failures = [item for item in benchmarks if item.budget_passed is False]
    if args.fail_on_budget and failures:
        for failure in failures:
            print(
                f"benchmark budget exceeded: {failure.full_id} upper bound "
                f"{format_duration(failure.upper_ns)} > "
                f"{format_duration(failure.budget.max_time_ns)}",
                file=sys.stderr,
            )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
