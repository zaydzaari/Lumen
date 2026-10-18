-- now_playing.lua — example plugin for streamers.
--
-- Writes the current track to `now_playing.txt` in this plugin's data folder
-- (shown in Settings → Plugins, and in `lumen.data_dir`). Point an OBS
-- "Text (GDI+/FreeType)" source at that file with "Read from file" enabled.
-- Also keeps a simple `history.txt` log of played tracks.

local FORMAT = "%s - %s"   -- artist, title
local PAUSED_TEXT = ""      -- what to show while paused ("" hides the overlay)

local function line_for(t)
  if not t then return "" end
  return string.format(FORMAT, t.artist, t.title)
end

local function write_current()
  local t = lumen.current_track()
  if lumen.is_playing() then
    lumen.write_file("now_playing.txt", line_for(t))
  else
    lumen.write_file("now_playing.txt", PAUSED_TEXT)
  end
end

lumen.on("track_change", function(t)
  lumen.write_file("now_playing.txt", line_for(t))
  local entry = os.time() .. "\t" .. line_for(t) .. "\n"
  -- Files are capped at 1 MB; start a fresh history when full.
  if not pcall(lumen.append_file, "history.txt", entry) then
    lumen.write_file("history.txt", entry)
  end
end)

lumen.on("play", write_current)
lumen.on("pause", write_current)

lumen.add_command("Show now-playing file location", function()
  lumen.notify("Writing to " .. lumen.data_dir .. "/now_playing.txt")
end)
