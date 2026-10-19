# Licensing and third-party components

Lumen's combined application releases from 0.2.0 are distributed under
**GPL-3.0-or-later**; see `LICENSE`. The pre-integration Lumen source remains
available under its original MIT terms, preserved in `LICENSE-MIT`.

## Sonora provider components

Copyright (C) 2026 Sonora Contributors.

Source: <https://github.com/sonorahq/sonora>
Revision: `716341a227e84ef1e99ea8d1cf387127b93353a8`
License: GPL-3.0-or-later.

The adapted `music`, `storage`, `widevine`, and `webview` crates and browser
attestation script are in `vendor/sonora`. Changes for Lumen are recorded in
`vendor/sonora/UPSTREAM.md`. Lumen retains its own UI and Spotify integration;
Sonora's duplicate Spotify engine is disabled.

The Widevine interface headers and host shim retain their respective notices in
`vendor/sonora/crates/widevine/shim/cdm/LICENSE` and
`vendor/sonora/crates/widevine/shim/LICENSE`. **Google's Widevine CDM binary is
not included in any release.** A user's installed module or a download reviewed
and accepted on that machine is required for Apple Music protected playback.
No decrypted protected streams, DRM licenses or content keys are exported.

The provider engine also uses the pinned ytmusic-rs and Rodio revisions named
in Cargo.lock. Their source and license files are included in the corresponding
source archive, along with the other Rust dependencies and build configuration.

## Other components

Figtree and Noto Kufi Arabic font licenses are under `assets/fonts/*-OFL.txt`.
The existing MIT-licensed Lumen code and bundled example plugins keep their
original notices. Cargo dependencies retain the licenses supplied with their
source packages. See the corresponding source archive for those notices.

## Corresponding source

Each combined binary release links its matching source archive on the same
GitHub release page. It includes the Lumen source, adapted Sonora components,
build and packaging scripts, Cargo.lock, and vendored Rust dependencies.
Build instructions are in `SOURCE-BUILD.md`. License notices must be preserved
when distributing modified or unmodified copies.
