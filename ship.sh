#!/usr/bin/env bash
# One-shot publish: push commits, tag the version from Info.plist, attach the
# matching .dmg to a GitHub Release. Requires `gh auth login` already done and
# the repo to already exist (origin remote set).
set -euo pipefail
cd "$(dirname "$0")"

if ! gh auth status >/dev/null 2>&1; then
    echo "gh is not authenticated. Run: gh auth login"
    exit 1
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' packaging/Info.plist)"
TAG="v${VERSION}"
DMG="dist/AtlasDictation-${VERSION}.dmg"

if [ ! -f "$DMG" ]; then
    echo "DMG missing at $DMG. Run ./build-dmg.sh first."
    exit 1
fi

# 1. Push the current branch.
git push origin HEAD

# 2. Tag this version and push the tag.
git tag -f "$TAG"
git push -f origin "$TAG"

# 3. Create (or replace) the Release and attach the DMG.
if gh release view "$TAG" >/dev/null 2>&1; then
    gh release delete "$TAG" --yes
fi
gh release create "$TAG" "$DMG" \
    --title "${TAG} — Local medical dictation" \
    --notes-file RELEASE_NOTES.md

echo
echo "Shipped → https://github.com/whotGZ/atlas-dictation/releases/tag/${TAG}"
