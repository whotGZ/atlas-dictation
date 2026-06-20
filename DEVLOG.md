# Atlas Intensive Care Dictation — Build Log

A plain-language record of what's been done, what works, what doesn't, and why we made the choices we did. Lives alongside the code so it survives any session, any tool, any restart.

- **Born:** 15 June 2026 (Somvati Amavasya, Adhik Maas)
- **Current version:** v0.3.1
- **Status:** Working signed `.app` in `/Applications` on **macOS (Apple Silicon) only**, menu-bar utility with a Quit item and a Microphone picker. Stable self-signed identity ("ATLAS Local dev") so Accessibility/mic grants survive rebuilds. NOT yet pushed to GitHub. Linux + Windows compile won't work today (afplay, /System/Library/Sounds, Cmd-V paste, Accessibility permission flow are all macOS-specific).
- **Build location:** `/Users/arun/C BHAIYA/atlas-dictation/`

---

## What works (verified in real use)

- **End-to-end dictation on Mac** — confirmed by the author dictating a multi-paragraph message into Claude desktop the same night the app was built.
- **Whisper Turbo model** (1.5 GB FP16) loads in seconds and transcribes locally with zero network calls.
- **Tilde key (`` ` ``)** toggles recording on and off.
- **Audio cues** — `Pop.aiff` on record-start, `Glass.aiff` on record-stop, played via macOS `afplay`.
- **Auto-paste** — after stopping recording, the cleaned transcript is typed at the cursor of whichever app is focused.
- **Right Option re-paste** — drops the same transcript again wherever the cursor is now. Internal buffer survives clipboard churn (so you can copy something else in between and still re-paste the last transcript).
- **Filler scrub** — removes uh / um / er / ah / hmm / "you know" / "I mean" / immediate word repetitions ("the the patient" → "the patient").
- **Medical prose biasing prompt** — ~150 words of natural clinical prose covering IM, ICU, surgery, OB/GYN, ID, neuro. Whisper biases on both vocabulary AND style, so prose outperforms comma-separated term lists.
- **On-demand microphone** — the mic stream is opened only during recording and dropped on stop, so the macOS orange privacy indicator turns off when the app is idle.
- **MIT license + trademark notice** — code is MIT, Atlas branding is reserved.
- **DISCLAIMER.md** — full small-print: not a medical device, not FDA-cleared, user must proofread every transcript.

## What doesn't work yet (known)

- **Tilde character prints into the focused app.** rdev's listen-only event tap can't suppress the keypress, so every time you press `` ` `` you get a stray `` ` `` character in your text. Fix planned for v0.2: replace with a Core Graphics event tap in active mode (~30 min of work).
- **Terminal owns the mic and accessibility permissions** instead of the app itself. macOS shows "Terminal is using your microphone" rather than "Atlas Dictation." Fix planned for v0.2: wrap the binary as a proper `.app` bundle.
- **No menubar icon.** App is invisible — you can tell it's running only because the hotkey works. Fix planned for v0.2 (UI option E from the brainstorm: menubar icon + sounds).
- **No way to quit gracefully without a Terminal window.** Ctrl-C in Terminal kills it; otherwise you have to use Activity Monitor. Fix planned for v0.2: menubar icon with Quit option.
- **Mac only.** Linux + Windows ports deferred to later releases.
- **No `.dmg` installer.** Users have to clone the repo and `cargo build --release` to run it.

---

## Version history

| Version | What changed | Why |
|---|---|---|
| v0.1.0 | First commit. F9 hotkey, auto-paste, 191-term comma-separated dictionary. | Get something working tonight. |
| v0.1.1 | F9 → tilde key. Caps Lock added as separate paste key. Startup disclaimer. | User found F9 collided with Mission Control; tilde matches 3M Fluency Direct convention. |
| v0.1.2 | Label hotkey as "tilde / backtick" everywhere. | Clinicians know it as the tilde key; the dual label avoids confusion. |
| v0.1.3 | Fixed transcription crash; disabled Metal GPU. | Regex backreference (`\1`) panicked the filler scrubber after every successful transcription; rewrote in plain Rust. Metal kernel JIT compile is broken with whisper-rs 0.13 + recent macOS Metal SDK — CPU/BLAS works fine and is plenty fast for Turbo on M1 Ultra. |
| v0.1.4 | Auto-paste restored + 96 OB/GYN & GI-surgery terms. | User wanted text to land at cursor automatically; Caps Lock became "re-paste." |
| v0.1.5 | Dropped Caps Lock entirely. Single-hotkey design + Cmd+V for re-paste. | Caps Lock required a System Settings tweak (Caps Lock → No Action) to stop toggling caps — too much friction for a commercial product. |
| v0.1.6 | Right Option re-paste key (rdev::AltGr). | User wanted single-key re-paste back (use case: dictate a med list once, deploy into chart + pharmacy order + patient handout). Right Option has no OS state weirdness, no app conflicts, works cross-platform. |
| v0.1.7 | Start/stop sound cues (Pop / Glass). | Audio feedback so users know recording is live without watching the terminal. |
| v0.1.8 | Mic stream opens on record-start, closes on record-stop. | macOS orange indicator now only shows during actual recording. Important for a medical product — privacy expectation. |
| v0.1.9 | Switched biasing prompt from 287-term list to ~150-word medical prose. | Whisper warned `too many resulting tokens: 1527 (max 1024)` — most of our term list was being silently dropped. Prose packs more medical signal into fewer tokens because Whisper biases on style + vocabulary together. |
| v0.1.10 | `/ponytail-review` cleanup: -25 net lines. Header doc, prompt truncate, hotkey listener Arc/clones, dedup helper, inlined `num_cpus_safe`. | Lazy senior-dev pass. No behavior change. Skipped the I16/U16 sample-format deletion (kept for v0.2 Linux/Win). |
| v0.2-phaseA | `AtlasDictation.app` bundle (Info.plist, Contents/Resources, ad-hoc codesign). `resolve()` picks bundle path or dev path. `LSUIElement=YES`. Stderr → `~/Library/Logs/AtlasDictation/dictation.log` when bundled. | Tried to wrap as a real Mac app. **Parked.** Bundle launches and the binary runs (log proves it reaches "Ready"), but: (1) no Accessibility grant on the new binary's signature means hotkeys silently don't fire; (2) `LSUIElement=YES` hides app from Force Quit so user has no quit affordance; (3) "not responding" cosmetic dialog from macOS expecting an AppKit run loop. Needs a proper NSApplication + status-bar icon — that's v0.2 phaseB work, not what we have now. For day-to-day use, `Start AIC Dictation.command` is the working launcher. |
| v0.3.1 | Three reliability/recovery changes. (1) **Transcript history**: every transcript appends to `~/Library/Logs/AtlasDictation/transcripts.txt`, auto-purged after 2h (on append + on the idle tick). (2) **Mic pinned per session** — stopped re-querying the system default on every recording. (3) **Tray "Microphone" picker** — CheckMenuItem per input device, choice persists to `Application Support/AtlasDictation/selected-mic.txt`; resolution priority is `$ATLAS_MIC` → saved choice → system default. Signed with stable "ATLAS Local dev" identity. | A real 450-word dictation was nearly lost: it had been overwritten in clipboard + RAM and the app deliberately never wrote transcripts to disk, so nothing was recoverable. The 2h history is the safety net (short window so PHI doesn't linger). The per-recording mic re-query was *following* the macOS default mid-session and silently feeding near-silence into dictations (captures came back 2–20 chars); pinning fixed it. The picker makes mic selection a first-run menu click instead of an env var — distribution-ready. |
| v0.4.0 | Five changes. (1) **Hotkey → single tap of Right Option (⌥)**, replacing the tilde toggle; tilde was a *printable* key that leaked a stray `` ` `` whenever macOS didn't honor CGEventTap suppression. Right Option is non-printing and passed through, so a stray char is impossible. Re-paste key dropped (Cmd+V covers it); `enigo`/`send_backspace` removed. (2) **Metal GPU** enabled by bumping `whisper-rs 0.13 → 0.16` (newer whisper.cpp fixed the Metal shader-compile bug); auto-falls back to CPU/BLAS. (3) **`collapse_repeats`** kills Whisper's silence-loop repetition (smallest-period-first, ≤40-word units). (4) **`apply_voice_punctuation`** — context-aware spoken punctuation (period/question mark/new line/new paragraph/exclamation), with denylists so "postoperative period"/"central line" stay words and "comma"/"colon" are never commands. (5) **scrub** strips Whisper's stray wrapping quotes/pipes. | The stray-mark and slowness complaints from real clinical use. A printable hotkey can silently corrupt a note; moving to a non-printing key removes the class entirely. Metal was the slowness fix — but only worked after the library bump (0.13.2 allocated Metal yet still ran inference on BLAS). Punctuation is clinically tuned because the obvious mappings collide with *coma/colon/period* in this user's notes. |

---

## Decisions log (alternatives tried and why we picked what we did)

| Question | We picked | Why |
|---|---|---|
| Build from scratch or fork OpenWhispr? | Build from scratch | OpenWhispr's UI is cluttered — we want minimal surface area. Forking inherits all that complexity. |
| Tauri or pure-Rust headless binary for v0.1? | Pure Rust | Faster to ship a working core. Tauri/menubar wrap deferred to v0.2. |
| Full Turbo (1.6 GB) or quantized (~800 MB)? | Full | Best accuracy for medical terms. Disk space is cheap; getting "cholecystitis" right matters. |
| Hotkey: F9, tilde, or other? | Tilde (`` ` ``) | F9 collides with Mission Control. Tilde is what 3M Fluency Direct uses. |
| Re-paste key: Caps Lock, Right Option, or none? | Right Option | Caps Lock needs System Settings tweak (commercial friction). Right Option has zero OS state issues and works cross-platform. |
| Biasing: term list or prose? | Prose | Whisper truncates long token lists silently; prose biases on style AND vocabulary, more signal per token. |
| Mic always on or on-demand? | On-demand | Privacy: orange indicator off when idle. Doctors / patients glancing at the menu bar shouldn't see "this app is using your microphone" if it's not actively recording. |
| License: MIT or proprietary? | MIT for code, trademark reserved for branding | Standard pattern (Firefox / Signal). Lets clinicians fork and modify, prevents look-alike products using the Atlas name. |
| GitHub: push now or wait? | Wait | First impressions matter. We want a `.dmg` installer and a real `.app` before strangers land on the repo. |

---

## v0.2 plan (next session)

In rough priority order:

1. **Wrap as a proper `.app` bundle WITH AppKit integration.** The bare bundle attempted in v0.2-phaseA isn't enough — macOS expects an NSApplication run loop, otherwise: hotkeys silently fail without Accessibility grant on the new binary, "not responding" dialogs appear, `LSUIElement=YES` removes Force Quit visibility. Real fix: integrate NSApplication + a status bar item (NSStatusItem) with a Quit menu. Hotkey listener and audio capture move to a background thread; the main thread runs the Cocoa event loop. ~2-3 hours.
2. **Menubar icon** (option E from the brainstorm). Folds into item 1 — same NSStatusItem provides both the status indicator and the Quit menu. Monochrome mic glyph per Apple HIG, subtle pulse during recording.
3. **Suppress tilde character.** Replace `rdev::listen` with Core Graphics event tap in active mode so the `` ` `` keypress is swallowed and doesn't print into the focused app.
4. **App icon** — inspired audio-waveform variant of the Atlas Intensive Care logo (navy + rust palette, squircle frame, but waveform instead of EKG line).
5. **GitHub Actions** to build `.dmg` for Mac, attach to a Release with the bundled model.
6. **Push to `github.com/whotGZ/atlas-dictation`** as public MIT once the `.dmg` is ready.
7. **Atlas Management Consulting website** with Stripe Payment Link for paid installer downloads (~$5).
8. **Linux + Windows builds** via the same GitHub Actions pipeline.
9. **Editable medical dictionary UI** (instead of editing the .txt by hand).

---

## Lessons learned (worth remembering for v0.2 and future Atlas products)

- **Ship the smallest thing that works first.** A headless Rust binary in Terminal was usable the same night. Wrapping it as a polished `.app` would have taken another session and we'd have learned less.
- **Test with the actual user's voice and vocabulary.** The 287-term dictionary looked great on paper; it took the user dictating real medical sentences to surface the tokenizer overflow.
- **Permission friction kills commercial adoption.** Anything that requires the user to dig into System Settings is a download-blocker. Caps Lock → No Action was a no-go because of this.
- **Audio feedback matters for invisible apps.** Without the menubar icon (v0.2 work), the start/stop sounds are the only confirmation that recording is live. Don't skip them.
- **Prose beats word lists for Whisper biasing.** This is OpenWhispr / Wispr Flow's approach for a reason. Pack vocabulary into natural clinical sentences.
- **macOS process attribution follows the launching process, not the binary.** A binary launched from Terminal looks like "Terminal" in System Settings. The only fix is a proper `.app` bundle.
- **For product copy, "born" beats "built."** Atlas products serve patients and clinicians — copy should feel like care, not like a release note.
