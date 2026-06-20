# Atlas Intensive Care Dictation

**Local medical dictation. Nothing leaves your device.**

> **Platform: macOS (Apple Silicon) only.** Linux and Windows ports planned for v0.2 but not yet built — the source will not compile on those platforms today.

A small, fast speech-to-text tool for clinicians. Hold a hotkey, talk, get a clean transcript pasted at your cursor. Uses [whisper.cpp](https://github.com/ggerganov/whisper.cpp) with the Whisper Turbo model running 100% on your machine — no cloud, no accounts, no telemetry. Ships with a built-in medical vocabulary so it spells *cholecystitis*, *sphincterotomy*, *choledocholithiasis*, and friends correctly.

Built by **Atlas Management Consulting** as part of our practice-efficiency suite.

---

## Why

Cloud dictation services send your patient audio to a third party. They want a Business Associate Agreement, charge per minute, and lose accuracy on procedure names. This tool does the opposite:

- **Local-only.** Audio is recorded, transcribed, and discarded on your computer.
- **No transcript storage.** Transcripts live in RAM until pasted at your cursor, never written to disk. No "save transcript" feature — keeping PHI off the filesystem by design.
- **No BAA needed** — no third party ever sees the data. (We do **not** claim "HIPAA certified" — HIPAA certifies covered entities, not software. This tool is designed for HIPAA-conscious workflows.)
- **Medical-aware.** Comes with a curated dictionary that biases Whisper's spelling toward terms doctors actually dictate.
- **Free for everyone, paid installer for convenience.** Source is MIT; we sell a one-click pre-built installer for those who don't want to compile.

## Status

**v0.4.1 — macOS (Apple Silicon) only.** Real `.app` bundle with a menubar icon and a Quit menu, GPU-accelerated (Metal) transcription, single-tap Right Option hotkey, clinically-tuned spoken punctuation, English-only output, and a voice-activity noise gate. The current build hard-depends on macOS-specific bits (`afplay`, `/System/Library/Sounds/`, Accessibility permission model, Cmd-V paste, Core Graphics event tap, Metal). Linux + Windows ports are on the roadmap.

## Install

### Pre-built installer (recommended)

Download `AtlasDictation-0.2.0.dmg` from the [latest Release](https://github.com/whotGZ/atlas-dictation/releases). Open the DMG, drag **AtlasDictation** onto the **Applications** alias, eject.

First launch: right-click → Open (bypasses Gatekeeper for ad-hoc-signed builds). Grant **Microphone** when prompted. Then open **System Settings → Privacy & Security → Accessibility**, click **+**, add Atlas Dictation, toggle it ON. Quit from the menu-bar icon and re-launch.

### Build from source (free, requires Rust)

```bash
# Prereqs: Rust, cmake, Xcode Command Line Tools (Mac)
git clone https://github.com/whotGZ/atlas-dictation
cd atlas-dictation

# Fetch the Whisper Turbo model (~1.5 GB) into models/
curl -L -o models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
# optional noise gate (~860 KB)
curl -L -o models/ggml-silero-v5.1.2.bin \
  https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin

# Build the .app bundle
./build-app.sh
open dist/AtlasDictation.app
```

## Use

1. Launch Atlas Dictation. After ~5 seconds the audio-bars icon appears in the top-right menu bar.
2. **Click into the app where you want the text** (TextEdit, browser, EHR, anything with a text field).
3. **Tap the Right Option (⌥) key once** (the ⌥ to the right of the space bar). Pop sound = recording.
4. Speak.
5. **Tap Right ⌥ once more.** Glass sound = transcribing. Cleaned text auto-pastes at your cursor.
6. To drop the same text somewhere else (a med list into the chart, the pharmacy order, and the patient handout), click there and press **Cmd+V** — the transcript stays on your clipboard until you dictate again.
7. Quit via the menu-bar icon → "Quit Atlas Dictation".

> The hotkey is Right Option — a non-printing key — on purpose: it can never leave a stray character in your text, even if a permission is misconfigured. It's also clear of macOS's own Dictation shortcut (Control/Fn/Command pressed twice).

## Punctuation

**You usually don't need to say it.** Whisper adds commas, periods, and most punctuation automatically from the rhythm and pauses of your speech — just talk normally. In particular, **you do not need to say "comma"**; pause naturally and the comma appears. (Saying "comma" out loud is also unreliable — speech recognition tends to hear it as the word *coma*, which matters in clinical notes, so it is intentionally **not** treated as a command.)

When you *do* want to force a mark, these spoken commands work:

| Say… | You get | Notes |
|------|---------|-------|
| "period" / "full stop" | `.` | Ends a sentence anywhere in the dictation. Skipped when it reads as a word — e.g. "postoperative **period**", "a recovery **period**" stay as text. |
| "question mark" | `?` | Always. |
| "exclamation point" / "exclamation mark" | `!` | Always. |
| "new line" / "next line" | line break | Skipped when it reads as a catheter — e.g. "a **new line**", "central **line**" stay as text. |
| "new paragraph" | blank line + new paragraph | Always. |

Notes for clinical use:
- **"comma" and "colon" are deliberately not commands** — they collide with *coma/comatose* and *ascending/sigmoid colon*. Speak naturally for commas; type a colon if you need one.
- The collision-aware rules ("period"/"new line") use a built-in list of medical phrasings. If a phrase you use slips through (a "period" that shouldn't have become a `.`), it's a one-line fix in `apply_voice_punctuation` in `src/main.rs`.
- After a spoken "period", the next word is left as-is (not auto-capitalized), to avoid mangling things like "5 mg. of".

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
