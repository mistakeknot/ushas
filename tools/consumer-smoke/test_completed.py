"""CPU regressions for the optional serial completed-render consumer trial."""
import copy
import importlib.util
import math
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("consumer_trial", Path(__file__).with_name("trial.py"))
trial = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(trial)


def report(arm="temporal", count=600, interval=10.0):
    mode, scale, msaa = {"native": ("Disabled", 1.0, 4), "temporal": ("Temporal", .5, 1),
                         "bilinear": ("Disabled", .5, 1)}[arm]
    scope = {"view_id": 7, "image_target": "fixed-image", "mode": mode, "scale": scale,
             "content_size": [round(1600 * scale), round(900 * scale)], "output_size": [1600, 900]}
    frames = [{"epoch": 2, "phase": "Measure", "frame_id": i + 50, "scope": scope,
               "admitted_ms": 100 + i * interval, "callback_observed_ms": 100 + (i + 1) * interval,
               "qualified": True, "failure": None,
               "effect": {"frame_id": i + 50, "scope": scope, "ready": True,
                          "state": "Disabled" if mode == "Disabled" else "OutputWritten"}}
              for i in range(count)]
    elapsed = count * interval / 1000
    return {"actual_msaa_samples": msaa, "measurement": "completed", "measurement_capture_count": 0,
            "completion": {"errors": [], "in_flight": None, "max_render_frames_in_flight": 1,
                "epochs": [{"epoch": 1, "phase": "Warmup", "drain_started_ms": 0, "drain_completed_ms": 1},
                    {"epoch": 2, "phase": "Measure", "valid": True,
                     "drain_started_ms": 90, "drain_completed_ms": 100,
                     "completed_frame_fences": count, "qualified_render_frames": count,
                     "elapsed_seconds": elapsed, "completed_render_fps": count / elapsed},
                    {"epoch": 3, "phase": "Drain", "drain_started_ms": 100 + count * interval,
                     "drain_completed_ms": 101 + count * interval}],
                "frames": frames}}


class CompletedTests(unittest.TestCase):
    def test_twelve_orders_balance_global_position_and_have_four_pairs(self):
        orders = trial.orders()
        self.assertEqual(orders, [("native", "temporal", "bilinear"),
                                  ("bilinear", "temporal", "native"),
                                  ("temporal", "bilinear", "native"),
                                  ("native", "bilinear", "temporal")])
        flattened = [arm for order in orders for arm in order]
        for arm in ("native", "temporal", "bilinear"):
            positions = [i + 1 for i, value in enumerate(flattened) if value == arm]
            self.assertEqual(len(positions), 4)
            self.assertEqual(sum(positions) / 4, 6.5)

    def test_metrics_require_closed_completed_epoch_and_actual_msaa(self):
        metrics = trial.metrics(report(), "temporal")
        self.assertAlmostEqual(metrics["mean_interval_ms"], 10)
        self.assertEqual(metrics["budget_miss_fraction"], 0)
        self.assertTrue(metrics["mean_meets_60_fps"])
        mutations = [lambda r: r.update(actual_msaa_samples=4),
                     lambda r: r.update(measurement_capture_count=1),
                     lambda r: r["completion"].update(in_flight={"epoch": 3}),
                     lambda r: r["completion"]["epochs"].pop(),
                     lambda r: r["completion"]["epochs"][1].update(valid=False),
                     lambda r: r["completion"]["epochs"][1].update(completed_render_fps=999),
                     lambda r: r["completion"]["epochs"][1].update(completed_frame_fences=600.0),
                     lambda r: r["completion"]["epochs"][1].update(drain_started_ms=101),
                     lambda r: r["completion"]["epochs"][1].update(drain_completed_ms=101),
                     lambda r: r["completion"]["epochs"][2].update(drain_started_ms=0),
                     lambda r: r["completion"]["frames"][4].update(qualified=False),
                     lambda r: r["completion"]["frames"][4]["effect"].update(frame_id=99),
                     lambda r: r["completion"]["frames"][4].update(callback_observed_ms=math.nan),
                     lambda r: r["completion"]["frames"][4].update(frame_id=50),
                     lambda r: r["completion"]["frames"][4].update(frame_id=54.0),
                     lambda r: r["completion"]["frames"][4]["scope"].update(view_id=7.0),
                     lambda r: r["completion"]["frames"][4]["scope"].update(content_size=[1600, 900])]
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                changed = copy.deepcopy(report())
                mutate(changed)
                with self.assertRaises(ValueError):
                    trial.metrics(changed, "temporal")

    def test_completed_fixture_uses_normal_app_exit_and_latches_completion(self):
        for name in ("probe.rs", "completion_bridge.rs"):
            source = Path(__file__).with_name(name).read_text()
            self.assertFalse("std::process::exit(" in source, name + " bypasses teardown")
        bridge = Path(__file__).with_name("completion_bridge.rs").read_text()
        self.assertIn("Some(probe.finish(true))", bridge)
        patcher = Path(__file__).with_name("run.py").read_text()
        self.assertIn("probe.is_finished()", patcher)
        self.assertIn("MessageWriter<AppExit>", patcher)
        self.assertIn('"fn main() -> AppExit {"', patcher)
        self.assertIn('"    app.run()\\n}"', patcher)

    def test_frame_gaps_are_included_in_cadence_and_budget_misses(self):
        value = report(count=400, interval=20)
        # Every later frame takes only2ms after admission; the18ms CPU gap must
        # remain in completed cadence instead of disappearing from the budget.
        for frame in value["completion"]["frames"][1:]:
            frame["admitted_ms"] = frame["callback_observed_ms"] - 2
        metrics = trial.metrics(value, "temporal")
        self.assertEqual(metrics["mean_interval_ms"], 20)
        self.assertEqual(metrics["budget_miss_fraction"], 1)
        self.assertFalse(metrics["mean_meets_60_fps"])
        self.assertEqual(metrics["p95_interval_ms"], 20)

    def test_confidence_uses_four_paired_runs_and_practical_threshold(self):
        improved = trial.paired([.85, .86, .87, .88])
        self.assertEqual(improved["decision"], "clear_improvement")
        self.assertLess(improved["ci95_ratio"][1], .92)
        self.assertEqual(improved, trial.paired([.85, .86, .87, .88]))
        self.assertEqual(trial.paired([.95] * 4)["decision"], "uncertain")
        self.assertEqual(trial.paired([1.09] * 4)["decision"], "clear_regression")
        for ratios in ([.5] * 3, [.5] * 5, [.5, .5, .5, math.nan], [0] * 4):
            with self.assertRaises(ValueError):
                trial.paired(ratios)

    def test_image_mode_and_completed_mode_require_their_own_exact_captures(self):
        import run
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory).resolve()
            value = report()
            value.update(valid=True, mode="Temporal", scale=.5, distinct_ready=20,
                         warmup_seconds=3.0, captures=[], configuration_valid=True)
            for name, epoch in (("warmup", 1), ("final", 3)):
                path = output / f"temporal_{name}.png"
                path.write_bytes(b"retained image; pixel validation is in Rust")
                value["captures"].append({"path": str(path), "valid": True, "request_completion_epoch": epoch})
            self.assertTrue(run.valid_report(value, output, "temporal", "completed"))
            self.assertFalse(run.valid_report(value, output, "temporal", "images"))
            value["actual_msaa_samples"] = 4
            self.assertFalse(run.valid_report(value, output, "temporal", "completed"))


if __name__ == "__main__":
    unittest.main()
