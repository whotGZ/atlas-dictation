# Disclaimer & Terms of Use

**Read this once. By using Atlas Intensive Care Dictation, you agree to all of it.**

## What this software is

Atlas Intensive Care Dictation ("the app") is a thin, locally-run user interface around the open-source [whisper.cpp](https://github.com/ggerganov/whisper.cpp) library and the Whisper Turbo speech-recognition model, augmented with a curated medical vocabulary. The app records audio from your microphone, processes it entirely on your own computer, and produces a text transcript that is placed on your system clipboard.

## What it is not

- It is **not** a medical device.
- It is **not** certified, cleared, or approved by the FDA, CE, MHRA, TGA, Health Canada, or any other regulatory body for any purpose.
- It is **not** a HIPAA-certified product. (HIPAA does not certify software; HIPAA compliance is a property of covered entities and their workflows.)
- It is **not** a replacement for human review.
- It does **not** guarantee accuracy, completeness, or fitness for any particular purpose.

## Your responsibility

Automated speech recognition makes mistakes. Whisper Turbo is currently among the best models available, but it still produces transcription errors — including, but not limited to:
- Misspelled medication names and dosages
- Misheard numeric values (lab results, vital signs, dates)
- Dropped or inserted negations ("no" / "not")
- Substituted clinically-significant terms
- Punctuation that changes the meaning of a sentence

**You, the user, are solely responsible for:**

1. **Reading every transcript before it is used.** Do not paste a transcript into a chart, prescription, billing system, legal document, or any consequential record without verifying every word, number, and negation.
2. **Cross-checking against the original source.** If you dictated a value, confirm the transcript shows the value you actually said.
3. **The downstream consequences** of any text the app produces — clinical, financial, legal, or otherwise. This includes but is not limited to: prescribing errors, billing errors, miscommunication with colleagues, documentation errors, and decisions made on the basis of erroneous text.
4. **Compliance with your local laws, regulations, and institutional policies** regarding documentation, recording, and use of automated tools.

## No warranty

The software is provided **"AS IS"**, without warranty of any kind, express or implied, including but not limited to the warranties of merchantability, fitness for a particular purpose, and non-infringement. In no event shall the authors, Atlas Management Consulting, Atlas Intensive Care, or any contributor be liable for any claim, damages, or other liability — whether in an action of contract, tort, or otherwise — arising from, out of, or in connection with the software or the use or other dealings in the software.

This includes (without limitation) any liability for clinical harm, financial loss, legal exposure, or reputational damage that results from acting on transcripts produced by the app.

## Privacy

The app records audio locally and processes it locally. It does **not** make network calls during transcription. It does **not** send your audio, transcripts, or any usage data to Atlas Management Consulting, Atlas Intensive Care, the authors, or any third party. The model file and dictionary are bundled with the application.

**Transcripts are never written to disk.** They live in RAM only between the moment of transcription and the moment they're pasted at your cursor (and on the macOS clipboard, like any other "copy" operation, until you copy something else). The app's diagnostic log file at `~/Library/Logs/AtlasDictation/dictation.log` records the *length* of each transcript (in characters and words) but never the content. There is intentionally no "save transcript" feature — storing PHI on disk would create downstream privacy, retention, and compliance obligations that this tool is not built to handle.

If you choose to compile the app from source yourself, you may verify these claims by reading the source code or running a network monitor.

## Acceptance

If you do not accept these terms, do not use the software. Continued use of the app constitutes acceptance.

---

Last updated: 2026-06-15
