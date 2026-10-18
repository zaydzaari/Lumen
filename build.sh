#!/usr/bin/env bash
# Build a release binary of Lumen into dist/ (Linux and macOS).
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is not installed. Installing via rustup (non-interactive)..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

if [[ "$(uname -s)" == "Linux" ]]; then
  missing=()
  for pkg in alsa gtk+-3.0 ayatana-appindicator3-0.1 xkbcommon webkit2gtk-4.1; do
    pkg-config --exists "$pkg" 2>/dev/null || missing+=("$pkg")
  done
  if ((${#missing[@]})); then
    echo "Missing system libraries: ${missing[*]}"
    echo "Debian/Ubuntu: sudo apt install build-essential pkg-config libasound2-dev libgtk-3-dev libayatana-appindicator3-dev libxdo-dev libxkbcommon-dev libgl1-mesa-dev libwebkit2gtk-4.1-dev libssl-dev cmake ffmpeg"
    echo "Fedora:        sudo dnf install alsa-lib-devel gtk3-devel libayatana-appindicator-gtk3-devel libxdo-devel libxkbcommon-devel mesa-libGL-devel webkit2gtk4.1-devel openssl-devel cmake ffmpeg"
    echo "The optional tray can be disabled (WebKitGTK is still required): cargo build --release --no-default-features --features media-controls"
    exit 1
  fi
fi

cargo build --release --locked
mkdir -p dist
cp target/release/lumen dist/lumen
cp -r plugins dist/example-plugins
if [[ "$(uname -s)" == "Linux" ]]; then
  bash packaging/linux/package.sh
fi
echo "Built dist/lumen ($(du -h dist/lumen | cut -f1))"
