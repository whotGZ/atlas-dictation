#!/usr/bin/env bash
# Build AtlasDictation.app — bundles the release binary, Whisper Turbo model,
# and medical dictionary into a single .app at dist/AtlasDictation.app.
# Drag the result into /Applications, double-click to launch.
set -euo pipefail
cd "$(dirname "$0")"

APP="dist/AtlasDictation.app"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp target/release/atlas-dictation "$APP/Contents/MacOS/atlas-dictation"
cp packaging/Info.plist             "$APP/Contents/Info.plist"
cp models/ggml-large-v3-turbo.bin   "$APP/Contents/Resources/ggml-large-v3-turbo.bin"
cp assets/medical-dictionary.txt    "$APP/Contents/Resources/medical-dictionary.txt"

# Ad-hoc sign so Gatekeeper doesn't reject the freshly built bundle outright.
# Replace with a Developer ID signature for distributed builds.
codesign --force --deep --sign - "$APP" 2>/dev/null || true

echo
echo "Built $APP"
echo "Try it: open '$APP'"
