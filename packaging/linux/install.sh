#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
prefix="${1:-$HOME/.local}"
install -Dm755 lumen "$prefix/bin/lumen"
install -Dm644 lumen.desktop "$prefix/share/applications/lumen.desktop"
install -Dm644 lumen.svg "$prefix/share/icons/hicolor/scalable/apps/lumen.svg"
printf 'Installed Lumen to %s/bin/lumen\n' "$prefix"
printf 'Ensure %s/bin is on your PATH.\n' "$prefix"
