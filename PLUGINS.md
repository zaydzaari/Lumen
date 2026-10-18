# Lumen plugins

Lumen runs plugins written in **Lua 5.4** (vendored, so nothing to install). Every
`*.lua` file in the plugins folder is loaded at startup:

| OS      | Plugins folder                                             |
|---------|------------------------------------------------------------|
| Windows | `%APPDATA%\lumen\Lumen\config\plugins\`                    |
| macOS   | `~/Library/Application Support/dev.lumen.Lumen/plugins/`   |
| Linux   | `~/.config/lumen/plugins/`                                 |

Settings → Plugins lists every plugin with an enable/disable checkbox, its errors (if
any), and buttons to open the plugins folder, open the plugin data folder, and reload.
The file name (without `.lua`) is the plugin's name; it may only contain letters,
digits, `.`, `_` and `-`.

## Sandbox

Each plugin runs in its own Lua state. Available:

* `table`, `string` (without `string.dump`), `math`, `utf8`, `coroutine`
* `os.time()` (Unix seconds) and `os.clock()` (seconds since the plugin loaded)
* `print(...)` writes to Lumen's log file
* the `lumen` table described below

**Not** available: `io`, `os.execute` / `os.remove` / `os.getenv` and the rest of
`os`, `require`, `package`, `load`, `loadfile`, `dofile`, `debug`,
`collectgarbage`. Plugins are loaded as text only (precompiled bytecode is rejected).

Limits:

* **Time** — loading a plugin may take at most 500 ms; each event handler or command
  call at most 100 ms. A plugin that exceeds this gets a "time budget" error (an
  infinite loop cannot freeze Lumen).
* **Memory** — 16 MB per plugin.
* **Files** — only inside the plugin's own data folder, max 1 MB per file.

Errors in handlers are shown as a notification and logged; they never crash Lumen.

## API

### Events

```lua
lumen.on(event, function(track) ... end)
```

| Event          | When                                  | Argument          |
|----------------|---------------------------------------|-------------------|
| `track_change` | a different track starts              | track table       |
| `play`         | playback starts or resumes            | track table / nil |
| `pause`        | playback pauses or stops              | track table / nil |

Several handlers per event are allowed. Unknown event names raise an error.

A **track table** has: `title`, `artist` (comma-separated names), `album`, `uri`
(`spotify:track:…` or `local:<absolute path>`), `duration_ms`.

Local files use the same `track_change`, `play`, `pause` events and current-track
table, including when no Spotify account is logged in. Treat `uri` as an opaque
identifier: local paths can contain spaces, Unicode, backslashes and colons (and
Windows canonical paths may include the `\\?\` prefix). Plugin file access remains
confined to its own data directory; a local URI does not grant access to audio files.

### Playback state

| Function                 | Returns                                    |
|--------------------------|--------------------------------------------|
| `lumen.current_track()`  | track table, or `nil` when nothing plays   |
| `lumen.is_playing()`     | boolean                                    |
| `lumen.position_ms()`    | position (ms) at the last state change     |

### Commands

```lua
lumen.add_command("Label", function() ... end)
```

Adds an entry to the **🧩 Plugins** menu at the bottom of the sidebar (shown as
`plugin_name: Label`). Max 32 commands per plugin.

### Notifications

```lua
lumen.notify("text")   -- shows a toast "plugin_name: text"
```

### Theme

```lua
lumen.set_theme {
  mode = "dark",          -- or "light" (optional)
  accent = "#ff7a45",     -- all colours optional, "#rrggbb"
  background = "#1a1220",
  panel = "#241829",
  text = "#f3e9e1",
}
```

Overrides the configured theme until plugins are reloaded or the plugin is disabled.
Invalid colours raise an error.

### Files (sandboxed)

```lua
lumen.write_file("name.txt", text)    -- create/replace
lumen.append_file("name.txt", text)   -- append (creates the file)
local s = lumen.read_file("name.txt") -- string, or nil if missing
```

Names must be plain file names (`[A-Za-z0-9._-]`, not starting with `.`); paths,
`..` and absolute paths are rejected. Files live in
`<data dir>/plugin-data/<plugin name>/`:

| OS      | Plugin data folder                                       |
|---------|----------------------------------------------------------|
| Windows | `%LOCALAPPDATA%\lumen\Lumen\data\plugin-data\<name>\`     |
| macOS   | `~/Library/Application Support/dev.lumen.Lumen/plugin-data/<name>/` |
| Linux   | `~/.local/share/lumen/plugin-data/<name>/`               |

### Info

* `lumen.plugin_name` — this plugin's name
* `lumen.data_dir` — absolute path of the plugin's data folder (for display)
* `lumen.version` — Lumen version

## Example plugins

Both are copied into the plugins folder on first run (sources in `plugins/`).

### `now_playing.lua` — for streamers (enabled by default)

Writes `Artist - Title` to `now_playing.txt` in its data folder whenever the track
changes, and clears it while paused (change `PAUSED_TEXT` at the top to keep the
text). It also appends to `history.txt`, starting a new file when it reaches 1 MB.

To use it in OBS: add a **Text (GDI+)** source (Windows) or **Text (FreeType 2)**
(macOS/Linux), tick **Read from file** and pick
`…/plugin-data/now_playing/now_playing.txt`. The plugin's command
*"Show now-playing file location"* displays the exact path.

### `theme_sunset.lua` — theme plugin (installed disabled)

Applies a warm purple/orange palette with `lumen.set_theme` and adds a
*"Toggle sunset theme"* command. Enable it in Settings → Plugins.

## Writing your own

```lua
-- skip_reminder.lua: remind me to stretch every 10 tracks
local count = 0
lumen.on("track_change", function(t)
  count = count + 1
  if count % 10 == 0 then
    lumen.notify("10 tracks in — time to stretch!")
  end
end)
```

Save it into the plugins folder and press **Reload plugins** in Settings.
