import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import benchmark_report


class BenchmarkReportTests(unittest.TestCase):
    def write_result(
        self,
        root: Path,
        full_id: str = "telemetry_emit_noop/transport_state",
        estimate_ns: float = 1000.0,
        change: tuple[float, float, float] | None = None,
    ) -> None:
        result = root / full_id / "new"
        result.mkdir(parents=True)
        (result / "benchmark.json").write_text(
            json.dumps(
                {
                    "group_id": full_id.split("/")[0],
                    "function_id": full_id.split("/")[-1],
                    "value_str": None,
                    "full_id": full_id,
                    "throughput": {"Bytes": 1000},
                }
            )
        )
        estimate = {
            "confidence_interval": {
                "confidence_level": 0.95,
                "lower_bound": estimate_ns * 0.95,
                "upper_bound": estimate_ns * 1.05,
            },
            "point_estimate": estimate_ns,
            "standard_error": 1.0,
        }
        estimates = {
            "mean": estimate,
            "median": estimate,
            "std_dev": {"point_estimate": estimate_ns * 0.02},
        }
        (result / "estimates.json").write_text(json.dumps(estimates))
        (result / "sample.json").write_text(
            json.dumps({"times": [950.0, 1000.0, 1200.0], "iters": [1.0, 1.0, 1.0]})
        )
        (result / "tukey.json").write_text(json.dumps([800.0, 900.0, 1100.0, 1300.0]))
        if change:
            change_dir = result.parent / "change"
            change_dir.mkdir()
            point, lower, upper = change
            (change_dir / "estimates.json").write_text(
                json.dumps(
                    {
                        "mean": {
                            "point_estimate": point,
                            "confidence_interval": {
                                "lower_bound": lower,
                                "upper_bound": upper,
                            },
                        }
                    }
                )
            )

    def metadata(self) -> dict:
        return {
            "regression_threshold_percent": 5,
            "noise_cv_threshold_percent": 10,
            "groups": {"telemetry_emit_noop": {"description": "Lifecycle telemetry."}},
            "budgets": [
                {
                    "pattern": "telemetry_emit_noop/*",
                    "name": "lifecycle",
                    "max_time_ns": 5000,
                }
            ],
        }

    def test_discovers_quality_throughput_change_and_budget(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_result(root, change=(0.12, 0.08, 0.16))
            results = benchmark_report.discover_benchmarks(root, self.metadata())

            self.assertEqual(len(results), 1)
            result = results[0]
            self.assertEqual(result.change_status, "regression")
            self.assertTrue(result.budget_passed)
            self.assertEqual(result.outlier_percent, 100 / 3)
            self.assertEqual(result.throughput, "953.67 MiB/s")

    def test_budget_uses_upper_confidence_bound(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_result(root, estimate_ns=4900)
            result = benchmark_report.discover_benchmarks(root, self.metadata())[0]

            self.assertFalse(result.budget_passed)

    def test_markdown_surfaces_summary_and_context(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_result(root)
            results = benchmark_report.discover_benchmarks(root, self.metadata())
            report = benchmark_report.render_markdown(results, self.metadata())

            self.assertIn("# Performance dashboard", report)
            self.assertIn("1/1 passing", report)
            self.assertIn("Lifecycle telemetry.", report)
            self.assertIn("interactive HTML charts", report)

    def test_natural_key_orders_numeric_parameters_by_value(self) -> None:
        values = ["payload/10000", "payload/100", "payload/1000"]

        self.assertEqual(
            sorted(values, key=benchmark_report.natural_key),
            ["payload/100", "payload/1000", "payload/10000"],
        )


if __name__ == "__main__":
    unittest.main()
