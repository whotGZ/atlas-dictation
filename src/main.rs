// Atlas Intensive Care Dictation — local medical speech-to-text, no network.
// Single tap of Right Option = record toggle (auto-pastes at cursor on stop).
//
// Architecture:
//   main thread       : tao event loop + NSStatusItem (menubar icon, Quit menu)
//   hotkey thread     : CGEventTap that watches Right Option and passes it
//                       through untouched. Fires Cmd::ToggleRecord on its own
//                       CFRunLoop.
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

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadParams};

use arboard::Clipboard;
use regex::Regex;

use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIconBuilder};

const MODEL_NAME: &str = "ggml-large-v3-turbo.bin";
const DICT_NAME: &str = "medical-dictionary.txt";
// Silero VAD model. When present, Whisper only transcribes detected speech and
// skips silence/background noise — kills the "garbage from ambient noise"
// hallucinations. Small (~860 KB). Optional: if missing, we run without VAD.
const VAD_MODEL_NAME: &str = "ggml-silero-v5.1.2.bin";
const TARGET_SR: u32 = 16_000;
// Hard cap on a single dictation. Long enough for a thoughtful HPI / SOAP
// (most run 2–3 min), short enough that "left it recording at lunch" can't
// eat unbounded RAM. ponytail: tune by editing the constant — no settings UI.
const MAX_RECORD_SECS: u64 = 180;
// How long a transcript survives in the on-disk history file before it's
// purged. The history exists so a long dictation can't be lost to a clipboard
// overwrite; the window is short so PHI doesn't linger.
const TRANSCRIPT_TTL_SECS: u64 = 2 * 3600;
// Prune the history no more than once a minute while idle — the record cap
// already ticks every second, so we piggyback on that loop.
const HISTORY_PRUNE_EVERY_SECS: u64 = 60;

// macOS virtual keycodes (kVK_*). Right Option is the one and only hotkey: a
// single quick tap toggles dictation. It's a non-printing modifier, so even if
// macOS ignores our event tap nothing can ever be typed into the focused app.
// It's deliberately NOT Control/Command/Fn — those are reserved by the system's
// own Dictation "press-twice" shortcut.
#[cfg(target_os = "macos")]
const KC_RIGHT_OPTION: i64 = 61;   // kVK_RightOption
#[cfg(target_os = "macos")]
const FLAG_RIGHT_OPTION_BIT: u64 = 0x00000040; // NX_DEVICERALTKEYMASK
// A tap = press and release within this window with no other key in between.
// Holding ⌥ longer (e.g. to type ø, ¬) is not a tap and won't toggle.
const TAP_MAX_MS: u128 = 500;

#[derive(Debug, Clone)]
enum Cmd {
    ToggleRecord,
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
    eprintln!("Hotkey: single tap Right Option (⌥) = start/stop dictation. Re-paste anywhere with Cmd+V.");
    eprintln!("Quit from the menu-bar icon (audio-bars glyph in the top-right).");
    eprintln!();

    let model_path = resolve(MODEL_NAME, "models/ggml-large-v3-turbo.bin");
    let dict_path = resolve(DICT_NAME, "assets/medical-dictionary.txt");
    // Optional — VAD just makes us skip non-speech; run without it if absent.
    let vad_path = {
        let p = resolve(VAD_MODEL_NAME, "models/ggml-silero-v5.1.2.bin");
        p.exists().then_some(p)
    };
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

    spawn_hotkey(tx_cmd.clone());

    let tx_state_pipeline = tx_state.clone();
    thread::spawn(move || {
        if let Err(e) = pipeline_main(rx_cmd, tx_state_pipeline, model_path, dict_path, vad_path) {
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
    vad_path: Option<PathBuf>,
) -> Result<()> {
    let (use_gpu, backend_label) = pick_backend();
    eprintln!("Loading Whisper Turbo model ({backend_label})...");
    let mut cparams = WhisperContextParameters::default();
    cparams.use_gpu(use_gpu);
    let ctx = WhisperContext::new_with_params(model_path.to_str().unwrap(), cparams)
        .context("failed to load whisper model")?;
    eprintln!("  Model ready.");

    // VAD model path as a string, ready to hand to each transcription's params.
    let vad_model_str: Option<String> =
        vad_path.and_then(|p| p.to_str().map(String::from));
    match &vad_model_str {
        Some(_) => eprintln!("  Noise gate: VAD on (skips silence + background noise)."),
        None => eprintln!("  Noise gate: VAD off (model not bundled)."),
    }

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
                            eprintln!(
                                "[REC]   speak now via {mic_name} ({} Hz, {} ch). \
                                 Tap Right ⌥ again, or auto-stop in {}s.",
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
                    // Hard-pin English. Whisper has ONE "en" (it covers US, Indian,
                    // British… accents — there is no per-region model), so this is
                    // the right setting for US + India English. Disabling auto-detect
                    // stops it drifting to Chinese/Japanese on noise or accent.
                    params.set_detect_language(false);
                    params.set_print_special(false);
                    params.set_print_progress(false);
                    params.set_print_realtime(false);
                    params.set_print_timestamps(false);
                    params.set_initial_prompt(&initial_prompt);
                    // Noise gate: when the VAD model is present, Whisper runs it
                    // first and only transcribes detected speech — silence and
                    // background noise never reach the decoder, so they can't be
                    // hallucinated into words. Defaults (Silero) are tuned well for
                    // dictation; we don't override them.
                    if let Some(ref vp) = vad_model_str {
                        params.set_vad_model_path(Some(vp));
                        params.set_vad_params(WhisperVadParams::new());
                        params.enable_vad(true);
                    }
                    state.full(params, &samples).context("whisper full() failed")?;

                    let n_seg = state.full_n_segments();
                    let mut text = String::new();
                    for i in 0..n_seg {
                        if let Some(seg) = state.get_segment(i) {
                            if let Ok(s) = seg.to_str_lossy() {
                                text.push_str(&s);
                            }
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

                    save_transcript(&cleaned);
                    if let Ok(mut cb) = Clipboard::new() {
                        let _ = cb.set_text(cleaned.clone());
                    }

                    if env_flag("ATLAS_NO_AUTO_PASTE") {
                        // Clipboard-only mode: bulletproof against Terminal /
                        // iTerm / Electron focus races. User Cmd+V's themselves.
                        eprintln!("        (on clipboard. Cmd+V to paste, or paste again anywhere.)");
                    } else {
                        match paste_cmd_v() {
                            Ok(_) => eprintln!("        (typed at cursor. Cmd+V re-pastes elsewhere.)"),
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
    app_data_dir().join("selected-mic.txt")
}

/// Home directory, cross-platform (HOME on unix, USERPROFILE on Windows).
fn home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default()
}

/// Per-OS directory for app settings (e.g. the saved mic). Caller creates it.
fn app_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    return PathBuf::from(format!("{}/Library/Application Support/AtlasDictation", home_dir()));
    #[cfg(target_os = "windows")]
    return PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| home_dir()))
        .join("AtlasDictation");
    #[cfg(all(unix, not(target_os = "macos")))]
    return PathBuf::from(
        std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{}/.local/share", home_dir())),
    )
    .join("AtlasDictation");
}

/// Per-OS directory for logs + transcript history. Caller creates it.
fn app_log_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    return PathBuf::from(format!("{}/Library/Logs/AtlasDictation", home_dir()));
    #[cfg(target_os = "windows")]
    return PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| home_dir()))
        .join("AtlasDictation")
        .join("logs");
    #[cfg(all(unix, not(target_os = "macos")))]
    return PathBuf::from(
        std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| format!("{}/.local/state", home_dir())),
    )
    .join("AtlasDictation");
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

// ─── Hotkey listener ─────────────────────────────────────────────────────────
// Record toggle = a single quick tap of the Right Option / Right Alt key (a
// non-printing modifier, so a passed-through tap can never type a stray char).
//   - macOS: a Core Graphics event tap on its own CFRunLoop (below).
//   - Windows/Linux: rdev::listen (listen-only is fine — we never suppress).

#[cfg(target_os = "macos")]
fn spawn_hotkey(tx: Sender<Cmd>) {
    spawn_event_tap(tx);
}

/// Windows/Linux hotkey via rdev. Detects a clean quick tap of Right Alt
/// (rdev `Key::AltGr` — the key the macOS build calls Right Option). Any other
/// key pressed while it's held cancels the tap, so Alt-combos don't toggle.
#[cfg(not(target_os = "macos"))]
fn spawn_hotkey(tx: Sender<Cmd>) {
    use rdev::{listen, EventType, Key};
    thread::spawn(move || {
        let mut pressed_at: Option<Instant> = None;
        let mut tap_candidate = false;
        let cb = move |event: rdev::Event| match event.event_type {
            EventType::KeyPress(Key::AltGr) => {
                pressed_at = Some(Instant::now());
                tap_candidate = true;
            }
            EventType::KeyRelease(Key::AltGr) => {
                let quick = pressed_at
                    .map(|t| t.elapsed().as_millis() <= TAP_MAX_MS)
                    .unwrap_or(false);
                if tap_candidate && quick {
                    let _ = tx.send(Cmd::ToggleRecord);
                }
                tap_candidate = false;
            }
            EventType::KeyPress(_) => tap_candidate = false, // a combo, not a solo tap
            _ => {}
        };
        if let Err(e) = listen(cb) {
            eprintln!("Hotkey listener failed: {e:?}");
            eprintln!("On Linux this needs X11 (Wayland global hotkeys are not supported).");
        }
    });
}

#[cfg(target_os = "macos")]
fn spawn_event_tap(tx: Sender<Cmd>) {
    thread::spawn(move || {
        use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
        use core_graphics::event::{
            CGEventTap, CGEventTapLocation, CGEventTapOptions,
            CGEventTapPlacement, CGEventType,
        };

        // Single-threaded state used only inside the tap callback.
        let right_option_held = std::cell::Cell::new(false);
        // When Right Option went down, and whether the press is still a clean
        // tap candidate (any other key pressed in between cancels it, so ⌥-combos
        // like ⌥o don't toggle).
        let ropt_pressed_at: std::cell::Cell<Option<Instant>> = std::cell::Cell::new(None);
        let ropt_tap_candidate = std::cell::Cell::new(false);

        let tap_result = CGEventTap::new(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default, // active — we can drop/modify events
            vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
            move |_proxy, ev_type, event| {
                let kc = event.get_integer_value_field(9 /* kCGKeyboardEventKeycode */);
                match ev_type {
                    // Record toggle = a single quick tap of Right Option. It's a
                    // modifier (prints nothing) and we pass it through untouched,
                    // so even if macOS ignores our event tap no stray character
                    // can ever land in the focused app — the stray-mark bug class
                    // is gone by construction.
                    CGEventType::FlagsChanged if kc == KC_RIGHT_OPTION => {
                        let now_held = (event.get_flags().bits() & FLAG_RIGHT_OPTION_BIT) != 0;
                        let was_held = right_option_held.get();
                        if now_held && !was_held {
                            ropt_pressed_at.set(Some(Instant::now()));
                            ropt_tap_candidate.set(true);
                        } else if !now_held && was_held {
                            let quick = ropt_pressed_at
                                .get()
                                .map(|t| t.elapsed().as_millis() <= TAP_MAX_MS)
                                .unwrap_or(false);
                            if ropt_tap_candidate.get() && quick {
                                let _ = tx.send(Cmd::ToggleRecord);
                            }
                            ropt_tap_candidate.set(false);
                        }
                        right_option_held.set(now_held);
                        Some(event.clone()) // never swallow — ⌥ must keep working
                    }
                    // Any other key pressed while ⌥ is down → it's a combo, not a
                    // solo tap. Cancel the candidate so ⌥o, ⌥-arrows, etc. don't toggle.
                    CGEventType::KeyDown => {
                        ropt_tap_candidate.set(false);
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
#[cfg(unix)]
fn redirect_stderr_when_bundled() {
    // When launched from the GUI (no controlling terminal) stderr goes nowhere,
    // so point it at a logfile. `isatty(2)` is the portable "are we in a
    // terminal?" check — covers a macOS .app and a Linux desktop launch alike.
    if unsafe { libc::isatty(2) } == 1 {
        return; // running in a terminal — leave stderr on screen
    }
    let dir = app_log_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("dictation.log"))
    {
        use std::os::unix::io::AsRawFd;
        unsafe { libc::dup2(f.as_raw_fd(), 2); }
        std::mem::forget(f);
    }
}

#[cfg(windows)]
fn redirect_stderr_when_bundled() {
    // ponytail: Windows GUI stderr redirect needs freopen/AllocConsole gymnastics;
    // skip for the first cross-platform cut. Run from a console to see logs.
}

/// Find a bundled resource. Checks, in order: the macOS `.app/Contents/
/// Resources/`, the directory next to the executable (how Windows/Linux ship —
/// model + assets sit beside the .exe / binary), a `resources/` subdir there,
/// then the dev-mode project-relative path. Same binary works in `cargo run`
/// and inside any packaged form.
fn resolve(bundle_name: &str, dev_path: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let exe_dir = exe.parent().map(|p| p.to_path_buf());
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &exe_dir {
            // macOS: exe is .../Contents/MacOS/<bin> → ../Resources/<name>
            if let Some(contents) = dir.parent() {
                candidates.push(contents.join("Resources").join(bundle_name));
            }
            candidates.push(dir.join(bundle_name));
            candidates.push(dir.join("resources").join(bundle_name));
        }
        for c in candidates {
            if c.exists() {
                return c;
            }
        }
    }
    PathBuf::from(dev_path)
}

/// Pick fastest *working* backend.
/// Use the Apple GPU (Metal). whisper.cpp loads the Metal backend when it's
/// available and transparently falls back to BLAS/CPU if Metal init fails, so
/// `use_gpu(true)` is safe on every Mac. The 0.13 Metal shader-compile bug that
/// forced CPU-only earlier is gone in whisper-rs 0.13.2 — verified by a clean
/// Metal load (no JIT error, `use gpu = 1`) on the M1 Ultra 2026-06-20.
/// ponytail: when the Linux/Windows port lands, branch here for CUDA/Vulkan/CPU.
#[cfg(target_os = "macos")]
fn pick_backend() -> (bool, String) {
    (true, format!("Metal GPU, {}", cpu_brand()))
}

/// Windows/Linux: CPU/BLAS by default. A CUDA/Vulkan build can flip this on
/// later via a cargo feature; for now the portable baseline is CPU, which runs
/// on every machine with no GPU driver requirements.
#[cfg(not(target_os = "macos"))]
fn pick_backend() -> (bool, String) {
    (false, "CPU/BLAS".to_string())
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn play_sound(name: &str) {
    let _ = Command::new("afplay")
        .arg(format!("/System/Library/Sounds/{name}.aiff"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
#[cfg(target_os = "macos")]
fn play_start_sound() { play_sound("Pop"); }
#[cfg(target_os = "macos")]
fn play_stop_sound() { play_sound("Glass"); }

// Windows: short console beeps via PowerShell (no extra crate needed).
#[cfg(target_os = "windows")]
fn beep(freq: u32, ms: u32) {
    let _ = Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("[console]::beep({freq},{ms})")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
#[cfg(target_os = "windows")]
fn play_start_sound() { beep(880, 110); }
#[cfg(target_os = "windows")]
fn play_stop_sound() { beep(520, 150); }

// Linux: best-effort via paplay of a freedesktop sound; silent if unavailable.
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_sound(file: &str) {
    let _ = Command::new("paplay")
        .arg(format!("/usr/share/sounds/freedesktop/stereo/{file}.oga"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
#[cfg(all(unix, not(target_os = "macos")))]
fn play_start_sound() { linux_sound("message"); }
#[cfg(all(unix, not(target_os = "macos")))]
fn play_stop_sound() { linux_sound("complete"); }

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
#[cfg(target_os = "macos")]
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

/// Windows/Linux: synthesize Ctrl+V via rdev. The transcript is already on the
/// clipboard; this just triggers the focused app's paste. (X11 only on Linux;
/// Wayland blocks synthetic input — there the user pastes with Ctrl+V.)
#[cfg(not(target_os = "macos"))]
fn paste_cmd_v() -> Result<()> {
    use rdev::{simulate, EventType, Key};
    let tap = |et: EventType| -> Result<()> {
        simulate(&et).map_err(|e| anyhow::anyhow!("rdev simulate: {e:?}"))?;
        thread::sleep(Duration::from_millis(20));
        Ok(())
    };
    tap(EventType::KeyPress(Key::ControlLeft))?;
    tap(EventType::KeyPress(Key::KeyV))?;
    tap(EventType::KeyRelease(Key::KeyV))?;
    tap(EventType::KeyRelease(Key::ControlLeft))?;
    Ok(())
}

#[cfg(target_os = "macos")]
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

// ─── Transcript history (local, auto-expiring) ───────────────────────────────
// Every transcript is appended to ~/Library/Logs/AtlasDictation/transcripts.txt
// so a long dictation can't be lost to a clipboard overwrite or the RAM TTL.
// Records older than TRANSCRIPT_TTL_SECS are purged on every append AND on the
// idle tick, so PHI doesn't sit on disk past the window. ponytail: flat file +
// full rewrite, not a DB — a 2h window only ever holds a handful of records.

fn transcript_history_path() -> PathBuf {
    app_log_dir().join("transcripts.txt")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Local human time for a header line. macOS/BSD `date -r`, Linux GNU `date -d`,
/// empty elsewhere — the machine-readable epoch in the header is what pruning
/// relies on, so the human string is cosmetic and safe to omit.
fn human_time(epoch: u64) -> String {
    #[cfg(target_os = "macos")]
    let args = vec!["-r".to_string(), epoch.to_string(), "+%Y-%m-%d %H:%M:%S".to_string()];
    #[cfg(target_os = "linux")]
    let args = vec![format!("-d@{epoch}"), "+%Y-%m-%d %H:%M:%S".to_string()];
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return String::new();

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        Command::new("date")
            .args(&args)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }
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

    #[test]
    fn scrub_strips_wrapper_quotes_keeps_contractions() {
        assert_eq!(scrub("|patient is stable|"), "patient is stable");
        assert_eq!(scrub("'the patient'"), "the patient");
        assert_eq!(scrub("\u{201C}no acute distress\u{201D}"), "no acute distress");
        // interior apostrophe must survive
        assert_eq!(scrub("patient's vitals are fine"), "patient's vitals are fine");
    }

    #[test]
    fn collapse_repeats_kills_whisper_phrase_loops() {
        // short loop: "question mark" spoken once, transcribed three times
        assert_eq!(collapse_repeats("question mark question mark question mark"), "question mark");
        // a whole sentence duplicated
        assert_eq!(collapse_repeats("can i help you can i help you"), "can i help you");
        // the real silence-loop bug: an 11-word sentence repeated many times
        // (whisper filling left-on-recording silence). Must collapse to ONE.
        let unit = "so please can we get that on the list as well";
        let looped = std::iter::repeat(unit).take(10).collect::<Vec<_>>().join(" ");
        assert_eq!(collapse_repeats(&looped), unit);
        // odd count (5×) must also collapse to one, not to N/2
        let five = std::iter::repeat(unit).take(5).collect::<Vec<_>>().join(" ");
        assert_eq!(collapse_repeats(&five), unit);
        // non-repeating text is untouched
        assert_eq!(collapse_repeats("the patient is stable today"), "the patient is stable today");
        assert_eq!(collapse_repeats("blood pressure is 120 over 80"), "blood pressure is 120 over 80");
    }

    #[test]
    fn voice_punctuation_is_clinically_safe() {
        // always-safe commands map anywhere
        assert_eq!(scrub("are you sure question mark"), "are you sure?");
        assert_eq!(scrub("first point new paragraph second point"), "first point\n\nsecond point");
        // "period" ends a sentence — works mid-note, not just at the very end
        assert_eq!(scrub("the patient is stable period"), "the patient is stable.");
        assert_eq!(
            scrub("the skin was macerated period now we called surgery"),
            "the skin was macerated. now we called surgery"
        );
        // ...but a clinical "period" noun survives untouched
        assert_eq!(scrub("the postoperative period was uneventful"), "the postoperative period was uneventful");
        assert_eq!(scrub("we observed her for a long period and discharged"), "we observed her for a long period and discharged");
        // "new line" ends a line, but a catheter "new line" is left alone
        assert_eq!(scrub("first item new line second item"), "first item\nsecond item");
        assert_eq!(scrub("we placed a new line in the arm"), "we placed a new line in the arm");
        // words we deliberately never map (ICU/GI collisions)
        assert_eq!(scrub("the ascending colon was normal"), "the ascending colon was normal");
        assert_eq!(scrub("the patient remained in a coma"), "the patient remained in a coma");
    }

    #[test]
    fn cjk_language_drift_is_stripped() {
        // CJK tokens mixed into English are removed; the sentence survives
        assert_eq!(scrub("the patient 你好 is stable"), "the patient is stable");
        assert_eq!(scrub("blood pressure \u{3068} is normal"), "blood pressure is normal");
        // an all-CJK hallucination collapses to nothing (→ "no speech")
        assert_eq!(scrub("\u{60a3}\u{8005}\u{306f}\u{5b89}\u{5b9a}"), "");
        // legitimate accented/Greek/punctuation English is untouched
        assert_eq!(scrub("café au lait spots and a 5 \u{00b5}g dose"), "café au lait spots and a 5 µg dose");
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

/// True for Chinese/Japanese/Korean characters (and their fullwidth/CJK
/// punctuation). This is an English-only clinical tool, so any of these in the
/// output is a Whisper language-drift hallucination — strip them. Deliberately
/// narrow: leaves Greek (α/β/µ), accented Latin (café), and normal punctuation
/// (— " ') untouched so real English text is never harmed.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F | 0x3040..=0x309F | 0x30A0..=0x30FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xAC00..=0xD7AF | 0x1100..=0x11FF
        | 0xFF00..=0xFFEF)
}

fn scrub(text: &str) -> String {
    // Strip CJK hallucinations first; the space-collapse below closes any gaps.
    let mut s: String = text.trim().chars().filter(|&c| !is_cjk(c)).collect();
    let fillers = Regex::new(
        r"(?i)\b(uh+|um+|er+|erm+|ah+|hmm+|mm+m*|like|you know|i mean|kind of|sort of)\b[,.]?\s*"
    ).unwrap();
    s = fillers.replace_all(&s, "").to_string();
    let spaces = Regex::new(r"\s+").unwrap();
    s = spaces.replace_all(&s, " ").to_string();
    s = dedup_adjacent_words(&s);
    s = collapse_repeats(&s);
    s = apply_voice_punctuation(&s);
    let pun = Regex::new(r"\s+([,.!?;:])").unwrap();
    s = pun.replace_all(&s, "$1").to_string();
    // Tidy spaces around any newline the punctuation commands inserted, keeping
    // a double newline (paragraph) as two.
    let nl = Regex::new(r"[ \t]*\n[ \t]*").unwrap();
    s = nl.replace_all(&s, "\n").to_string();
    // whisper large-v3-turbo likes to wrap an utterance in stray quotes/pipes at
    // the very start and end (recurring "|...|" / "'...'" bug). Strip wrapper
    // junk off both ends only — interior apostrophes (contractions) are safe.
    s.trim_matches(|c: char| c.is_whitespace() || "|'\"`\u{2018}\u{2019}\u{201C}\u{201D}".contains(c))
        .to_string()
}

/// Spoken-punctuation commands, tuned for multi-sentence CLINICAL dictation.
/// Always-safe (rarely a medical word): "question mark", "new paragraph",
/// "exclamation point/mark", "full stop".
/// Context-sensitive, because the word is also clinical vocabulary:
///  - "period" → "." UNLESS it reads as a noun ("postoperative period", "a
///    recovery period", "the period of observation"). Converts mid-note so each
///    sentence can end with "period".
///  - "new line"/"next line" → newline UNLESS it reads as a catheter ("a new
///    line", "central line").
/// NOT mapped at all: "comma" (whisper writes it as "coma" — collides with
/// Glasgow Coma Scale / comatose) and "colon" (ascending/sigmoid colon). Whisper
/// already auto-inserts most commas from prosody, so this is a small loss.
/// ponytail: word-scan with a small denylist, not a parser — a rare clinical
/// "period"/"line" phrasing outside the list may still convert; widen the list
/// if one bites.
fn apply_voice_punctuation(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    let norm: Vec<String> = words
        .iter()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_lowercase())
        .collect();
    let n = words.len();

    // "period" is the noun, not the command, in these contexts → keep as a word.
    const PERIOD_NOUN_PREV: &[&str] = &[
        "postoperative", "post-operative", "perioperative", "intraoperative", "recovery",
        "grace", "incubation", "refractory", "latency", "latent", "menstrual", "gestational",
        "observation", "prodromal", "neonatal", "newborn", "quiet", "rest", "time", "window",
        "long", "short", "brief", "extended", "prolonged", "given", "same",
    ];
    const PERIOD_NOUN_NEXT: &[&str] = &[
        "of", "was", "is", "were", "are", "where", "during", "lasted", "lasts", "ended",
        "began", "begins", "had", "has", "without", "with",
    ];
    let determiner = |w: &str| {
        matches!(w, "a" | "an" | "the" | "this" | "that" | "his" | "her" | "its" | "each" | "another" | "one" | "any" | "no")
    };
    // "line" is a catheter, not a newline command, in these contexts.
    let line_is_catheter = |w: &str| {
        determiner(w)
            || matches!(w, "central" | "arterial" | "peripheral" | "picc" | "iv" | "venous"
                | "midline" | "femoral" | "subclavian" | "jugular" | "second" | "third")
    };

    let mut out: Vec<String> = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let w = norm[i].as_str();
        let next = norm.get(i + 1).map(|s| s.as_str()).unwrap_or("");
        let prev = if i >= 1 { norm[i - 1].as_str() } else { "" };
        let prev2 = if i >= 2 { norm[i - 2].as_str() } else { "" };

        // two-word, always-safe
        if w == "question" && next == "mark" { out.push("?".into()); i += 2; continue; }
        if w == "new" && next == "paragraph" { out.push("\n\n".into()); i += 2; continue; }
        if w == "exclamation" && (next == "mark" || next == "point") { out.push("!".into()); i += 2; continue; }
        if w == "full" && next == "stop" { out.push(".".into()); i += 2; continue; }

        // "new line" / "next line" → newline unless it's a catheter
        if (w == "new" || w == "next") && next == "line" && !line_is_catheter(prev) {
            out.push("\n".into());
            i += 2;
            continue;
        }

        // "period" → "." unless it reads as the noun
        if w == "period" {
            let is_noun = PERIOD_NOUN_PREV.contains(&prev)
                || determiner(prev)
                || determiner(prev2)
                || PERIOD_NOUN_NEXT.contains(&next);
            if !is_noun {
                out.push(".".into());
                i += 1;
                continue;
            }
        }

        out.push(words[i].to_string());
        i += 1;
    }
    out.join(" ")
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

/// Collapse an immediately-repeated phrase (length 2+) down to one copy.
/// Whisper — especially on a short clip with a long biasing prompt — sometimes
/// emits the same phrase 2–3× in a row ("question mark question mark question
/// mark") or a whole sentence twice. We find the longest block at position i
/// that's immediately followed by one or more exact copies of itself and drop
/// the copies. Comparison is case-insensitive and ignores edge punctuation.
/// Single-word runs are left to dedup_adjacent_words. ponytail: O(n²·k) but
/// transcripts are a few hundred words; upgrade to a suffix structure only if a
/// giant paste ever drags. Ceiling: a deliberate exact phrase repeat ("bye bye
/// bye") collapses too — rare in clinical dictation, acceptable.
fn collapse_repeats(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    let norm: Vec<String> = words
        .iter()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_lowercase())
        .collect();
    let n = words.len();
    let mut keep = vec![true; n];
    let mut i = 0;
    while i < n {
        // Cap at 40-word units — covers any sentence whisper might loop on while
        // bounding the no-repeat scan cost. SMALLEST k first: the fundamental
        // period collapses N copies down to one (largest-first would only halve
        // them, leaving N/2 for odd counts).
        let max_k = ((n - i) / 2).min(40);
        let mut collapsed = false;
        for k in 2..=max_k {
            if norm[i..i + k] == norm[i + k..i + 2 * k] {
                // Drop every immediately-following identical k-block.
                let mut j = i + k;
                while j + k <= n && norm[i..i + k] == norm[j..j + k] {
                    keep[j..j + k].iter_mut().for_each(|b| *b = false);
                    j += k;
                }
                i = j;
                collapsed = true;
                break;
            }
        }
        if !collapsed {
            i += 1;
        }
    }
    words
        .iter()
        .zip(keep)
        .filter_map(|(w, k)| k.then_some(*w))
        .collect::<Vec<_>>()
        .join(" ")
}
