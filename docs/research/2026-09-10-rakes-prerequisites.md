# Rakes integration prerequisites, 2026-09-10

Base: `5b150ba7d7989aaea7e44c7b2cf2680332ae048c`.

The plugin now installs `MetalFxHistoryReset` before disabled/unsupported
returns. Temporal cameras enforce single-sampled prepasses in `PostUpdate`,
including cameras already carrying every prepass component and subsequent
MSAA settings changes. Depth and motion resolve bindings use source texture
identity; views and bindings are created only on a cache miss. The binding owns
its source view. Destination render attachments are not binding inputs.

Verification on Apple M5 Max/macOS 26.5.2:

- Before the lifecycle fixes: 127 existing library tests passed, and both new
  lifecycle regressions failed with the expected missing-resource/MSAA errors.
- After: `cargo test --locked --lib --all-features --quiet`: 129 passed,
  one explicit hardware test ignored.
- `cargo test --locked --lib --no-default-features --quiet`: 108 passed,
  one explicit hardware test ignored.
- `cargo clippy --locked --lib --all-features -- -D warnings`: passed.
- Explicit real-device test, which failed before the cache fix and passed after:
  `cargo test --locked --lib --all-features resolve_bindings_reuse -- --ignored --nocapture`.
  It resolves both depth and motion repeatedly with one source, then checks
  invalidation for a same-size replacement. A missing Metal adapter fails this
  explicit invocation; it is not reported as a pass.
- Release smoke binary rebuilt successfully. Native 3024 × 1964 display smoke
  failed while the display was asleep and session locked: zero measured frames,
  all-zero readback. No passing render or performance claim follows.

Independent source review through the governed `validation` seat returned PASS.
It checked the current diff and relied on the executor's test results. Policy
SHA256: `f39be8adfc2f14271b3f5f83a1a6ba9bf875a1c200494e9f0dfc0651d37f9ceb`.
The first review returned only progress and was not counted as approval.

Remaining boundaries: non-macOS execution remains unverified. The cache has one
slot per input, so double-buffered prepasses can still alternate misses. The
plugin's existing single-active-view contract remains. Application systems that
change MSAA after enforcement can still conflict with temporal requirements.
Experimental presentation ownership/resize/synchronization repair and validated
full-frame CPU/GPU/presentation measurement remain separate prerequisites.
Adaptive scaling and interpolation were not enabled or changed by this patch.

A fleet CI migration task remains pending: registration is disabled and has no
required checks or isolated guest evidence. This patch does not migrate CI or
authorize publication.
