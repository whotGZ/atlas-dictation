#!/usr/bin/env bash
# Build packaging/AppIcon.icns from assets/icon.svg using macOS built-in tools
# (qlmanage for SVG->PNG, sips for resize, iconutil for ICNS).
# Run when icon.svg changes; build-app.sh just copies the result into the bundle.
set -euo pipefail
cd "$(dirname "$0")"

SVG="assets/icon.svg"
ICONSET="$(mktemp -d)/AppIcon.iconset"
OUT="packaging/AppIcon.icns"

mkdir -p "$ICONSET"

# Master render at 1024x1024.
MASTER="$ICONSET/master_1024.png"
qlmanage -t -s 1024 -o "$ICONSET" "$SVG" >/dev/null 2>&1
mv "$ICONSET/$(basename "$SVG").png" "$MASTER"

# Generate every size Apple's iconset expects.
for SPEC in \
    "16:icon_16x16.png" \
    "32:icon_16x16@2x.png" \
    "32:icon_32x32.png" \
    "64:icon_32x32@2x.png" \
    "128:icon_128x128.png" \
    "256:icon_128x128@2x.png" \
    "256:icon_256x256.png" \
    "512:icon_256x256@2x.png" \
    "512:icon_512x512.png" \
    "1024:icon_512x512@2x.png" ; do
    SIZE="${SPEC%%:*}"
    NAME="${SPEC##*:}"
    sips -z "$SIZE" "$SIZE" "$MASTER" --out "$ICONSET/$NAME" >/dev/null
done

rm "$MASTER"
iconutil --convert icns "$ICONSET" --output "$OUT"
rm -rf "$ICONSET"

echo "Built $OUT ($(du -h "$OUT" | cut -f1))"
