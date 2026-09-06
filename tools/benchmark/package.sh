#!/bin/bash
# Package a prebuilt, frozen renderer. No renderer is executed by this script.
set -euo pipefail

package_root="$(cd "$(dirname "$0")" && pwd)"
renderer_path=""
output_root="$package_root/dist"
swift_configuration=release
scratch_root="${USHAS_SWIFT_BUILD_DIR:-${TMPDIR:-/tmp}/ushas-bench-swift-build}"
usage() {
  cat <<'USAGE'
Usage: package.sh [--binary /path/to/ushas-bench] [--out /new/package/folder]
                  [--configuration release|debug]

Builds the Swift launcher and packages an existing arm64 renderer. Without
--binary, looks in ${CARGO_TARGET_DIR:-tools/benchmark/target}/release.
The output folder must not exist. Produces Ushas Bench.app and Ushas Bench.zip.
Requires Xcode 26. This is an ad-hoc-signed preview, without notarization.
USAGE
}
while (($#)); do
  case "$1" in
    --binary|--out|--configuration)
      (($# >= 2)) || { usage >&2; exit 2; }
      case "$1" in
        --binary) renderer_path="$2" ;;
        --out) output_root="$2" ;;
        --configuration) swift_configuration="$2" ;;
      esac
      shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
[[ "$swift_configuration" == release || "$swift_configuration" == debug ]] || { usage >&2; exit 2; }
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'Packaging requires Apple Silicon macOS.' >&2; exit 1; }
if [[ -z "$renderer_path" ]]; then
  renderer_path="${CARGO_TARGET_DIR:-$package_root/target}/release/ushas-bench"
fi
[[ -f "$renderer_path" && -x "$renderer_path" ]] || {
  echo "Prebuilt renderer missing: $renderer_path" >&2
  echo 'Build it with cargo build --release --locked --manifest-path tools/benchmark/Cargo.toml, or pass --binary.' >&2
  exit 1
}
[[ ! -e "$output_root" && ! -L "$output_root" ]] || { echo 'Choose a new output folder; existing packages are preserved.' >&2; exit 1; }
renderer_path="$(cd "$(dirname "$renderer_path")" && pwd)/$(basename "$renderer_path")"
/usr/bin/lipo "$renderer_path" -verify_arch arm64
if [[ -z "${DEVELOPER_DIR:-}" && -d /Applications/Xcode.app/Contents/Developer ]]; then
  export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
fi
export SWIFTPM_MODULECACHE_OVERRIDE="$scratch_root/module-cache"
export CLANG_MODULE_CACHE_PATH="$scratch_root/clang-cache"
mkdir -p "$scratch_root" "$scratch_root/cache" "$scratch_root/configuration" "$scratch_root/security"
swift_args=(--package-path "$package_root/macos" --scratch-path "$scratch_root/build"
  --cache-path "$scratch_root/cache" --config-path "$scratch_root/configuration"
  --security-path "$scratch_root/security" --disable-sandbox --configuration "$swift_configuration" --arch arm64)
/usr/bin/xcrun swift build "${swift_args[@]}" --product UshasBench
/usr/bin/xcrun swift build "${swift_args[@]}" --product ushas-video-encoder
swift_binary_dir="$(/usr/bin/xcrun swift build "${swift_args[@]}" --show-bin-path)"
mkdir -p "$output_root"
output_root="$(cd "$output_root" && pwd)"
app_path="$output_root/Ushas Bench.app"
mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Helpers" "$app_path/Contents/Resources"
/bin/cp -f "$swift_binary_dir/UshasBench" "$app_path/Contents/MacOS/UshasBench"
/bin/cp -f "$renderer_path" "$app_path/Contents/Helpers/ushas-bench"
/bin/cp -f "$swift_binary_dir/ushas-video-encoder" "$app_path/Contents/Helpers/ushas-video-encoder"
/bin/chmod 755 "$app_path/Contents/MacOS/UshasBench" "$app_path/Contents/Helpers/ushas-bench" "$app_path/Contents/Helpers/ushas-video-encoder"
/bin/cp -f "$package_root/../claude-model/CHARACTER.md" "$app_path/Contents/Resources/CHARACTER.md"
/bin/cp -f "$package_root/../../LICENSE-MIT" "$app_path/Contents/Resources/LICENSE-MIT"
/bin/cp -f "$package_root/../../LICENSE-APACHE" "$app_path/Contents/Resources/LICENSE-APACHE"
/usr/bin/xcrun swift "$package_root/macos/Support/MakeIcon.swift" "$scratch_root/UshasBench.iconset"
/usr/bin/iconutil -c icns "$scratch_root/UshasBench.iconset" -o "$app_path/Contents/Resources/UshasBench.icns"
/bin/cat > "$app_path/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.ushas.bench</string>
<key>CFBundleName</key><string>Ushas Bench</string>
<key>CFBundleDisplayName</key><string>Ushas Bench</string>
<key>CFBundleExecutable</key><string>UshasBench</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.0</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleIconFile</key><string>UshasBench</string>
<key>LSMinimumSystemVersion</key><string>26.0</string>
<key>LSArchitecturePriority</key><array><string>arm64</string></array>
<key>NSHighResolutionCapable</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict></plist>
PLIST
/usr/bin/plutil -lint "$app_path/Contents/Info.plist"
# Sign nested executable first, then seal its containing app.
/usr/bin/codesign --force --sign - --timestamp=none "$app_path/Contents/Helpers/ushas-bench"
/usr/bin/codesign --force --sign - --timestamp=none "$app_path/Contents/Helpers/ushas-video-encoder"
/usr/bin/codesign --force --sign - --timestamp=none "$app_path/Contents/MacOS/UshasBench"
/usr/bin/codesign --force --sign - --timestamp=none "$app_path"
/usr/bin/codesign --verify --deep --strict "$app_path"
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app_path" "$output_root/Ushas Bench.zip"
(
  cd "$output_root"
  /usr/bin/shasum -a 256 "Ushas Bench.app/Contents/Helpers/ushas-bench" "Ushas Bench.app/Contents/Helpers/ushas-video-encoder" "Ushas Bench.app/Contents/MacOS/UshasBench" "Ushas Bench.zip" > SHA256SUMS
)
echo "Packaged: $app_path"
echo "Archive:  $output_root/Ushas Bench.zip"
