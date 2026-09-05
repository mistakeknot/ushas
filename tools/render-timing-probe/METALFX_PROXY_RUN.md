# MetalFX proxy probe: bounded execution protocol

This is an isolated compatibility experiment. `ObservationLedger.available`
means only that the supplied command buffer's observed factories and local
samples passed its guards. It does not establish all MetalFX work, a Bevy frame,
exclusive GPU occupancy or a validated governor input.

Root has compiled the observer and executable with strict Clang warnings and
run 65 fake-delegate checks plus three CLI checks successfully. The analyzer's
ten CPU tests pass, including malformed identity/lifetime records, exact output
parity, overlap unions, decoded PNGs and retained raw-output corruption. No
MetalFX proxy hardware result is recorded yet. Root schedules every build and
GPU invocation; peer review precedes hardware.

The standalone build has no Cargo or repository library dependency:

```sh
xcrun clang -O2 -fobjc-arc -fblocks -Wall -Wextra -Werror \
  tools/render-timing-probe/ObservationProxy.m \
  tools/render-timing-probe/MetalfxProxyProbe.m \
  -framework Foundation -framework Metal -framework MetalFX \
  -framework CoreGraphics -framework ImageIO -o /private/tmp/metalfx-proxy
/private/tmp/metalfx-proxy --self-test
python3 tools/render-timing-probe/analyze_proxy.py --self-test
```

Retain the exact compiler command, exit status, compiler/SDK/OS identity,
source hashes, binary hash and build log in the root execution receipt. The
native executable accepts only a fresh output directory. It records support,
selectors, actual command-buffer status/errors/labels and original frame
identities in `samples.jsonl`; every completed frame saves its exact MetalFX
RGBA16Float output (`.rgba16`, little endian) and an opaque composed PNG.
The PNG's first pixel is an explicit composition sentinel. The raw output
hash and its changing input are separate proof; that sentinel is not evidence
that MetalFX preserved a particular pixel.

Each frame owns its textures and GPU readback buffers. A final same-queue blit
copies that frame's actual MetalFX output before callback hashing. All 16 small
frame resource sets remain alive until process exit; only two frames are
admitted concurrently. The counter callback resolves shared sample storage
directly after actual Metal completion. There is no `waitUntilCompleted`,
frame-loop polling wait or readback resolve command buffer. This bounded
synthetic admission policy is not the intended production sampling policy.

## Root-owned hardware invocation

Run each arm once in a fresh process with an external watchdog. The internal
15-second check starts after synchronous setup and shares the encode queue, so
it cannot interrupt stalled setup/encoding. This example retains a process
receipt and kills the entire process group on the 65-second outer timeout:

```sh
python3 - /private/tmp/metalfx-proxy /private/tmp/proxy-spatial-off-02 spatial off <<'PY'
import hashlib,json,os,signal,subprocess,sys,time
from pathlib import Path
binary,out,mode,observe=Path(sys.argv[1]),Path(sys.argv[2]),sys.argv[3],sys.argv[4]
assert not out.exists()
log=Path(str(out)+'.log');receipt=Path(str(out)+'.run.json')
assert not log.exists() and not receipt.exists()
argv=[str(binary),'--mode',mode,'--observe',observe,'--out',str(out)]
before=hashlib.sha256(binary.read_bytes()).hexdigest();env=dict(os.environ,MTL_DEBUG_LAYER='1')
def interrupted(signum,frame):raise InterruptedError(f'signal {signum}')
signal.signal(signal.SIGTERM,interrupted)
started=time.monotonic();timed_out=False;interruption=None
with log.open('xb') as stream:
    child=subprocess.Popen(argv,stdout=stream,stderr=subprocess.STDOUT,env=env,start_new_session=True)
    try:code=child.wait(timeout=65)
    except (subprocess.TimeoutExpired,KeyboardInterrupt,InterruptedError) as error:
        timed_out=isinstance(error,subprocess.TimeoutExpired);interruption=None if timed_out else str(error) or 'keyboard interruption'
        signal.signal(signal.SIGINT,signal.SIG_IGN);signal.signal(signal.SIGTERM,signal.SIG_IGN)
        try:
            try:os.killpg(child.pid,signal.SIGTERM)
            except ProcessLookupError:pass
            try:child.wait(timeout=2)
            except subprocess.TimeoutExpired:pass
        finally:
            # The leader may exit before a helper: sweep the group regardless.
            try:os.killpg(child.pid,signal.SIGKILL)
            except ProcessLookupError:pass
        code=child.wait(timeout=2)
result=dict(argv=argv,child_exit_code=code,timed_out=timed_out,interruption=interruption,elapsed_seconds=time.monotonic()-started,
            binary_sha256=before,binary_sha256_after=hashlib.sha256(binary.read_bytes()).hexdigest(),
            log_sha256=hashlib.sha256(log.read_bytes()).hexdigest(),metal_debug_layer='1',
            structurally_valid=None,validated_for_governor=False)
with receipt.open('x') as stream:json.dump(result,stream,indent=2)
sys.exit(0 if code==0 and not timed_out and not interruption and result['binary_sha256_after']==before else 1)
PY
python3 tools/render-timing-probe/analyze_proxy.py \
  --run /private/tmp/proxy-spatial-off-02 \
  --out /private/tmp/proxy-spatial-off-02.analysis.json
```

The sample receipt deliberately leaves structural validity unknown until the
analyzer completes; never rewrite a failed execution receipt as a pass. A
separate combined review may reference the execution, analyzer and PNG hashes.
Capture any unrelated active GPU/CPU work in the parent receipt. Do not dump
all environment variables: retain only explicit measurement-related values.

First run **Spatial off**, then **Spatial calls** with the same frozen binary
and configuration. Analyze calls with `--reference` pointing to the off run;
all 16 exact raw and composed pixel hashes must agree. If proxy calls, labels,
status or output parity fail, preserve the failure and stop the first attempt.
Do not conceal an unknown selector by modifying the allowlist after seeing it.
Only after review of compatible calls proceed to the second attempt,
**Spatial counters**, again comparing to off. Repeat this same sequence for
Temporal before considering an Ushas integration. A known-unsupported mode is
an unavailable result, not a missing run that can be counted as successful.

`analyze_proxy.py` reads original evidence only. `--out` requires a new JSON
file; omission prints JSON. It fails closed on malformed/missing records,
reused slots, noncontiguous frames, unknown selectors, ambiguous labels,
missing callbacks, stale delivery, failed command buffers, missing raw/PNG
files, or mismatched pixel hashes. Counter intervals are unioned across
encoders and frames; overlapping stage durations are never summed as busy
time. Local raw counter pairs do not impose fullscreen-triangle ordering on
arbitrary MetalFX render passes. No successful analyzer result establishes
trace completeness or an instrumentation-overhead threshold.

## Subsequent trace gate

A counters run must reconcile every target-process encoder, including setup
and final readback. Expected per-frame setup labels are `scene-input` and
`output-clear`, plus Temporal `depth-input`, `motion-input`, `exposure-input`;
final labels are `compose` and `readback`. The dedicated MetalFX buffer label is
`proxy/frame=F/view=1/epoch=1/slot=S/gen=G/metalfx`; its internal encoder labels
must remain the framework's actual unique labels from the observation ledger.
The trace must also inventory process work outside those labeled buffers,
including any framework-created device/queue work. Unattributed work prevents
a complete scope claim. Do not fit GPU timestamps or CPU creation order to
invent missing ownership. Retain endpoint residuals, Idle, driver waits,
cross-frame and other-process overlap, and actual sample delivery ages.

The completed native `StageProbe` evidence remains a separate experiment.
Even a successful MetalFX proxy trace only clears the way to a reviewed,
isolated Bevy/wgpu ownership patch and full rendered-fixture validation.
