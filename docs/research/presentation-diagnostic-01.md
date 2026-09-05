# Presentation diagnostic: blocked preflight

**The attempt stopped at preflight with exit 3 (`environment_unavailable`).**
No renderer launched, no GPU arm ran, and no screenshots or presentation
measurements were produced. Single- and dual-present interpolation were skipped;
they did not pass or fail a performance comparison.

The invocation ran September 5, 2026, 03:58:01.055–03:58:01.141 UTC from clean
runner revision `55755d15eea77513704192262b4c0112e8e50f97`. It selected the
previously frozen smoke executable with SHA-256
`5c773aba90f70229d03f1ac87045045f11a6f2e435bb9bc21f55e1b0266b4744`
and expected compiled revision `56a3b16c8c1c8f12a5320adc5082c6d20b6378c1`.
The binary hash matches the separately archived executable; this attempt did
not execute it or produce a fresh runtime source attestation.

Both retained samples reported main display **1 asleep**, session **locked**,
and session **on console**. The explicit lock boolean came from
`CGSessionCopyCurrentDictionary`; neither query reported an error. The samples
were taken at 03:58:01.107447 and 03:58:01.107796 UTC, approximately 0.35 ms
apart around the rejected preflight. They describe that instant, not a
continuous rendering period. The empty parent log, null child exit code and
absence of child artifacts agree with the blocked-launch path.

The planned arms were Temporal only, interpolation with one present, and
interpolation with two presents, using static Claude at 1280×720, LDR, half
scale and load 20,000, with two seconds warmup and six seconds measurement.
The refresh value 120 Hz was a sink assumption, not an observed refresh rate.
Only the first arm reached preflight; the other two retain explicit skipped
records. Exact argv, parent metadata, environment hash, tool hashes and samples
are in the [structured record](presentation-diagnostic-01.json).

This blocked attempt leaves net benefit **not established**. A fresh invocation
with observed awake, unlocked and on-console conditions is needed to collect
the diagnostic arms. Even a successful run of this harness cannot establish
per-frame generated/real ordering, image content, panel pixels, input latency
or net benefit from the existing aggregate `presentedTime` sink.

The entire attempt and exact committed probe, `run.py`, and frozen smoke
PNG-helper/capture-callsite sources are retained under
`/Users/sma/projects/docs/ushas/evidence/presentation-diagnostic-01/`.
All seven payloads (84,815 bytes) were hash-verified. The helper sources are
included for provenance; the child-side helpers were not executed here.
The archive-manifest SHA-256 is
`edb26b35c30b986a11ffc5bd2d63b8a593e3d1c4db83d6615df2c415fd27d46c`.
The original attempt remains unchanged.
