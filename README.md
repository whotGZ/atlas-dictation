# Atlas Intensive Care Dictation

**Local medical dictation. Nothing leaves your device.**

A small, fast speech-to-text tool for clinicians. Hold a hotkey, talk, get a clean transcript pasted at your cursor. Uses [whisper.cpp](https://github.com/ggerganov/whisper.cpp) with the Whisper Turbo model running 100% on your machine — no cloud, no accounts, no telemetry. Ships with a built-in medical vocabulary so it spells *cholecystitis*, *sphincterotomy*, *choledocholithiasis*, and friends correctly.

Built by **Atlas Management Consulting** as part of our practice-efficiency suite.

---

## Why

Cloud dictation services send your patient audio to a third party. They want a Business Associate Agreement, charge per minute, and lose accuracy on procedure names. This tool does the opposite:

- **Local-only.** Audio is recorded, transcribed, and discarded on your computer.
- **No BAA needed** — no third party ever sees the data. (We do **not** claim "HIPAA certified" — HIPAA certifies covered entities, not software. This tool is designed for HIPAA-conscious workflows.)
- **Medical-aware.** Comes with a curated dictionary that biases Whisper's spelling toward terms doctors actually dictate.
- **Free for everyone, paid installer for convenience.** Source is MIT; we sell a one-click pre-built installer for those who don't want to compile.

## Status

v0.1 — Mac (Apple Silicon) only. Linux + Windows builds coming.

## Install

### One-click installer (recommended for non-developers)

→ Coming soon at [atlasmc.com](#) (paid, ~$5, supports development).

### Build from source (free, requires Rust)

```bash
# Prereqs: Rust, cmake, Xcode Command Line Tools (Mac)
git clone https://github.com/whotGZ/atlas-dictation
cd atlas-dictation

# Fetch the Whisper Turbo model (~1.5 GB) into models/
curl -L -o models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin

# Build
cargo build --release

# Run
./target/release/atlas-dictation
```

## Use

1. Run `atlas-dictation` (or double-click `Start AIC Dictation.command`). It loads the model and waits.
2. First run, grant two macOS permissions:
   - **Microphone** — popup on first ` press
   - **Accessibility** — System Settings → Privacy & Security → Accessibility → add Terminal (or whatever launched the app) and toggle it on. Quit and relaunch the app after granting.
3. Press **`** (tilde / backtick key, top-left of keyboard) → `[REC]`. Talk.
4. Press **`** again → transcribes, scrubs *uh/um*, puts cleaned text on your clipboard.
5. Move your cursor to any app (EHR, browser, Notes, Word, anything with a text field).
6. Press **Caps Lock** → text pastes at the cursor.

**Tip:** in System Settings → Keyboard → Modifier Keys, set Caps Lock to "No Action" so it stops toggling caps when you use it as paste.

## Disclaimer (please read once)

This software is **not** a medical device, **not** FDA-cleared, and makes **no** accuracy guarantee. Speech recognition produces errors — including misspelled medications, wrong dosages, dropped negations, and misheard numbers. **You are solely responsible for proofreading every transcript** before it is used for patient care, billing, legal records, or any other consequential purpose. By using this software you accept that responsibility. Full terms in [DISCLAIMER.md](DISCLAIMER.md).

## Customizing the dictionary

Edit `assets/medical-dictionary.txt`. One term per line. Lines starting with `#` are comments. Restart the app to reload.

Whisper's "initial prompt" has a soft cap (~224 tokens) — when the list grows large, the **earliest** entries are dropped first. Keep your most-mistranscribed terms toward the bottom of the file.

## How it works

```
[mic] -> cpal capture -> resample 16kHz -> whisper.cpp Turbo (Metal GPU)
                                                |
                          medical-dictionary.txt -> initial prompt
                                                |
                                       text -> filler scrubber -> clipboard -> Cmd-V
```

Every step is local. No sockets are opened.

## Roadmap

- [ ] Menubar app + system tray (no terminal window required)
- [ ] Settings UI for hotkey + microphone + dictionary editing
- [ ] Linux build
- [ ] Windows build
- [ ] Bundled `.dmg` / `.exe` / `.AppImage` installers via GitHub Actions
- [ ] Configurable filler-word list

## License

Source code: **MIT** (see [LICENSE](LICENSE)).

The "Atlas Intensive Care," "Atlas Management Consulting," and "AIC Dictation" names and logos are trademarks of their respective owners and are **not** granted under the MIT license. If you fork, please use your own branding.

## Support / Contact

Issues: [GitHub Issues](https://github.com/whotGZ/atlas-dictation/issues)
Atlas Management Consulting: [atlasmc.com](#)
