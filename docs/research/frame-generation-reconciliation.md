# Frame-generation harness reconciliation

This is a source audit and a proposed trial procedure, not a new presentation
result. No Shadow Work files were edited and no GPU workload was launched for
this audit. Frame interpolation remains experimental; positive drawable
timestamps alone do not establish image ordering, content, latency, or native UI
and HDR composition.

## Inspected versions

The historical consumer worktree is
`/Users/sma/projects/shadow-work/.claude/worktrees/metalfx-m5-research`, at
`2a49dfcb294a69283e9e4cf9aa0662b61c51495a`. The requested old `tools/research`
paths do not exist there. The actual scripts are
`crates/sw-renderer/scripts/gate1-gauge.sh` and
`crates/gpu-load-bench/scripts/sweep.sh`.

Its root Cargo patch points to the live `/Users/sma/projects/ushas` directory.
The resolved package still says `bevy_metalfx 0.4.2`; that version does not pin the
local implementation. Ushas was at `7db0818b6374f99475995b9939afdf9032aa18ce`
when its current sink and CI invariants were inspected. Subsequent candidate
builds must record their own exact source state and executable hash. Unrelated
untracked files exist in the consumer worktree and were left untouched.
The historical-result recovery and `PresentSink` semantics below were checked
again against Ushas `28c7de3db78635e17f36eccfbad45b3ec9e30025`.

| Inspected consumer file | SHA-256 |
| --- | --- |
| `crates/sw-renderer/scripts/gate1-gauge.sh` | `e68ad7c24d37ad4c2275b03f71a0dd1423c71caf82f32c22f522d69ce40a6f54` |
| `crates/gpu-load-bench/scripts/sweep.sh` | `dc8fc0a5038651ca80eca41ea68519600cb4d34f672f5a089f735dc342e90b14` |
| `crates/sw-renderer/src/main.rs` | `da675d3e81cd85107e6530b3572787f15af842644a9a29fdfb2858c3bd3ea87b` |
| `crates/gpu-load-bench/src/measure.rs` | `c375759d55a3113e83e07f61fb632eb35169b5643ca6a07f7f1ce15979de8337` |
| `Cargo.toml` | `32d9a4dcecf1830af52f9fc15dbe145338fd2045999fb226cdcb0ee5c899074f` |
| `Cargo.lock` | `5a75cb6553aa6db799f7e653d604769661303a4922b7a6d6fcbd5ecc82846f61` |

The scripts preserve historical defect accounts: a double-counted presented
rate, a ring occupancy reported as a cumulative count, and intermittent process
shutdown hangs. Their comments also preserve periods when drawable timestamps
were unavailable. Those accounts are historical; they do not establish the
current candidate's behavior or an enduring machine limitation. This audit did
not re-run those historical revisions or recover a new immutable historical
binary. Current static reconstruction and marker-trace evidence have their own
scope in [temporal-quality.md](temporal-quality.md) and
[marker-scope-01.md](marker-scope-01.md).

## Recovered historical results

The following are immutable **committed accounts of runs**, recovered from
Shadow Work's git history. They are not newly reproduced measurements or a
recovered collection of raw logs and executable hashes.

| Revision and date | Recorded result | Disposition |
| --- | --- | --- |
| [`05f81248ecdc2750922fb33898c4dfeec66903a1`](https://github.com/mistakeknot/shadow-work/commit/05f81248ecdc2750922fb33898c4dfeec66903a1), July 25, 2026 | Single: 403 presents, 403 callbacks, 26.9 render fps. Dual: 802 presents, 801 callbacks, 26.7 render fps. Both use the owned layer and compute interpolation; cap 30 fps. | The recorded 1.99× gain is accepted presents. The account explicitly withholds a presented-fps pass and records a separate positive control with 881 callbacks and zero positive presentation timestamps. Identical configured interpolation work does not establish identical measured GPU cost. |
| [`4253273976ba58a454b6cf00ceaa11bd896db8a1`](https://github.com/mistakeknot/shadow-work/commit/4253273976ba58a454b6cf00ceaa11bd896db8a1), July 27, 2026 | Awake: **13 passed / 0 failed / 0 skipped, PASS (strong)**. Same tree asleep: **10 passed / 0 failed / 3 skipped, PASS (weak)**. | This fixes D1's double-counted rate, D2's ring-occupancy counter, and D3's unbounded bench exit. It records the frozen external gate's outcomes and a subsequent repair to two shell-comment apostrophes. The asleep result deliberately withholds the three presentation criteria. |
| [`493a06f83f9edf65b10a297281ffe6a166233f37`](https://github.com/mistakeknot/shadow-work/commit/493a06f83f9edf65b10a297281ffe6a166233f37), July 27, 2026 | Both pilot arms independently rerun by the validator: **13 passed / 0 failed / 0 skipped, PASS (strong)**. | The merge records a tied gauge result. Arm B was selected on implementation and harness quality, not a larger measured performance gain. |

The July 27 merge explicitly describes a run that measured positive drawable
timestamps. It supersedes the earlier account's claim that timestamps never
populate on this machine. That older claim also remained in the
[research document at the merge revision](https://github.com/mistakeknot/shadow-work/blob/493a06f83f9edf65b10a297281ffe6a166233f37/crates/bevy_metalfx/docs/m5-max-performance-research.md);
its persistence is documentation drift, not evidence of a permanent hardware
limitation.

The [frozen gauge at that revision](https://github.com/mistakeknot/shadow-work/blob/493a06f83f9edf65b10a297281ffe6a166233f37/crates/sw-renderer/scripts/gate1-gauge.sh)
has ten general checks and three presentation checks. The latter test that the
cumulative displayed counter can exceed 480, that single-present rate is no
more than 1.15× render rate, and that dual-present rate is between 1.7× and
2.3× render rate. Single-present has no lower-bound check. The gauge checks
display state at the end rather than continuously. Its strong result therefore
establishes the historical gauge's criteria, including reported presentation
cadence; it does not independently establish frame content, real/interpolated
ordering, input latency, or physical scanout.

The original **27.1 → 54.1 fps** claim remains **uncorroborated at that numerical
precision**. Targeted recovery of the gauge, relevant research documents,
commit messages, and tracked result paths found the accounts above, but no raw
record or immutable binary for those two numbers. Neither the 26.9/26.7
render-rate account nor a count of thirteen passing checks reconstructs them.
Preserve the claim as historical and unresolved; do not silently replace it
with accepted callbacks or promote it to current-candidate evidence.

## PresentSink measurement semantics

The [audited current sink](https://github.com/mistakeknot/ushas/blob/28c7de3db78635e17f36eccfbad45b3ec9e30025/src/present/sink.rs)
shares one callback across real and interpolated drawables. It retains positive
timestamps and aggregate counters, without source-frame ID, frame kind, view,
configuration epoch, or drawable ID.

Apple defines [`presentedTime`](https://developer.apple.com/documentation/metal/mtldrawable/presentedtime)
as the host timestamp of reported onscreen presentation; zero means not yet
presented or dropped. A positive value is consequently stronger evidence than
callback receipt alone. It is Metal's presentation report, not an independent
measurement of panel pixels, content correctness, or end-to-end input latency.
The [`addPresentedHandler` documentation](https://developer.apple.com/documentation/metal/mtldrawable/addpresentedhandler(_:))
describes per-drawable notification, but the inspected documentation provides
no cross-drawable callback delivery-order guarantee.

The current `inversions` counter increments when a retained positive timestamp
is less than **or equal to** the previous retained callback's timestamp. Its
actual meaning is **non-increasing timestamps in callback lock-acquisition
order**. The source comment equating this to out-of-order displayed frames is
too strong: reordered callback delivery could trigger it, while wrong
real/interpolated content order could still have increasing timestamps and
zero inversions. Equal timestamps also count. This audit identifies the
semantic limitation; it does not assert observed callback reordering or alter
the implementation.

Other limits matter when interpreting a run:

- `stats()` sorts the latest 480 retained timestamps and drops nonpositive
  intervals. Its rate and interval spread describe that timestamp sample,
  combining both frame kinds. Sorting cannot recover intended content order.
- Callback and positive-timestamp counters use separate nonblocking locks.
  Contention can lose either update independently, with no telemetry-loss
  counter. `dropped` instead counts presents that could not be issued; it does
  not account for those lost samples. Treat the counters as observed events,
  not complete lossless accounting.
- `committed` counts command-buffer completion callbacks, which do not prove
  presentation. Reset clears counters and samples but retains the prior
  timestamp; callbacks from pre-reset work can enter the new window because
  they carry no measurement epoch.

Before an ordering or precise loss-rate gate, record an intended presentation
ordinal, real/interpolated kind, source frame/view/configuration identity,
drawable ID, requested presentation time, reported presentation time, and CPU
callback-receipt time. Track telemetry loss separately and attribute in-flight
callbacks to their original measurement epoch. Compare intended order against
reported presentation times rather than assuming callback arrival order is
display order.

## Smallest useful repairs

1. **Separate the relocated invariant from hardware skips.** Gate A4
   unconditionally calls `skip` at line 155. Every nonzero `SKIP` then selects
   `PASS (weak)` at lines 380–388 and inaccurately calls all skips
   presented-fps criteria. Thus strong PASS is unreachable. Keep A4, but record
   a separate `verified_external`, `missing_external_evidence`, or `failed`
   outcome. Require evidence for the exact resolved Ushas source: its current
   `static invariants` CI job and
   `present::sink::tests::displayed_counts_every_frame_while_the_ring_saturates`.
   A green CI result from a different commit cannot satisfy A4. Maintain
   separate accepted-callback, timestamp-rate, and external-invariant verdicts;
   a missing invariant must not disappear into a hardware skip.

2. **Make each arm durable and attributable.** Add an explicit
   `--bench-results=PATH` option to `sw-renderer`; propagate write failures to
   a nonzero exit. It currently ignores the result of writing
   `/tmp/sw-bench-results.txt` at line 767. Give the gate a fresh `GATE_RUN_DIR`,
   retain separate baseline/dual logs and results, and refuse overwrites.
   Record argv, both source revisions and dirty diffs, Cargo metadata/lock hash,
   binary hash before/after, adapter, OS, window dimensions, scale, and refresh
   assumption. The existing gate rebuilds, but omits `--locked` and has no
   immutable binary provenance. Do not reuse the old fixed files concurrently.

3. **Measure the intended work and window.** Both arms must first demonstrate
   distinct fresh `OutputWritten` observations for the requested mode, scale,
   view, and dimensions. The old consumer's warmup is only elapsed time, and
   its `frames` counter counts main-loop samples. Retain rendered-frame IDs
   separately. Drain or label callbacks crossing the measurement boundary;
   sink reset alone does not assign an in-flight callback to an originating
   frame. Reject missing, nonfinite, malformed, zero-denominator, or impossible
   counters before calculating ratios. Add a lower bound to the single-present
   rate check: the current S2 checks only an upper bound of 1.15×.

4. **Bound and observe execution.** Keep the per-arm watchdog, and add cleanup
   on interruption for the child process group. Wrap the bounded trial in
   `caffeinate -d`, which prevents idle display sleep but does not unlock a
   session. Sample display/lock state during each arm, not just at its end;
   errors reading that state must remain unknown, not become awake/unlocked.
   A display loss voids timestamp/panel claims while preserving separately
   labelled callback diagnostics. `sw-renderer::finish_bench` calls
   `std::process::exit(0)`, so current D3 establishes bounded process exit,
   not repaired orderly Bevy/Metal teardown.

5. **Close the sweep's failure paths before reusing it.** This is an upscaling
   crossover harness, not a frame-generation benchmark: it deliberately
   includes only disabled, spatial, and temporal modes. Its watchdog at lines
   224–237 samples lock and CPU load but never display sleep or failed probes.
   `run_one` ignores the child exit status, has no per-run timeout, and replaces
   `last.log`/`current.txt` on every attempt. It can ingest results from a failed
   process. Always rebuild or verify an immutable supplied binary, retain every
   attempt, and require successful exit plus fresh render proof. Start the
   watchdog only after `SWEEP_LIB_ONLY` returns; today even sourcing its unit
   test starts the watcher. Use unique output directories, fail if raw-copy or
   report writes fail, and preserve working evidence if archival fails. At
   lines 526–532 capture both pipeline statuses: reading only `PIPESTATUS[0]`
   discards a failed `tee`. Propagate nonzero status for VOID/missing runs and
   failed per-level analysis as well as crossover failure.

6. **Preserve timing names and limits.** The consumer still divides dedicated
   MetalFX command-buffer elapsed time by CPU-loop time and prints a likely
   GPU-bound verdict (`main.rs:618–644`). That ratio is not a validated frame
   utilization signal. The raw timer includes upstream waits; the measured
   disabled control is an empty command buffer. The newer marker trace also
   contains idle/drawable waits and overlapping frames. Neither signal can be
   used to calculate isolated upscaler cost or authorize adaptation. Keep the
   sweep's CPU-load and empirical speed floors as diagnostics, not proof that
   the requested GPU passes ran. Require actual render evidence and the
   expected encoder families in a trace.

These repairs need small, CPU-only regression fixtures: planted external-check
pass/missing/failure; zero displayed timestamps; absent/nonfinite result fields;
nonzero child exit after writing a plausible result; timeout; display loss and
probe error; failed report/raw-copy/tee; and library-only sourcing with no
watchdog. The current sweep guard test covers speed floors, not those execution
or persistence failures.

## Bounded candidate trial

First complete and review the above harness changes. The proposed
`GATE_RUN_DIR`, `USHAS_ROOT`, and `USHAS_EXPECTED_REV` variables below are new
interfaces to implement; the inspected gauge does not honor them. Do not run
this block against the unchanged gauge and assume the paths or source pin took
effect.

```bash
set -euo pipefail
consumer=/Users/sma/projects/shadow-work/.claude/worktrees/metalfx-m5-research
ushas=/Users/sma/projects/ushas
trial=$(mktemp -d /private/tmp/ushas-fg-candidate.XXXXXX)
candidate=$(git -C "$ushas" rev-parse HEAD)
git -C "$ushas" status --porcelain > "$trial/ushas-status.txt"
git -C "$consumer" status --porcelain > "$trial/consumer-status.txt"
git -C "$ushas" diff --binary > "$trial/ushas.patch"
git -C "$consumer" diff --binary > "$trial/consumer.patch"
git -C "$consumer" rev-parse HEAD > "$trial/consumer-revision.txt"
printf '%s\n' "$candidate" > "$trial/ushas-revision.txt"

cargo +1.97.1 test --locked --manifest-path "$ushas/Cargo.toml" \
  --all-features --lib \
  present::sink::tests::displayed_counts_every_frame_while_the_ring_saturates \
  -- --exact > "$trial/cumulative-counter-test.log" 2>&1

cd "$consumer"
cargo +1.97.1 metadata --locked --format-version 1 > "$trial/cargo-metadata.json"
cargo +1.97.1 build --locked -p sw-renderer --release \
  > "$trial/build.log" 2>&1
shasum -a 256 target/release/sw-renderer Cargo.lock > "$trial/build.sha256"

GATE_RUN_DIR="$trial/gate" USHAS_ROOT="$ushas" \
USHAS_EXPECTED_REV="$candidate" CAP_FPS=30 WATCHDOG=90 \
  caffeinate -d bash crates/sw-renderer/scripts/gate1-gauge.sh \
  > "$trial/gate.log" 2>&1
```

The repaired gate must verify that metadata resolves `bevy_metalfx` to the
expected source and that the source remains unchanged across its rebuild and
both runs. Review dirty files explicitly; a base hash plus a moving path is
insufficient. Ensure the globe's required textures exist before either arm:
`crates/sw-renderer/assets/textures/blue_marble.jpg` and `heightmap.jpg`.
Do not infer textured scene output from a successful build.

Use the existing consumer arguments exactly; its parser uses equals signs:

```text
baseline: --bench-quick --cap-fps=30 --scale=0.5 --metalfx=interpolate --dual-present --present-single
dual:     --bench-quick --cap-fps=30 --scale=0.5 --metalfx=interpolate --dual-present
```

Both arms compute interpolation and use the same owned layer; only the second
present differs. Run them serially with no competing GPU job. Start with this
two-arm pilot, inspect both logs and captures, and stop on missing effect or
timestamp evidence. If it is valid, repeat in reverse order before extending
the campaign. At 30 rendered frames/s, the display must have enough verified
refresh capacity for the proposed doubled delivery. The old consumer does not
set `with_refresh_interval`, so it inherits Ushas's 1/120-second assumption;
record that assumption and make it configurable before another display target.

Report three separate results: callback issuance/completion, positive drawable
timestamp cadence, and independent visual evidence. The old callback criteria
(at least 99% callbacks/encoded, at least 1.9× normalized callback gain, and no
more than 10% render-rate loss) remain historical gauge thresholds, not a claim
that interpolation is free or visually correct. A Bevy screenshot proves its
render target, not the separate owned layer's panel content. Assess moving
geometry, disocclusion, real/interpolated ordering, UI, and latency on the actual
visible output before making a product claim. An offscreen trial cannot close
presentation gates. Preserve skips and failures; no fresh panel result is
asserted by this document.

## Current candidate disposition and remaining gates

At the audited Ushas revision, the `0.5.0-rc.1` frame-interpolation path remains
**experimental, with no fresh candidate presentation verdict**. The historical
strong pass is preserved above. It does not transfer across the renderer,
dependency, timing, and history changes to this candidate. Spatial/Temporal
reconstruction now runs before postprocessing and UI, while interpolation
retains the late presentation path; early reconstruction's native UI and HDR
evidence does not validate that separate composition path. An offscreen
consumer camera-cut trial or fixed-scale CPU-cadence campaign cannot close
these presentation gates.

The remaining acceptance work is bounded and separable:

1. **Freeze the repaired gauge and candidate.** Satisfy the relocated invariant
   against the exact dependency, retain executable/source/lock hashes and
   distinct arm artifacts, and require fresh render observations. Record actual
   display refresh, dimensions, and HDR/format configuration. Verify display
   awake/unlocked/visible throughout; an unavailable state probe stays unknown.
2. **Reproduce the presentation-rate comparison.** Run the single/dual arms
   above with the same interpolation work and owned layer, first forward and
   then reversed, with no competing GPU workload. Require positive timestamp
   evidence and explicit measurement-window attribution. Keep callback rate,
   timestamp cadence, telemetry loss, and render cadence separate. The
   historical 13-check gauge needs the lower-bound and ordering limitations
   above addressed before a stronger current acceptance claim.
3. **Verify delivered content and latency independently.** Use identifiable
   real/interpolated frame content and an owned-layer capture/trace to check
   order and pacing. A physical-panel delivery or input-to-photon claim needs
   a suitable external capture and timing reference; OS screenshots or screen
   recording alone establish neither physical scanout nor that latency.
4. **Check the product path.** Inspect motion/disocclusion, camera cuts and
   history resets, native-resolution UI, HDR/exposure composition, resize,
   occlusion/minimize, and resume on the actual output. Retain failures and
   unavailable cases as such. Do not infer these properties from accepted
   callbacks, zero `inversions`, or a rate near twice the render-loop rate.

These gates leave the candidate usable for explicit experiments while
withholding a production frame-generation, physical-panel, or latency claim.
Neither dedicated MetalFX buffer timing nor the current marker envelope is a
validated full-frame GPU cost signal or permission for autonomous adaptation.
