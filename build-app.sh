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
cp packaging/AppIcon.icns           "$APP/Contents/Resources/AppIcon.icns"
cp models/ggml-large-v3-turbo.bin   "$APP/Contents/Resources/ggml-large-v3-turbo.bin"
cp assets/medical-dictionary.txt    "$APP/Contents/Resources/medical-dictionary.txt"
# VAD model intentionally NOT bundled (v0.4.2): the Silero gate was clipping real
# speech to silence on some mics. Code still supports it (model-gated) — re-add
# this cp line once the threshold is tuned and re-tested. See DEVLOG v0.4.2.

# Sign with a stable identity so the TCC Accessibility grant survives rebuilds.
# Prefers the self-signed local cert; falls back to ad-hoc if absent.
# Replace with a Developer ID signature for distributed builds.
SIGN_IDENTITY="${ATLAS_SIGN_IDENTITY:-ATLAS Local dev}"
if security find-identity -p codesigning -v | grep -q "$SIGN_IDENTITY"; then
    codesign --force --deep --sign "$SIGN_IDENTITY" "$APP"
    echo "Signed with: $SIGN_IDENTITY"
else
    codesign --force --deep --sign - "$APP" 2>/dev/null || true
    echo "WARNING: '$SIGN_IDENTITY' not found in keychain — fell back to ad-hoc."
    echo "         TCC Accessibility grant will be lost on next rebuild."
fi

echo
echo "Built $APP"
echo "Try it: open '$APP'"
