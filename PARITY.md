# Lumen vs the Spotify desktop app

A feature-by-feature check against Spotify's desktop app (October 2026).
✅ = in Lumen · ➖ = partly · ❌ = missing · 🚫 = can't be done from a third-party client.

## Shell and navigation
| Spotify | Lumen |
|---|---|
| Home, Search, Your Library, back/forward | ✅ |
| Profile menu (Account, Profile, Settings, Log out, Private session) | ✅ |
| Friend Activity panel | ✅ (needs Spotify to share it with Lumen's login; otherwise it says so) |
| What's New (new releases from followed artists, bell) | ❌ |
| Keyboard shortcuts overview (Ctrl+/) | ✅ |
| Mini player, full screen, Now Playing view | ✅ |

## Your Library
| Spotify | Lumen |
|---|---|
| Playlists / Albums / Artists filters, search, sort (Recents, Alphabetical, Creator, Custom) | ✅ |
| Pin, drag to reorder, right-click menu, Create playlist | ✅ |
| One click opens, double-click plays | ✅ |
| View as Compact / List | ✅ |
| View as Grid | ❌ |
| Playlist folders | ➖ (folders' playlists are listed, folders themselves aren't shown) |
| Podcasts & Shows, Audiobooks | ❌ |
| Downloaded (offline) | ✅ permanent provider/local files, independent of Spotify streaming |

## Playlist and album pages
| Spotify | Lumen |
|---|---|
| Header, play, shuffle, ⋯ menu, add/remove from library, radio, share | ✅ |
| Find in playlist, sort (title, artist, album, duration), column-header sort | ✅ |
| Multi-select, drag songs, Delete key | ✅ |
| Edit details, delete, add/remove songs, reorder | ✅ |
| "Recommended" songs under your own playlists | ✅ |
| Album footer: release date, copyright, "More by …" | ✅ |
| "Date added" column | ❌ |
| Change playlist cover image, public/private, collaborative | ❌ |

## Artist page
| Spotify | Lumen |
|---|---|
| Banner, Play, Shuffle, Follow, ⋯ | ✅ |
| Popular (5, See more) | ✅ |
| Liked Songs by this artist | ✅ |
| Discography with Popular releases / Albums / Singles and EPs / Compilations, Show all | ✅ |
| Fans also like, Appears On, About | ✅ |
| Monthly listeners, top cities, verified badge, play counts | 🚫 not available through the services Lumen can use (Lumen shows popularity instead) |
| Concerts, merch, artist pick, Canvas videos | ❌ |

## Songs
| Spotify | Lumen |
|---|---|
| Song menu: add to playlist, remove, like, queue, radio, artist, album, credits, share, open in app | ✅ |
| Save (+) and the "Saved in" popover | ✅ |
| Lyrics (synced), full-screen lyrics | ✅ (plus a fallback source) |
| Credits | ✅ |

## Playback
| Spotify | Lumen |
|---|---|
| Play/pause, next/prev, seek, shuffle, repeat, queue, autoplay | ✅ |
| Spotify Connect: device picker, transfer, controlled from phone | ✅ |
| Normalize volume and Volume level (Loud/Normal/Quiet) | ✅ |
| Audio quality, gapless | ✅ |
| Crossfade, Automix, Smart Shuffle, equalizer, mono audio | ❌ (the playback engine has no crossfade/equalizer) |
| Resume where you left off | ✅ |

## Search
| Spotify | Lumen |
|---|---|
| Top result, Songs, Artists, Albums, Playlists, recent searches | ✅ |
| Podcasts, Profiles, Genres & moods browse | ❌ |

## Lumen-only
Listening stats, Spicetify theme marketplace, local files, Discord status, plugins,
mini player that stays tiny in memory.


## Independent mode without Premium

| Feature | Availability |
|---|---|
| Open Lumen without Spotify login | Yes: Continue without Premium |
| Personal playlists and saved tracks | Import account-data JSON; retained locally |
| Public track/album/playlist links | Provider metadata connection; live acceptance still needs an attended-session check |
| Search | Imported playlists and local music; not full online catalogue search |
| Playback | Download requested missing tracks, then play local files; provider coverage varies |
| Queue, shuffle, repeat, seek, media keys | Local playback supports these |
| Lyrics | Existing local/cache/LRCLIB path, where available |
| Live private-account sync / library edits / Connect | Not supported in independent mode |
