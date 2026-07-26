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
#   ./verify-published.sh            # against the published 0.2 on crates.io
#   ./verify-published.sh --packaged # against the .crate tarball (pre-publish)
#
# --packaged repackages and extracts the tarball itself rather than reusing
# target/package/<name>-<ver>/. That directory and the .crate beside it can fall
# out of sync — `cargo publish --dry-run` refreshes the directory but leaves a
# stale tarball — and the tarball is what actually gets uploaded.
#
# Exits non-zero on the first failure.

set -euo pipefail

VERSION="${BEVY_METALFX_VERSION:-0.2}"
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

if [[ "${1:-}" == "--packaged" ]]; then
    CRATE_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$CRATE_DIR/Cargo.toml" | head -1)"
    echo "==> repackaging bevy_metalfx $CRATE_VERSION"
    ( cd "$REPO_ROOT" && cargo package -p bevy_metalfx --quiet )
    TARBALL="$REPO_ROOT/target/package/bevy_metalfx-$CRATE_VERSION.crate"
    [[ -f "$TARBALL" ]] || { echo "no tarball at $TARBALL" >&2; exit 2; }

    EXTRACT="$(mktemp -d)"
    trap 'rm -rf "$EXTRACT"' EXIT
    tar xzf "$TARBALL" -C "$EXTRACT"
    PKG="$EXTRACT/bevy_metalfx-$CRATE_VERSION"

    echo "    sha256 $(shasum -a 256 "$TARBALL" | awk '{print $1}')"
    echo "    $(find "$PKG" -type f | wc -l | tr -d ' ') files"
    for required in LICENSE-MIT LICENSE-APACHE README.md Cargo.toml; do
        [[ -f "$PKG/$required" ]] || { echo "MISSING from tarball: $required" >&2; exit 1; }
    done
    DEP="bevy_metalfx = { path = \"$PKG\" }"
    SOURCE="tarball $TARBALL"
else
    DEP="bevy_metalfx = \"$VERSION\""
    SOURCE="crates.io $VERSION"
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
bevy = { version = "0.18", default-features = false, features = [
  "bevy_camera", "bevy_render", "bevy_core_pipeline", "bevy_window",
] }
$DEP

[features]
temporal = ["bevy_metalfx/temporal"]
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
        app.add_plugins(MinimalPlugins).add_plugins(plugin);

        // Public accessor added in 0.2 — reports the mode that survived any
        // runtime fallback, which is the whole reason it is readable.
        let reported = app
            .world()
            .get_resource::<bevy_metalfx::MetalFxModeResource>()
            .map(|m| m.get());
        assert_eq!(reported, Some(mode), "{label}: plugin did not report its mode");
        println!("  {label}: OK (reported {reported:?})");
    }
    println!("OK: public API consumed from outside the workspace");
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
                 "--features frame-interpolation"; do
        # shellcheck disable=SC2086
        ( cd "$REPO_ROOT" && cargo check -p bevy_metalfx --target "$CROSS_TARGET" $feats --quiet )
        echo "    ok: ${feats:-default}"
    done
else
    echo "==> skipping cross-check: rustup target add $CROSS_TARGET"
fi

echo "==> verifying bevy_metalfx from $SOURCE"
echo "--- spatial (default features) ---"
cargo run --quiet --manifest-path "$WORK/Cargo.toml"
echo "--- temporal ---"
cargo run --quiet --manifest-path "$WORK/Cargo.toml" --features temporal
echo "==> PASS"
