#!/usr/bin/env python3
"""Audit exported Metal System Trace marker intervals. Never validates a governor input."""

import argparse
import collections
import json
from pathlib import Path
import re
import statistics
import sys
import unittest
import xml.etree.ElementTree as ET


def merged(intervals):
    result = []
    for start, end in sorted(intervals):
        if end <= start:
            continue
        if result and start <= result[-1][1]:
            result[-1] = (result[-1][0], max(end, result[-1][1]))
        else:
            result.append((start, end))
    return result


def clipped_duration(intervals, start, end):
    return sum(max(0, min(b, end) - max(a, start)) for a, b in intervals)


def intersect(left, right):
    result, i, j = [], 0, 0
    while i < len(left) and j < len(right):
        a, b = left[i]
        c, d = right[j]
        if max(a, c) < min(b, d):
            result.append((max(a, c), min(b, d)))
        if b < d:
            i += 1
        else:
            j += 1
    return merged(result)


def read_table(path, schema):
    root = ET.parse(path).getroot()
    ids = {element.get("id"): element for element in root.iter() if element.get("id")}

    def value(element):
        while element.get("ref"):
            element = ids[element.get("ref")]
        if len(element):
            return element.get("fmt", "")
        return element.text or element.get("fmt", "")

    for node in root.findall("node"):
        definition = node.find("schema")
        if definition is not None and definition.get("name") == schema:
            names = [col.findtext("mnemonic") for col in definition.findall("col")]
            return [dict(zip(names, map(value, row), strict=True)) for row in node.findall("row")]
    raise ValueError(f"{path}: schema {schema!r} not found; export this table separately")


def interval(row):
    start = int(row["start"])
    return start, start + int(row["duration"])


def stats(values):
    values = list(values)
    return {"mean": statistics.mean(values), "median": statistics.median(values),
            "min": min(values), "max": max(values)} if values else None


MARKER = re.compile(r"ushas_view_timing_(begin|end) frame=(\d+) view=(\d+) generation=(\d+)")
FAMILIES = (
    "bin unpacking", "early_mesh_preprocessing", "early_prepass_indirect_parameters_building",
    "early prepass", "main_opaque_pass_3d", "metalfx_motion_resolve", "metalfx_depth_resolve",
    "MetalFX_Temporal_PreProcessing", "MetalFX_Temporal_MidProcessing",
    "MetalFX_Temporal_PostProcessing", "metalfx_reconstruct_main", "ui", "upscaling",
)


def analyze(args):
    smoke = json.loads(args.smoke.read_text())
    cpu = read_table(args.encoders, "metal-application-encoders-list")
    gpu = read_table(args.gpu, "metal-gpu-intervals")
    state = read_table(args.states, "metal-gpu-state-intervals")
    process_pattern = re.compile(rf"\({args.pid}\)$")
    cpu = [row for row in cpu if process_pattern.search(row["process"])]
    target = [row for row in gpu if process_pattern.search(row["process"])]
    encoders = {int(row["encoder-id"]): row for row in cpu}
    stages = collections.defaultdict(list)
    for row in target:
        stages[int(row["encoder-id"])].append(row)
    markers = collections.defaultdict(dict)
    complete_encoders = []
    for identity, rows in stages.items():
        encoder = encoders.get(identity)
        if encoder is None:
            continue
        start = min(interval(row)[0] for row in rows)
        end = max(interval(row)[1] for row in rows)
        label = encoder["encoder-label"]
        complete_encoders.append((start, end, label, identity))
        match = MARKER.fullmatch(label)
        if match:
            kind, frame, view, generation = match.groups()
            key = (int(frame), int(view), int(generation))
            if kind in markers[key]:
                raise ValueError(f"duplicate marker {kind}: {key}")
            markers[key][kind] = (start, end, rows)
    target_activity = merged(map(interval, target))
    all_activity = merged(interval(row) for row in gpu if row["state"] == "Active")
    state_active = merged(interval(row) for row in state if row["state"] == "Active")
    state_idle = merged(interval(row) for row in state if row["state"] == "Idle")
    optional_intervals = {}
    optional_counts = {}
    if args.application:
        application = read_table(args.application, "metal-application-intervals")
        waits = [row for row in application if process_pattern.search(row["process"])
                 and row["event-type"] == "Wait for Next Drawable"]
        wait_intervals = merged(map(interval, waits))
        optional_intervals["drawable_wait_union_ms"] = wait_intervals
        optional_intervals["idle_during_drawable_wait_ms"] = intersect(state_idle, wait_intervals)
        optional_counts["drawable_wait_events"] = len(waits)
    if args.driver:
        driver = read_table(args.driver, "metal-driver-intervals")
        waits = [row for row in driver if process_pattern.search(row["process"])
                 and row["gpu-driver-name"] == "MTLEvent"]
        optional_intervals["driver_mtlevent_union_ms"] = merged(map(interval, waits))
        optional_counts["driver_mtlevent_events"] = len(waits)
    rows, missing = [], []
    all_envelopes = [(pair["begin"][0], pair["end"][1], key)
                     for key, pair in markers.items() if "begin" in pair and "end" in pair]
    for sample in smoke["experimental_timing"]["observations"]:
        key = (sample["frame"], sample["view"], sample["generation"])
        pair = markers.get(key, {})
        if "begin" not in pair or "end" not in pair:
            missing.append(key)
            continue
        start, end = pair["begin"][0], pair["end"][1]
        if end <= start:
            raise ValueError(f"invalid marker ordering: {key}")
        duration = end - start
        full_counts = collections.Counter(label for a, b, label, _ in complete_encoders
                                          if a >= start and b <= end)
        family_counts = {label: full_counts[label] for label in FAMILIES}
        activity = clipped_duration(all_activity, start, end)
        active = clipped_duration(state_active, start, end)
        idle = clipped_duration(state_idle, start, end)
        rows.append({
            "frame": key[0], "view": key[1], "generation": key[2],
            "start_ns": start, "end_ns": end,
            "envelope_ms": duration / 1e6, "query_ms": sample["gpu_elapsed_ms"],
            "query_minus_trace_ns": round(sample["gpu_elapsed_ms"] * 1e6) - duration,
            "target_scheduled_union_ms": clipped_duration(target_activity, start, end) / 1e6,
            "all_process_scheduled_union_ms": activity / 1e6,
            "state_active_union_ms": active / 1e6, "state_idle_union_ms": idle / 1e6,
            "state_partition_delta_ns": active + idle - duration,
            "state_active_minus_gpu_intervals_ns": active - activity,
            "begin_cpu_to_gpu_latency_ms": min(int(row["start-latency"])
                                               for row in pair["begin"][2]) / 1e6,
            "overlapping_envelopes": [other[0] for a, b, other in all_envelopes
                                      if other != key and a < end and b > start],
            "complete_encoder_counts": family_counts,
            **{name: clipped_duration(intervals, start, end) / 1e6
               for name, intervals in optional_intervals.items()},
        })
    summary_fields = ("envelope_ms", "query_ms", "query_minus_trace_ns",
                      "target_scheduled_union_ms", "all_process_scheduled_union_ms",
                      "state_active_union_ms", "state_idle_union_ms",
                      "state_partition_delta_ns", "state_active_minus_gpu_intervals_ns",
                      "begin_cpu_to_gpu_latency_ms", *optional_intervals.keys())
    result = {
        "validated_for_governor": False,
        "scope": "Offline marker-envelope audit; scheduled intervals are not hardware busy counters.",
        "inputs": {key: str(getattr(args, key)) for key in
                   ("smoke", "encoders", "gpu", "states", "application", "driver")
                   if getattr(args, key)},
        "pid": args.pid, "source_revision": smoke.get("source_revision"),
        "source_dirty_at_build": smoke.get("source_dirty_at_build"),
        "trace_marker_pairs": len(all_envelopes), "retained_samples": len(rows),
        "missing_marker_samples": missing,
        "target_gpu_rows": len(target), "all_process_gpu_rows": len(gpu),
        "target_gpu_rows_without_cpu_encoder": sum(
            int(row["encoder-id"]) not in encoders for row in target),
        **optional_counts,
        "summary": {key: stats(row[key] for row in rows) for key in summary_fields},
        "samples_with_overlapping_envelopes": sum(bool(row["overlapping_envelopes"]) for row in rows),
        "complete_encoder_histograms": {label: dict(collections.Counter(
            row["complete_encoder_counts"][label] for row in rows)) for label in FAMILIES},
        "rows": rows,
    }
    return result


class AnalysisTests(unittest.TestCase):
    def test_idle_wait_overlap_excludes_other_idle_and_other_wait(self):
        self.assertEqual(intersect([(0, 5), (8, 20)], [(3, 10), (15, 25)]),
                         [(3, 5), (8, 10), (15, 20)])

    def test_overlapping_channels_are_not_double_counted(self):
        self.assertEqual(merged([(12, 20), (0, 10), (5, 15)]), [(0, 20)])
        self.assertEqual(clipped_duration([(0, 20)], 5, 15), 10)
        self.assertEqual(clipped_duration([(0, 10), (12, 20)], 5, 15), 8)
        self.assertEqual(merged([(10, 10), (12, 11)]), [])

    def test_xml_references_are_resolved_across_rows(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "table.xml"
            path.write_text('''<trace-query-result><node><schema name="test">
              <col><mnemonic>start</mnemonic></col><col><mnemonic>label</mnemonic></col>
              </schema><row><time id="1" fmt="1 ms">1000000</time>
              <label id="2" fmt="full label"><string>nested</string></label></row>
              <row><time ref="1"/><label ref="2"/></row></node></trace-query-result>''')
            rows = read_table(path, "test")
            self.assertEqual(rows[1], {"start": "1000000", "label": "full label"})


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        for name in ("smoke", "encoders", "gpu", "states", "out"):
            parser.add_argument(f"--{name}", required=True, type=Path)
        for name in ("application", "driver"):
            parser.add_argument(f"--{name}", type=Path)
        parser.add_argument("--pid", required=True, type=int)
        args = parser.parse_args()
        report = analyze(args)
        args.out.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({key: value for key, value in report.items() if key != "rows"}, indent=2))
