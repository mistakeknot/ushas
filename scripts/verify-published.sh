#!/usr/bin/env bash
# Verify a published bevy_metalfx by consuming it the way a downstream crate
# does: a throwaway package outside this workspace, depending on the registry
# artifact, touching only the public API.
#
# The point is to catch what an in-workspace `cargo test` structurally cannot —
# a file that exists on disk but was excluded from the package, an item that is
# reachable inside the crate but not from outside, a feature that resolves only
# because a sibling workspace member enabled it.
#
#   ./verify-published.sh            # against the published 0.4 on crates.io
#   ./verify-published.sh --packaged # against the .crate tarball (pre-publish)
#
# --packaged repackages and extracts the tarball itself rather than reusing
# target/package/<name>-<ver>/. That directory and the .crate beside it can fall
# out of sync — `cargo publish --dry-run` refreshes the directory but leaves a
# stale tarball — and the tarball is what actually gets uploaded.
#
# Exits non-zero on the first failure.

set -euo pipefail

VERSION="${BEVY_METALFX_VERSION:-0.4}"
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The crate is the repository root here. It was two levels down when this crate
# lived in a monorepo, and the two paths are the same directory now -- keep both
# names so the `cd "$REPO_ROOT" && cargo ... -p bevy_metalfx` calls below still
# read as "run from where the manifest cargo resolves against lives".
REPO_ROOT="$CRATE_DIR"

if [[ "${1:-}" == "--packaged" ]]; then
    # One awk, no pipe: `sed ... | head -1` under `set -o pipefail` returns 141
    # whenever the producer outstruns the pipe buffer, and this script also runs
    # `set -e`. Cargo.toml is small enough today that it never fires -- which is
    # a fact about the file, not about the code.
    CRATE_VERSION="$(awk -F'"' '/^version = /{print $2; exit}' "$CRATE_DIR/Cargo.toml")"
    echo "==> repackaging bevy_metalfx $CRATE_VERSION"
    ( cd "$REPO_ROOT" && cargo package -p bevy_metalfx --target-dir "$REPO_ROOT/target" --quiet )
    TARBALL="$REPO_ROOT/target/package/bevy_metalfx-$CRATE_VERSION.crate"
    [[ -f "$TARBALL" ]] || { echo "no tarball at $TARBALL" >&2; exit 2; }

    EXTRACT="$(mktemp -d)"
    trap 'rm -rf "$EXTRACT"' EXIT
    tar xzf "$TARBALL" -C "$EXTRACT"
    PKG="$EXTRACT/bevy_metalfx-$CRATE_VERSION"

    echo "    sha256 $(shasum -a 256 "$TARBALL" | awk '{print $1}')"
    echo "    $(find "$PKG" -type f | wc -l | tr -d ' ') files"
    for required in LICENSE-MIT LICENSE-APACHE README.md CHANGELOG.md Cargo.toml; do
        [[ -f "$PKG/$required" ]] || { echo "MISSING from tarball: $required" >&2; exit 1; }
    done

    # Assert the shipped SOURCE, not the working tree. `cargo package` leaves an
    # extracted directory and a .crate beside it and they drift — `--dry-run`
    # refreshes the directory but can leave a stale tarball, and only the
    # tarball uploads. Without this, a stale tarball publishes the PREVIOUS
    # release's bytes under the new version number: the manifest reads 0.3.0,
    # the consumer check passes, and the breaking change simply is not in it.
    #
    # Each entry is "description<TAB>grep -E pattern"; a `!` prefix asserts the
    # pattern is ABSENT. Update these when the release changes public API.
    echo "==> asserting the candidate API surface is in the tarball"
    API_CHECKS=(
        # 0.3 surface — kept, because a regression here is exactly the stale-
        # tarball failure these assertions exist to catch.
        "counts() returns five u64	pub fn counts\(&self\) -> \(u64, u64, u64, u64, u64\)"
        "PresentStats exposes presented_fps	pub presented_fps: f32"
        "!no interp_fps anywhere	interp_fps"
        # 0.4 surface.
        "MetalFxScaleRange is public	pub struct MetalFxScaleRange"
        "scale range converts to upscale ratios	pub fn as_upscale_ratios"
        "MetalFxHistoryReset is public	pub struct MetalFxHistoryReset"
        "history reset can be requested	pub fn request\(&mut self\)"
        "the pass is a system, not a ViewNode	pub fn metalfx_upscale"
        "!no ViewNode impl survives	impl ViewNode for"
        "!no render_graph imports survive	bevy::render::render_graph"
        # 0.4.2 — the resolution-override fix and the retained device handle.
        # Neither changes public API, so the "update these when the release
        # changes public API" rule above would not have added anything here.
        # They are asserted for the other reason this list exists: a stale
        # tarball would upload 0.4.1's bytes under 0.4.2's version number, and
        # every check above would still pass while the release's entire point
        # was missing.
        "resolution override reaches the render world	fn extract_resolution_override"
        "detached scaler threads take a retained device	struct SendDevice"
        "!no unretained pointer crosses a thread boundary	struct SendablePtr"
        # 0.4.1 — the crash fix. 0.4.0 panicked on the first MetalFX encode
        # because the raw Metal work shared a command encoder with wgpu calls,
        # which wgpu 29 forbids at runtime. The dedicated encoder is the fix, so
        # assert it is in the bytes that upload rather than only in the tree.
        "raw encoding gets its own command encoder	metalfx_raw_encode"
        "!raw encode never rides the context encoder	let encoder = render_context.command_encoder\(\);"
    )
    API_CHECKS+=(
        "effect observations are public	pub struct MetalFxEffectStatus"
        "pure adaptive controller is public	pub struct AdaptiveController"
        "validated sample boundary is public	pub struct ValidatedGpuFrameCost"
        "adaptive status is public	pub struct MetalFxAdaptiveStatus"
        "history has durable acknowledgements	acknowledge"
        "diagnostic creation controls are public	pub struct MetalFxDiagnosticFault"
        "diagnostic creation outcomes are explicit	pub enum ScalerCreationFault"
        "interpolation uses real time	fn update_frame_timing\(time: Res<Time<Real>>"
        "!legacy app-time governor is absent	pub struct AdaptiveScaleState"
    )
    for check in "${API_CHECKS[@]}"; do
        desc="${check%%$'\t'*}"; pat="${check#*$'\t'}"
        if [[ "$desc" == !* ]]; then
            if grep -rEq -- "$pat" "$PKG/src"; then
                echo "API CHECK FAILED (${desc#!}): found /$pat/ in the tarball" >&2
                grep -rEn -- "$pat" "$PKG/src" >&2
                exit 1
            fi
        elif ! grep -rEq -- "$pat" "$PKG/src"; then
            echo "API CHECK FAILED ($desc): /$pat/ not in the tarball" >&2
            exit 1
        fi
        echo "    ok: ${desc#!}"
    done
    DEP="bevy_metalfx = { path = \"$PKG\" }"
    SOURCE="tarball $TARBALL"
    CANDIDATE_FEATURES="candidate"
    DIAGNOSTIC_DEPENDENCY='"bevy_metalfx/diagnostic-fault-injection"'
else
    DEP="bevy_metalfx = \"$VERSION\""
    SOURCE="crates.io $VERSION"
    CANDIDATE_FEATURES=""
    DIAGNOSTIC_DEPENDENCY=''
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK" "${EXTRACT:-}"' EXIT
mkdir -p "$WORK/src"

cat > "$WORK/Cargo.toml" <<EOF
[package]
name = "bevy-metalfx-consumer-check"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
bevy = { version = "0.19", default-features = false, features = [
  "bevy_camera", "bevy_render", "bevy_core_pipeline", "bevy_window",
] }
$DEP

[features]
temporal = ["bevy_metalfx/temporal"]
candidate = []
diagnostic = [$DIAGNOSTIC_DEPENDENCY]
EOF

cat > "$WORK/src/main.rs" <<'EOF'
use bevy::prelude::*;
use bevy_metalfx::{MetalFxMode, MetalFxPlugin};

fn main() {
    println!("is_available = {}", bevy_metalfx::is_available());
    for (label, mode) in [
        ("spatial", MetalFxMode::Spatial),
        #[cfg(feature = "temporal")]
        ("temporal", MetalFxMode::Temporal),
    ] {
        // `..default()` is the documented construction path and the reason the
        // plugin fields added in 0.2 are not a break for callers who use it.
        let plugin = MetalFxPlugin { render_scale: 0.5, mode, ..default() };
        assert_eq!(plugin.mode, mode);

        let mut app = App::new();
        #[cfg(feature = "candidate")]
        app.insert_resource(bevy_metalfx::MetalFxAdaptiveConfig {
            target: bevy_metalfx::MetalFxAdaptiveTarget::Explicit(60.0),
            minimum_scale: 0.5,
            ..default()
        });
        app.add_plugins(MinimalPlugins).add_plugins(plugin);
        #[cfg(feature = "candidate")]
        verify_candidate(&app);
        #[cfg(all(feature = "candidate", feature = "diagnostic"))]
        verify_diagnostic(&mut app);

        // Public accessor added in 0.2 — reports the mode that survived any
        // runtime fallback, which is the whole reason it is readable.
        let reported = app
            .world()
            .get_resource::<bevy_metalfx::MetalFxModeResource>()
            .map(|m| m.get());
        assert_eq!(reported, Some(mode), "{label}: plugin did not report its mode");

        // 0.4 surface, exercised from outside the crate: the scale band must be
        // published and must contain the scale the plugin was built with,
        // otherwise `contains()` would reject the app's own configuration.
        let range = app
            .world()
            .get_resource::<bevy_metalfx::MetalFxScaleRange>()
            .copied()
            .expect("MetalFxScaleRange must be inserted by the plugin");
        assert!(
            range.contains(0.5),
            "{label}: configured scale 0.5 outside the reported band {:?}",
            range.as_range()
        );
        assert!(
            *range.as_upscale_ratios().start() >= 1.0,
            "{label}: upscale ratio below 1.0 would make the scaler nil"
        );

        // And the reset request must be reachable and default to off.
        let mut reset = app
            .world_mut()
            .get_resource_mut::<bevy_metalfx::MetalFxHistoryReset>()
            .expect("MetalFxHistoryReset must be inserted by the plugin");
        assert!(!reset.is_requested(), "{label}: reset must default to off");
        reset.request();
        assert!(reset.is_requested(), "{label}: request() had no effect");

        println!("  {label}: OK (reported {reported:?}, band {:?})", range.as_range());
    }
    println!("OK: public API consumed from outside the workspace");
}

#[cfg(feature = "candidate")]
fn verify_candidate(app: &App) {
    use bevy_metalfx::{MetalFxAdaptiveConfig, MetalFxAdaptiveContext, MetalFxAdaptiveStatus,
        MetalFxAdaptiveTarget, MetalFxAdaptiveTargetSource,
        MetalFxEffectState, MetalFxEffectStatus, MetalFxFrameCostInput};
    use bevy_metalfx::adaptive::{AdaptiveConfig, AdaptiveController};
    let controller = AdaptiveController::new(AdaptiveConfig::default(), vec![0.5, 1.0], 1.0).unwrap();
    assert_eq!(controller.current_scale(), 1.0);
    assert_eq!(MetalFxAdaptiveConfig::default().target, MetalFxAdaptiveTarget::Monitor);
    let configured = app.world().resource::<MetalFxAdaptiveConfig>();
    assert_eq!(configured.target, MetalFxAdaptiveTarget::Explicit(60.0));
    assert_eq!(configured.minimum_scale, 0.5);
    assert_eq!(MetalFxAdaptiveTargetSource::default(), MetalFxAdaptiveTargetSource::Unresolved);
    let effects = app.world().resource::<MetalFxEffectStatus>();
    let missing = effects.snapshot(42, 1);
    assert_eq!(missing.state(), MetalFxEffectState::NoRender);
    assert!(!missing.is_fresh(2, std::time::Duration::from_millis(500)));
    assert!(app.world().contains_resource::<MetalFxAdaptiveStatus>());
    assert!(app.world().contains_resource::<MetalFxAdaptiveContext>());
    assert!(app.world().resource::<MetalFxFrameCostInput>().latest(42).is_none());
}

#[cfg(all(feature = "candidate", feature = "diagnostic"))]
fn verify_diagnostic(app: &mut App) {
    use bevy_metalfx::{MetalFxDiagnosticFault, ScalerCreationFault};
    let mut control = app.world_mut().resource_mut::<MetalFxDiagnosticFault>();
    assert_eq!(control.snapshot().fault, ScalerCreationFault::Off);
    let original = control.snapshot();
    control.set(ScalerCreationFault::HoldPending);
    let held = control.snapshot();
    assert_eq!(held.fault, ScalerCreationFault::HoldPending);
    assert!(held.generation > original.generation);
    control.clear();
    assert_eq!(control.snapshot().fault, ScalerCreationFault::Off);
    assert!(control.snapshot().generation > held.generation);
}
EOF

# Cross-check the non-macOS path. docs.rs builds on Linux, and a macOS-only
# crate is exactly the kind that never gets a non-macOS build until a user
# reports one. `--features temporal` was broken on every non-macOS target from
# 0.1.0 until this check was added: `mod jitter` carried a spurious
# `target_os = "macos"` while its caller was gated on the feature alone.
CROSS_TARGET="${CROSS_TARGET:-x86_64-unknown-linux-gnu}"
if rustup target list --installed 2>/dev/null | grep -qx "$CROSS_TARGET"; then
    echo "==> cross-checking $CROSS_TARGET (the docs.rs path)"
    for feats in "--no-default-features --features spatial" "" "--features temporal" \
                 "--features frame-interpolation" "--features diagnostic-fault-injection"; do
        # shellcheck disable=SC2086
        cargo check --manifest-path "${PKG:-$REPO_ROOT}/Cargo.toml" --target "$CROSS_TARGET" $feats --quiet
        echo "    ok: ${feats:-default}"
    done
else
    echo "==> skipping cross-check: rustup target add $CROSS_TARGET"
fi

echo "==> verifying bevy_metalfx from $SOURCE"
echo "--- spatial (default features) ---"
RUN_ARGS=(--quiet --manifest-path "$WORK/Cargo.toml")
if [[ -n "$CANDIDATE_FEATURES" ]]; then
    RUN_ARGS+=(--features "$CANDIDATE_FEATURES")
fi
cargo run "${RUN_ARGS[@]}"
echo "--- temporal ---"
cargo run --quiet --manifest-path "$WORK/Cargo.toml" --features "temporal${CANDIDATE_FEATURES:+,$CANDIDATE_FEATURES}"
if [[ -n "$CANDIDATE_FEATURES" ]]; then
    echo "--- opt-in diagnostic API ---"
    cargo run --quiet --manifest-path "$WORK/Cargo.toml" --features candidate,diagnostic
fi
echo "==> PASS"
