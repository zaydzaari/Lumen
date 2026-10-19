# Building the corresponding source

Combined Lumen releases from 0.2.0 are GPL-3.0-or-later. Preserve LICENSE,
LICENSE-MIT, THIRD-PARTY.md, and the component notices when redistributing.

The `lumen-VERSION-source.tar.gz` asset on the release page contains this tree
and the Rust dependency sources. Its `.cargo/config.toml` points Cargo at the
included `third-party/` directory, so those dependencies can be built offline.
The system compiler, Rust toolchain and platform development libraries must be
installed separately. No service cookies, user library data or Widevine binary
is included.

Use Rust stable (at least 1.95), a C/C++ compiler and CMake. On Ubuntu 24.04:

```sh
sudo apt-get install build-essential pkg-config cmake libasound2-dev \
  libgtk-3-dev libayatana-appindicator3-dev libxdo-dev libxkbcommon-dev \
  libxkbcommon-x11-0 libgl1-mesa-dev libwebkit2gtk-4.1-dev libssl-dev ffmpeg
cargo build --release --locked --offline
bash packaging/linux/package.sh
```

Windows needs Visual Studio C++ Build Tools, CMake and Rust's MSVC toolchain:

```powershell
cargo build --release --locked --offline
```

Inno Setup compiles `packaging/windows/lumen.iss` for the per-user installer.
The checked-in GitHub workflow gives the exact build, test and packaging steps.
The Flatpak variant is built with `--no-default-features --features media-controls`
and packaged by `packaging/flatpak/package.sh` using the GNOME 50 runtime.
System libraries, packaging tools and the proprietary Widevine CDM are not Rust
dependencies and are not supplied in the source archive.

For a Git checkout, omit `--offline` on the initial build; Cargo fetches the exact
registry and Git dependency revisions from Cargo.lock. To recreate the complete
source archive on Linux, run `python3 packaging/source.py` from the repo root.
