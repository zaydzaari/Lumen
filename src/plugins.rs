//! Lua 5.4 plugins (vendored, sandboxed) loaded from `<config>/plugins/*.lua`.
//!
//! Each plugin runs in its own Lua state with only `table`, `string`, `math`,
//! `utf8` and `coroutine` available, plus a tiny `os` (`time`, `clock`) and the
//! `lumen` API table. There is no `io`, `os.execute`, `require`, `load`,
//! `dofile`, `loadfile`, `debug` or `string.dump`. File access is limited to
//! `lumen.read_file` / `write_file` / `append_file` inside the plugin's own data
//! folder. Each call into a plugin has a time budget (instruction hook) and each
//! state a memory cap, so a broken plugin cannot freeze or bloat the app.
//!
//! See PLUGINS.md for the full API.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, Value, VmState};

use crate::config::parse_hex_color;

const MEMORY_LIMIT: usize = 16 * 1024 * 1024;
const LOAD_BUDGET: Duration = Duration::from_millis(500);
const CALL_BUDGET: Duration = Duration::from_millis(100);
const MAX_FILE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub uri: String,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeOverride {
    pub accent: Option<[u8; 3]>,
    pub background: Option<[u8; 3]>,
    pub panel: Option<[u8; 3]>,
    pub text: Option<[u8; 3]>,
    /// "dark" / "light"
    pub mode: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PluginAction {
    Notify { plugin: String, text: String },
    Theme(ThemeOverride),
}

#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub name: String,
    pub enabled: bool,
    pub error: Option<String>,
    pub commands: usize,
}

/// State readable by plugins (`lumen.current_track()` etc.).
#[derive(Default)]
struct Shared {
    track: Option<TrackInfo>,
    playing: bool,
    position_ms: u32,
    actions: Vec<PluginAction>,
}

struct Plugin {
    name: String,
    lua: Lua,
    deadline: Rc<Cell<Instant>>,
    commands: Rc<RefCell<Vec<String>>>,
}

pub struct PluginHost {
    plugins: Vec<Plugin>,
    infos: Vec<PluginInfo>,
    shared: Rc<RefCell<Shared>>,
    dir: PathBuf,
}

/// Plugin names become folder names, so restrict them.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.contains("..")
}

impl PluginHost {
    pub fn empty() -> Self {
        Self {
            plugins: Vec::new(),
            infos: Vec::new(),
            shared: Rc::new(RefCell::new(Shared::default())),
            dir: PathBuf::new(),
        }
    }

    /// Load every `*.lua` file in `plugins_dir`. Errors are recorded per plugin.
    pub fn load_dir(plugins_dir: &Path, data_root: &Path, disabled: &[String]) -> Self {
        let mut host = Self::empty();
        host.dir = plugins_dir.to_path_buf();
        let mut files: Vec<PathBuf> = std::fs::read_dir(plugins_dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "lua"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for file in files {
            let Some(name) = file
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if !valid_name(&name) {
                tracing::warn!("skipping plugin with invalid name: {}", file.display());
                continue;
            }
            let enabled = !disabled.contains(&name);
            let mut info = PluginInfo {
                name: name.clone(),
                enabled,
                error: None,
                commands: 0,
            };
            if enabled {
                match std::fs::read_to_string(&file) {
                    Ok(src) => match host.load_source(&name, &src, &data_root.join(&name)) {
                        Ok(()) => {
                            info.commands = host
                                .plugins
                                .last()
                                .map(|p| p.commands.borrow().len())
                                .unwrap_or(0);
                            tracing::info!("loaded plugin {name}");
                        }
                        Err(e) => {
                            tracing::warn!("plugin {name} failed to load: {e}");
                            info.error = Some(e.to_string());
                        }
                    },
                    Err(e) => info.error = Some(e.to_string()),
                }
            }
            host.infos.push(info);
        }
        host
    }

    pub fn infos(&self) -> &[PluginInfo] {
        &self.infos
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Load one plugin from source (also used by tests).
    pub fn load_source(&mut self, name: &str, src: &str, data_dir: &Path) -> mlua::Result<()> {
        let plugin = new_sandbox(name, data_dir, self.shared.clone())?;
        plugin.deadline.set(Instant::now() + LOAD_BUDGET);
        plugin
            .lua
            .load(src)
            .set_name(format!("@{name}.lua"))
            .set_mode(mlua::ChunkMode::Text)
            .exec()?;
        self.plugins.push(plugin);
        Ok(())
    }

    pub fn set_state(&self, track: Option<TrackInfo>, playing: bool, position_ms: u32) {
        let mut s = self.shared.borrow_mut();
        s.track = track;
        s.playing = playing;
        s.position_ms = position_ms;
    }

    /// Fire an event (`track_change`, `play`, `pause`) on every plugin.
    pub fn emit(&mut self, event: &str) {
        let track = self.shared.borrow().track.clone();
        for p in &self.plugins {
            let handlers: Option<Table> = p
                .lua
                .named_registry_value::<Table>("lumen_handlers")
                .ok()
                .and_then(|t| t.get::<Option<Table>>(event).ok().flatten());
            let Some(handlers) = handlers else { continue };
            for f in handlers.sequence_values::<Function>().flatten() {
                p.deadline.set(Instant::now() + CALL_BUDGET);
                let arg = match &track {
                    Some(t) => track_table(&p.lua, t)
                        .map(Value::Table)
                        .unwrap_or(Value::Nil),
                    None => Value::Nil,
                };
                if let Err(e) = f.call::<()>(arg) {
                    tracing::warn!("plugin {} error in {event}: {e}", p.name);
                    self.shared.borrow_mut().actions.push(PluginAction::Notify {
                        plugin: p.name.clone(),
                        text: format!("error in {event} handler: {}", first_line(&e.to_string())),
                    });
                }
            }
        }
    }

    /// (plugin index, command index, label) for every registered command.
    pub fn commands(&self) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        for (pi, p) in self.plugins.iter().enumerate() {
            for (ci, label) in p.commands.borrow().iter().enumerate() {
                out.push((pi, ci, format!("{}: {label}", p.name)));
            }
        }
        out
    }

    pub fn run_command(&mut self, plugin: usize, command: usize) {
        let Some(p) = self.plugins.get(plugin) else {
            return;
        };
        let f: Option<Function> = p
            .lua
            .named_registry_value::<Table>("lumen_commands")
            .ok()
            .and_then(|t| t.get::<Option<Function>>(command + 1).ok().flatten());
        if let Some(f) = f {
            p.deadline.set(Instant::now() + CALL_BUDGET);
            if let Err(e) = f.call::<()>(()) {
                self.shared.borrow_mut().actions.push(PluginAction::Notify {
                    plugin: p.name.clone(),
                    text: format!("command failed: {}", first_line(&e.to_string())),
                });
            }
        }
    }

    pub fn drain_actions(&mut self) -> Vec<PluginAction> {
        std::mem::take(&mut self.shared.borrow_mut().actions)
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

fn track_table(lua: &Lua, t: &TrackInfo) -> mlua::Result<Table> {
    let tbl = lua.create_table()?;
    tbl.set("title", t.title.as_str())?;
    tbl.set("artist", t.artist.as_str())?;
    tbl.set("album", t.album.as_str())?;
    tbl.set("uri", t.uri.as_str())?;
    tbl.set("duration_ms", t.duration_ms)?;
    Ok(tbl)
}

fn safe_file(dir: &Path, name: &str) -> mlua::Result<PathBuf> {
    if !valid_name(name) {
        return Err(mlua::Error::RuntimeError(format!(
            "invalid file name '{name}': use letters, digits, '.', '_' or '-' only"
        )));
    }
    Ok(dir.join(name))
}

fn color_arg(t: &Table, key: &str) -> mlua::Result<Option<[u8; 3]>> {
    match t.get::<Option<String>>(key)? {
        None => Ok(None),
        Some(s) => parse_hex_color(&s).map(Some).ok_or_else(|| {
            mlua::Error::RuntimeError(format!("{key}: expected '#rrggbb', got '{s}'"))
        }),
    }
}

fn new_sandbox(name: &str, data_dir: &Path, shared: Rc<RefCell<Shared>>) -> mlua::Result<Plugin> {
    let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
    let lua = Lua::new_with(libs, LuaOptions::new())?;
    lua.set_memory_limit(MEMORY_LIMIT)?;

    let deadline = Rc::new(Cell::new(Instant::now() + LOAD_BUDGET));
    {
        let deadline = deadline.clone();
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(2000),
            move |_, _| {
                if Instant::now() > deadline.get() {
                    Err(mlua::Error::RuntimeError(
                        "plugin exceeded its time budget".into(),
                    ))
                } else {
                    Ok(VmState::Continue)
                }
            },
        )?;
    }

    let g = lua.globals();
    for banned in [
        "dofile",
        "loadfile",
        "load",
        "require",
        "collectgarbage",
        "debug",
        "io",
        "package",
    ] {
        g.raw_set(banned, Value::Nil)?;
    }
    let string: Table = g.get("string")?;
    string.raw_set("dump", Value::Nil)?;

    // Minimal, harmless `os`.
    let os = lua.create_table()?;
    let start = Instant::now();
    os.set(
        "time",
        lua.create_function(|_, ()| {
            Ok(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0))
        })?,
    )?;
    os.set(
        "clock",
        lua.create_function(move |_, ()| Ok(start.elapsed().as_secs_f64()))?,
    )?;
    g.set("os", os)?;

    let pname = name.to_string();
    g.set(
        "print",
        lua.create_function(move |_, args: mlua::Variadic<Value>| {
            let parts: Vec<String> = args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| "?".into()))
                .collect();
            tracing::info!(plugin = %pname, "{}", parts.join("\t"));
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value("lumen_handlers", lua.create_table()?)?;
    lua.set_named_registry_value("lumen_commands", lua.create_table()?)?;

    let api = lua.create_table()?;
    api.set("plugin_name", name)?;
    api.set("version", env!("CARGO_PKG_VERSION"))?;

    api.set(
        "on",
        lua.create_function(|lua, (event, f): (String, Function)| {
            if !matches!(event.as_str(), "track_change" | "play" | "pause") {
                return Err(mlua::Error::RuntimeError(format!(
                    "unknown event '{event}' (expected track_change, play or pause)"
                )));
            }
            let handlers: Table = lua.named_registry_value("lumen_handlers")?;
            let list: Table = match handlers.get::<Option<Table>>(event.as_str())? {
                Some(t) => t,
                None => {
                    let t = lua.create_table()?;
                    handlers.set(event.as_str(), &t)?;
                    t
                }
            };
            list.push(f)?;
            Ok(())
        })?,
    )?;

    let commands = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let commands = commands.clone();
        api.set(
            "add_command",
            lua.create_function(move |lua, (label, f): (String, Function)| {
                if commands.borrow().len() >= 32 {
                    return Err(mlua::Error::RuntimeError("too many commands".into()));
                }
                let label: String = label.chars().take(60).collect();
                let tbl: Table = lua.named_registry_value("lumen_commands")?;
                tbl.push(f)?;
                commands.borrow_mut().push(label);
                Ok(())
            })?,
        )?;
    }

    {
        let shared = shared.clone();
        api.set(
            "current_track",
            lua.create_function(move |lua, ()| match &shared.borrow().track {
                Some(t) => Ok(Value::Table(track_table(lua, t)?)),
                None => Ok(Value::Nil),
            })?,
        )?;
    }
    {
        let shared = shared.clone();
        api.set(
            "is_playing",
            lua.create_function(move |_, ()| Ok(shared.borrow().playing))?,
        )?;
    }
    {
        let shared = shared.clone();
        api.set(
            "position_ms",
            lua.create_function(move |_, ()| Ok(shared.borrow().position_ms))?,
        )?;
    }
    {
        let shared = shared.clone();
        let pname = name.to_string();
        api.set(
            "notify",
            lua.create_function(move |_, text: String| {
                let mut s = shared.borrow_mut();
                if s.actions.len() < 64 {
                    s.actions.push(PluginAction::Notify {
                        plugin: pname.clone(),
                        text: text.chars().take(300).collect(),
                    });
                }
                Ok(())
            })?,
        )?;
    }
    {
        let shared = shared.clone();
        api.set(
            "set_theme",
            lua.create_function(move |_, t: Table| {
                let mode = t.get::<Option<String>>("mode")?;
                if let Some(m) = &mode {
                    if m != "dark" && m != "light" {
                        return Err(mlua::Error::RuntimeError(
                            "mode must be 'dark' or 'light'".into(),
                        ));
                    }
                }
                let ov = ThemeOverride {
                    accent: color_arg(&t, "accent")?,
                    background: color_arg(&t, "background")?,
                    panel: color_arg(&t, "panel")?,
                    text: color_arg(&t, "text")?,
                    mode,
                };
                shared.borrow_mut().actions.push(PluginAction::Theme(ov));
                Ok(())
            })?,
        )?;
    }

    // Sandboxed file access inside the plugin's data folder only.
    let dir = data_dir.to_path_buf();
    {
        let dir = dir.clone();
        api.set(
            "write_file",
            lua.create_function(move |_, (file, text): (String, mlua::String)| {
                let bytes = text.as_bytes();
                if bytes.len() > MAX_FILE_BYTES {
                    return Err(mlua::Error::RuntimeError(
                        "file too large (max 1 MB)".into(),
                    ));
                }
                let path = safe_file(&dir, &file)?;
                std::fs::create_dir_all(&dir).map_err(mlua::Error::external)?;
                std::fs::write(path, &bytes[..]).map_err(mlua::Error::external)?;
                Ok(())
            })?,
        )?;
    }
    {
        let dir = dir.clone();
        api.set(
            "append_file",
            lua.create_function(move |_, (file, text): (String, mlua::String)| {
                use std::io::Write as _;
                let path = safe_file(&dir, &file)?;
                let existing = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as usize;
                let bytes = text.as_bytes();
                if existing + bytes.len() > MAX_FILE_BYTES {
                    return Err(mlua::Error::RuntimeError(
                        "file too large (max 1 MB)".into(),
                    ));
                }
                std::fs::create_dir_all(&dir).map_err(mlua::Error::external)?;
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(mlua::Error::external)?;
                f.write_all(&bytes[..]).map_err(mlua::Error::external)?;
                Ok(())
            })?,
        )?;
    }
    {
        let dir = dir.clone();
        api.set(
            "read_file",
            lua.create_function(move |lua, file: String| {
                let path = safe_file(&dir, &file)?;
                match std::fs::read(path) {
                    Ok(b) if b.len() <= MAX_FILE_BYTES => Ok(Value::String(lua.create_string(&b)?)),
                    Ok(_) => Err(mlua::Error::RuntimeError("file too large".into())),
                    Err(_) => Ok(Value::Nil),
                }
            })?,
        )?;
    }
    api.set("data_dir", dir.display().to_string())?;
    g.set("lumen", api)?;

    Ok(Plugin {
        name: name.to_string(),
        lua,
        deadline,
        commands,
    })
}

/// Event dispatch helper used by the UI: compares the previous and new playback
/// state and fires `track_change` / `play` / `pause`.
#[derive(Default)]
pub struct EventTracker {
    last_uri: Option<String>,
    last_playing: bool,
}

impl EventTracker {
    pub fn events(&mut self, uri: Option<&str>, playing: bool) -> Vec<&'static str> {
        let mut out = Vec::new();
        if uri.is_some() && uri != self.last_uri.as_deref() {
            out.push("track_change");
        }
        if playing != self.last_playing {
            out.push(if playing { "play" } else { "pause" });
        }
        self.last_uri = uri.map(str::to_string);
        self.last_playing = playing;
        out
    }
}

/// Example plugins shipped in `plugins/` and installed on first run.
pub const EXAMPLES: &[(&str, &str)] = &[
    ("theme_sunset", include_str!("../plugins/theme_sunset.lua")),
    ("now_playing", include_str!("../plugins/now_playing.lua")),
];

/// Copy the example plugins into the user's plugin folder once (first run).
/// The theme example is installed disabled so it doesn't override the user's theme.
pub fn install_examples(plugins_dir: &Path, marker: &Path) -> Vec<String> {
    let mut disabled = Vec::new();
    if marker.exists() {
        return disabled;
    }
    for (name, src) in EXAMPLES {
        let path = plugins_dir.join(format!("{name}.lua"));
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, src) {
                tracing::warn!("could not install example plugin {name}: {e}");
            }
        }
        if *name == "theme_sunset" {
            disabled.push(name.to_string());
        }
    }
    let _ = std::fs::write(marker, "examples installed\n");
    disabled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_with(src: &str) -> (PluginHost, tempfile::TempDir, mlua::Result<()>) {
        let dir = tempfile::tempdir().unwrap();
        let mut host = PluginHost::empty();
        let res = host.load_source("test", src, &dir.path().join("test"));
        (host, dir, res)
    }

    #[test]
    fn dangerous_globals_are_removed() {
        let src = r#"
            assert(io == nil, "io")
            assert(os.execute == nil, "os.execute")
            assert(os.remove == nil, "os.remove")
            assert(os.getenv == nil, "os.getenv")
            assert(require == nil, "require")
            assert(dofile == nil, "dofile")
            assert(loadfile == nil, "loadfile")
            assert(load == nil, "load")
            assert(debug == nil, "debug")
            assert(package == nil, "package")
            assert(string.dump == nil, "string.dump")
            assert(type(os.time()) == "number")
            assert(type(string.format) == "function")
        "#;
        let (_h, _d, res) = host_with(src);
        res.unwrap();
    }

    #[test]
    fn file_access_is_confined_to_data_dir() {
        let (mut h, dir, res) = host_with(
            r#"
            lumen.write_file("np.txt", "hello")
            lumen.append_file("np.txt", " world")
            assert(lumen.read_file("np.txt") == "hello world")
            assert(lumen.read_file("missing.txt") == nil)
            local ok = pcall(lumen.write_file, "../escape.txt", "x")
            assert(not ok, "parent dir must be rejected")
            ok = pcall(lumen.write_file, "/etc/passwd", "x")
            assert(not ok, "absolute path must be rejected")
            ok = pcall(lumen.write_file, "sub/x.txt", "x")
            assert(not ok, "subdirs must be rejected")
            ok = pcall(lumen.read_file, "..")
            assert(not ok)
            ok = pcall(lumen.write_file, ".hidden", "x")
            assert(not ok)
        "#,
        );
        res.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("test/np.txt")).unwrap(),
            "hello world"
        );
        assert!(!dir.path().join("escape.txt").exists());
        assert!(h.drain_actions().is_empty());
    }

    #[test]
    fn infinite_loops_are_stopped() {
        let (_h, _d, res) = host_with("while true do end");
        let err = res.unwrap_err().to_string();
        assert!(err.contains("time budget"), "{err}");

        // Also inside an event handler.
        let (mut h, _d, res) = host_with("lumen.on('play', function() while true do end end)");
        res.unwrap();
        let t0 = Instant::now();
        h.emit("play");
        assert!(t0.elapsed() < Duration::from_secs(2));
        let actions = h.drain_actions();
        assert!(matches!(&actions[0], PluginAction::Notify { text, .. } if text.contains("play")));
    }

    #[test]
    fn memory_is_capped() {
        let (_h, _d, res) =
            host_with("local t = {} for i = 1, 1e8 do t[i] = string.rep('x', 1024) .. i end");
        assert!(res.is_err());
    }

    #[test]
    fn bytecode_cannot_be_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = PluginHost::empty();
        // A precompiled chunk starts with "\x1bLua".
        let res = host.load_source("bin", "\x1bLuaS\0garbage", dir.path());
        assert!(res.is_err());
    }

    #[test]
    fn events_commands_notifications_theme() {
        let (mut h, _d, res) = host_with(
            r##"
            seen = {}
            lumen.on("track_change", function(t) seen[#seen+1] = t.title .. " by " .. t.artist end)
            lumen.on("pause", function() lumen.notify("paused " .. (lumen.current_track() or {title="?"}).title) end)
            lumen.add_command("Say hi", function() lumen.notify("hi") end)
            lumen.set_theme({ accent = "#ff8800", mode = "light" })
            assert(not pcall(lumen.on, "bogus", function() end))
            assert(not pcall(lumen.set_theme, { accent = "orange" }))
        "##,
        );
        res.unwrap();
        h.set_state(
            Some(TrackInfo {
                title: "Song".into(),
                artist: "Band".into(),
                ..Default::default()
            }),
            false,
            0,
        );
        h.emit("track_change");
        h.emit("pause");
        let cmds = h.commands();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].2, "test: Say hi");
        h.run_command(0, 0);
        let actions = h.drain_actions();
        assert!(actions.contains(&PluginAction::Theme(ThemeOverride {
            accent: Some([0xff, 0x88, 0]),
            mode: Some("light".into()),
            ..Default::default()
        })));
        assert!(actions.contains(&PluginAction::Notify {
            plugin: "test".into(),
            text: "paused Song".into()
        }));
        assert!(actions.contains(&PluginAction::Notify {
            plugin: "test".into(),
            text: "hi".into()
        }));
        let seen: String = h.plugins[0].lua.load("return seen[1]").eval().unwrap();
        assert_eq!(seen, "Song by Band");
    }

    #[test]
    fn local_tracks_reach_plugin_events_with_path_uri() {
        let (mut host, _dir, loaded) = host_with(
            r#"
            lumen.on("track_change", function(t) lumen.notify(t.uri .. "|" .. t.title) end)
        "#,
        );
        loaded.unwrap();
        host.set_state(
            Some(TrackInfo {
                uri: "local:C:\\Music\\owned.flac".into(),
                title: "Owned song".into(),
                ..Default::default()
            }),
            true,
            0,
        );
        host.emit("track_change");
        assert!(host.drain_actions().contains(&PluginAction::Notify {
            plugin: "test".into(),
            text: "local:C:\\Music\\owned.flac|Owned song".into()
        }));
    }

    #[test]
    fn load_dir_respects_disabled_and_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let pdir = dir.path().join("plugins");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join("good.lua"),
            "lumen.add_command('x', function() end)",
        )
        .unwrap();
        std::fs::write(pdir.join("bad.lua"), "this is not lua").unwrap();
        std::fs::write(pdir.join("off.lua"), "error('should not run')").unwrap();
        std::fs::write(pdir.join("readme.txt"), "ignored").unwrap();
        let host = PluginHost::load_dir(&pdir, &dir.path().join("data"), &["off".to_string()]);
        let infos = host.infos();
        assert_eq!(infos.len(), 3);
        let get = |n: &str| infos.iter().find(|i| i.name == n).unwrap();
        assert!(get("good").error.is_none() && get("good").commands == 1);
        assert!(get("bad").error.is_some());
        assert!(!get("off").enabled && get("off").error.is_none());
    }

    #[test]
    fn event_tracker() {
        let mut t = EventTracker::default();
        assert_eq!(t.events(Some("a"), true), vec!["track_change", "play"]);
        assert!(t.events(Some("a"), true).is_empty());
        assert_eq!(t.events(Some("a"), false), vec!["pause"]);
        assert_eq!(t.events(Some("b"), false), vec!["track_change"]);
        assert!(t.events(None, false).is_empty());
    }

    #[test]
    fn shipped_example_plugins_load() {
        let dir = tempfile::tempdir().unwrap();
        for (name, src) in crate::plugins::EXAMPLES {
            let mut host = PluginHost::empty();
            host.load_source(name, src, &dir.path().join(name))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            host.set_state(
                Some(TrackInfo {
                    title: "T".into(),
                    artist: "A".into(),
                    album: "Al".into(),
                    uri: "spotify:track:x".into(),
                    duration_ms: 1000,
                }),
                true,
                0,
            );
            host.emit("track_change");
            host.emit("play");
            host.emit("pause");
            for (p, c, _) in host.commands() {
                host.run_command(p, c);
            }
        }
        let np = std::fs::read_to_string(dir.path().join("now_playing/now_playing.txt")).unwrap();
        assert!(np.contains("A - T"), "{np}");
    }
}
