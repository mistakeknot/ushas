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
#   ./verify-published.sh --packaged # against ../../../target/package/... (pre-publish)
#
# Exits non-zero on the first failure.

set -euo pipefail

VERSION="${BEVY_METALFX_VERSION:-0.2}"
CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$CRATE_DIR/../.." && pwd)"

if [[ "${1:-}" == "--packaged" ]]; then
    PKG="$REPO_ROOT/target/package/bevy_metalfx-0.2.0"
    [[ -d "$PKG" ]] || {
        echo "no packaged artifact at $PKG — run: cargo package -p bevy_metalfx" >&2
        exit 2
    }
    DEP="bevy_metalfx = { path = \"$PKG\" }"
    SOURCE="packaged artifact at $PKG"
else
    DEP="bevy_metalfx = \"$VERSION\""
    SOURCE="crates.io $VERSION"
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
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

echo "==> verifying bevy_metalfx from $SOURCE"
echo "--- spatial (default features) ---"
cargo run --quiet --manifest-path "$WORK/Cargo.toml"
echo "--- temporal ---"
cargo run --quiet --manifest-path "$WORK/Cargo.toml" --features temporal
echo "==> PASS"
