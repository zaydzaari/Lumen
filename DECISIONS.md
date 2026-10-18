# Design decisions and deviations

## Authentication (researched against ncspot and spotify-player, Sept 2026)

* **Streaming login**: librespot 0.8 dropped password login. Like ncspot and
  spotify-player, Lumen runs an OAuth authorization-code + PKCE flow against
  Spotify's official desktop client ID (`65b708073fc0480ea92a077233ca87bd`, the ID
  librespot itself uses) with a loopback redirect `http://127.0.0.1:<random port>/login`,
  then passes the token to librespot. After the first connection librespot returns
  **reusable credentials**, which Lumen stores so the user logs in once.
* **I wrote the PKCE flow myself** instead of using `librespot-oauth`, because its
  callback server treats the first incoming connection as the redirect, so a stray
  browser request (favicon, prefetch) breaks login. spotify-player works around the
  same problem. Lumen's server ignores unrelated requests and checks `state`.
* **Web API token**: the spec asked to reuse the librespot token for the Web API so no
  developer app is needed. Both reference projects moved away from that: spotify-player
  now uses a second PKCE flow with a third-party client ID (ncspot's by default), and
  ncspot does the same with its own client ID. The ncspot author asks others not to use
  that client ID without permission, so Lumen **asks the user for their own client ID**
  on the first-run screen, with step-by-step instructions. The librespot-token approach
  is kept as an **experimental fallback** when the field is left empty (login5 token;
  Spotify may rate-limit it). This is the "unavoidable client ID" case from the spec.
* **Secret storage**: Windows Credential Manager / macOS Keychain via `keyring`; on
  Linux a `0600` file in the data dir (no hard dependency on a Secret Service daemon).
  If the keychain fails, the file fallback is used.

## Spotify Web API restrictions (Feb 2026 developer-mode changes)

* Playlist items are only returned for playlists the user owns or collaborates on. For
  other playlists Lumen loads the track list through librespot's internal API
  (`librespot-metadata`) and fetches track metadata lazily, only for visible rows.
* `GET /artists/{id}/top-tracks` was removed → artist pages use librespot metadata.
* Search is limited to 10 results per type.
* The new `/playlists/{id}/items` endpoint and `item`/`items` field names are used,
  with fallbacks for the old `track`/`tracks` names.

## Playback architecture

* **Two librespot `Player`s share one session and one soft mixer**: one is driven by
  Lumen's own queue (play from any list, add/remove/reorder, shuffle, repeat
  off/all/one, gapless preloading via `TimeToPreloadNextTrack`), the other belongs to
  `Spirc` (Spotify Connect). librespot's `Spirc` has no queue add/remove/reorder API,
  and sharing one `Player` is unsafe because Spirc adopts *every* play-request id
  emitted by the player. When a phone starts playback on Lumen, Lumen switches to
  "Connect mode" (local player paused; transport controls are routed to Spirc). Playing
  anything from Lumen's UI switches back. Limitation: while Lumen plays from its own
  queue, the phone does not show it as the active device.
* **`LazySink`** (own rodio/cpal sink) opens the audio device only while sound plays
  and never panics/exits. librespot's stock rodio sink opens the device at start-up and
  `unwrap()`s errors, which would abort the app if no audio device exists.
* Normalisation, quality and gapless changes rebuild the local player and resume the
  current track at the same position. Spotify Connect picks them up on the next start.
* If Spirc can't start (e.g. the dealer connection fails) Lumen still logs in with a
  plain session and shows a toast saying Connect is unavailable.

## Libraries / versions

* `librespot` 0.8.0 (latest), as required.
* `eframe`/`egui` pinned to **0.36.2**. 0.37.0 was released on the day of
  development, so I used the previous release, which had been out for a month.
* `vergen` pinned to 9.0.6 in `Cargo.lock`: with vergen 9.1 the librespot-core 0.8.0
  build script fails to compile (two `vergen-lib` versions). Build with `--locked`
  (the scripts do).
* `rspotify` was not used. Direct `reqwest` calls plus tiny private serde structs keep
  memory low (full responses are never kept) and make it easy to handle the 2026
  field renames.
* TLS: rustls with the OS root store everywhere, so no OpenSSL dependency on any OS.
* `mlua` 0.11 with vendored Lua 5.4, and `tray-icon` 0.21: versions whose APIs I could
  check against their sources.
* `souvlaki` uses the pure-Rust `zbus` backend on Linux (no libdbus). Its MPRIS thread
  panics when no D-Bus session bus exists, and with `panic = "abort"` that would kill
  the app. Lumen therefore checks that the session bus socket is reachable before
  enabling media keys.
* Tray on Linux needs GTK + libayatana-appindicator. It runs on its own GTK thread.
  It is a cargo feature (`tray`, on by default) and can be turned off.

## UI / performance

* egui `glow` backend, no multisampling or depth/stencil buffers.
* Repaint only on input, events from background tasks (each `UiTx::send` wakes the UI),
  twice a second while playing (seek bar), and exactly at the next lyric line when the
  lyrics panel is open.
* Long lists use `ScrollArea::show_rows`, so only visible rows are laid out. Rows are
  painted manually.
* Caps on every cache: textures (LRU, default 150), failed-image set (256), track lists
  (12), artist album lists (8), history (200), toasts (5), art disk cache (64 MB),
  lyrics disk cache (16 MB), audio cache (librespot `FsSizeLimiter`, default 2 GB),
  queue preview sent to the UI (100 items).
* Album art: the smallest image ≥ 64 px is downloaded (≥ 300 px for the large view),
  decoded and downscaled to 96 px / 256 px on a blocking thread before upload.
* Tokio runtime with 2 worker threads.

## Scope notes

* No feature exports, decrypts or saves audio as standalone files. The audio cache stays
  in librespot's own encrypted format. "Clear cache" deletes that directory.
* Audio-cache size changes apply on restart (librespot creates the limiter when the
  session is created).
* Shortcuts are edited in `config.toml`. The Settings page lists them and can reload
  them.
* Milestone commits: the code was written in one pass, then committed in milestone
  order as separate commits (one per group of modules), not as live, incremental
  checkpoints.

## Build environment caveat

Development ran on Linux x86_64 (Ubuntu 24.04 container). The Windows target's std
could not be downloaded in that sandbox (rustup's server was blocked), so **the
Windows build was not compiled there**. Windows-only code is small: the keyring calls
(checked against keyring 3.6 sources), HWND extraction for media keys (checked against
raw-window-handle 0.6) and the `windows_subsystem` attribute. `build.ps1` builds it
natively.

## Windows build verification — 2026-10-16

* Installed Rust stable 1.99.1 for x86_64-pc-windows-msvc in the task workspace
  (`work/rustup`, `work/cargo`), without changing the user's global PATH.
* Reused the installed Visual Studio Community 2026 MSVC 14.50 C++ toolset,
  detected by vswhere, instead of installing a redundant Visual Studio version.
* Native release compilation succeeds with the original Cargo.lock (vergen 9.0.6).
  No Windows-only source compilation errors occurred.
* MSVC lib.exe could not replace vendored Lua's archive (LNK1114, access denied).
  Installing rustup's llvm-tools and selecting llvm-ar resolves this. build.ps1
  now discovers that optional archiver and respects CARGO_TARGET_DIR.
* Computer Use access to Lumen was rejected by automatic approval review. Visual
  login-screen inspection and F12 input cannot be claimed as verified.

* Baseline verification: 49 unit tests pass; cargo fmt --check passes. Windows
  idle working set is 137.2 MiB after 12 seconds (fresh portable config), process
  alive with window title Lumen. F12 was not used because UI access was denied.

## Local Files implementation — 2026-10-16

* Offline means only the user's own files. Spotify's encrypted audio cache is
  unchanged; no Spotify audio downloads/exports, recording or decryption keys added.
* Rejected librespot 0.8's native local-file lookup after reading its source:
  extensions are limited to mp3/mp4/m4p/flac, metadata-derived URIs can collide,
  and the loader divides by duration.as_secs() (unsafe for sub-second audio).
* Local decoding uses Symphonia 0.5.5 plus bundled libopus, on a dedicated thread,
  with the same rodio output factory as LazySink. One packet of PCM is retained.
  The output device closes while paused/stopped. Worker blocks completely at idle.
* A permanent queue engine starts without login using an unconnected Session.
  Successful Spotify authorization attaches session/Connect/player resources to
  that engine. Queue, current local track, position, shuffle/repeat and volume survive.
* [local] folder paths and scan_on_start are backward-compatible config defaults.
  walkdir scans off the UI thread, does not follow symlinks, deduplicates overlapping
  roots, and caps the index at 20,000 entries / 64 MiB. JSON contains compact tags,
  canonical paths, size and nanosecond mtime. Only changed files are tag-parsed.
* Lofty reads no covers during scans. Individual tag allocations are capped at
  4 MiB; strings are truncated to 1024 characters. Embedded covers are read on demand
  through ArtCache's existing worker concurrency and texture limits. Local covers
  bypass the disk image cache so rescan/texture invalidation cannot reuse stale art.
* Local Files has a virtualized filtered list and existing play/queue row actions.
  Folder add/remove paths, rescan, file count and startup scanning are in Settings.
* Offline detection uses a five-second connectivity probe; saved login is retried
  every 30 seconds in the background. Repeated failures do not repeatedly navigate
  away from Settings or display the same connectivity toast.
* Sibling .lrc (up to 1 MiB) takes precedence over cache/LRCLIB; reading runs on a
  blocking worker. Plugins receive local:<absolute path> via the same events.
* CMake builds vendored libopus; build.ps1 finds Visual Studio's bundled CMake.
  Raised the declared MSRV to 1.95 to match the already-pinned egui 0.36.2.
  Cargo.lock retains vergen 9.0.6 and the release profile is unchanged.
* Synthetic fixtures validate tags, duration, decoding and seeking for all six formats.
  Unit tests cover changed/deleted files, unchanged reuse, overlapping roots,
  malformed input, local lyric lookup, offline startup and queue/session attachment.
  The opt-in real audio-device test passed on Windows at zero volume: play, pause,
  seek, resume, next (WAV to Opus), end of queue, without Spotify login/network.
* Limit: local transitions are not sample-accurate gapless; Spotify normalization
  applies to Spotify playback only. Live Spotify login/Connect requires an account
  and was not exercised. Windows GUI/F12 automation was denied by approval review.

## Final regression checks

* Final review fixed restarting a finished local queue: the engine distinguishes a
  loaded/paused file from a stopped decoder and reloads on Play or seek after stop.
  The real-device regression test now verifies restarting after the queue ends.
* 60 automated tests pass, plus the opt-in Windows audio-device test passes.
  cargo clippy --all-targets --locked -- -D warnings and cargo fmt --check pass.

## Final Windows release

* build.ps1 completed with cargo build --release --locked; dist/lumen.exe is
  17.3 MiB. The final executable remained alive with the Lumen window after 12 s,
  measuring 162.9 MiB working-set RSS (fresh portable config, no login).
* Automatic approval review again rejected Computer Use access to Lumen after the
  filesystem permissions changed. The final login screen and F12 could not be
  inspected. An extra whole-app offline/proxy smoke command was also blocked by
  policy; offline engine/decoder, startup routing and real-device tests passed.

## Library recovery and Spotify-style UI — 2026-10-16

* User's actual logs showed successful authentication followed by repeated HTTP 429
  responses to /me, playlists, liked songs and home endpoints. No client ID was
  configured. The old shared login5-to-Web-API mode caused the empty library.
* Default browsing now uses existing librespot authenticated metadata/context methods:
  paginated rootlist for playlists, collection context for liked songs, search context
  for tracks, and metadata for albums/artists/playlists. It does not call the public
  Web API with the shared desktop token. Dedicated developer-app tokens keep the
  full Web API route. This is not rate-limit rotation or repeated retrying: the
  failing shared Web API route is no longer used.
* Rootlist pagination advances by raw entries (including skipped folders), normalizes
  legacy playlist URIs and reads decorated names/owners/art. Missing artwork loads
  later with four concurrent metadata requests, using a track cover when necessary.
  Session metadata calls have deadlines; context browsing is capped at 20,000 tracks.
* Public Web API 429 responses return immediately and start a shared Retry-After
  cooldown. A local HTTP-server test verifies no wait/retry storm. 401 keeps one
  token-refresh retry. Missing optional Web API authorization no longer blocks
  streaming login; Settings can authorize it while playback/queue continue.
* Used the ui-ux-pro-max skill for content-first dark music-player layout. Adapted
  its suggestions to native egui: persistent header search, Home playlist cards,
  clearer library navigation/status and retry controls, vector transport icons,
  higher-contrast muted text, and a full-page lyrics view with large timed lines.
  Kept virtualization, bounded art caches and event-driven repainting.
* Full-page lyrics schedules lyric-line repaints and follows track changes like the
  sidebar panel. Retry is available after transient errors. Extreme LRC timestamps
  and offsets are checked/saturated rather than overflowing.
* Live saved-login verification succeeded: 20 playlists (22 root entries including
  folders), first playlist's 20 tracks, 50 liked songs on each of two pages, 20 search
  tracks, a 13-track album, fallback cover artwork, LRCLIB lyrics and a short muted
  Spotify stream. No credentials or tokens were printed or exported.
* Spotify downloads were not added. The supported APIs do not provide the official
  client's offline-download mechanism here. Searching/saving never exports or
  decrypts Spotify audio; the existing encrypted replay cache is unchanged. Added
  Open in Spotify to the row menu so users can manage official downloads there.
  Local Files remains the offline path in Lumen.
* Optional developer-app authorization, saved albums/followed artists/write actions
  and phone-controlled Connect still require interactive validation. Windows visual
  automation was previously denied; no claim of screenshot/F12 validation is made.

* Added a UI pagination regression: rootlist offsets include folders, so collection
  merging compares against the requested next offset rather than visible row count.
  Final offline-independent suite: 64 tests passed; live session test passed separately.
  Strict Clippy and formatting checks pass.

* Updated release build.ps1 completed successfully (17.4 MiB), with vergen 9.0.6
  still locked and the release profile unchanged. The executable was launched using
  the normal user config and authenticated with the saved Spotify account. No new
  warnings/errors appeared after that launch. Working set after 12 seconds: 99.8 MiB.
  A separate output copy is named lumen-updated.exe to distinguish it from the old build.

## 2026-10-16 — Native desktop design refresh

* Followed the user's Spotify screenshots for density and hierarchy: black shell,
  rounded content surfaces with small gutters, a 72 px artwork library rail with
  a persistent expand/collapse preference, compact responsive shortcut tiles,
  horizontal cover shelves, and a slimmer 80 px player. Kept Windows window controls.
* Fixed the actual card layout defect: explicit artwork/title/subtitle rectangles
  no longer inherit a horizontal parent layout. Labels truncate inside their bounds
  and align to the artwork edge. Home uses the real library; recommendations,
  credits, artist biographies and listening history are not fabricated.
* Removed the permanent technical connection banner. In-progress authorization
  still appears; library setup remains available in Settings. Added a closable
  Now Playing pane with large artwork, album/artist details, lyrics access and the
  first upcoming track. Queue and synced lyrics remain available from the player.
* Replaced remaining shell/volume emoji with vector icons. Thin seek/volume rails
  keep larger input targets, pointer seeking, keyboard arrows/Home/End, disabled
  state, tooltips and accessibility metadata. Muting restores the previous volume.
* Artwork requests now stop at the viewport boundary, including horizontally
  scrolled shelves. The library still uses virtualized rows and artwork retains
  its hard-capped LRU and bounded workers. No continuously animated decorations or
  idle repaint loop were added; release profile and vergen pin remain unchanged.
* Checks: 68 tests passed (two existing live-network/audio-device tests excluded),
  strict Clippy and formatting pass. New regressions cover card bounds, responsive
  shortcuts, centered/non-overlapping player regions, and headless egui pointer,
  keyboard and disabled slider input.
* Initial release runs launched and authenticated with the saved account. A settled
  process sample measured 102.4 MiB working set and 0 ms CPU time across 10 seconds.
  Initial load was busier; these are idle measurements, not a benchmark against
  Spotify or a controlled before/after comparison.
* Computer Use screenshot capture was rejected: "Computer Use was not approved
  to use lumen". Used the user's screenshots and source/layout tests; no alternate
  screenshot or UI automation bypass was used. Visual verification remains open.

* Final visible-window release validation: build.ps1 succeeded (17.4 MiB), copied
  to outputs/lumen-design.exe with matching SHA-256. It launched with saved login,
  remained alive, and logged no new warnings/errors. Working set: 168.0 MiB after
  25 seconds settling plus a 10-second idle sample; CPU delta 0 ms. This supersedes
  the earlier 102.4 MiB hidden-launch sample as the current release measurement.
  Window visibility and other conditions differ; no before/after speedup is claimed.

## Spotify-style redesign (second pass)

* **Layout** follows Spotify's desktop app: black base, three rounded `#121212`
  panels (library, main view, right panel), a top bar with a centred home button and
  search pill, and a three-zone player bar. Lumen keeps its own name and logo; no
  Spotify artwork or logos are bundled.
* **Fonts**: Spotify's Circular is proprietary, so I bundled **Figtree** (similar
  geometric sans, OFL) in four weights through named egui font families, with
  **Noto Kufi Arabic** as fallback. egui 0.36 shapes text with HarfBuzz (harfrust),
  so Arabic letters join correctly. egui has no bidi reordering yet, so mixed
  Arabic/Latin titles may show segments in an unexpected order.
* **Colour from artwork**: the art cache computes a saturation-weighted dominant
  colour per cover when it decodes it (no extra downloads). That colour drives the
  playlist header gradient, the full-screen lyrics background, the lyrics card in
  "Now playing", and the home tint (it follows the hovered shortcut tile).
* **Virtualisation kept**: playlist pages use `ScrollArea::show_viewport` and paint
  only the header and the visible rows, so a 10 000-song playlist stays cheap.
  Artist pages (≤10 popular tracks + discography) use a normal scroll.
* **Icons** are drawn as vectors (no icon font or image assets) and scale cleanly.
* **Profile**: `/me` now also returns the avatar URL (smallest image). In session-only
  mode there is no avatar, so an initial letter is shown.
* **Discord Rich Presence**: `discord-rich-presence` 1.1 on a dedicated thread with
  reconnect and change-detection (Discord rate-limits presence updates). It uses
  activity type *Listening* with `status_display_type = Details`, so the member list
  shows the song name. Users supply their own Discord Application ID: shipping a
  shared ID would tie everyone's status to an app someone else controls, and you asked
  for a custom one.
* **Card / tile play buttons** set an "autoplay" request that starts the list once its
  first page arrives, so you can press play without opening the page first.
* `--demo` (generated demo mode) now also uses generated covers when `LUMEN_DEMO_ART`
  points to a folder of `coverN.png` files. I used it for the README screenshots.


## Third pass: polish and memory

* **Custom title bar**: the window is undecorated (`with_decorations(false)`) and
  Lumen draws Windows-style minimise/maximise/close buttons in the top bar. Dragging
  uses `ViewportCommand::StartDrag`, double-click toggles maximise, and 5 px edge
  zones send `BeginResize`. macOS keeps native decorations (the traffic lights would
  clash). `ui.custom_title_bar = false` brings back the OS frame.
* **Sticky header**: once a playlist page scrolls past its header, a bar in the cover
  colour fades in with a play button and the title, as in Spotify.
* **Library search** filters the sidebar by name/owner without new API calls.
* **Switches** replace checkboxes everywhere in Settings (custom animated widget).
* **Scroll fades disabled**: egui 0.36 fades scroll edges using the panel colour,
  which drew dark bands over the coloured lyrics view and the gradients. Spotify
  doesn't fade lists, so `scroll.fade.strength = 0`.
* **Heart icon** is now one polygon (parametric heart, fan-filled from the tip), so it
  has no seams when drawn with transparency.
* **Memory**:
  * Large textures (playlist headers, now-playing art, 300–640 px) have their own LRU
    capped at 64, separate from the thumbnail cache, so browsing many playlists can't
    accumulate hundreds of big textures in GPU/driver memory.
  * Page titles are measured once at a reference size and scaled, not laid out at
    each of the candidate sizes (fewer glyph sizes in the font atlas).
  * On Windows, `memory::Trimmer` calls `K32EmptyWorkingSet` 10 s after start, after
    60 s without input (once per idle period) and when the window is minimised,
    hidden or occluded, at most every 30 s. Pages come back on demand. This is how
    the working set shown in Task Manager stays well under 200 MB while idle; the
    NVIDIA/AMD/Intel OpenGL driver is the biggest part of it and Lumen can't shrink
    the driver itself. `ui.trim_memory = false` turns it off.

* **Liked state** comes from three places: the Liked Songs pages already loaded, the
  user's own save/remove clicks (optimistic, rolled back if Spotify refuses), and one
  `GET /me/library/contains` per newly played track. Rows of other playlists show the
  check only when Lumen already knows the song is liked; it never fires a request per
  visible row. Removing uses `DELETE /me/library`, the counterpart of the
  `PUT /me/library` call used for saving since the Feb 2026 API changes. If those
  endpoints are refused, the check simply doesn't appear.
* **Narrow windows**: below ~1 100 px the right panel is limited to 260–280 px, and the
  library is shown as the artwork rail while the main view would drop under 480 px.
  Pressing "expand" in that state closes the right panel to make room.
* **Page fade**: switching pages fades the main view in over 180 ms (repaints only
  during the fade; idle CPU stays at zero).

## Themes, full screen and animations — 2026-10-16

* **Spicetify compatibility, colours only.** Spicetify themes are `user.css` (rules for
  Spotify's web DOM) plus `color.ini`. A native egui app has no DOM, so the CSS can't
  apply; the colour schemes can, and they're what makes most themes recognisable. The
  marketplace mirrors the Spicetify Marketplace's sources: the official repo's
  `manifest.json` and GitHub's `topic:spicetify-themes` search (each repo's root
  `manifest.json`, object or array), filtering archived repos and the Marketplace
  blacklist. Unauthenticated GitHub search allows 10 requests a minute, so the list is
  cached for 12 h, and a stale cache is used when GitHub is unreachable.
* **Install = `color.ini` only**, parsed into named schemes and saved as JSON under the
  config folder. Nothing from a theme is executed (`theme.js` extensions are ignored).
* **Mapping heuristics**: `player` is the window/player-bar backdrop (Spotify's layout
  shares it); `sidebar`, `card` and `player` are only used as surfaces when text has at
  least 4:1 contrast on them and they're close to `main` in brightness, because some
  themes (e.g. *text*) use those keys for highlight colours. Text on the accent switches
  between black and white automatically.
* **Live preview**: hovering a scheme sets a per-frame preview; leaving it reverts.
  Every palette change cross-fades over 350 ms by lerping palettes.
* **Full screen**: an overlay screen plus `ViewportCommand::Fullscreen`. The backdrop is
  the 96 px thumbnail stretched over the window; GPU bilinear filtering makes it soft,
  so there is no blur shader and no extra memory.
* **Animations** use egui's `animate_bool_with_time` (state per widget id) and only
  request repaints while something is moving. The equaliser repaints at ~12 fps only
  while it's visible and playing; everything is idle again within ~0.4 s. Settings →
  Appearance → Animations turns them off.
* **Theme previews** are decoded at 384 px into the capped large-texture cache, and only
  for cards on screen, to keep memory down.

## Profile, playlists, Spotify lyrics, settings — 2026-10-17

* **Profile without a developer app**: in session mode the Web API is off, so the top
  bar used to show the raw username. Lumen now asks Spotify's profile service
  (`user-profile-view/v3`, through librespot's `spclient`) for the display name, photo,
  follower counts, public playlists and recently played artists. Image references
  (`spotify:image:`, `spotify:mosaic:`) are turned into CDN URLs.
* **Playlist editing**: with a developer app, the documented Web API (`/me/playlists`,
  `/playlists/{id}`, `/playlists/{id}/items` with `/tracks` fallback, `/followers`).
  Without one, or if the Web API refuses, Spotify's own playlist service: protobuf
  `ListChanges` (`playlist4_external`) posted to `/playlist/v2/playlist/{id}/changes`,
  on top of the current revision; new playlists via `ListUpdateRequest` to
  `/playlist/v2/playlist`, then added to the user's rootlist. Removing uses
  `items_as_key`, so every copy of the song is removed, like Spotify. New playlists are
  named "My Playlist #N" and the description is only sent if the user edited it (Lumen
  doesn't always know the current one). I couldn't test these calls against a live
  account from the build environment; failures show a clear toast.
* **Lyrics**: Spotify's `color-lyrics/v2` first (line-synced or plain), then LRCLIB.
  The provider name is cached next to the lyrics.
* **Settings** were rebuilt as rows (label + description left, control right), with
  custom dropdowns, filled text boxes, outlined pill buttons, keycaps, accent swatches,
  section chips that scroll to a section, and a search that filters rows.

## Instant lists, Spotify-like motion, listening stats — 2026-10-17

* **Stale-while-revalidate list cache**: track lists are written to
  `cache/lists/` (JSON, 64 MB cap with eviction) every few seconds when they change,
  and on exit. Opening a list shows the saved copy at once and requests only the first
  page: if its total and first URIs match, the cache is kept (with all its loaded
  metadata), otherwise the list restarts from the fresh page. Session-mode playlists
  (URI lists + lazy metadata) are merged by URI, so cached metadata isn't fetched again.
  Offline, the cached copy simply stays. The playlist sidebar is cached the same way
  and shown during auto-login.
* **Prefetch on hover**: hovering a playlist/album card or library row for ~0.15 s
  starts loading it (capped by the in-memory list LRU), like Spotify's web player.
* **Motion**: covers fade in when they arrive, library rows and seek/volume bars ease
  their hover state (the knob grows in, the fill turns accent). These join the existing
  card/row/panel animations; all request repaints only while moving.
* **Listening history** is Lumen's own: Spotify's listening history isn't available
  without a developer app and Web API scopes. A play is recorded when the song
  changes or the app exits, if it was actually heard for ≥ 30 s (wall-clock time while
  playing, capped at the song's length). History lives in `history.jsonl` in the data
  folder; stats are computed on a background thread in local time (chrono).

## Library menu, drag and drop, radio, Connect picker, artist pages, mini player — 2026-10-17

* **No "Download" in the library menu**: offline downloading means saving tracks as
  audio files, which the project rules out. The menu has everything else Spotify's has.
* **Collection service without a developer app**: saved albums, followed artists and
  save/remove/follow go through spclient `/collection/v2/paging` and `/collection/v2/write`
  (`application/vnd.collection-v2.spotify.proto`). librespot-protocol doesn't compile
  `collection2v2.proto`, so `src/collection_pb.rs` encodes/decodes the few messages by
  hand (with tests). With a developer client id the Web API is used instead.
* **Artist info** (bio, portrait, related artists, popularity) comes from the artist
  metadata Lumen already loads; the About card shows three lines and "Read more" opens
  the full text in a dialog.
* **Home from local history**: `history::home_mix` builds Jump back in (distinct recent
  albums), Recently played, top artists and On repeat (last four weeks) on a background
  thread. The Web API's lists win when they exist; history fills whatever is empty.
  "Your mixes" cards start artist radio (`inspiredby-mix`).
* **Recent searches** are stored in `config.toml` (`ui.recent_searches`, max 10): an
  item is added when you open a result from a non-empty search or play a song from it.
* **Private session** is local only (history, Discord, recent searches). It isn't
  persisted, like Spotify's, which ends on its own.
* **Mini player** reuses the main window (no second viewport, so no extra GPU context
  or memory): it saves the window size, lowers the minimum size, resizes to 340×340
  and sets always-on-top; leaving restores both. The window drags from anywhere.
* **Keyboard navigation**: fixed Spotify keys (Alt+arrows, mouse side buttons,
  Alt+Shift+B, Ctrl+Shift+M) are handled before the configurable shortcuts. Song rows
  have a selection (click or ↑/↓, Home/End) that scrolls into view; Enter plays it.
  Arrow keys are ignored while a text box has focus.
* Memory after this batch, same Linux stress run as before: 168 MB at start, 178 MB
  peak (about 64 MB of it is the software GL renderer in the test VM).

## Permanent files from matched recordings — 2026-10-17

* The user explicitly requested automatic permanent music files and authorized
  choosing a GitHub downloader. Added matched YouTube audio through yt-dlp rather
  than exporting Spotify audio or retaining Spotify decryption keys. The existing
  Spotify streaming/cache mechanism is unchanged.
* Playing a known Spotify song schedules one background job. Title, primary artist,
  version markers and duration must match; official/Topic results are preferred.
  Completed files are tagged MP3s in the user's Music/Lumen Downloads folder.
  A persistent index maps the original Spotify URI to the file and source URL.
* Saved files have no expiry or automatic eviction and are separate from cache
  clearing. Later Lumen queue playback uses the file first, including with no
  authenticated session; downloaded tracks also appear under Local Files on startup.
  The active stream is not switched mid-song. Phone-controlled Connect still streams.
* One worker and a deduplicated 32-item queue bound resource usage. Helpers run
  without windows, time out, and report failures. Partial files are not indexed.
  No browser cookies are read. Private sessions do not schedule downloads.
* Settings/Storage exposes automatic saving, folder access, status and Retry song.
  Existing files remain usable with automatic saving disabled. A custom folder in
  config applies after restart. yt-dlp, FFmpeg and Node are already installed here.
* Live verification downloaded, tagged, indexed, reopened and decoded a matched
  recording successfully. Some other results required YouTube sign-in; such failures
  remain visible rather than being reported as saved. Matching/coverage is heuristic.
* 101 normal tests pass, with three external-service/audio tests excluded by default.
  The live matched-download test passed separately. Strict Clippy and formatting
  pass; also fixed an existing identity-map lint in the library pin ordering.
* Automatic review rejected two combined shell edit/check commands with a generic
  policy message. Direct file patches and separate checks succeeded. The optional
  process-tree cleanup enhancement was omitted; existing child cancellation remains.
* The Windows release launch exposed an existing Connect watcher startup panic:
  watch_devices spawned on the UI thread outside Tokio. Enter the existing shared
  runtime while registering it. This crash was caught before delivering the build.
* Corrected Windows release: build.ps1 succeeded (19.8 MiB). The process stayed
  alive after saved-account login with a Lumen window and no new warning/error logs.
  Music/Lumen Downloads was created. outputs/lumen-downloads.exe matches dist by
  SHA-256. Interactive screenshot validation remains unavailable due to the earlier
  Computer Use approval restriction; downloader/decoder and offline routing were
  validated with live and automated tests instead.

## Public-source fallback after the reported YouTube block — 2026-10-17

* Reproduced the screenshot's sign-in/bot error for Tom Odell's When I Close My
  Eyes. The complete official yt-dlp helper also received it. Metadata and lyrics
  do not establish audio provenance; the suggested website's private backend source
  could not be verified from its public frontend. Its page discloses third-party
  services outside Spotify and requires a CAPTCHA.
* Honored the user's request for no cookies or account setup. Added public SoundCloud
  fallback with the same title/artist/duration checks. Reject 30-second previews,
  unwanted versions and external URLs. Try at most two matches per provider.
* YouTube authentication/rate-limit errors temporarily suspend that provider for
  30 minutes instead of repeatedly hitting it. SoundCloud remains available. Normal
  playback continues even when neither source has a usable full-length recording.
* Download errors now use short human-readable messages. Technical details are
  logged instead of being dumped into a large red toast over the player. Source
  attempts start fresh so partial bytes from different recordings cannot be mixed.
* Installed the official yt-dlp 2026.08.19 Windows executable, verified against the
  release SHA-256 sums, in Lumen's app data tools folder. It includes the EJS scripts
  absent from the previous Python helper. Lumen prefers this bundled helper.
* Live public-source regression downloaded and decoded the exact reported song
  from a full-length SoundCloud result without login/cookies. The publisher's own
  30-second result was rejected. Matching remains heuristic; coverage is not universal.
* 103 offline-independent tests pass. New tests cover public-source URL validation,
  preview rejection and short error messages. The live fallback test passed separately.

## Spotify-like playback starts, find/sort, multi-select, resume — 2026-10-17

* **Whole-list play respects shuffle**: `App::play_whole` starts at a random song when
  shuffle is on (Spotify's play button does this); the player then shuffles the rest.
  Lists still loading are queued through `autoplay` and use the shuffle state at that
  moment. The page's shuffle button starts the list shuffled if it isn't the one playing.
* **Library rows**: double-click plays; a hover play/pause button sits on the cover.
  `toggle_play_view` pauses/resumes when the row is already the current context.
  Cards and Home tiles play in place instead of opening the page.
* **Resume**: `session.json` (data folder) holds the current song, its list (up to 5 000
  songs) and position; written when the song changes (background thread) and at exit.
  On launch it's sent as `PlayerCmd::Restore`, which loads paused and does nothing if
  something is already playing, Connect is in control, or (for a Spotify song) the
  session isn't up — so a failed restore can never skip ahead and start other music.
  Local songs restore before login; Spotify songs right after it.
* **Find/sort** works on list indices through an order vector (cached by a signature of
  query, sort, length and stub count). While active, the remaining pages and missing
  song names are loaded (three metadata chunks per frame), drag-reordering is off, and
  playing uses the shown order as the context.
* **Multi-select** stores list indices (stable under sorting); a changed list length
  drops the selection. Menu actions on a selected row apply to the whole selection;
  dragging a selection carries all its songs (no reorder for multiple).
* "Save to Liked Songs" is no longer disabled without a developer app: saving uses the
  collection service in session mode (since the previous batch).

## Song menu everywhere, save popover, credits — 2026-10-17

* One song menu (`track_menu_items`) in Spotify's order serves list rows, the player
  bar, Now playing and the queue; outside lists "Remove from this playlist" and "Play"
  are hidden. Left-click on the player-bar title/artist still goes to the album/artist,
  as in Spotify; right-click opens the menu.
* **Save popover**: ticks for playlists are pre-filled only where Lumen has the playlist
  loaded and can see the song in it; changes are applied on Done (like, add, remove).
  Clicking outside or Esc cancels.
* **Credits** come from spclient `/track-credits-view/v0/experimental/{id}/credits`
  (JSON: `roleCredits[].artists[]` with `subroles`, plus `sourceNames`). It's an internal
  endpoint; when it fails the dialog says Spotify has no credits for the song.

## SpotiDownloader integration, local token setting — 2026-10-17

* Integrated the public client's API contract in Rust: POST the Spotify track ID
  to api.spotidownloader.com/download with a Bearer token, then stream the returned
  audio link. Verified the contract against the Python client and the archived Go
  client (afkarxyz/SpotiDownloader, 3caa8aa63189) available through the Go module proxy.
  This is a client integration, not the website's private backend source code.
* Preserved the latest song-menu, save-popover, resume and library changes. The only
  added UI is a masked SpotiDownloader token row with Save/Clear in Settings > Storage.
  Existing automatic-save description now explains provider selection. Browser
  instructions are available on hover and in README. No Chrome/CAPTCHA automation.
* Token uses the existing local SecretStore, never config.toml. Git ignores runtime
  secrets even under a portable install. Input clears after Save; token replacements
  apply on the next job. No real user token was accessed, added or committed.
* A saved token selects SpotiDownloader; removing it restores public-source matching.
  A service failure is reported without silently substituting a different recording.
  Permanent files, duration checks, atomic finalization and offline routing are reused.
* Token headers are sensitive, confined to the API origin, and never forwarded to
  third-party audio hosts. Signed audio URLs and raw service response bodies are not
  logged or persisted. HTTPS links and redirects are checked; requests, reply sizes
  and audio size are bounded; shutdown cancels network work.
* Mock HTTP tests cover request authentication, expired-token handling, valid and
  malformed replies, response bounds, secret redaction, origin scoping and cancellation.
  Live service download remains unverified until a valid token is saved locally.
* Validation: 112 tests passed (4 external/device tests ignored), strict all-target
  Clippy and cargo fmt checks passed. Cargo.toml, Cargo.lock and release settings
  are unchanged; portable secret paths are confirmed ignored by Git.

## AAC/MP4 download response compatibility — 2026-10-17

* User supplied a browser audio request to aac.saavncdn.com ending in _320.mp4.
  This identifies the delivery host for that request, not every song's upstream
  source. The separate pasted file contained a Cloudflare challenge, not an API
  Authorization header. No token was extracted or challenge request replayed.
* Service audio is now identified by its bytes, since the returned data can be
  AAC in MP4 despite the MP3 staging filename. FFmpeg copies MP3 inputs and converts
  other codecs to MP3 before permanent tagging/indexing; conversion has a 180s limit.
* Synthetic AAC/MP4 regression passed: staged under an MP3 filename, converted,
  title tags checked, MP3 type verified and local playback decoded. 112 normal
  tests pass; the FFmpeg regression was run separately. No GUI changes.

## Attended in-app verification, on-demand WebView2 — 2026-10-17

* Preserved the user's latest UI polish commit d7a56a5. Added a small verification
  panel and a Settings > Storage connection row, keeping manual tokens under an
  Advanced disclosure. No full Chrome process, browser profile access or extensions.
* Windows uses its existing WebView2 runtime through pinned wry/webview2-com/windows
  dependencies. The lockfile is updated for these dependencies; vergen 9.0.6 and the
  release profile stay unchanged. Other platforms keep the manual workflow.
* Load the real provider origin, trigger its ordinary search to display its existing
  widget, and present the widget without surrounding page content. The user performs
  all challenge interaction. No CAPTCHA solver, simulated challenge clicks, cookie
  extraction, challenge replay, browser identity spoofing or Cloudflare bypass.
* Capture only the successful provider /session response. IPC is accepted only from
  that exact HTTPS origin with bounded messages. Block popups, downloads, autofill,
  browser menus and permission requests. Allow network resources only from the
  provider and challenges.cloudflare.com. Keep provider widget internals intact.
* The isolated WebView2 profile is app data, ignored by Git. No browser is created at
  startup, while idle or to periodically refresh a token. Create on demand when a
  download lacks a session, its expiry is near, or the API rejects it; renew at most
  once per job. Verification does not block music playback. Close/cancel/timeout
  releases the child view; shutdown cancels any waiting job. Bring Lumen forward
  when attendance is needed. A Show page fallback handles layout changes.
* Verification success saves only to the existing local SecretStore and resumes the
  waiting job. Disconnect cancels verification, removes the token and disables this
  connection mode; existing permanent files and offline playback are unaffected.
* 118 regular tests pass. Added expiry/rejection, request/resume/cancel, origin/frame
  isolation and resource-allowlist checks. The JavaScript bridge regression passed.
  A separate native Windows test created a hidden WebView2 child, loaded synthetic
  local HTML, received IPC and closed the view successfully. No production challenge
  was solved automatically. Live provider acceptance still requires the user to
  complete its real verification and may depend on embedded-browser support.

## UI polish pass — 2026-10-17

* Scrollbars float over content (egui `ScrollStyle::floating`), thin until hovered,
  in a translucent grey instead of the accent; no track. Plain widgets no longer grow
  an outline on hover. Text boxes get a text-coloured caret and a soft accent selection.
* Loading uses skeleton placeholders (pulsing blocks shaped like the page) instead of
  spinners: playlist pages, Home, library grids and artist "Popular".
* "Your themes" is a card grid; each card paints a tiny Spotify window in the theme's
  own colours (main, sidebar, cards, text, accent) instead of a list of links. Clicking a
  card applies it; its colour schemes show underneath; right-click to remove.
* Text in menus isn't justified any more (egui stretches word spacing in justified
  layouts), and the developer-app description in Settings is shorter.

## Library clicks, card menus, Now playing cards — 2026-10-17

* **Your Library clicks**: in the cover rail the cover is the whole row, so the hover
  play button made a single click play instead of open. The rail now has no play
  button (click opens, double-click plays); in the list view only the play icon itself
  (28 px) plays, the rest of the row opens.
* **Add to / Remove from Your Library** for albums (`/me/albums` or the collection
  "collection" set), followed playlists (`/playlists/{id}/followers` or a rootlist add)
  and artists (follow). Optimistic, rolled back with a message if Spotify refuses.
  Removing your own playlist still asks first (it deletes it).
* Cards use the library menu on right-click; song cards use the song menu.
* Now playing: "About the artist" reuses the artist info cache; "Credits" loads once
  per song (64-song cache) and shares results with the credits dialog.

## Spotify parity pass: artist page, albums, friends — 2026-10-17

* **Artist header** uses the artist's portrait (from the artist metadata) before any
  album cover; with a portrait it's a full-width banner like Spotify's. Monthly
  listeners and play counts only exist in Spotify's web GraphQL API (persisted query
  hashes that change), so Lumen shows popularity instead of guessing.
* **Releases** carry their group (album / single / compilation / appears on), year and
  popularity (`model::Release`); cards say "2024 • Single" like Spotify.
* **Album footer** from album metadata (date, label, copyrights; ℗ is drawn as "(P)"
  because the bundled fonts lack the glyph). **Recommended** = the playlist's radio
  minus songs already in it; Refresh reshuffles.
* **Friend Activity** calls spclient `/presence-view/v1/buddylist`. That endpoint may
  refuse Lumen's login token; the panel then explains it instead of showing an error.
* **Volume level** maps to librespot's normalisation pre-gain: Loud +3 dB, Normal 0,
  Quiet -6 dB.
* PARITY.md lists what's still missing and what a third-party client can't do.



## Independent playback and personal library import — 2026-10-17

* Preserve the latest d40eaa8 GUI/parity baseline and the pinned dependencies.
  Add an opt-in no-Premium entry, Storage controls and scoped local search.
* Do not modify librespot's Premium check. In independent mode skip streaming
  auto-login entirely: librespot-core 0.8 exits the process for Free accounts.
* Personal library metadata comes from user-provided Spotify account-data JSON
  or an exact Spotify link through the existing attended provider connection.
  No Spotify cookies, browser credentials or streaming keys are extracted.
* Persist imported playlists outside the evictable list cache, including order
  and duplicates, so they survive restart without Spotify login. Bound imports
  to 64 MB, 1,000 playlists and 50,000 tracks. Keep personal library files ignored.
* Missing requested tracks enter a download wait before playback. Completion
  only affects the currently awaited URI; pause/seek intentions survive the wait.
  Failed downloads stop for explicit retry, avoiding accidental bulk downloads.
* Explicit playback requests are separate from the background autosave switch.
  Cached originals retain Spotify URIs for queues/lyrics but load from disk.
* Export files often lack duration. Only exact-ID download-provider results may
  omit reference-duration checking; decode and measure the resulting audio before
  indexing. Generic YouTube/SoundCloud matching still rejects unknown duration.
* Provider metadata requests are fixed-origin HTTPS with sensitive Authorization
  headers, no redirects, bounded responses, bounded advancing pagination and a
  total timeout. Tokens remain in the existing SecretStore. No bypass solver.
* This is not live Free-account Spotify sync or a promise of full provider
  coverage. Public link imports need a real attended-session check. Private
  account data has the JSON import route. Search is over imported/local music.

* Validation: 127 tests passed (6 external/device tests excluded), strict all-target Clippy passed; the synthetic AAC-to-MP3 FFmpeg integration and verification JavaScript bridge tests also passed. Live provider import acceptance remains a user-session check.


## Public repository and Linux port — 2026-10-17

* User authorized publishing the complete project publicly as zaydzaari/Lumen.
  Preserve existing commits after scanning history for token/private-key patterns;
  keep runtime tokens, browser profiles, personal libraries, caches and downloads
  outside tracked source and release archives.
* Use system WebKitGTK 4.1 for attended Linux verification. A short-lived helper
  process avoids mixing eframe/winit and GTK/tray main loops and supports GTK's
  X11/Wayland backends. The same executable launches its internal helper; token
  handoff uses private bounded pipes, never command-line arguments or logs.
* Install a default-deny WebKit content filter before loading the provider. The
  stable content-filter APIs are called through webkit2gtk::ffi because the Rust
  wrapper omits them. Keep permission/popup/download rejection and origin checks.
  No browser-cookie extraction, CAPTCHA solving or sandbox disabling in the app.
* Add Linux desktop/icon integration, user-local installer, portable tarball and
  Ubuntu-style .deb packaging. CI tests and builds Linux on Ubuntu 24.04 and
  Windows separately; Linux runtime dependencies are deliberately explicit.
* Keep dependency versions and release optimization settings pinned. GTK is now
  required for Linux verification even with the optional tray disabled. Native
  live provider acceptance and physical Linux audio need real-session/hardware
  checks; synthetic runtime tests do not establish those.


## Broad Linux distribution packaging — 2026-10-18

* User requested support for friends on Mint and varied gaming/Linux distributions.
  Add a Flatpak bundle using the supported GNOME 50 runtime alongside native .deb
  and tarball downloads. Runtime inspection confirmed GTK3, WebKitGTK 4.1, ALSA
  configuration and FFmpeg with libmp3lame are present.
* Build the Flatpak variant without the optional tray feature, retaining media
  controls. This avoids shipping host libxdo/AppIndicator shared libraries or
  relying on a distribution's tray integration. No Chromium is bundled.
* Flatpak exposes home folders for the existing local-file/import workflows,
  network, graphics, audio and MPRIS. WebKit's Flatpak portal access is retained
  for its own sandbox. No production sandbox disabling is used.

Linux release startup is checked under Xvfb as well as through unit tests. The
X11 backend loads libxkbcommon-x11 dynamically, so native packages explicitly
depend on libxkbcommon-x11-0. Packaging version parsing also accepts CRLF Cargo
metadata when invoked from a Windows checkout through WSL.

## Account library OAuth and verified update downloads

Library OAuth uses the registered public Lumen Client ID and read-only scopes,
with a separate protected refresh token. It never opens a librespot streaming
session. Account snapshots use a per-account ID namespace; denied playlists do
not erase the existing library, and stale/cancelled login results are ignored.
The UI distinguishes library connection from Premium streaming. Spotify's
allowlist, ownership and account restrictions still apply.

The updater checks only stable releases of zaydzaari/Lumen. It checks size and
SHA-256 from the release manifest, downloads off the UI thread, and hands off to
the normal platform installer after a user click. It does not replace a running
executable itself. Windows setup is per-user through Inno Setup; Linux packaging
remains Flatpak, Debian and a portable archive. Initial 0.1.0 users must manually
install 0.1.1. Release checksum manifests must cover every published installer.
