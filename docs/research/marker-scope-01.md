# Metal System Trace: marker-scope-01

The experimental timestamp query measures the GPU marker envelope accurately in
this capture. It is **not a validated per-frame GPU cost for adaptation**. The
envelope includes drawable waiting and recorded GPU Idle time, overlaps adjacent
frames, and does not establish hardware execution-unit utilization. The
experiment remains `ObservedUnvalidated`; it must not feed the governor.

## Capture and provenance

- Completed Metal System Trace, 2026-09-04, duration 8.339403 seconds;
  target `ushas-smoke`, PID 39992, Apple M5 Max, macOS 26.5.2 (25F84),
  Instruments 26.0 (17C519). Recording ended when the target exited.
- Build revision `f4e6995bdc212aa0c31f1143ad6f63a36c7c3a8d`,
  `source_dirty_at_build=true`: this identifies the base, not an immutable build.
- Arguments: `--mode temporal --scale .5 --experimental-timing
  --pixel-iterations 1000 --warmup 3 --seconds 4`.
  Output 1280×720, rendered input 640×360, fixed camera, native AA off,
  no injected CPU delay, normal window presentation.
- Smoke JSON records `valid=true`, 553 measurement frames, and 237 retained
  experimental observations (frames 610–846); render validity is a separate gate
  from timing validity and panel delivery.
- Shader Timeline was disabled and no GPU counter set was selected. This is a
  scheduling trace, not a shader-utilization or physical-busy measurement.
  Trace instrumentation can affect scheduling; these durations are not an
  uninstrumented performance comparison.

Local artifacts are under `/private/tmp/ushas-roadmap-evidence/`:
`marker-scope-01.trace`, `.json`, `.log`, `.toc.xml`, `.gpu.xml`,
`.gpu-intervals.xml`, `.gpu-states.xml`, `.application.xml`, `.driver.xml`,
`.signposts.xml`, and `.analysis.json`. The trace and large exports are not
checked into the repository. The `.analysis.json` produced by the script below is
the authoritative analysis output; earlier scratch exports are not used here.

## Findings

The trace has 851 begin/end marker pairs, 41,922 target GPU interval rows, and
49,843 GPU interval rows across all processes. All 237 retained JSON observations
match marker labels by frame, main-world view identity, and generation. Query
duration minus traced outer marker duration is exactly 0 ns for 136 samples,
−1 ns for 47, and +1 ns for 54. This establishes clock/boundary agreement within
the exports' nanosecond rounding, not cost validity.

| Quantity, over the same 237 marker envelopes | Mean | Median | Maximum |
| --- | ---: | ---: | ---: |
| Marker elapsed duration | 5.071 ms | 5.671 ms | 15.071 ms |
| Target scheduled GPU intervals, union | 2.002 ms | 1.372 ms | 5.968 ms |
| All-process scheduled GPU intervals, union | 2.043 ms | 1.440 ms | 6.047 ms |
| GPU state labelled Active, union | 2.044 ms | 1.440 ms | 6.047 ms |
| GPU state labelled Idle, union | 3.027 ms | 2.994 ms | 13.069 ms |
| Application Wait for Next Drawable, overlap | 4.130 ms | 4.986 ms | 14.124 ms |
| Idle simultaneous with drawable wait | 2.624 ms | 2.703 ms | 13.062 ms |

Recorded Idle totals 59.70% of the summed marker durations. Drawable waiting
overlaps 86.66% of that Idle time. This is direct temporal correlation, not proof
that one wait causes every gap. The driver also records 5,097 MTLEvent intervals
over the capture, including waits for `metalfx_raw_encode` to signal. Such waits
and CPU scheduling must remain distinct from GPU shader cost.

Each retained envelope contains one complete GPU encoder instance for the main
opaque pass, motion resolve, depth resolve, MetalFX temporal Pre/Mid/PostProcessing,
reconstruction, UI, and final upscaling. Encoders are joined by their exported
encoder IDs and all their GPU stages are checked. CPU creation order is not a
valid frame assignment: raw MetalFX and resolve encoders can be created before
the corresponding begin-marker encoder and executed later in the proper GPU
sequence. Counting by CPU creation alone produces a false missing-scope result.
There are also 795 target GPU interval rows without matching CPU encoder metadata;
they remain in the scheduled-activity union but cannot contribute to named
encoder-family coverage counts.

Twenty-two retained samples overlap at least one neighboring frame's marker
envelope. Vertex work can overlap another frame's fragment work, so even an
overlap-safe union inside an envelope is not exclusively attributable to that
frame. Stage presence establishes coverage for this fixture, not a universal
full-frame scope across all Bevy render configurations.

The early preprocessing boundary is incomplete even in this fixture: five
envelopes contain no complete `bin unpacking` encoder and one contains no
complete `early_mesh_preprocessing` encoder. Conversely, twelve and nine
envelopes respectively contain two complete instances. Those counts are another
reason the marker envelope cannot be called an isolated full-frame cost.

The state exports have small internal discrepancies: Active/Idle unions overlap
in two sampled envelopes (at most 6,917 ns), and state Active differs from the GPU
interval union in four samples (at most 140,709 ns). The analyzer reports these
differences instead of forcing the tables to agree. No rows were present in the
exported Metal command-buffer error table; this does not prove panel delivery.

## Reproduce the offline audit

Do not launch a new GPU workload to run this analysis. Wait until the original
recording has finished before exporting. On this host, Instruments export needs
access to its normal cache directory outside the workspace sandbox.

```bash
trace=/private/tmp/ushas-roadmap-evidence/marker-scope-01.trace
evidence=/private/tmp/ushas-roadmap-evidence
xcrun xctrace export --input "$trace" --toc --output "$evidence/marker-scope-01.toc.xml"
```

Export these tables separately with the same command shape, substituting the
schema and output filename from the table. Separate exports preserve the column
schema in each file.

```bash
xcrun xctrace export --input "$trace" \
  --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-application-encoders-list"]' \
  --output "$evidence/marker-scope-01.encoders.xml"
```

| Schema | Output suffix |
| --- | --- |
| `metal-application-encoders-list` | `.encoders.xml` |
| `metal-gpu-intervals` | `.gpu-intervals.xml` |
| `metal-gpu-state-intervals` | `.gpu-states.xml` |
| `metal-application-intervals` | `.application.xml` |
| `metal-driver-intervals` | `.driver.xml` |

The original combined `.gpu.xml` also works for `--encoders` because the encoder
table is its first schema-bearing node. From the repository root:

```bash
python3 tools/timing-probe/analyze_metal_trace.py --self-test
python3 tools/timing-probe/analyze_metal_trace.py \
  --smoke "$evidence/marker-scope-01.json" \
  --encoders "$evidence/marker-scope-01.encoders.xml" \
  --gpu "$evidence/marker-scope-01.gpu-intervals.xml" \
  --states "$evidence/marker-scope-01.gpu-states.xml" \
  --application "$evidence/marker-scope-01.application.xml" \
  --driver "$evidence/marker-scope-01.driver.xml" \
  --pid 39992 --out "$evidence/marker-scope-01.analysis.json"
```

The standard-library script resolves XML references, joins CPU/GPU encoder IDs,
unions overlapping channels before clipping, and exposes missing markers,
neighbor overlap, and state-table discrepancies. Its output always retains
`validated_for_governor=false`. Parser, union, and interval-intersection tests
were observed failing before implementation and pass in the completed script.
An independent review reproduced the complete output and checked the interval
math against 500 discrete-interval oracle cases; no blocking findings remained.

A future producer must establish frame ownership and meaningful GPU-cost scope
without charging drawable/CPU waits as shader work, then pass independent CPU
delay and pixel-load controls. This trace supports that remaining requirement;
it does not close it or change historical presentation evidence.

The finalized trace is also archived at
`/Users/sma/projects/docs/ushas/evidence/marker-scope-01.trace`, with a sibling
`.trace.manifest.json` containing verified hashes and link metadata. The
`marker-scope-01-artifacts` directory beside it retains the exported tables,
reports and analysis, with its own hash manifest. The original temporary
artifacts remain intact.
