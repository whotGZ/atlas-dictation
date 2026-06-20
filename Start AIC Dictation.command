#!/usr/bin/env bash
# Atlas Intensive Care Dictation — launcher.
# Double-click this file from Finder to start the app.

set -e
cd "$(dirname "$0")"

# Mic is chosen in the menu-bar "Microphone" submenu and persists to
# ~/Library/Application Support/AtlasDictation/selected-mic.txt. Set ATLAS_MIC
# here to a device-name substring only if you want to force-override that choice.

if [ ! -f "models/ggml-large-v3-turbo.bin" ]; then
  echo
  echo "==> Whisper Turbo model not found at models/ggml-large-v3-turbo.bin"
  echo "    Downloading (~1.5 GB, one-time)..."
  echo
  mkdir -p models
  curl -L --fail -o "models/ggml-large-v3-turbo.bin" \
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
fi

if [ ! -x "target/release/atlas-dictation" ]; then
  echo
  echo "==> First-run build (a few minutes, one-time)..."
  echo
  export PATH="$HOME/.cargo/bin:$PATH"
  cargo build --release
fi

exec ./target/release/atlas-dictation

