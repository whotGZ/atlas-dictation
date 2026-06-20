# Cross-platform port — status

The app is macOS-first. This branch adds Windows + Linux support behind
`#[cfg(...)]`. **The Windows/Linux paths are written but NOT yet tested on real
hardware** — they compile in CI; runtime behavior needs a tester on each OS.

## What is platform-specific, and how each OS does it

| Concern | macOS | Windows | Linux |
|---|---|---|---|
| Global hotkey (Right Option / Right Alt tap) | Core Graphics event tap | `rdev` listen | `rdev` listen (**X11 only**) |
| Paste at cursor | AppleScript ⌘V | `rdev` Ctrl+V | `rdev` Ctrl+V (X11) |
| Sounds | `afplay` | PowerShell `[console]::beep` | `paplay` (best-effort) |
| GPU | Metal | CPU (CUDA/Vulkan = future feature) | CPU |
| Resource lookup | `.app/Contents/Resources` | next to the `.exe` | next to the binary |
| Data/log dirs | `~/Library/...` | `%APPDATA%` / `%LOCALAPPDATA%` | `$XDG_*` / `~/.local` |
| GUI stderr → logfile | isatty + dup2 | (skipped) | isatty + dup2 |

## Known gaps / risks (need a tester)

- **Linux Wayland**: `rdev` is X11-only. On Wayland (default on modern Ubuntu/
  Fedora) the global hotkey and synthetic paste will not work. Workaround:
  run an X11 session, or paste manually with Ctrl+V (the text is on the
  clipboard regardless).
- **Right Alt = AltGr** on some layouts (used for special characters). If that's
  a problem on a tester's keyboard, we'll switch the trigger key.
- **Windows/Linux ship CPU-only** for now — slower than the Mac's Metal. CUDA/
  Vulkan can be added as a build feature once the baseline works.
- **Tray icon on Linux** needs a system tray (GTK + appindicator); some minimal
  desktops don't show it.

## To run a CI-built binary (for testers)

1. Download the binary artifact for your OS from the GitHub Actions run.
2. Put the Whisper model next to it (the app prints the exact `curl` command if
   it's missing). Optional noise-gate model: `ggml-silero-v5.1.2.bin`.
3. Run it. Grant input/accessibility permissions if the OS asks.
