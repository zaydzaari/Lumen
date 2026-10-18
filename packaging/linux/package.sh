#!/usr/bin/env bash
# Run from any directory after cargo build --release --locked.
set -euo pipefail
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1 | tr -d '\r')
arch=$(uname -m)
mkdir -p dist
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
bundle="$stage/lumen"
mkdir -p "$bundle"
install -m755 target/release/lumen "$bundle/lumen"
install -m755 packaging/linux/install.sh "$bundle/install.sh"
install -m644 packaging/linux/lumen.desktop assets/lumen.svg LICENSE "$bundle/"
cp -R plugins "$bundle/example-plugins"
cp README.md "$bundle/README.md"
tar -C "$stage" -czf "dist/lumen-${version}-linux-${arch}.tar.gz" lumen
if command -v dpkg-deb >/dev/null 2>&1 && [[ "$arch" == x86_64 ]]; then
  deb="$stage/deb"
  install -Dm755 target/release/lumen "$deb/usr/bin/lumen"
  install -Dm644 packaging/linux/lumen.desktop "$deb/usr/share/applications/lumen.desktop"
  install -Dm644 assets/lumen.svg "$deb/usr/share/icons/hicolor/scalable/apps/lumen.svg"
  install -Dm644 LICENSE "$deb/usr/share/doc/lumen/copyright"
  mkdir -p "$deb/DEBIAN"
  cat > "$deb/DEBIAN/control" <<CONTROL
Package: lumen
Version: ${version}
Section: sound
Priority: optional
Architecture: amd64
Maintainer: Lumen contributors <noreply@github.com>
Depends: libasound2t64, libgtk-3-0t64, libayatana-appindicator3-1, libxdo3, libwebkit2gtk-4.1-0, libxkbcommon0, libxkbcommon-x11-0, libgl1, ca-certificates, ffmpeg
Recommends: xdg-utils
Description: Lightweight native music player
 Local files, personal playlist imports, lyrics and independent playback.
 Spotify streaming requires a Premium account.
CONTROL
  dpkg-deb --root-owner-group --build "$deb" "dist/lumen_${version}_amd64.deb"
fi
