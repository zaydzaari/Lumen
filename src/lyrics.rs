//! Lyrics: Spotify's own synced lyrics first (the same service the official app
//! uses, provided by Musixmatch), then the public LRCLIB database
//! (<https://lrclib.net>) for songs Spotify has none for. Results are cached on
//! disk as small text files (`.lrc` synced, `.txt` plain, `.none` "not found",
//! `.src` the provider's name).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::cache::DiskCache;
use crate::model::Track;

const LRCLIB: &str = "https://lrclib.net/api";
/// How long a "no lyrics found" answer is trusted before asking again.
const NOT_FOUND_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LrcLine {
    pub time_ms: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lyrics {
    Synced(Vec<LrcLine>),
    Plain(String),
    NotFound,
}

/// Parse LRC text. Supports `[mm:ss]`, `[mm:ss.xx]`, `[mm:ss.xxx]`, `[mm:ss:xx]`,
/// several timestamps on one line, an `[offset:+/-ms]` tag, and ignores other
/// metadata tags (`[ar:...]`, `[ti:...]`) and malformed lines. Output is sorted.
pub fn parse_lrc(text: &str) -> Vec<LrcLine> {
    let mut offset_ms: i64 = 0;
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(end) = stripped.find(']') else { break };
            let tag = &stripped[..end];
            rest = &stripped[end + 1..];
            if let Some(ms) = parse_timestamp(tag) {
                stamps.push(ms);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset_ms = v.trim().parse().unwrap_or(0);
            }
        }
        if stamps.is_empty() {
            continue;
        }
        let line = rest.trim().to_string();
        for ms in stamps {
            // A positive LRC offset means lyrics should appear earlier.
            let t = (ms as i64)
                .saturating_sub(offset_ms)
                .clamp(0, u32::MAX as i64) as u32;
            out.push(LrcLine {
                time_ms: t,
                text: line.clone(),
            });
        }
    }
    out.sort_by_key(|l| l.time_ms);
    out
}

fn parse_timestamp(tag: &str) -> Option<u32> {
    let (min, rest) = tag.split_once(':')?;
    let min: u32 = min.trim().parse().ok()?;
    let (sec, frac) = match rest.find(['.', ':']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let sec: u32 = sec.trim().parse().ok()?;
    if sec >= 60 {
        return None;
    }
    let frac_ms = match frac {
        None => 0,
        Some(f) if f.is_empty() || !f.chars().all(|c| c.is_ascii_digit()) => return None,
        Some(f) => {
            let digits: String = f.chars().take(3).collect();
            let v: u32 = digits.parse().ok()?;
            match digits.len() {
                1 => v * 100,
                2 => v * 10,
                _ => v,
            }
        }
    };
    min.checked_mul(60_000)?
        .checked_add(sec * 1000)?
        .checked_add(frac_ms)
}

/// Index of the line that should be highlighted at `pos_ms` (last line whose time
/// is <= position), or `None` before the first line.
pub fn current_line(lines: &[LrcLine], pos_ms: u32) -> Option<usize> {
    let idx = lines.partition_point(|l| l.time_ms <= pos_ms);
    idx.checked_sub(1)
}

/// Milliseconds until the next line starts (for scheduling a repaint).
pub fn ms_until_next(lines: &[LrcLine], pos_ms: u32) -> Option<u32> {
    let idx = lines.partition_point(|l| l.time_ms <= pos_ms);
    lines.get(idx).map(|l| l.time_ms.saturating_sub(pos_ms))
}

#[derive(Deserialize)]
struct LrclibRecord {
    #[serde(rename = "syncedLyrics")]
    synced: Option<String>,
    #[serde(rename = "plainLyrics")]
    plain: Option<String>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    duration: Option<f64>,
}

fn cache_key(track: &Track) -> String {
    format!(
        "{}|{}|{}|{}",
        track.artists.to_lowercase(),
        track.name.to_lowercase(),
        track.album.to_lowercase(),
        track.duration_ms / 1000
    )
}

#[derive(Deserialize)]
struct SpotifyLyricsResp {
    lyrics: SpotifyLyrics,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpotifyLyrics {
    #[serde(default)]
    sync_type: String,
    #[serde(default)]
    lines: Vec<SpotifyLine>,
    #[serde(default)]
    provider_display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpotifyLine {
    #[serde(default)]
    start_time_ms: serde_json::Value,
    #[serde(default)]
    words: String,
}

/// Parse Spotify's `color-lyrics` JSON. Returns the lyrics and the provider
/// name (usually "Musixmatch"); `None` when there is nothing usable.
pub fn parse_spotify(bytes: &[u8]) -> Option<(Lyrics, String)> {
    let r: SpotifyLyricsResp = serde_json::from_slice(bytes).ok()?;
    let provider = r
        .lyrics
        .provider_display_name
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "Spotify".into());
    let words: Vec<String> = r
        .lyrics
        .lines
        .iter()
        .map(|l| {
            let w = l.words.trim();
            if w == "♪" {
                String::new()
            } else {
                w.to_string()
            }
        })
        .collect();
    if words.iter().all(|w| w.is_empty()) {
        return None;
    }
    if r.lyrics.sync_type == "LINE_SYNCED" {
        let mut lines: Vec<LrcLine> = r
            .lyrics
            .lines
            .iter()
            .zip(words.iter().cloned())
            .filter_map(|(l, text)| {
                let ms = match &l.start_time_ms {
                    serde_json::Value::String(s) => s.parse::<u64>().ok()?,
                    serde_json::Value::Number(n) => n.as_u64()?,
                    _ => return None,
                };
                Some(LrcLine {
                    time_ms: ms.min(u32::MAX as u64) as u32,
                    text,
                })
            })
            .collect();
        lines.sort_by_key(|l| l.time_ms);
        if !lines.is_empty() {
            return Some((Lyrics::Synced(lines), provider));
        }
    }
    Some((Lyrics::Plain(words.join("\n")), provider))
}

/// Provider name stored next to cached lyrics (`None` for old cache entries).
pub fn cached_source(cache: &DiskCache, track: &Track) -> Option<String> {
    cache
        .get(&cache_key(track), "src")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .filter(|s| !s.is_empty())
}

/// Read cached lyrics without touching the network.
pub fn from_cache(cache: &DiskCache, track: &Track) -> Option<Lyrics> {
    let key = cache_key(track);
    if let Some(bytes) = cache.get(&key, "lrc") {
        let lines = parse_lrc(&String::from_utf8_lossy(&bytes));
        if !lines.is_empty() {
            return Some(Lyrics::Synced(lines));
        }
    }
    if let Some(bytes) = cache.get(&key, "txt") {
        return Some(Lyrics::Plain(String::from_utf8_lossy(&bytes).into_owned()));
    }
    if cache.contains(&key, "none") {
        if cache
            .age(&key, "none")
            .is_some_and(|age| age < NOT_FOUND_TTL)
        {
            return Some(Lyrics::NotFound);
        }
        cache.remove(&key, "none");
    }
    None
}

fn record_to_lyrics(rec: LrclibRecord) -> Lyrics {
    if let Some(s) = rec.synced.filter(|s| !s.trim().is_empty()) {
        let lines = parse_lrc(&s);
        if !lines.is_empty() {
            return Lyrics::Synced(lines);
        }
    }
    if let Some(p) = rec.plain.filter(|s| !s.trim().is_empty()) {
        return Lyrics::Plain(p);
    }
    if rec.instrumental {
        return Lyrics::Plain("♪ Instrumental ♪".into());
    }
    Lyrics::NotFound
}

/// Fetch lyrics: local `.lrc` file, disk cache, Spotify (when `spotify` is
/// given), then LRCLIB `/api/get` and `/api/search`. Stores the result on disk.
/// Returns the lyrics and who provided them.
pub async fn fetch(
    http: &reqwest::Client,
    cache: Arc<DiskCache>,
    track: &Track,
    spotify: Option<Arc<crate::api::Api>>,
) -> Result<(Lyrics, String)> {
    if crate::local::path(&track.uri).is_some() {
        let track = track.clone();
        if let Some(lyrics) =
            tokio::task::spawn_blocking(move || crate::local::sibling_lyrics(&track)).await??
        {
            return Ok((lyrics, "your .lrc file".into()));
        }
    }
    if let Some(l) = from_cache(&cache, track) {
        let src = cached_source(&cache, track).unwrap_or_else(|| "LRCLIB".into());
        return Ok((l, src));
    }
    if let Some(api) = spotify.filter(|_| track.uri.starts_with("spotify:track:")) {
        match api.spotify_lyrics(&track.uri).await {
            Ok(Some((lyrics, provider))) => {
                store(&cache, track, &lyrics, &provider);
                return Ok((lyrics, provider));
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("Spotify lyrics unavailable, trying LRCLIB: {e:#}"),
        }
    }
    let lyrics = lrclib(http, track).await?;
    store(&cache, track, &lyrics, "LRCLIB");
    Ok((lyrics, "LRCLIB".into()))
}

async fn lrclib(http: &reqwest::Client, track: &Track) -> Result<Lyrics> {
    let first_artist = track.artists.split(", ").next().unwrap_or(&track.artists);
    let duration_s = (track.duration_ms / 1000).to_string();
    let resp = http
        .get(format!("{LRCLIB}/get"))
        .query(&[
            ("artist_name", first_artist),
            ("track_name", track.name.as_str()),
            ("album_name", track.album.as_str()),
            ("duration", duration_s.as_str()),
        ])
        .timeout(Duration::from_secs(15))
        .send()
        .await?;

    let lyrics = if resp.status().is_success() {
        record_to_lyrics(resp.json::<LrclibRecord>().await?)
    } else if resp.status() == reqwest::StatusCode::NOT_FOUND {
        // Fuzzy fallback: search and take the closest duration match.
        let results: Vec<LrclibRecord> = http
            .get(format!("{LRCLIB}/search"))
            .query(&[
                ("artist_name", first_artist),
                ("track_name", track.name.as_str()),
            ])
            .timeout(Duration::from_secs(15))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let want = track.duration_ms as f64 / 1000.0;
        results
            .into_iter()
            .filter(|r| r.duration.is_none_or(|d| (d - want).abs() <= 4.0))
            .map(record_to_lyrics)
            .find(|l| *l != Lyrics::NotFound)
            .unwrap_or(Lyrics::NotFound)
    } else {
        anyhow::bail!("LRCLIB returned HTTP {}", resp.status());
    };
    Ok(lyrics)
}

fn store(cache: &DiskCache, track: &Track, lyrics: &Lyrics, provider: &str) {
    let key = cache_key(track);
    let res = match lyrics {
        Lyrics::Synced(lines) => {
            let text: String = lines
                .iter()
                .map(|l| {
                    format!(
                        "[{:02}:{:02}.{:02}]{}\n",
                        l.time_ms / 60_000,
                        (l.time_ms / 1000) % 60,
                        (l.time_ms % 1000) / 10,
                        l.text
                    )
                })
                .collect();
            cache.put(&key, "lrc", text.as_bytes())
        }
        Lyrics::Plain(p) => cache.put(&key, "txt", p.as_bytes()),
        Lyrics::NotFound => cache.put(&key, "none", b""),
    };
    if let Err(e) = res {
        tracing::warn!("could not cache lyrics: {e:#}");
    }
    let _ = cache.put(&key, "src", provider.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spotify_color_lyrics() {
        let json = br#"{"lyrics":{"syncType":"LINE_SYNCED","lines":[
            {"startTimeMs":"12960","words":"Second","syllables":[],"endTimeMs":"0"},
            {"startTimeMs":"960","words":"First","syllables":[],"endTimeMs":"0"},
            {"startTimeMs":"20000","words":"\u266a","syllables":[],"endTimeMs":"0"}],
            "provider":"MusixMatch","providerDisplayName":"Musixmatch"},"colors":{"background":-1}}"#;
        let (l, p) = parse_spotify(json).unwrap();
        assert_eq!(p, "Musixmatch");
        let Lyrics::Synced(lines) = l else { panic!() };
        assert_eq!(
            lines[0],
            LrcLine {
                time_ms: 960,
                text: "First".into()
            }
        );
        assert_eq!(lines[2].text, "");
        let plain = br#"{"lyrics":{"syncType":"UNSYNCED","lines":[{"startTimeMs":"0","words":"a"},{"startTimeMs":"0","words":"b"}]}}"#;
        assert_eq!(
            parse_spotify(plain).unwrap().0,
            Lyrics::Plain("a\nb".into())
        );
        assert!(parse_spotify(b"{}").is_none());
    }

    #[test]
    fn extreme_timestamps_and_offsets_do_not_overflow() {
        assert!(parse_lrc("[999999999:00]too large").is_empty());
        let lines = parse_lrc("[offset:-9223372036854775808]\n[00:00.01]safe");
        assert_eq!(lines[0].time_ms, u32::MAX);
    }

    #[test]
    fn parses_standard_lrc() {
        let lrc = "[ar:Someone]\n[ti:Song]\n[00:12.34]First line\n[00:15.5]Second\n[01:02.123] Third \nnot a lyric\n";
        let lines = parse_lrc(lrc);
        assert_eq!(
            lines,
            vec![
                LrcLine {
                    time_ms: 12_340,
                    text: "First line".into()
                },
                LrcLine {
                    time_ms: 15_500,
                    text: "Second".into()
                },
                LrcLine {
                    time_ms: 62_123,
                    text: "Third".into()
                },
            ]
        );
    }

    #[test]
    fn multiple_timestamps_and_sorting() {
        let lines = parse_lrc("[00:30.00][00:10.00]Chorus\n[00:20.00]Verse");
        let times: Vec<u32> = lines.iter().map(|l| l.time_ms).collect();
        assert_eq!(times, vec![10_000, 20_000, 30_000]);
        assert_eq!(lines[0].text, "Chorus");
        assert_eq!(lines[2].text, "Chorus");
    }

    #[test]
    fn offset_and_odd_formats() {
        let lines = parse_lrc("[offset:+500]\n[00:10]A\n[00:20:50]B\n[00:00.10]C");
        assert_eq!(lines[0].time_ms, 0); // 100 - 500 clamps to 0
        assert_eq!(lines[1].time_ms, 9_500);
        assert_eq!(lines[2].time_ms, 20_000); // 20.50 s - 500 ms offset
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_lrc("").is_empty());
        assert!(parse_lrc("[xx:yy]nope\n[00:75.00]bad seconds\n[00:10.ab]bad").is_empty());
        assert!(parse_lrc("just text").is_empty());
        // Unclosed bracket must not panic.
        assert!(parse_lrc("[00:10.00").is_empty());
    }

    #[test]
    fn empty_lines_are_kept_as_pauses() {
        let lines = parse_lrc("[00:01.00]Hi\n[00:05.00]\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text, "");
    }

    #[test]
    fn current_line_lookup() {
        let lines = parse_lrc("[00:10.00]a\n[00:20.00]b\n[00:30.00]c");
        assert_eq!(current_line(&lines, 0), None);
        assert_eq!(current_line(&lines, 10_000), Some(0));
        assert_eq!(current_line(&lines, 25_000), Some(1));
        assert_eq!(current_line(&lines, 99_000), Some(2));
        assert_eq!(ms_until_next(&lines, 12_000), Some(8_000));
        assert_eq!(ms_until_next(&lines, 31_000), None);
    }

    #[test]
    fn record_mapping() {
        let r = LrclibRecord {
            synced: Some("[00:01.00]x".into()),
            plain: Some("x".into()),
            instrumental: false,
            duration: None,
        };
        assert!(matches!(record_to_lyrics(r), Lyrics::Synced(_)));
        let r = LrclibRecord {
            synced: None,
            plain: Some("plain".into()),
            instrumental: false,
            duration: None,
        };
        assert_eq!(record_to_lyrics(r), Lyrics::Plain("plain".into()));
        let r = LrclibRecord {
            synced: Some("  ".into()),
            plain: None,
            instrumental: false,
            duration: None,
        };
        assert_eq!(record_to_lyrics(r), Lyrics::NotFound);
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path(), 1 << 20).unwrap();
        let t = Track {
            uri: "spotify:track:1".into(),
            name: "Song".into(),
            artists: "Artist".into(),
            album: "Album".into(),
            duration_ms: 180_000,
            ..Default::default()
        };
        assert!(from_cache(&cache, &t).is_none());
        cache
            .put(&cache_key(&t), "lrc", b"[00:01.00]hello\n")
            .unwrap();
        assert!(matches!(from_cache(&cache, &t), Some(Lyrics::Synced(l)) if l[0].text == "hello"));
    }
}
