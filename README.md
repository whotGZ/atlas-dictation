# Atlas Intensive Care Dictation

**Local medical dictation. Nothing leaves your device.**

> **Platform: macOS (Apple Silicon) only.** Linux and Windows ports planned for v0.2 but not yet built — the source will not compile on those platforms today.

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

**v0.1.10 — macOS (Apple Silicon) only.** The current build hard-depends on macOS-specific bits (the `afplay` system sound player, the `/System/Library/Sounds/` paths, Apple's Accessibility permission model, Cmd-V paste keystroke). It will compile and run on a Mac and nowhere else right now. Linux + Windows ports are on the v0.2 roadmap, including the cross-platform sample-format handling we currently keep around for that eventual port.

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
3. **Click into the app where you want the text** (TextEdit, browser, EHR, anything with a text field).
4. Press **`** (tilde / backtick key, top-left of keyboard) → `[REC]`. Talk.
5. Press **`** again → transcribes, scrubs *uh/um*, **auto-pastes at your cursor**.
6. To drop the same text somewhere else (e.g. a medication list into the chart, the pharmacy order, and the patient handout), click there and press **Right Option** — re-pastes from the in-app buffer. Cmd+V works too, as long as nothing else has overwritten the clipboard.

## Disclaimer (please read once)

This software is **not** a medical device, **not** FDA-cleared, and makes **no** accuracy guarantee. Speech recognition produces errors — including misspelled medications, wrong dosages, dropped negations, and misheard numbers. **You are solely responsible for proofreading every transcript** before it is used for patient care, billing, legal records, or any other consequential purpose. By using this software you accept that responsibility. Full terms in [DISCLAIMER.md](DISCLAIMER.md).

## Customizing the dictionary

Edit `assets/medical-dictionary.txt`. One term per line. Lines starting with `#` are comments. Restart the app to reload.

Whisper's "initial prompt" has a soft cap (~224 tokens) — when the list grows large, the **earliest** entries are dropped first. Keep your most-mistranscribed terms toward the bottom of the file.

## Origin

AIC Dictation was born on **Somvati Amavasya, 15 June 2026** — the rare Monday new moon falling within Adhik Maas, considered in Hindu tradition a day of spiritual renewal and the clearing of darkness. We hope the tool gives the same kind of quiet renewal to the clinicians who use it.

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
