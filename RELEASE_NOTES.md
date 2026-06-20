# Atlas Intensive Care Dictation v0.4.0

Local medical dictation that never sends a byte to the cloud.

## What it is

Tap Right Option (⌥) once anywhere on your Mac → speak → tap Right ⌥ once more → cleaned medical text auto-pastes at your cursor. The whole pipeline — audio capture, Whisper Turbo transcription, medical-vocabulary biasing — runs on your own machine. No network calls, no accounts, no transcript storage.

Built for clinicians who want efficient note-taking without handing patient audio to a third-party transcription service.

## New in v0.4.0

- **GPU acceleration (Apple Metal).** Transcription now runs on your Mac's GPU, with automatic fallback to CPU on machines where Metal isn't available. Noticeably faster on Apple Silicon.
- **New hotkey: a single tap of Right Option (⌥).** Replaces the old tilde key. It's a non-printing key, so it can *never* leave a stray character in your text — and it's clear of macOS's own Dictation shortcut.
- **Spoken punctuation, tuned for clinical notes.** Say "period", "question mark", "new line", "new paragraph", "exclamation point". Collision-aware: "postoperative **period**" and "central **line**" stay as words. "comma"/"colon" are intentionally *not* commands (they collide with *coma* and *colon*) — Whisper adds commas automatically anyway. See the README for the full table.
- **Repetition / silence-loop cleanup.** If you leave it recording during silence, Whisper can loop the last sentence; that's now collapsed back to a single copy.
- **Cleaner output.** Stray wrapping quotes/pipes that Whisper sometimes adds are stripped.
- **Lost-dictation safety net.** A short rolling 2-hour local history (`~/Library/Logs/AtlasDictation/transcripts.txt`), auto-deleted so PHI doesn't linger.
- **Microphone picker** in the menu-bar menu; your choice is remembered. Mic is pinned per session (no more mid-session drift to the wrong input).

## Highlights

- **Tap Right ⌥ once to record, tap again to stop.**
- **Cmd+V re-pastes the last transcript** anywhere (drop a med list into chart + pharmacy order + handout) — it stays on the clipboard until you dictate again.
- **Audio cues:** Pop on record-start, Glass on stop.
- **Mic indicator turns off when idle.** Mic stream opens only during dictation.
- **Menu-bar utility.** Quit from the audio-bars icon in the top-right.
- **Bundled Whisper Turbo model (~1.5 GB).** No first-run download.
- **Bundled medical biasing prompt** — IM, ICU, surgery, OB/GYN, ID, neuro vocabulary baked into Whisper's initial prompt.

## Install

1. Download `AtlasDictation-0.4.0.dmg` from this release.
2. Open the DMG, drag **AtlasDictation** onto the **Applications** alias, eject.
3. Launch from /Applications (right-click → Open the first time to bypass Gatekeeper).
4. macOS will prompt for **Microphone** — allow.
5. The hotkey needs **Accessibility** and **Input Monitoring**: System Settings → Privacy & Security → add Atlas Dictation under *both* Accessibility and Input Monitoring, toggle ON. Quit from the menu-bar icon, then re-launch.

## Build from source

```bash
git clone https://github.com/whotGZ/atlas-dictation
cd atlas-dictation
curl -L -o models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
./build-app.sh
open dist/AtlasDictation.app
```

Requires Rust, cmake, Xcode CLT.

## Disclaimer

Not a medical device. Not FDA-cleared. You are solely responsible for proofreading every transcript before using it for patient care, billing, or legal records. Full terms in [DISCLAIMER.md](DISCLAIMER.md).

## Origin

AIC Dictation was born on **Somvati Amavasya, 15 June 2026** — the rare Monday new moon falling within Adhik Maas, considered in Hindu tradition a day of spiritual renewal and the clearing of darkness. We hope the tool gives the same kind of quiet renewal to the clinicians who use it.

## What's next

- Linux + Windows ports
- App Store / signed-installer distribution
- Editable medical-dictionary UI

## License

MIT for source code. "Atlas Intensive Care," "Atlas Management Consulting," and "AIC Dictation" names + logos are trademarks of their respective owners, not granted under the MIT license.
