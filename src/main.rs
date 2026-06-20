// Atlas Intensive Care Dictation — local medical speech-to-text, no network.
// ` = record toggle (auto-pastes at cursor on stop). Right Option = re-paste.
//
// Architecture:
//   main thread       : tao event loop + NSStatusItem (menubar icon, Quit menu)
//   hotkey thread     : CGEventTap in active mode (swallows tilde keydown,
//                       passes Right Option through). Fires Cmd::ToggleRecord
//                       / Cmd::RepasteLast on its own CFRunLoop.
//   pipeline thread   : owns WhisperContext + cpal::Device + state. Processes Cmd.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

use crossbeam_channel::{unbounded, Receiver, Sender};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key as EKey, Keyboard, Settings};
use regex::Regex;

use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIconBuilder};

const MODEL_NAME: &str = "ggml-large-v3-turbo.bin";
const DICT_NAME: &str = "medical-dictionary.txt";
const TARGET_SR: u32 = 16_000;
// Hard cap on a single dictation. Long enough for a thoughtful HPI / SOAP
// (most run 2–3 min), short enough that "left it recording at lunch" can't
// eat unbounded RAM. ponytail: tune by editing the constant — no settings UI.
const MAX_RECORD_SECS: u64 = 180;
// How long to keep the last transcript in RAM for Right-Option re-paste.
// After this, last_text is dropped so PHI doesn't sit around in memory.
const LAST_TEXT_TTL_SECS: u64 = 120;
// How long a transcript survives in the on-disk history file before it's
// purged. The history exists so a long dictation can't be lost to a clipboard
// overwrite or the RAM TTL above; the window is short so PHI doesn't linger.
const TRANSCRIPT_TTL_SECS: u64 = 2 * 3600;
// Prune the history no more than once a minute while idle — the cap and
// last_text TTL already tick every second, so we piggyback on that loop.
const HISTORY_PRUNE_EVERY_SECS: u64 = 60;

// macOS virtual keycodes (kVK_*).
const KC_TILDE: i64 = 50;          // kVK_ANSI_Grave
const KC_RIGHT_OPTION: i64 = 61;   // kVK_RightOption
// NX_DEVICERALTKEYMASK — set in CGEventFlags when right Option is currently held.
const FLAG_RIGHT_OPTION_BIT: u64 = 0x00000040;

#[derive(Debug, Clone)]
enum Cmd {
    ToggleRecord,
    RepasteLast,
    SetMic(String),
}

#[derive(Debug, Clone, Copy)]
enum TrayState {
    Idle,
    Recording,
}

fn main() -> Result<()> {
    redirect_stderr_when_bundled();
    eprintln!("Atlas Intensive Care Dictation v0.3");
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

    spawn_event_tap(tx_cmd.clone());

    let tx_state_pipeline = tx_state.clone();
    thread::spawn(move || {
        if let Err(e) = pipeline_main(rx_cmd, tx_state_pipeline, model_path, dict_path) {
            eprintln!("PIPELINE FATAL: {e:#}");
        }
    });

    run_event_loop_with_tray(rx_state, tx_cmd)
}

// ─── Pipeline (worker thread) ────────────────────────────────────────────────

#[allow(unused_assignments)]
fn pipeline_main(
    rx_cmd: Receiver<Cmd>,
    tx_state: Sender<TrayState>,
    model_path: PathBuf,
    dict_path: PathBuf,
) -> Result<()> {
    let (use_gpu, backend_label) = pick_backend();
    eprintln!("Loading Whisper Turbo model ({backend_label})...");
    let mut cparams = WhisperContextParameters::default();
    cparams.use_gpu(use_gpu);
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

    // Pick the mic ONCE and keep it for the whole session. Re-querying the
    // default device per recording used to "follow" macOS when the system
    // default changed mid-session (AirPods connect, USB unplug) — which silently
    // fed silence/garbage into a dictation. Pin it instead; set $ATLAS_MIC to a
    // substring of the device name to lock a specific mic.
    let host = cpal::default_host();
    let mut input_device = select_input_device(&host).context("no usable input device")?;
    eprintln!(
        "  Mic (pinned for this session): {}",
        input_device.name().unwrap_or_else(|_| "default".into())
    );
    // Placeholders — overwritten on every recording start before any read.
    // Declared here so the stop branch can read them across loop iterations.
    let mut input_sr: u32 = TARGET_SR;
    let mut channels: usize = 1;

    // 30s @ 48 kHz mono is the realistic worst case for one dictation.
    // cpal callbacks extend the vec if we go over.
    let buffer: Arc<Mutex<Vec<f32>>> =
        Arc::new(Mutex::new(Vec::with_capacity(48_000 * 30)));
    let mut active_stream: Option<cpal::Stream> = None;
    let mut recording_started: Option<Instant> = None;
    let last_text: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut last_text_at: Option<Instant> = None;
    let mut last_history_prune = Instant::now();

    eprintln!("Ready.");

    loop {
        // Tick once a second so we can enforce MAX_RECORD_SECS and LAST_TEXT_TTL_SECS
        // without a dedicated timer thread.
        let cmd = match rx_cmd.recv_timeout(Duration::from_secs(1)) {
            Ok(c) => c,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if last_history_prune.elapsed() >= Duration::from_secs(HISTORY_PRUNE_EVERY_SECS) {
                    prune_transcript_history();
                    last_history_prune = Instant::now();
                }
                if let Some(at) = last_text_at {
                    if at.elapsed() >= Duration::from_secs(LAST_TEXT_TTL_SECS) {
                        *last_text.lock().unwrap() = None;
                        last_text_at = None;
                        eprintln!("[TTL]   dropped last transcript from RAM.");
                    }
                }
                if let Some(started) = recording_started {
                    if started.elapsed() >= Duration::from_secs(MAX_RECORD_SECS) {
                        eprintln!("[CAP]   {}s reached — auto-stopping.", MAX_RECORD_SECS);
                        Cmd::ToggleRecord
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            Err(_) => break,
        };

        match cmd {
            Cmd::SetMic(name) => {
                match find_input_by_substr(&host, &name) {
                    Some(d) => {
                        let resolved = d.name().unwrap_or_else(|_| name.clone());
                        input_device = d;
                        save_mic_pref(&resolved);
                        eprintln!("[MIC]   switched to {resolved} (saved; applies to next dictation).");
                    }
                    None => eprintln!("[MIC]   '{name}' not found; keeping current mic."),
                }
            }
            Cmd::RepasteLast => {
                let stored = last_text.lock().unwrap().clone();
                match stored {
                    Some(t) => {
                        if let Ok(mut cb) = Clipboard::new() {
                            let _ = cb.set_text(t);
                        }
                        last_text_at = Some(Instant::now()); // reset TTL on active use
                        if env_flag("ATLAS_NO_AUTO_PASTE") {
                            eprintln!("[PASTE] (re-loaded to clipboard. Cmd+V to paste.)");
                        } else {
                            match paste_cmd_v() {
                                Ok(_) => eprintln!("[PASTE] (re-paste at cursor)"),
                                Err(e) => eprintln!("[PASTE] failed: {e}"),
                            }
                        }
                    }
                    None => eprintln!("[PASTE] (nothing dictated yet — press ` first)"),
                }
            }
            Cmd::ToggleRecord => {
                if active_stream.is_none() {
                    buffer.lock().unwrap().clear();
                    let device = &input_device; // pinned at startup — never re-queried
                    let supported = match device.default_input_config() {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("[REC]   query input config failed: {e}");
                            continue;
                        }
                    };
                    input_sr = supported.sample_rate().0;
                    channels = supported.channels() as usize;
                    let mic_name = device.name().unwrap_or_else(|_| "default".into());
                    match build_stream(device, &supported, channels, buffer.clone()) {
                        Ok(s) => {
                            if let Err(e) = s.play() {
                                eprintln!("[REC]   failed to start mic: {e}");
                                continue;
                            }
                            active_stream = Some(s);
                            recording_started = Some(Instant::now());
                            play_start_sound();
                            let _ = tx_state.send(TrayState::Recording);
                            // Tilde-leak cleanup. Off by default — the CGEventTap
                            // should swallow the keypress on a clean Mac. Set
                            // ATLAS_BACKSPACE_GUARD=1 if a third-party HID tap
                            // (Wacom/OBS/MOTIV) is re-emitting the `\`` despite
                            // our None-return. See feedback-atlas-dictation-tcc
                            // rule 5. ponytail: env flag now, tray CheckMenuItem
                            // when settings UI lands.
                            if env_flag("ATLAS_BACKSPACE_GUARD") {
                                send_backspace();
                            }
                            eprintln!(
                                "[REC]   speak now via {mic_name} ({} Hz, {} ch). \
                                 ` again, or auto-stop in {}s.",
                                input_sr, channels, MAX_RECORD_SECS
                            );
                        }
                        Err(e) => eprintln!("[REC]   failed to open mic: {e}"),
                    }
                } else {
                    drop(active_stream.take());
                    recording_started = None;
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
                    last_text_at = Some(Instant::now());
                    save_transcript(&cleaned);
                    if let Ok(mut cb) = Clipboard::new() {
                        let _ = cb.set_text(cleaned.clone());
                    }

                    if env_flag("ATLAS_NO_AUTO_PASTE") {
                        // Clipboard-only mode: bulletproof against Terminal /
                        // iTerm / Electron focus races. User Cmd+V's themselves.
                        eprintln!("        (on clipboard. Cmd+V to paste, Right Option re-pastes.)");
                    } else {
                        if env_flag("ATLAS_BACKSPACE_GUARD") {
                            send_backspace();
                        }
                        match paste_cmd_v() {
                            Ok(_) => eprintln!("        (typed at cursor. Right Option re-pastes.)"),
                            Err(e) => eprintln!("        auto-paste failed: {e}. Cmd+V manually."),
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// ─── Event loop + tray icon (main thread) ────────────────────────────────────

fn run_event_loop_with_tray(rx_state: Receiver<TrayState>, tx_cmd: Sender<Cmd>) -> Result<()> {
    let event_loop = EventLoopBuilder::new().build();

    let icon = load_tray_icon()?;
    let menu = Menu::new();

    // Microphone submenu — one CheckMenuItem per input device, check on the
    // active one. Clicking sends Cmd::SetMic to the pipeline, which switches +
    // persists the choice. Built once at launch; relaunch to see hot-plugged mics.
    let host = cpal::default_host();
    let mic_names: Vec<String> = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let active_mic = select_input_device(&host).and_then(|d| d.name().ok());

    let mic_menu = Submenu::new("Microphone", true);
    let mut mic_items: Vec<(MenuId, String, CheckMenuItem)> = Vec::new();
    for name in &mic_names {
        let checked = active_mic.as_deref() == Some(name.as_str());
        let item = CheckMenuItem::new(name, true, checked, None);
        let _ = mic_menu.append(&item);
        mic_items.push((item.id().clone(), name.clone(), item));
    }
    menu.append(&mic_menu)
        .map_err(|e| anyhow::anyhow!("menu append: {e}"))?;
    let _ = menu.append(&PredefinedMenuItem::separator());

    let quit_item = MenuItem::new("Quit Atlas Dictation", true, None);
    menu.append(&quit_item)
        .map_err(|e| anyhow::anyhow!("menu append: {e}"))?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
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
            } else if let Some((_, name, _)) =
                mic_items.iter().find(|(id, _, _)| *id == menu_event.id)
            {
                eprintln!("Mic selected from menu: {name}");
                let _ = tx_cmd.send(Cmd::SetMic(name.clone()));
                // Reflect the choice immediately: check the picked one, clear rest.
                for (_, n, item) in &mic_items {
                    item.set_checked(n == name);
                }
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

/// First input device whose name contains `want` (case-insensitive). None on
/// empty/no-match.
fn find_input_by_substr(host: &cpal::Host, want: &str) -> Option<cpal::Device> {
    let want = want.trim().to_ascii_lowercase();
    if want.is_empty() {
        return None;
    }
    host.input_devices().ok()?.find(|d| {
        d.name().map(|n| n.to_ascii_lowercase().contains(&want)).unwrap_or(false)
    })
}

/// Where the tray mic-picker's choice persists. Plain text (one device name),
/// no JSON dep needed for a single value. ponytail: text file, not a config DB.
fn mic_pref_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!(
        "{home}/Library/Application Support/AtlasDictation/selected-mic.txt"
    ))
}

fn saved_mic_pref() -> Option<String> {
    let s = std::fs::read_to_string(mic_pref_path()).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn save_mic_pref(name: &str) {
    let path = mic_pref_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, name);
}

/// Pick the input device ONCE per session (or when the tray picker switches it).
/// Priority: $ATLAS_MIC override → saved tray choice → system default. The
/// substring match means a mid-session default-device swap (AirPods, USB unplug)
/// can't silently feed silence into a dictation.
fn select_input_device(host: &cpal::Host) -> Option<cpal::Device> {
    if let Ok(env) = std::env::var("ATLAS_MIC") {
        if let Some(d) = find_input_by_substr(host, &env) {
            return Some(d);
        }
        if !env.trim().is_empty() {
            eprintln!("  WARNING: ATLAS_MIC=\"{}\" matched no input device.", env.trim());
        }
    }
    if let Some(saved) = saved_mic_pref() {
        if let Some(d) = find_input_by_substr(host, &saved) {
            return Some(d);
        }
        eprintln!("  WARNING: saved mic \"{saved}\" not found; using default.");
    }
    host.default_input_device()
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
            vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
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
                    CGEventType::KeyUp if kc == KC_TILDE => None, // belt+suspenders: swallow KeyUp too
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

/// Pick fastest *working* backend.
/// whisper-rs 0.13 Metal JIT-compile crashes on Apple Silicon (shader error at
/// program_source:6735 → ggml_backend_metal_init fails → silent fallback to
/// BLAS anyway). So just go straight to BLAS — no wasted Metal allocation,
/// honest label. ponytail: revisit when bumping whisper-rs past 0.13.
fn pick_backend() -> (bool, String) {
    (false, format!("CPU/BLAS, {}", cpu_brand()))
}

fn cpu_brand() -> String {
    Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown CPU".into())
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

/// Cmd+V via AppleScript / System Events (key code 9 = `v`).
///
/// Why this instead of synthetic HID keystrokes (enigo)? AppleScript routes
/// the paste through System Events, which delivers it as the frontmost app's
/// standard Cmd+V menu action. Survives:
///   - Electron focus races (chat boxes, Cursor, VS Code)
///   - Terminal/iTerm "block synthetic keystrokes" security settings
///   - Browsers that drop the first HID-layer keystroke after focus change
/// OpenWhispr ships the same path. ponytail: replace with a bundled CGEvent
/// fast-paste helper only if osascript ever shows latency in practice.
fn paste_cmd_v() -> Result<()> {
    if osa_paste()? {
        return Ok(());
    }
    // One retry with breathing room for the target window's focus settle.
    // Matches OpenWhispr's pasteMacOS retry (200 ms).
    thread::sleep(Duration::from_millis(200));
    if osa_paste()? {
        return Ok(());
    }
    anyhow::bail!("osascript paste returned non-zero twice")
}

fn osa_paste() -> Result<bool> {
    let status = Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to key code 9 using command down",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("spawn osascript")?;
    Ok(status.success())
}

/// Read a boolean-ish env var. `1`, `true`, `yes`, any non-empty non-`0` value
/// is true. Unset, empty, `0`, `false` is false. ponytail: env flags now,
/// promote to tray CheckMenuItems when a real settings UI ships.
fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "no"
        }
        Err(_) => false,
    }
}

/// Send one Backspace via enigo. Used to clean up a leaked ` character when
/// the CGEventTap's None-return doesn't actually suppress (third-party event
/// taps from Wacom / OBS / etc. can re-emit at HID and we can't see what they
/// inserted into the chain). ponytail: cheaper than chasing the right tap
/// ordering. Costs at most one keystroke per dictation cycle.
fn send_backspace() {
    let Ok(mut enigo) = Enigo::new(&Settings::default()) else { return };
    let _ = enigo.key(EKey::Backspace, Direction::Click);
}

// ─── Transcript history (local, auto-expiring) ───────────────────────────────
// Every transcript is appended to ~/Library/Logs/AtlasDictation/transcripts.txt
// so a long dictation can't be lost to a clipboard overwrite or the RAM TTL.
// Records older than TRANSCRIPT_TTL_SECS are purged on every append AND on the
// idle tick, so PHI doesn't sit on disk past the window. ponytail: flat file +
// full rewrite, not a DB — a 2h window only ever holds a handful of records.

fn transcript_history_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/Library/Logs/AtlasDictation/transcripts.txt"))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Local human time for a header line, via BSD `date -r`. Empty on failure —
/// the machine-readable epoch in the header is what pruning relies on.
fn human_time(epoch: u64) -> String {
    Command::new("date")
        .args(["-r", &epoch.to_string(), "+%Y-%m-%d %H:%M:%S"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Parse history into (epoch, body) records. Header line is
/// "### <epoch>  <human time>"; the body is every line until the next header.
fn parse_history(s: &str) -> Vec<(u64, String)> {
    let mut recs = Vec::new();
    let mut ts: Option<u64> = None;
    let mut body = String::new();
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("### ") {
            if let Some(t) = ts.take() {
                recs.push((t, std::mem::take(&mut body)));
            }
            ts = rest.split_whitespace().next().and_then(|t| t.parse().ok());
        } else if ts.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(t) = ts.take() {
        recs.push((t, body));
    }
    recs
}

fn render_history(recs: &[(u64, String)]) -> String {
    let mut out = String::new();
    for (ts, body) in recs {
        out.push_str(&format!("### {ts}  {}\n{}\n\n", human_time(*ts), body.trim_end()));
    }
    out
}

/// Append `text` to the history, dropping anything past the TTL in the same pass.
fn save_transcript(text: &str) {
    let path = transcript_history_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cutoff = unix_now().saturating_sub(TRANSCRIPT_TTL_SECS);
    let mut recs = parse_history(&std::fs::read_to_string(&path).unwrap_or_default());
    recs.retain(|(ts, _)| *ts >= cutoff);
    recs.push((unix_now(), text.trim_end().to_string()));
    let _ = std::fs::write(&path, render_history(&recs));
}

/// Drop expired records while idle, so PHI clears within the window even if no
/// new dictation triggers a rewrite. No-op (and no write) if nothing expired.
fn prune_transcript_history() {
    let path = transcript_history_path();
    let Ok(existing) = std::fs::read_to_string(&path) else { return };
    let cutoff = unix_now().saturating_sub(TRANSCRIPT_TTL_SECS);
    let mut recs = parse_history(&existing);
    let before = recs.len();
    recs.retain(|(ts, _)| *ts >= cutoff);
    if recs.len() != before {
        let _ = std::fs::write(&path, render_history(&recs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_roundtrips_and_prunes() {
        let now = unix_now();
        let recs = vec![
            (now - 3 * 3600, "old PHI".to_string()),   // older than 2h
            (now - 60, "recent note".to_string()),     // fresh
        ];
        let rendered = render_history(&recs);
        let parsed = parse_history(&rendered);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].1.trim(), "recent note");

        let cutoff = now.saturating_sub(TRANSCRIPT_TTL_SECS);
        let kept: Vec<_> = parsed.into_iter().filter(|(ts, _)| *ts >= cutoff).collect();
        assert_eq!(kept.len(), 1, "the 3h-old record must be pruned");
        assert_eq!(kept[0].1.trim(), "recent note");
    }
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
