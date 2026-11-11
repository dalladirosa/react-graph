#!/usr/bin/env bash
# Packages target/release/rendermap into "Rendermap.app" and a drag-to-Applications .dmg.
# Usage (on macOS, after `cargo build --release`): installer/macos/bundle.sh <version> [arch]
# Writes dist/Rendermap.app and dist/rendermap-<version>-macos-<arch>.dmg.
set -euo pipefail

version="${1:?usage: bundle.sh <version> [arch]}"
arch="${2:-arm64}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
dist="$root/dist"
app="$dist/Rendermap.app"

rm -rf "$app" && mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/target/release/rendermap" "$app/Contents/MacOS/rendermap"
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

# With MACOS_SIGN_IDENTITY ("Developer ID Application: …") the app is signed for notarization;
# without it, an ad-hoc signature (enough to run on Apple Silicon, but Gatekeeper asks to "Open Anyway").
identity="${MACOS_SIGN_IDENTITY:-}"
if [ -n "$identity" ]; then
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
else
  codesign --force --deep --sign - "$app"
fi

# install window: background with an arrow, the app and an Applications shortcut (needs `pip install dmgbuild`)
dmg="$dist/rendermap-$version-macos-$arch.dmg"
background="$dist/dmg-background.tiff"
tiffutil -cathidpicheck "$root/installer/macos/dmg-background.png" \
  "$root/installer/macos/dmg-background@2x.png" -out "$background"
rm -f "$dmg"
python3 -m dmgbuild -s "$root/installer/macos/dmg_settings.py" -D app="$app" -D background="$background" "Rendermap" "$dmg"
rm -f "$background"

# Notarize so it opens without the Gatekeeper warning (needs the Developer ID signature above)
if [ -n "$identity" ] && [ -n "${APPLE_ID:-}" ]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
  xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "${APPLE_TEAM_ID:?}" \
    --password "${APPLE_APP_PASSWORD:?}" --wait
  xcrun stapler staple "$dmg"
fi
echo "Built $dmg"
