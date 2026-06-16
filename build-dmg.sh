#!/usr/bin/env bash
# Build dist/AtlasDictation-<version>.dmg — a draggable installer disk image
# containing AtlasDictation.app and a shortcut to /Applications.
# Users mount the DMG, drag the app onto the Applications alias, eject. Done.
set -euo pipefail
cd "$(dirname "$0")"

# Rebuild the app first so the DMG is always up to date with current source.
./build-app.sh

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' packaging/Info.plist)"
APP="dist/AtlasDictation.app"
DMG="dist/AtlasDictation-${VERSION}.dmg"
VOL="Atlas Dictation"

STAGING="$(mktemp -d)/dmg"
mkdir -p "$STAGING"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"

rm -f "$DMG"
hdiutil create \
    -volname "$VOL" \
    -srcfolder "$STAGING" \
    -ov -format UDZO \
    "$DMG" >/dev/null

rm -rf "$STAGING"

echo "Built $DMG ($(du -h "$DMG" | cut -f1))"
echo "Mount: open '$DMG'"
