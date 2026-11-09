#!/usr/bin/env bash
# Packages target/release/react-graph into "React Graph.app" and a drag-to-Applications .dmg.
# Usage (on macOS, after `cargo build --release`): installer/macos/bundle.sh <version> [arch]
# Writes dist/React Graph.app and dist/react-graph-<version>-macos-<arch>.dmg.
set -euo pipefail

version="${1:?usage: bundle.sh <version> [arch]}"
arch="${2:-arm64}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
dist="$root/dist"
app="$dist/React Graph.app"

rm -rf "$app" "$dist/dmg" && mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/react-graph" "$app/Contents/MacOS/react-graph"
sed "s/@VERSION@/$version/g" "$root/installer/macos/Info.plist" > "$app/Contents/Info.plist"

# app icon: every size iconutil wants, from the 512px PNG
iconset="$dist/AppIcon.iconset"
rm -rf "$iconset" && mkdir -p "$iconset"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$root/assets/icon.png" --out "$iconset/icon_${s}x${s}.png" >/dev/null
  if [ "$s" -le 256 ]; then
    d=$((s * 2))
    sips -z "$d" "$d" "$root/assets/icon.png" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
  fi
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

# ad-hoc signature: required to run on Apple Silicon (not notarized)
codesign --force --deep --sign - "$app"

# disk image with an Applications shortcut to drag the app onto
mkdir -p "$dist/dmg"
cp -R "$app" "$dist/dmg/"
ln -s /Applications "$dist/dmg/Applications"
hdiutil create -volname "React Graph" -srcfolder "$dist/dmg" -ov -format UDZO \
  "$dist/react-graph-$version-macos-$arch.dmg"
rm -rf "$dist/dmg"
echo "Built $dist/react-graph-$version-macos-$arch.dmg"
