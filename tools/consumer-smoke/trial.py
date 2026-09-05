"""Declared run-level analysis for the serial completed-render consumer trial."""
import math
import random
import statistics

TARGET_FPS = 60.0
BENEFIT = 0.08
MEASURE_SECONDS = 6.0
SEED = 21434
ARMS = {"native": ("Disabled", 1.0, 4), "temporal": ("Temporal", 0.5, 1),
        "bilinear": ("Disabled", 0.5, 1)}


def orders():
    # Two forward/reverse pairs; every arm has mean global launch position 6.5.
    return [("native", "temporal", "bilinear"), ("bilinear", "temporal", "native"),
            ("temporal", "bilinear", "native"), ("native", "bilinear", "temporal")]


def number(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError("completion timestamp/rate must be finite")
    return value


def identity(value):
    if type(value) is not int or value < 0:
        raise ValueError("frame, view, epoch and fence identities must be unsigned integers")
    return value


def metrics(report, arm):
    """Recheck raw frame identity and closed epoch, not just a JSON valid flag."""
    try:
        return _metrics(report, arm)
    except (KeyError, TypeError, IndexError) as error:
        raise ValueError("incomplete completion evidence") from error


def _metrics(report, arm):
    mode, scale, msaa = ARMS[arm]
    ledger = report["completion"]
    if (report["measurement"] != "completed" or report["actual_msaa_samples"] != msaa
            or report["measurement_capture_count"] != 0 or ledger["errors"] != []
            or ledger["in_flight"] is not None or ledger["max_render_frames_in_flight"] != 1):
        raise ValueError("completion scope, MSAA, capture boundary, or drain failed")
    epochs = ledger["epochs"]
    if [(identity(e["epoch"]), e["phase"]) for e in epochs] != [(1, "Warmup"), (2, "Measure"), (3, "Drain")]:
        raise ValueError("measurement must have warmup and a later drained epoch")
    boundaries = {}
    previous_end = 0
    for epoch in epochs:
        start, end = number(epoch["drain_started_ms"]), number(epoch["drain_completed_ms"])
        if start < previous_end or end < start:
            raise ValueError("epoch drains must be ordered and nonoverlapping")
        boundaries[epoch["epoch"]] = (start, end)
        previous_end = end
    previous_frame, previous_callback = None, None
    for frame in ledger["frames"]:
        epoch, frame_id = identity(frame["epoch"]), identity(frame["frame_id"])
        admitted, callback = number(frame["admitted_ms"]), number(frame["callback_observed_ms"])
        if (epoch not in boundaries or admitted < boundaries[epoch][1] or callback < admitted
                or (epoch < 3 and callback > boundaries[epoch + 1][0])
                or (previous_frame is not None and frame_id <= previous_frame)
                or (previous_callback is not None and admitted < previous_callback)):
            raise ValueError("frame escaped its drained epoch or serial order")
        previous_frame, previous_callback = frame_id, callback
    summary = epochs[1]
    frames = [frame for frame in ledger["frames"] if frame["epoch"] == 2]
    if (summary["valid"] is not True or len(frames) < 20
            or len(frames) != identity(summary["completed_frame_fences"])
            or len(frames) != identity(summary["qualified_render_frames"])):
        raise ValueError("missing or unqualified measured completions")
    scope = frames[0]["scope"]
    identity(scope["view_id"])
    if (scope["mode"] != mode or scope["scale"] != scale or scope["view_id"] is None
            or not scope["image_target"] or scope["output_size"] != [1600, 900]
            or scope["content_size"] != [round(1600 * scale), round(900 * scale)]):
        raise ValueError("wrong measured view, mode, scale or dimensions")
    last_frame, last_callback, intervals = None, None, []
    first_admission = number(frames[0]["admitted_ms"])
    for frame in frames:
        admitted, callback = number(frame["admitted_ms"]), number(frame["callback_observed_ms"])
        effect = frame["effect"]
        identity(effect["frame_id"])
        identity(effect["scope"]["view_id"])
        if (frame["phase"] != "Measure" or frame["qualified"] is not True
                or frame["failure"] is not None or frame["scope"] != scope
                or effect["scope"] != scope or effect["frame_id"] != frame["frame_id"]
                or effect["ready"] is not True
                or effect["state"] != ("Disabled" if mode == "Disabled" else "OutputWritten")
                or (last_frame is not None and frame["frame_id"] <= last_frame)
                or admitted < 0 or callback < admitted
                or (last_callback is not None and admitted < last_callback)):
            raise ValueError("measured frame/proof identity or timestamp mismatch")
        intervals.append(callback - (last_callback if last_callback is not None else admitted))
        last_frame, last_callback = frame["frame_id"], callback
    elapsed = (last_callback - first_admission) / 1000
    fps = len(frames) / elapsed if elapsed > 0 else 0
    if (elapsed < MEASURE_SECONDS or number(epochs[2]["drain_completed_ms"]) < last_callback
            or not math.isclose(number(summary["elapsed_seconds"]), elapsed, rel_tol=1e-8)
            or not math.isclose(number(summary["completed_render_fps"]), fps, rel_tol=1e-8)):
        raise ValueError("completion duration, summary rate or final drain mismatch")
    ordered = sorted(intervals)
    mean = elapsed * 1000 / len(frames)
    return {"completed_frames": len(frames), "elapsed_seconds": elapsed,
            "completed_render_fps": fps, "mean_interval_ms": mean,
            "p95_interval_ms": ordered[math.ceil(.95 * len(ordered)) - 1],
            "p99_interval_ms": ordered[math.ceil(.99 * len(ordered)) - 1],
            "budget_miss_fraction": sum(value > 1000 / TARGET_FPS for value in intervals) / len(intervals),
            "mean_meets_60_fps": mean <= 1000 / TARGET_FPS}


def paired(ratios):
    """Four equally weighted paired-run ratios; frames are not replicates."""
    if len(ratios) != 4 or any(number(value) <= 0 for value in ratios):
        raise ValueError("exactly four finite positive paired ratios required")
    rng = random.Random(SEED)
    draws = sorted(statistics.mean(rng.choices(ratios, k=4)) for _ in range(10_000))
    lower, upper = draws[249], draws[9749]
    decision = ("clear_improvement" if upper < 1 - BENEFIT else
                "clear_regression" if lower > 1 + BENEFIT else "uncertain")
    return {"paired_interval_ratios": ratios, "mean_ratio": statistics.mean(ratios),
            "ci95_ratio": [lower, upper], "decision": decision,
            "method": "10000 paired-run mean bootstrap draws; seed 21434; four pairs; pointwise 95% interval"}


def summarize(runs):
    if len(runs) != 12 or any(not run["valid"] for run in runs):
        raise ValueError("all twelve independent arm runs must pass")
    indexed = {(run["repetition"], run["arm"]): run["metrics"] for run in runs}
    if len(indexed) != 12:
        raise ValueError("duplicate repetition/arm")
    comparisons = {}
    for numerator, denominator in (("temporal", "native"), ("bilinear", "native"), ("temporal", "bilinear")):
        ratios = [indexed[(rep, numerator)]["mean_interval_ms"] / indexed[(rep, denominator)]["mean_interval_ms"]
                  for rep in range(4)]
        comparisons[numerator + "_vs_" + denominator] = paired(ratios)
    return {"comparisons": comparisons,
            "all_run_means_meet_60_fps": {arm: all(indexed[(rep, arm)]["mean_meets_60_fps"] for rep in range(4))
                                         for arm in ARMS},
            "recommendation_scope": "Timing eligibility only; retain native unless consumer image quality and production pacing support enabling Temporal. Four-pair intervals are exploratory, not universal performance guarantees."}
