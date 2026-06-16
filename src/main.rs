// Atlas Intensive Care Dictation — local medical speech-to-text, no network.
// ` = record toggle (auto-pastes at cursor on stop). Right Option = re-paste.
//
// Architecture:
//   main thread       : tao event loop + NSStatusItem (menubar icon, Quit menu)
//   hotkey thread     : rdev::listen, fires Cmd::ToggleRecord / RepasteLast
//   pipeline thread   : owns WhisperContext + cpal::Device + state. Processes Cmd.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

use crossbeam_channel::{unbounded, Receiver, Sender};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key as EKey, Keyboard, Settings};
use regex::Regex;

use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

const MODEL_NAME: &str = "ggml-large-v3-turbo.bin";
const DICT_NAME: &str = "medical-dictionary.txt";
const TARGET_SR: u32 = 16_000;

// macOS virtual keycodes (kVK_*).
const KC_TILDE: i64 = 50;          // kVK_ANSI_Grave
const KC_RIGHT_OPTION: i64 = 61;   // kVK_RightOption
// NX_DEVICERALTKEYMASK — set in CGEventFlags when right Option is currently held.
const FLAG_RIGHT_OPTION_BIT: u64 = 0x00000040;

#[derive(Debug, Clone, Copy)]
enum Cmd {
    ToggleRecord,
    RepasteLast,
}

#[derive(Debug, Clone, Copy)]
enum TrayState {
    Idle,
    Recording,
}

fn main() -> Result<()> {
    redirect_stderr_when_bundled();
    eprintln!("Atlas Intensive Care Dictation v0.2");
    eprintln!("====================================");
    eprintln!();
    eprintln!("NOTICE: Local dictation around whisper.cpp Turbo + medical vocabulary.");
    eprintln!("Speech recognition is not perfect. You are responsible for proofreading");
    eprintln!("every transcript before clinical, billing, or legal use. See DISCLAIMER.md.");
    eprintln!();
    eprintln!("Hotkeys: ` (tilde) = start/stop dictation. Right Option = re-paste last.");
    eprintln!("Quit from the menu-bar icon (audio-bars glyph in the top-right).");
    eprintln!();

    let model_path = resolve(MODEL_NAME, "models/ggml-large-v3-turbo.bin");
    let dict_path = resolve(DICT_NAME, "assets/medical-dictionary.txt");
    if !model_path.exists() {
        anyhow::bail!(
            "Model file missing at {}.\n\
             Download with:\n  \
             curl -L -o {} https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
            model_path.display(),
            model_path.display()
        );
    }

    let (tx_cmd, rx_cmd) = unbounded::<Cmd>();
    let (tx_state, rx_state) = unbounded::<TrayState>();

    spawn_event_tap(tx_cmd);

    let tx_state_pipeline = tx_state.clone();
    thread::spawn(move || {
        if let Err(e) = pipeline_main(rx_cmd, tx_state_pipeline, model_path, dict_path) {
            eprintln!("PIPELINE FATAL: {e:#}");
        }
    });

    run_event_loop_with_tray(rx_state)
}

// ─── Pipeline (worker thread) ────────────────────────────────────────────────

fn pipeline_main(
    rx_cmd: Receiver<Cmd>,
    tx_state: Sender<TrayState>,
    model_path: PathBuf,
    dict_path: PathBuf,
) -> Result<()> {
    eprintln!("Loading Whisper Turbo model (CPU/BLAS)...");
    let mut cparams = WhisperContextParameters::default();
    cparams.use_gpu(false); // Metal JIT-compile broken in whisper-rs 0.13; BLAS is fast enough.
    let ctx = WhisperContext::new_with_params(model_path.to_str().unwrap(), cparams)
        .context("failed to load whisper model")?;
    eprintln!("  Model ready.");

    let dict_raw = std::fs::read_to_string(&dict_path).unwrap_or_default();
    let mut initial_prompt: String = dict_raw
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    const PROMPT_CHAR_BUDGET: usize = 1100;
    if initial_prompt.len() > PROMPT_CHAR_BUDGET {
        eprintln!(
            "  WARNING: biasing prompt is {} chars, truncating to {}.",
            initial_prompt.len(),
            PROMPT_CHAR_BUDGET
        );
        initial_prompt.truncate(PROMPT_CHAR_BUDGET);
    }
    eprintln!(
        "  Biasing prompt: {} chars (~{} words).",
        initial_prompt.len(),
        initial_prompt.split_whitespace().count()
    );

    let host = cpal::default_host();
    let device = host.default_input_device().context("no default input device")?;
    eprintln!("  Mic: {}", device.name().unwrap_or_else(|_| "default".into()));
    let supported = device
        .default_input_config()
        .context("failed to query input config")?;
    let input_sr = supported.sample_rate().0;
    let channels = supported.channels() as usize;

    let buffer: Arc<Mutex<Vec<f32>>> =
        Arc::new(Mutex::new(Vec::with_capacity(input_sr as usize * 30)));
    let mut active_stream: Option<cpal::Stream> = None;
    let last_text: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    eprintln!("Ready.");

    loop {
        let cmd = match rx_cmd.recv() {
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
                    None => eprintln!("[PASTE] (nothing dictated yet — press ` first)"),
                }
            }
            Cmd::ToggleRecord => {
                if active_stream.is_none() {
                    buffer.lock().unwrap().clear();
                    match build_stream(&device, &supported, channels, buffer.clone()) {
                        Ok(s) => {
                            if let Err(e) = s.play() {
                                eprintln!("[REC]   failed to start mic: {e}");
                                continue;
                            }
                            active_stream = Some(s);
                            play_start_sound();
                            let _ = tx_state.send(TrayState::Recording);
                            eprintln!("[REC]   speak now... (` again to stop)");
                        }
                        Err(e) => eprintln!("[REC]   failed to open mic: {e}"),
                    }
                } else {
                    drop(active_stream.take());
                    play_stop_sound();
                    let _ = tx_state.send(TrayState::Idle);
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
                    params.set_n_threads(
                        std::thread::available_parallelism()
                            .map(|n| n.get())
                            .unwrap_or(4)
                            .min(8) as _,
                    );
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
                    // Never log transcript text — it might contain PHI and the log
                    // persists when launched as a .app. Length only confirms success.
                    eprintln!(
                        "        -> ({} chars, {} words)",
                        cleaned.len(),
                        cleaned.split_whitespace().count()
                    );

                    *last_text.lock().unwrap() = Some(cleaned.clone());
                    if let Ok(mut cb) = Clipboard::new() {
                        let _ = cb.set_text(cleaned.clone());
                    }

                    match paste_cmd_v() {
                        Ok(_) => eprintln!("        (typed at cursor. Right Option re-pastes.)"),
                        Err(e) => eprintln!("        auto-paste failed: {e}. Cmd+V manually."),
                    }
                }
            }
        }
    }
    Ok(())
}

// ─── Event loop + tray icon (main thread) ────────────────────────────────────

fn run_event_loop_with_tray(rx_state: Receiver<TrayState>) -> Result<()> {
    let event_loop = EventLoopBuilder::new().build();

    let icon = load_tray_icon()?;
    let menu = Menu::new();
    let quit_item = MenuItem::new("Quit Atlas Dictation", true, None);
    menu.append(&quit_item)
        .map_err(|e| anyhow::anyhow!("menu append: {e}"))?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Atlas Dictation")
        .build()
        .map_err(|e| anyhow::anyhow!("tray build: {e}"))?;

    let quit_id = quit_item.id().clone();

    event_loop.run(move |_event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200));

        if let Ok(menu_event) = MenuEvent::receiver().try_recv() {
            if menu_event.id == quit_id {
                eprintln!("Quit selected.");
                *control_flow = ControlFlow::ExitWithCode(0);
            }
        }

        if let Ok(state) = rx_state.try_recv() {
            match state {
                TrayState::Idle => {
                    // Tooltip updates only — same icon, distinguished by status.
                    // (set_tooltip on tray requires Arc; v0.3 polish.)
                }
                TrayState::Recording => {
                    // Same — see above.
                }
            }
        }
    });
}

fn load_tray_icon() -> Result<Icon> {
    let bytes = include_bytes!("../packaging/tray-icon.png");
    let img = image::load_from_memory(bytes)
        .context("decode tray icon")?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h)
        .map_err(|e| anyhow::anyhow!("icon from rgba: {e}"))
}

// ─── Audio capture (cpal) ────────────────────────────────────────────────────

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

// ─── Hotkey listener: Core Graphics event tap (active mode) ──────────────────
//
// Replaces rdev::listen. Differences that matter:
//   - The tap is in *active* mode (CGEventTapOptions::Default), so returning
//     `None` from the callback SWALLOWS the keypress. The tilde never reaches
//     the focused app; no stray `\`` characters get typed.
//   - The tap runs on its own thread driving a CFRunLoop. macOS delivers
//     events from the kernel into that run loop.

fn spawn_event_tap(tx: Sender<Cmd>) {
    thread::spawn(move || {
        use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
        use core_graphics::event::{
            CGEventTap, CGEventTapLocation, CGEventTapOptions,
            CGEventTapPlacement, CGEventType,
        };

        // Single-threaded state used only inside the tap callback.
        let right_option_held = std::cell::Cell::new(false);

        let tap_result = CGEventTap::new(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default, // active — we can drop/modify events
            vec![CGEventType::KeyDown, CGEventType::FlagsChanged],
            move |_proxy, ev_type, event| {
                let kc = event.get_integer_value_field(9 /* kCGKeyboardEventKeycode */);
                match ev_type {
                    CGEventType::KeyDown if kc == KC_TILDE => {
                        let autorepeat = event.get_integer_value_field(
                            8, /* kCGKeyboardEventAutorepeat */
                        );
                        if autorepeat == 0 {
                            let _ = tx.send(Cmd::ToggleRecord);
                        }
                        None // swallow — no `\`` ever lands in the focused app
                    }
                    CGEventType::FlagsChanged if kc == KC_RIGHT_OPTION => {
                        let bits = event.get_flags().bits();
                        let now_held = (bits & FLAG_RIGHT_OPTION_BIT) != 0;
                        let was_held = right_option_held.get();
                        if now_held && !was_held {
                            let _ = tx.send(Cmd::RepasteLast);
                        }
                        right_option_held.set(now_held);
                        // Pass modifier through — don't break other apps that use Right Option.
                        Some(event.clone())
                    }
                    _ => Some(event.clone()),
                }
            },
        );

        let tap = match tap_result {
            Ok(t) => t,
            Err(_) => {
                eprintln!("CGEventTap create failed (likely missing Accessibility).");
                eprintln!("Grant Accessibility: System Settings -> Privacy & Security -> Accessibility.");
                eprintln!("Add Atlas Dictation, toggle ON, Quit (menu-bar icon), and re-launch.");
                return;
            }
        };

        let loop_source = match tap.mach_port.create_runloop_source(0) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("CFRunLoop source create failed.");
                return;
            }
        };
        unsafe {
            CFRunLoop::get_current().add_source(&loop_source, kCFRunLoopCommonModes);
        }
        tap.enable();
        CFRunLoop::run_current(); // blocks; the OS delivers events here.
    });
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// When launched as a `.app` (no Terminal), redirect stderr to a log file so
/// the [REC]/[STOP]/transcript output is recoverable. Tail with:
///   tail -f ~/Library/Logs/AtlasDictation/dictation.log
fn redirect_stderr_when_bundled() {
    let in_bundle = std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS"))
        .unwrap_or(false);
    if !in_bundle {
        return;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = format!("{home}/Library/Logs/AtlasDictation");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{dir}/dictation.log"))
    {
        use std::os::unix::io::AsRawFd;
        unsafe { libc::dup2(f.as_raw_fd(), 2); }
        std::mem::forget(f);
    }
}

/// Prefer the `.app` bundle's Contents/Resources/<name>, fall back to the
/// dev-mode project-relative path. Same binary works in `cargo run` and inside
/// AtlasDictation.app.
fn resolve(bundle_name: &str, dev_path: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos) = exe.parent() {
            if let Some(contents) = macos.parent() {
                let p = contents.join("Resources").join(bundle_name);
                if p.exists() {
                    return p;
                }
            }
        }
    }
    PathBuf::from(dev_path)
}

fn play_sound(name: &str) {
    let _ = Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{name}.aiff"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
fn play_start_sound() { play_sound("Pop"); }
fn play_stop_sound() { play_sound("Glass"); }

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
    let mut s = text.trim().to_string();
    let fillers = Regex::new(
        r"(?i)\b(uh+|um+|er+|erm+|ah+|hmm+|mm+m*|like|you know|i mean|kind of|sort of)\b[,.]?\s*"
    ).unwrap();
    s = fillers.replace_all(&s, "").to_string();
    let spaces = Regex::new(r"\s+").unwrap();
    s = spaces.replace_all(&s, " ").to_string();
    s = dedup_adjacent_words(&s);
    let pun = Regex::new(r"\s+([,.!?;:])").unwrap();
    s = pun.replace_all(&s, "$1").to_string();
    s.trim().to_string()
}

fn dedup_adjacent_words(s: &str) -> String {
    let strip = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    let mut out: Vec<&str> = Vec::with_capacity(64);
    for w in s.split(' ').filter(|w| !w.is_empty()) {
        let this = strip(w);
        let last = out.last().map(|p| strip(p)).unwrap_or_default();
        if !this.is_empty() && this.eq_ignore_ascii_case(&last) {
            continue;
        }
        out.push(w);
    }
    out.join(" ")
}
