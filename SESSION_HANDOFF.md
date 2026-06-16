# Atlas Dictation — session handoff to main agent

**Date:** 2026-06-16
**Working dir:** `/Users/arun/C BHAIYA/atlas-dictation/`
**Last fully verified version:** v0.1.13 (Terminal-launched via `Start AIC Dictation.command`)
**In-progress version:** v0.2.0 (NSStatusItem + CGEventTap) — functional, one cosmetic bug left

## What's done in this session

| Phase | Change | Status |
|---|---|---|
| v0.1.11 | Stop logging transcript text — log length only (PHI safety) | ✅ committed |
| v0.1.12 | App icon: navy squircle + rust circle + audio bars | ✅ committed |
| v0.1.13 | Brighter rust on app icon (#ee5a2a) | ✅ committed |
| v0.2-phaseB | NSApplication via tao + NSStatusItem via tray-icon. Pipeline on worker thread. Quit menu. | ✅ working |
| v0.2-phaseC | rdev replaced by CGEventTap (active mode). Tilde keypress is swallowed — no stray `` ` `` printed. | ✅ working |

## Verified working end-to-end (Terminal launch)

User dictated "What are you doing today?" through `.app` binary:

- Tilde toggle: ✅ no stray `` ` `` character (CGEventTap suppression confirmed)
- Sounds: ✅ Pop on start, Glass on stop
- Transcription: ✅ correct, auto-pasted at cursor
- Right Option re-paste: ✅
- Menu-bar Quit: ✅ ("Quit selected." in log, clean exit)

## Open bug — must fix before commit

`src/main.rs` `run_event_loop_with_tray`: the `TrayIconBuilder` chain is missing `.with_icon(icon)` after a sed cleanup deleted both `.with_icon_as_template(true)` AND the icon binding. Build warns `unused variable: icon`. Result: menu-bar status item has no glyph and is invisible.

**Fix:** add `.with_icon(icon)` back to the builder chain:

```rust
let _tray = TrayIconBuilder::new()
    .with_menu(Box::new(menu))
    .with_icon(icon)            // ← restore this line
    .with_tooltip("Atlas Dictation")
    .build()
    .map_err(|e| anyhow::anyhow!("tray build: {e}"))?;
```

Do NOT add `.with_icon_as_template(true)` back — qlmanage's PNG output isn't producing the alpha-channel structure macOS expects for templates, and the icon went fully transparent. Ship as non-template (visible black bars on light menu bar, harder to see on dark menu bar). Polish template behaviour later when we have a proper rsvg-convert or hand-tweaked PNG.

## .app launch gotcha (not a bug, but document for users)

Each fresh build changes the binary signature, so macOS treats the `.app` as a new app for Accessibility purposes. After every rebuild, user must:

1. System Settings → Privacy & Security → Accessibility
2. Remove old "Atlas Dictation" entry if present, add `dist/AtlasDictation.app`
3. Toggle ON
4. Quit + relaunch the app

Long-term fix is a stable Developer ID code signature ($99/year Apple Developer Program). Tonight: accept the one-time grant.

The CGEventTap fail-fast message in the log is correct UX:

```
CGEventTap create failed (likely missing Accessibility).
Grant Accessibility: System Settings -> Privacy & Security -> Accessibility.
Add Atlas Dictation, toggle ON, Quit (menu-bar icon), and re-launch.
```

## Architecture (as built)

- **Main thread:** `tao` event loop drives NSApplication + NSStatusItem (via `tray-icon` crate). Polls menu events every 200ms. Quit menu sets `ControlFlow::ExitWithCode(0)`.
- **Hotkey thread:** native CGEventTap (in `spawn_event_tap`, replaces previous `rdev::listen`). Watches kVK_ANSI_Grave (50) and kVK_RightOption (61). Sends `Cmd::ToggleRecord` / `Cmd::RepasteLast` to channel. Returns `nil` from tap callback for tilde → swallows the keystroke (this is what eliminates the stray `` ` ``).
- **Pipeline thread:** owns `WhisperContext` + `cpal::Device` + active `cpal::Stream`. Receives `Cmd` from channel. Manages record start/stop, transcription, clipboard, paste.
- **Channels:** `Cmd` (event tap → pipeline), `TrayState::Idle|Recording` (pipeline → event loop, for future icon swap).
- **Logging:** stderr → `~/Library/Logs/AtlasDictation/dictation.log` when bundle path detected. Transcript text NEVER logged (PHI safety) — only `(N chars, N words)`.

## Cargo deps added this session

```
tao = "0.30"
tray-icon = "0.19"
image = { version = "0.25", default-features = false, features = ["png"] }

[target.'cfg(target_os = "macos")'.dependencies]
libc = "0.2"
core-foundation = "0.9"
core-graphics = "0.23"
```

## What's left for "shippable today"

After the icon fix above:

1. **Commit v0.2.0** — one big commit with the architecture refactor + tray + CGEventTap. Message draft: `v0.2.0: NSStatusItem + CGEventTap key suppression`.
2. **v0.2-phaseD: `.dmg` installer** — use `hdiutil create -volname "Atlas Dictation" -srcfolder dist/AtlasDictation.app -ov -format UDZO dist/AtlasDictation-0.2.0.dmg`. ~20 min.
3. **v0.2-phaseE: GitHub push** — `gh repo create whotGZ/atlas-dictation --public --source=. --description="Local medical dictation, no cloud" && git push -u origin main`. Then `gh release create v0.2.0 dist/AtlasDictation-0.2.0.dmg`. ~10 min.

Atlas MC website + Stripe Payment Link is a separate project — see `project_atlas_mc_website.md` in auto-memory.

## Useful commands

```bash
# Build everything
cd "/Users/arun/C BHAIYA/atlas-dictation" && ./build-app.sh

# Run from terminal (stderr to terminal)
./target/release/atlas-dictation

# Run as .app (stderr to log file)
open dist/AtlasDictation.app
tail -f ~/Library/Logs/AtlasDictation/dictation.log

# Kill stuck instances
pkill -9 -f atlas-dictation

# Latest commits
git log --oneline | head -10
```

## Known limitations (document but don't fix tonight)

- Menu-bar icon is non-template (doesn't auto-invert for dark mode); template flag broke visibility entirely
- Toggling Accessibility while running may silently kill the process; user must relaunch
- Whisper Metal GPU disabled (whisper-rs 0.13 has broken JIT-compile against recent macOS Metal SDK) — CPU/BLAS still fast on M1 Ultra
- macOS-only — Linux/Windows is deferred work
- No code signing beyond ad-hoc → each rebuild requires re-grant of Accessibility on the .app
