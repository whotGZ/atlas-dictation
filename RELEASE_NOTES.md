# Atlas Intensive Care Dictation v0.2.0

**The first public release.** Local medical dictation that never sends a byte to the cloud.

## What it is

Press tilde (`` ` ``) anywhere on your Mac → speak → press tilde again → cleaned medical text auto-pastes at your cursor. The whole pipeline — audio capture, Whisper Turbo transcription, medical-vocabulary biasing — runs on your own machine. No network calls, no accounts, no transcript storage.

Built for clinicians who want efficient note-taking without handing patient audio to a third-party transcription service.

## Highlights

- **Hold tilde to record, press again to stop.** Cleaned text pastes at your cursor.
- **Right Option** re-pastes the last transcript (drop a med list into chart + pharmacy order + handout).
- **Audio cues:** Pop on record-start, Glass on stop.
- **Mic indicator turns off when idle.** Mic stream opens only during dictation.
- **Menu-bar utility.** Quit from the audio-bars icon in the top-right.
- **Bundled Whisper Turbo model (1.5 GB).** No first-run download.
- **Bundled medical biasing prompt** — IM, ICU, surgery, OB/GYN, ID, neuro vocabulary baked into Whisper's initial prompt.
- **Filler scrubber** — strips "uh", "um", "you know", repeated words.
- **Transcripts never written to disk.** RAM + clipboard only. Diagnostic log records length, not content.

## Install

1. Download `AtlasDictation-0.2.0.dmg` (1.4 GB) from this release.
2. Open the DMG, drag **AtlasDictation** onto the **Applications** alias, eject.
3. Launch from /Applications (right-click → Open the first time to bypass Gatekeeper).
4. macOS will prompt for **Microphone** — allow.
5. The hotkey won't work until you grant **Accessibility**: System Settings → Privacy & Security → Accessibility → click **+** → add Atlas Dictation → toggle ON. Quit from the menu-bar icon, then re-launch.

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
