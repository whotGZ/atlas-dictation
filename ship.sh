#!/usr/bin/env bash
# One-shot publish: create the public GitHub repo, push all commits + tags,
# attach the .dmg to a v0.2.0 Release. Requires `gh auth login` already done.
set -euo pipefail
cd "$(dirname "$0")"

if ! gh auth status >/dev/null 2>&1; then
    echo "gh is not authenticated. Run: gh auth login"
    exit 1
fi

DMG="dist/AtlasDictation-0.2.0.dmg"
if [ ! -f "$DMG" ]; then
    echo "DMG missing at $DMG. Run ./build-dmg.sh first."
    exit 1
fi

# 1. Create the public repo and push the existing local branch.
gh repo create whotGZ/atlas-dictation --public \
    --description "Local medical dictation. Whisper Turbo on your Mac, never sends a byte to the cloud." \
    --source=. --remote=origin --push

# 2. Tag the release and push the tag.
git tag -f v0.2.0
git push origin v0.2.0

# 3. Create the Release and attach the DMG.
gh release create v0.2.0 "$DMG" \
    --title "v0.2.0 — Local medical dictation" \
    --notes-file RELEASE_NOTES.md

echo
echo "Shipped → https://github.com/whotGZ/atlas-dictation/releases/tag/v0.2.0"
