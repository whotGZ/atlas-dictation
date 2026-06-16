# Session Handoff — Atlas Intensive Care Dictation, v0.2.0

Hand this to the next agent. Everything they need to ship is in here.

## What this is

A local-only medical dictation app. Press `` ` `` (tilde) → speak → press `` ` `` again → cleaned medical text auto-pastes at the cursor. Right Option re-pastes the last transcript. macOS-only, Apple Silicon, no network calls, Whisper Turbo + curated medical biasing prompt baked in.

## Where things live

- **Project root:** `/Users/arun/C BHAIYA/atlas-dictation/`
- **Working launcher (Terminal-only fallback):** `Start AIC Dictation.command`
- **Real `.app`:** `dist/AtlasDictation.app` (1.5 GB — model embedded)
- **Installer:** `dist/AtlasDictation-0.2.0.dmg` (1.4 GB compressed)
- **Source of truth for version history:** `DEVLOG.md` (in this same directory)
- **Auto-memory:** `/Users/arun/.claude/projects/-Users-arun-C-BHAIYA/memory/project_atlas_dictation.md` — also has the parking note for the earlier bare-bundle attempt

## Architecture (v0.2.0)

Three threads:

1. **Main thread** — `tao::EventLoop` runs the AppKit run loop. `tray-icon::TrayIconBuilder` puts an `NSStatusItem` in the menu bar with a "Quit Atlas Dictation" menu item. The event loop polls `MenuEvent::receiver()` and the `TrayState` channel from the pipeline every 200ms.
2. **Hotkey thread** — `spawn_event_tap()` in `src/main.rs`. A Core Graphics `CGEventTap` in **active mode** (`CGEventTapOptions::Default`) attached to its own `CFRunLoop`. Tilde keydown (kc=50) is swallowed; right-Option flagsChanged (kc=61) is passed through, transitions tracked via `FLAG_RIGHT_OPTION_BIT`. Sends `Cmd::ToggleRecord` / `Cmd::RepasteLast` to the pipeline.
3. **Pipeline thread** — `pipeline_main()`. Owns `WhisperContext`, `cpal::Device`, mic stream, last-text buffer. Runs the recv→record/transcribe/paste loop. Opens the mic only during recording (cpal Stream dropped on stop so macOS turns off the orange privacy indicator).

The header comment in `src/main.rs` still says "rdev::listen" — that's stale doc, not stale code. The actual hotkey path is CGEventTap. Worth updating the comment in a tiny cleanup commit.

## What's done (16 commits, all local)

```
ef4747e Add RELEASE_NOTES.md for v0.2.0 GitHub release
5e6ab05 v0.2-phaseD: AtlasDictation-0.2.0.dmg installer
70a14ec v0.2-phaseC: CGEventTap suppression — no stray backticks
7bf8bfe v0.2-phaseB: NSApplication + NSStatusItem (menubar icon, real Quit)
666c43a v0.1.13: brighter rust on icon (#c0532e -> #ee5a2a)
e05894d v0.1.12: app icon — navy squircle, rust circle + audio bars
435466d v0.1.11: never log transcript text — log length only
38080b9 docs: park v0.2-phaseA .app bundle, document why
6d16d10 v0.2-phaseA.1: fix '.app not responding' hang
4e60a5b v0.2-phaseA: package as AtlasDictation.app bundle
518d539 docs: mark v0.1 as macOS-only explicitly
c129a3b v0.1.10: ponytail cleanup pass
... (earlier history in DEVLOG)
```

The build is green. `cargo build --release` produces `target/release/atlas-dictation` (~3 MB). `./build-app.sh` assembles `dist/AtlasDictation.app`. `./build-dmg.sh` produces `dist/AtlasDictation-0.2.0.dmg`. Latest end-to-end smoke test on 2026-06-16: bundle launched, model loaded, dictation produced output, auto-paste worked, user clicked the menu-bar Quit and the app exited cleanly (`Quit selected.` in `~/Library/Logs/AtlasDictation/dictation.log`).

## The one open code concern: `.with_icon(icon)` / menu-bar icon visibility

`load_tray_icon()` in `src/main.rs` decodes `packaging/tray-icon.png` (44×44 monochrome black-on-transparent) via the `image` crate and hands the raw RGBA to `tray_icon::Icon::from_rgba`. It compiles and `TrayIconBuilder::new().with_icon(icon)...build()` returns `Ok` — the previous smoke test proved the menu's Quit item works, which implies the icon is clickable.

**What still needs visual verification:** on this user's Mac, does the actual glyph render correctly in the menu bar (right shape, right size, tint behaves in light/dark mode)? Or does the click target exist but show a blank/black square?

If the icon looks wrong:
- macOS menu-bar icons should be **template images** — black-on-transparent, OS auto-tints to match theme. Our PNG is in the right format, but `tray_icon::Icon` may not flag it as template by default. Try `tray.set_icon_as_template(true)` after building (check the crate's API surface — method name may differ between minor versions).
- Or: regenerate the PNG with `sips` rather than `qlmanage` and double-check the alpha channel.
- Or: image dimensions — macOS prefers 22×22 pt (44×44 px @2x); we're at 44×44 so should be fine.

If `.with_icon(icon)` itself fails to compile against a future tray-icon version: the API may have shifted to `with_icon(Some(icon))` or `with_icon_as_template(icon, true)`. Check `tray-icon` crate docs for the current signature.

## What's left to ship

Per the user, these are the only steps between right now and "shippable":

1. **Verify the menu-bar icon visually.** Run `open dist/AtlasDictation.app`, look at the top-right of the screen. Should see the audio-bars glyph. Click it → "Quit Atlas Dictation" appears. If icon looks bad, see fixes above. Commit any tweak.
2. **Push the repo + tag the Release.**
   - `gh` is installed (Homebrew, v2.94.0) but **not authenticated**. The user needs to run `gh auth login` themselves (web browser flow) — the next agent cannot do that step.
   - After auth: `gh repo create whotGZ/atlas-dictation --public --description "Local medical dictation. Whisper Turbo on your Mac, never sends a byte to the cloud." --source=. --remote=origin --push`
   - Tag: `git tag v0.2.0 && git push origin v0.2.0`
   - Release with DMG: `gh release create v0.2.0 dist/AtlasDictation-0.2.0.dmg --title "v0.2.0 — Local medical dictation" --notes-file RELEASE_NOTES.md`
3. **First-user smoke test:** download the DMG from the GitHub release on a fresh-ish Mac (or just from a different folder), drag the app to /Applications, grant Mic + Accessibility, dictate one medical sentence. Confirm no regressions vs the local build.

## Critical user-facing setup steps (for the README / Release notes)

Each time the binary is rebuilt, its codesign hash changes → macOS treats it as a new app for permission purposes. The user has to:

1. Drag `AtlasDictation.app` into `/Applications`
2. Right-click → Open the first time (bypass Gatekeeper warning for ad-hoc-signed apps)
3. Grant **Microphone** on the popup
4. Open System Settings → Privacy & Security → **Accessibility** → click **+** → add `AtlasDictation.app` → toggle ON
5. Quit via the menu-bar icon, re-launch

## Boundaries (don't break these)

- **No network calls.** Anywhere. The whole pitch is local-only. If a dep starts phoning home, replace it.
- **No transcript text in logs.** `~/Library/Logs/AtlasDictation/dictation.log` records lengths only (see v0.1.11 commit and `scrub()` / log lines in `pipeline_main`).
- **Don't drop the I16/U16 cpal branches in `build_stream()`.** They're dead on Mac but needed for the v0.3 Linux/Windows port. Marked as such in DEVLOG.
- **Don't edit the Origin section in README.md.** It dedicates the project to Somvati Amavasya, 15 June 2026 — that's a permanent dedication per user request. Memory file `project_atlas_dictation.md` flags this explicitly.
- **For user-facing copy**, "born" over "built". Atlas products serve clinicians; copy reads like care, not like a release note. See `feedback_product_voice.md` in the auto-memory directory.

## Quick reference

| Want to | Run |
|---|---|
| Build the binary | `cargo build --release` |
| Build the `.app` | `./build-app.sh` |
| Build the `.dmg` | `./build-dmg.sh` |
| Rebuild the app icon from SVG | `./build-icon.sh` |
| Launch the `.app` from Finder-equivalent | `open dist/AtlasDictation.app` |
| Watch the log live | `tail -f ~/Library/Logs/AtlasDictation/dictation.log` |
| Quit the running app | menu-bar audio-bars glyph → Quit Atlas Dictation |
| Kill stuck instances | `pkill -9 -f atlas-dictation` |

## Final state for the main agent

`./build-app.sh && ./build-dmg.sh` both green. Bundle structure verified. DMG mounts cleanly. Only manual step left for the user is `gh auth login`. After that, three commands push the repo and ship the Release. Then it's downloadable from anywhere.
