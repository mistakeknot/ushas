# Frozen Shadow Work consumer trial

This tool archives Shadow Work commit `2a49dfcb294a69283e9e4cf9aa0662b61c51495a`
and an explicitly selected, committed Ushas revision into a new private directory.
It patches the archived consumer only. The live consumer worktree, its benchmark
path, and the Ushas source checkout remain unchanged.

The trial keeps the consumer's textured globe, pinned camera, hard camera cut,
and six screenshots (`before`, `p0`, `p1`, `p2`, `p4`, `p8`). Its offscreen branch
uses the existing image target, a metadata window at 1600×900 physical pixels
and scale factor 1, and `ScheduleRunnerPlugin` with Winit disabled. This measures
image correctness and CPU effect decisions. It provides no panel-presentation,
GPU-duration, performance, or frame-generation result.

## Prepare and compile

Run from the Ushas repository with Python 3.12+ and Rust 1.97.1. Supply the exact
candidate commit rather than a mutable worktree. Preparation needs approximately
6GB because it retains the complete consumer archive and extracted tracked data.

```sh
python3 tools/consumer-smoke/run.py prepare --revision <ushas-commit>
# The command prints a unique /private/tmp/ushas-consumer-... directory.
python3 tools/consumer-smoke/run.py build <directory> --check \
  --target-dir /Users/sma/projects/shadow-work/.claude/worktrees/metalfx-m5-research/target
python3 tools/consumer-smoke/run.py build <directory> \
  --target-dir /Users/sma/projects/shadow-work/.claude/worktrees/metalfx-m5-research/target
```

`--check` compiles without launching a renderer. The second command creates a
release binary and copies it into the private directory immediately, recording
its SHA-256. A shared target directory is a compilation cache only; use it only
while no other consumer build is replacing that cache's binary. Omit the option
for a separate cache. Each check/build attempt is retained once; prepare a fresh
directory after a failure or source change.

Cargo runs offline. Only the archived lockfile may change to accommodate the
candidate's exact version and the added JSON instrumentation; each lock diff is
retained. The preserved workspace can still resolve registry 0.4.2 for its separate
`gpu-load-bench` package. Host-filtered Cargo metadata must show that **sw-renderer**
directly resolves MetalFX to the archived Ushas directory before a build passes.

## Run the three arms

These commands launch the GPU workload. Run them sequentially on the intended
Apple device after compilation:

```sh
python3 tools/consumer-smoke/run.py run <directory> --arm native --timeout 90
python3 tools/consumer-smoke/run.py run <directory> --arm temporal --timeout 90
python3 tools/consumer-smoke/run.py run <directory> --arm bilinear --timeout 90
```

Native requests `Disabled` at scale 1; temporal requests `Temporal` at 0.5;
bilinear requests `Disabled` at 0.5. Every arm writes to a new directory. The
scripted counter waits for both textures, three seconds of warmup, and 20 distinct
consecutive ready observations. Subsequent steps require a new accepted frame.
Acceptance checks the main-world camera identity, exact requested/effective mode,
scale and physical dimensions, and a maximum observation age of three frames and
250ms. Temporal requires `OutputWritten`; native/bilinear require fresh `Disabled`.
The cut requests the existing durable history reset. Script offsets count accepted
observations, not necessarily consecutive app frames; each screenshot records its
request's main-frame identity and arrives asynchronously.

The probe has a 75-second deadline and the outer process-group timeout defaults to
90 seconds. SIGINT/SIGTERM stop and reap the child process group and retain a
non-success manifest. Failure retains its log, available screenshots, and effect
events. Successful capture validation requires all six decoded RGBA images
at 1600×900, fully opaque alpha, at least 64 sampled RGB colors and 1% visible pixels. PNGs preserve the original alpha. This rejects blank or transparent
captures; visual comparison is still required to assess reconstruction quality.

## Provenance and isolation

`prepared.json` records commits, archive hashes, complete extracted tree hashes,
the exact patch, and instrumentation hashes. `check.json`/`build.json` record
compilation, lock hashes and resolved dependency; each arm's `manifest.json`
records its command, binary hash and retained artifacts. `effect.jsonl` preserves
up to 20,000 per-frame effect observations plus capture records. `effect.json`
records final readiness and decoded capture results.

The full git archive retains tracked symlink metadata. Extraction omits and records
links escaping the frozen source tree, so it cannot follow the consumer's sibling
`shadow-work-data` link. No other live or untracked data is copied except these two
explicitly required, locally available NASA texture files, which the consumer
intentionally ignores in Git:

- `crates/sw-renderer/assets/textures/blue_marble.jpg`
- `crates/sw-renderer/assets/textures/heightmap.jpg`

Both must be regular JPEG files and remain hash-identical while copying. They are
recorded separately and loaded before readiness can pass. The offscreen wrapper
disables the optional webview environment flag and redirects the consumer's frame
CSV into the arm directory. It neither edits nor downloads assets into the live tree.

CPU tests:

```sh
python3 -m unittest discover -s tools/consumer-smoke -p test_run.py -v
rustc +1.97.1 --test tools/consumer-smoke/readiness.rs -o /tmp/consumer-readiness-tests
/tmp/consumer-readiness-tests
```
