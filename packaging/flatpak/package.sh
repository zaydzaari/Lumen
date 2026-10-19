#!/usr/bin/env bash
# Package the no-tray Linux binary against GNOME's shared runtime.
# Build binary: cargo build --release --locked --no-default-features --features media-controls
set -euo pipefail
cd "$(dirname "$0")/../.."
app_id=io.github.zaydzaari.Lumen
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1 | tr -d '\r')
arch=$(uname -m)
mkdir -p dist
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
# This packages a prebuilt binary, so the runtime is sufficient as the build SDK.
flatpak build-init "$stage/build" "$app_id" org.gnome.Platform org.gnome.Platform 50
install -Dm755 target/release/lumen "$stage/build/files/bin/lumen"
install -Dm644 assets/lumen.svg "$stage/build/files/share/icons/hicolor/scalable/apps/$app_id.svg"
sed "s/^Icon=lumen$/Icon=$app_id/" packaging/linux/lumen.desktop > "$stage/lumen.desktop"
install -Dm644 "$stage/lumen.desktop" "$stage/build/files/share/applications/$app_id.desktop"
install -Dm644 LICENSE "$stage/build/files/share/licenses/$app_id/LICENSE"
install -Dm644 packaging/flatpak/io.github.zaydzaari.Lumen.metainfo.xml "$stage/build/files/share/metainfo/$app_id.metainfo.xml"
mkdir -p "$stage/build/files/share/licenses/$app_id/fonts"
cp assets/fonts/*-OFL.txt "$stage/build/files/share/licenses/$app_id/fonts/"
install -m644 LICENSE-MIT "$stage/build/files/share/licenses/$app_id/LICENSE-MIT"
install -m644 THIRD-PARTY.md "$stage/build/files/share/licenses/$app_id/THIRD-PARTY.md"
install -m644 vendor/sonora/UPSTREAM.md "$stage/build/files/share/licenses/$app_id/Sonora-UPSTREAM.md"
install -m644 vendor/sonora/crates/widevine/shim/LICENSE "$stage/build/files/share/licenses/$app_id/Widevine-shim-LICENSE"
install -m644 vendor/sonora/crates/widevine/shim/cdm/LICENSE "$stage/build/files/share/licenses/$app_id/CDM-interface-LICENSE"
flatpak build-finish --command=lumen \
  --share=network --share=ipc --socket=wayland --socket=fallback-x11 \
  --socket=pulseaudio --device=dri --filesystem=home \
  --own-name=org.mpris.MediaPlayer2.lumen \
  --talk-name=org.freedesktop.portal.Flatpak "$stage/build"
flatpak build-export "$stage/repo" "$stage/build" stable
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$stage/repo" "dist/lumen-${version}-linux-${arch}.flatpak" "$app_id" stable
