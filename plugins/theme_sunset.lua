-- theme_sunset.lua — example theme plugin for Lumen.
--
-- Recolours the UI with a warm "sunset" palette. Installed disabled by default;
-- enable it in Settings → Plugins. Adds a command to toggle it on and off.

local sunset = {
  mode = "dark",
  accent = "#ff7a45",
  background = "#1a1220",
  panel = "#241829",
  text = "#f3e9e1",
}

local default = {
  mode = "dark",
  accent = "#1ed760",
  background = "#121212",
  panel = "#181818",
  text = "#e8e8e8",
}

local active = true
lumen.set_theme(sunset)

lumen.add_command("Toggle sunset theme", function()
  active = not active
  lumen.set_theme(active and sunset or default)
  lumen.notify(active and "Sunset theme on" or "Sunset theme off")
end)
