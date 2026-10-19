# Sonora components in Lumen

Source: https://github.com/sonorahq/sonora
Revision: 716341a227e84ef1e99ea8d1cf387127b93353a8
License: GPL-3.0-or-later (see COPYING)
Copyright (C) 2026 Sonora Contributors.

The music, storage, webview and Widevine host crates are retained here with their own
third-party notices, including the CDM interface/shim licenses. Lumen adapts the
credential/cache paths and connects these components to its existing native UI.
The provider-attestation script is adapted from `crates/state/src/potoken.js`.
Provider browsers run in a disposable Lumen helper process. The duplicate
Sonora Spotify module is disabled; Lumen keeps its existing Spotify backend.
The Google Widevine CDM itself is not included or redistributed.

Lumen code released before this integration remains available under its original
MIT license (LICENSE-MIT at the repository root). Combined releases containing
these components are distributed under GPL-3.0-or-later. Existing copyright and
license notices remain applicable to their respective source files.
