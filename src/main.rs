// Atlas Intensive Care Dictation
// Local medical speech-to-text. No network calls, ever.
//
// Hotkeys:
//   ` (tilde / backtick key)  - toggle recording. Press once to start, again to stop.
//   Caps Lock      - paste the last dictated text wherever your cursor is.
//
// Flow: press ` -> "REC". Talk. Press ` again -> transcribes, scrubs fillers,
// puts the cleaned text on the clipboard. Move cursor to any app (EHR, browser,
// Notes, Word), press Caps Lock to paste.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

use rdev::{listen, Event, EventType, Key as RKey};
use crossbeam_channel::{unbounded, Sender};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key as EKey, Keyboard, Settings};
use regex::Regex;

const MODEL_PATH: &str = "models/ggml-large-v3-turbo.bin";
const DICT_PATH: &str = "assets/medical-dictionary.txt";
const RECORD_KEY: RKey = RKey::BackQuote;
const PASTE_KEY: RKey = RKey::AltGr; // Right Option on Mac; Right Alt on Win/Linux
const TARGET_SR: u32 = 16_000;

#[derive(Debug, Clone, Copy)]
enum Cmd {
    ToggleRecord,
    RepasteLast,
}

fn main() -> Result<()> {
    eprintln!("Atlas Intensive Care Dictation v0.1");
    eprintln!("====================================");
    eprintln!();
    eprintln!("NOTICE: This is a local dictation shell around whisper.cpp (Turbo model)");
    eprintln!("with a curated medical vocabulary. Speech recognition is not perfect.");
    eprintln!("You are responsible for proofreading every transcript before it is used");
    eprintln!("for patient care, billing, legal records, or any other consequential");
    eprintln!("purpose. By using this software you accept that responsibility.");
    eprintln!("See DISCLAIMER.md for full terms.");
    eprintln!();

    if !Path::new(MODEL_PATH).exists() {
        anyhow::bail!(
            "Model file missing at {}.\n\
             Download it with:\n  \
             curl -L -o {} https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
            MODEL_PATH, MODEL_PATH
        );
    }

    eprintln!("Loading Whisper Turbo model (CPU/BLAS)...");
    let mut cparams = WhisperContextParameters::default();
    // Metal kernel JIT-compile is broken with whisper-rs 0.13 + recent macOS Metal SDK.
    // CPU+BLAS is plenty fast for Turbo on Apple Silicon; revisit when whisper-rs ships a fix.
    cparams.use_gpu(false);
    let ctx = WhisperContext::new_with_params(MODEL_PATH, cparams)
        .context("failed to load whisper model")?;
    eprintln!("  Model ready.");

    let dict_raw = std::fs::read_to_string(DICT_PATH).unwrap_or_default();
    let initial_prompt: String = dict_raw
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    // Whisper's initial prompt is hard-limited to ~1024 tokens; effective biasing
    // budget is ~220 tokens (~150 words). Truncate by characters as a safety net
    // — better than letting the tokenizer silently drop content mid-sentence.
    const PROMPT_CHAR_BUDGET: usize = 1100;
    let initial_prompt = if initial_prompt.len() > PROMPT_CHAR_BUDGET {
        eprintln!("  WARNING: biasing prompt is {} chars, truncating to {}.",
                  initial_prompt.len(), PROMPT_CHAR_BUDGET);
        let mut s = initial_prompt;
        s.truncate(PROMPT_CHAR_BUDGET);
        s
    } else {
        initial_prompt
    };
    eprintln!("  Biasing prompt: {} chars (~{} words).",
              initial_prompt.len(),
              initial_prompt.split_whitespace().count());

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no default input device")?;
    eprintln!("  Mic: {}", device.name().unwrap_or_else(|_| "default".into()));
    let supported = device
        .default_input_config()
        .context("failed to query input config")?;
    let input_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(input_sr as usize * 30)));

    // Mic stream is created on record-start and dropped on record-stop. While idle, the
    // mic is NOT open — macOS hides the orange privacy indicator and the menubar
    // doesn't say "Terminal is using your microphone." Important for a medical product.
    let mut active_stream: Option<cpal::Stream> = None;

    let (tx, rx) = unbounded::<Cmd>();
    spawn_hotkey_listener(tx);

    let last_text: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    eprintln!();
    eprintln!("Ready.");
    eprintln!("  ` (tilde / backtick key)  - start / stop dictation");
    eprintln!("  Right Option              - re-paste the last dictated transcript");
    eprintln!("  Ctrl-C                    - quit");
    eprintln!();
    eprintln!("HOW TO USE:");
    eprintln!("  1. Click into the app where you want the text (TextEdit, browser, EHR, etc.).");
    eprintln!("  2. Press ` (tilde). Speak. Press ` again.");
    eprintln!("  3. Cleaned text auto-pastes at your cursor.");
    eprintln!("  4. To drop the same text somewhere else (e.g. a med list into the chart,");
    eprintln!("     pharmacy order, and patient handout), click there and press Right Option.");
    eprintln!();
    eprintln!("ONE-TIME SETUP (only on first run):");
    eprintln!("  Accessibility: System Settings -> Privacy & Security -> Accessibility.");
    eprintln!("     Add Terminal (or whatever app launched this) and toggle it ON.");
    eprintln!("     Quit and re-launch this app after granting.");
    eprintln!();

    loop {
        let cmd = match rx.recv() {
            Ok(c) => c,
            Err(_) => break,
        };

        match cmd {
            Cmd::RepasteLast => {
                let stored = last_text.lock().unwrap().clone();
                match stored {
                    Some(t) => {
                        if let Ok(mut cb) = Clipboard::new() {
                            let _ = cb.set_text(t);
                        }
                        match paste_cmd_v() {
                            Ok(_) => eprintln!("[PASTE] (re-paste at cursor)"),
                            Err(e) => eprintln!("[PASTE] failed: {e}"),
                        }
                    }
                    None => eprintln!("[PASTE] (nothing dictated yet — press ` to dictate first)"),
                }
            }
            Cmd::ToggleRecord => {
                if active_stream.is_none() {
                    // START — open the mic
                    buffer.lock().unwrap().clear();
                    match build_stream(&device, &supported, channels, buffer.clone()) {
                        Ok(s) => {
                            if let Err(e) = s.play() {
                                eprintln!("[REC]   failed to start mic: {e}");
                                continue;
                            }
                            active_stream = Some(s);
                            play_start_sound();
                            eprintln!("[REC]   speak now... (` again to stop)");
                        }
                        Err(e) => {
                            eprintln!("[REC]   failed to open mic: {e}");
                            continue;
                        }
                    }
                } else {
                    // STOP — drop the stream so macOS turns off the mic indicator
                    drop(active_stream.take());
                    play_stop_sound();
                    eprintln!("[STOP]  transcribing...");

                    let raw = {
                        let mut buf = buffer.lock().unwrap();
                        std::mem::take(&mut *buf)
                    };
                    if raw.len() < (input_sr as usize) / 4 {
                        eprintln!("        (too short, skipped)");
                        continue;
                    }

                    let samples = resample_linear(&raw, input_sr, TARGET_SR);

                    let mut state = ctx.create_state().context("create_state failed")?;
                    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
                    params.set_n_threads(num_cpus_safe());
                    params.set_translate(false);
                    params.set_language(Some("en"));
                    params.set_print_special(false);
                    params.set_print_progress(false);
                    params.set_print_realtime(false);
                    params.set_print_timestamps(false);
                    params.set_initial_prompt(&initial_prompt);
                    state.full(params, &samples).context("whisper full() failed")?;

                    let n_seg = state.full_n_segments().context("seg count failed")?;
                    let mut text = String::new();
                    for i in 0..n_seg {
                        if let Ok(s) = state.full_get_segment_text(i) {
                            text.push_str(&s);
                        }
                    }

                    let cleaned = scrub(&text);
                    if cleaned.is_empty() {
                        eprintln!("        (no speech detected)");
                        continue;
                    }
                    eprintln!("        -> \"{}\"", cleaned);

                    *last_text.lock().unwrap() = Some(cleaned.clone());
                    if let Ok(mut cb) = Clipboard::new() {
                        let _ = cb.set_text(cleaned.clone());
                    }

                    match paste_cmd_v() {
                        Ok(_) => eprintln!("        (typed at cursor. Right Option re-pastes it.)"),
                        Err(e) => eprintln!("        auto-paste failed: {e}. Press Cmd+V to paste manually."),
                    }
                }
            }
        }
    }
    Ok(())
}

fn build_stream(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    channels: usize,
    buffer: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream> {
    let config: cpal::StreamConfig = supported.config();
    let err_fn = |err| eprintln!("audio stream error: {err}");

    let stream = match supported.sample_format() {
        SampleFormat::F32 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _: &_| {
                    let mut b = buf.lock().unwrap();
                    if channels == 1 {
                        b.extend_from_slice(data);
                    } else {
                        for frame in data.chunks(channels) {
                            let mono: f32 = frame.iter().copied().sum::<f32>() / channels as f32;
                            b.push(mono);
                        }
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I16 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _: &_| {
                    let mut b = buf.lock().unwrap();
                    for frame in data.chunks(channels) {
                        let mono: f32 = frame
                            .iter()
                            .map(|&s| s as f32 / 32768.0)
                            .sum::<f32>()
                            / channels as f32;
                        b.push(mono);
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U16 => {
            let buf = buffer.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _: &_| {
                    let mut b = buf.lock().unwrap();
                    for frame in data.chunks(channels) {
                        let mono: f32 = frame
                            .iter()
                            .map(|&s| (s as f32 - 32768.0) / 32768.0)
                            .sum::<f32>()
                            / channels as f32;
                        b.push(mono);
                    }
                },
                err_fn,
                None,
            )?
        }
        fmt => anyhow::bail!("unsupported sample format: {fmt:?}"),
    };
    Ok(stream)
}

fn spawn_hotkey_listener(tx: Sender<Cmd>) {
    thread::spawn(move || {
        let rec_held = Arc::new(AtomicBool::new(false));
        let paste_held = Arc::new(AtomicBool::new(false));
        let rh = rec_held.clone();
        let ph = paste_held.clone();
        let tx_clone = tx.clone();
        if let Err(e) = listen(move |event: Event| match event.event_type {
            EventType::KeyPress(k) if k == RECORD_KEY => {
                if !rh.swap(true, Ordering::Relaxed) {
                    let _ = tx_clone.send(Cmd::ToggleRecord);
                }
            }
            EventType::KeyRelease(k) if k == RECORD_KEY => {
                rh.store(false, Ordering::Relaxed);
            }
            EventType::KeyPress(k) if k == PASTE_KEY => {
                if !ph.swap(true, Ordering::Relaxed) {
                    let _ = tx_clone.send(Cmd::RepasteLast);
                }
            }
            EventType::KeyRelease(k) if k == PASTE_KEY => {
                ph.store(false, Ordering::Relaxed);
            }
            _ => {}
        }) {
            eprintln!("hotkey listener failed: {e:?}");
            eprintln!("Grant Accessibility permission in System Settings -> Privacy & Security -> Accessibility.");
            eprintln!("Then quit (Ctrl-C) and re-launch this app.");
        }
    });
}

fn resample_linear(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate {
        return input.to_vec();
    }
    let ratio = in_rate as f64 / out_rate as f64;
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    let last = input.len().saturating_sub(1);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let i0 = (src.floor() as usize).min(last);
        let i1 = (i0 + 1).min(last);
        let frac = (src - i0 as f64) as f32;
        out.push(input[i0] * (1.0 - frac) + input[i1] * frac);
    }
    out
}

fn scrub(text: &str) -> String {
    let mut s = text.to_string();

    // remove leading/trailing whitespace per segment join
    s = s.trim().to_string();

    // Common filler tokens (case-insensitive). Be conservative: only kill when surrounded by word boundaries.
    let fillers = Regex::new(
        r"(?i)\b(uh+|um+|er+|erm+|ah+|hmm+|mm+m*|like|you know|i mean|kind of|sort of)\b[,.]?\s*"
    ).unwrap();
    s = fillers.replace_all(&s, "").to_string();

    // collapse multiple spaces first so word-boundary scanning is simple
    let spaces = Regex::new(r"\s+").unwrap();
    s = spaces.replace_all(&s, " ").to_string();

    // immediate word repetition: "the the patient" -> "the patient"
    // The `regex` crate doesn't support backreferences, so do this in plain Rust.
    s = dedup_adjacent_words(&s);

    // tidy space-before-punctuation
    let pun = Regex::new(r"\s+([,.!?;:])").unwrap();
    s = pun.replace_all(&s, "$1").to_string();

    s.trim().to_string()
}

fn dedup_adjacent_words(s: &str) -> String {
    let mut out: Vec<&str> = Vec::with_capacity(64);
    for w in s.split(' ').filter(|w| !w.is_empty()) {
        let last_alnum: String = out
            .last()
            .map(|p| p.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .unwrap_or_default();
        let this_alnum: String = w
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        if !this_alnum.is_empty() && this_alnum == last_alnum {
            continue;
        }
        out.push(w);
    }
    out.join(" ")
}

/// Play a short macOS system sound, non-blocking. Failure is silent (no audio
/// is annoying but not a reason to crash the app).
fn play_sound(name: &str) {
    let _ = Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{name}.aiff"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn play_start_sound() {
    // Short, high, "go" tick. Doesn't delay the user from talking.
    play_sound("Pop");
}

fn play_stop_sound() {
    // Cleaner "done" tone, lower than start. Distinct enough to know apart.
    play_sound("Glass");
}

fn paste_cmd_v() -> Result<()> {
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| anyhow::anyhow!("enigo init: {e:?}"))?;
    thread::sleep(Duration::from_millis(80));
    enigo
        .key(EKey::Meta, Direction::Press)
        .map_err(|e| anyhow::anyhow!("press cmd: {e:?}"))?;
    enigo
        .key(EKey::Unicode('v'), Direction::Click)
        .map_err(|e| anyhow::anyhow!("click v: {e:?}"))?;
    enigo
        .key(EKey::Meta, Direction::Release)
        .map_err(|e| anyhow::anyhow!("release cmd: {e:?}"))?;
    Ok(())
}

fn num_cpus_safe() -> std::os::raw::c_int {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    n.min(8) as std::os::raw::c_int
}
