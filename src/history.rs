//! Listening history and stats.
//!
//! Every song played for at least 30 seconds (Spotify's own rule for counting
//! a stream) is appended to `history.jsonl` in the data folder, one JSON object
//! per line. The stats page reads that file on a background thread and
//! aggregates it: minutes listened, top songs/artists/albums, listening by hour
//! and weekday, the last 30 days, and the current streak.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

/// Minimum listening time for a play to count.
pub const MIN_PLAY_MS: u32 = 30_000;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Play {
    /// Unix seconds when the play ended.
    pub ts: i64,
    /// Milliseconds actually listened.
    pub ms: u32,
    pub uri: String,
    pub name: String,
    pub artists: String,
    #[serde(default)]
    pub artist_uri: Option<String>,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_uri: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// Song length (for playing it again from the stats page).
    #[serde(default)]
    pub duration_ms: u32,
}

impl Play {
    pub fn to_track(&self) -> crate::model::TrackRef {
        std::sync::Arc::new(crate::model::Track {
            uri: self.uri.clone(),
            name: self.name.clone(),
            artists: self.artists.clone(),
            artist_uri: self.artist_uri.clone(),
            album: self.album.clone(),
            album_uri: self.album_uri.clone(),
            duration_ms: self.duration_ms,
            image: self.image.clone(),
            image_large: self.image.clone(),
            explicit: false,
        })
    }

    pub fn primary_artist(&self) -> &str {
        self.artists.split(", ").next().unwrap_or(&self.artists)
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("history.jsonl")
}

pub fn append(file: &Path, p: &Play) -> Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    let mut line = serde_json::to_vec(p)?;
    line.push(b'\n');
    f.write_all(&line)?;
    Ok(())
}

/// Read all plays (malformed lines are skipped).
pub fn load(file: &Path) -> Vec<Play> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Range {
    #[default]
    Month,
    HalfYear,
    All,
}

impl Range {
    pub fn label(self) -> &'static str {
        match self {
            Range::Month => "Last 4 weeks",
            Range::HalfYear => "Last 6 months",
            Range::All => "All time",
        }
    }
    fn since(self, now: i64) -> i64 {
        match self {
            Range::Month => now - 28 * 86_400,
            Range::HalfYear => now - 182 * 86_400,
            Range::All => i64::MIN,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TopTrack {
    pub play: Play,
    pub plays: u32,
    pub ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct TopGroup {
    pub name: String,
    pub uri: Option<String>,
    pub image: Option<String>,
    /// Artist names (albums only).
    pub detail: String,
    pub plays: u32,
    pub ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub range: Range,
    pub plays: u32,
    pub ms: u64,
    pub tracks: usize,
    pub artists: usize,
    pub top_tracks: Vec<TopTrack>,
    pub top_artists: Vec<TopGroup>,
    pub top_albums: Vec<TopGroup>,
    /// Milliseconds per local hour of day.
    pub by_hour: [u64; 24],
    /// Milliseconds per weekday, Monday first.
    pub by_weekday: [u64; 7],
    /// Milliseconds per day for the last 30 days, oldest first.
    pub last_30_days: Vec<u64>,
    /// Consecutive days with listening, ending today (or yesterday).
    pub streak: u32,
    pub first_ts: Option<i64>,
}

fn local(ts: i64) -> chrono::DateTime<Local> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).unwrap())
}

pub fn compute(all: &[Play], range: Range, now: i64) -> Stats {
    let since = range.since(now);
    let plays: Vec<&Play> = all
        .iter()
        .filter(|p| p.ts >= since && p.ts <= now + 60)
        .collect();
    let mut s = Stats {
        range,
        plays: plays.len() as u32,
        ms: plays.iter().map(|p| p.ms as u64).sum(),
        first_ts: all.iter().map(|p| p.ts).min(),
        ..Default::default()
    };

    let mut tracks: HashMap<&str, TopTrack> = HashMap::new();
    let mut artists: HashMap<String, TopGroup> = HashMap::new();
    let mut albums: HashMap<String, TopGroup> = HashMap::new();
    for p in &plays {
        let t = tracks.entry(p.uri.as_str()).or_insert_with(|| TopTrack {
            play: (*p).clone(),
            ..Default::default()
        });
        t.plays += 1;
        t.ms += p.ms as u64;
        if p.ts > t.play.ts {
            t.play = (*p).clone(); // newest metadata wins
        }
        let a_key = p
            .artist_uri
            .clone()
            .unwrap_or_else(|| p.primary_artist().to_lowercase());
        let a = artists.entry(a_key).or_insert_with(|| TopGroup {
            name: p.primary_artist().to_string(),
            uri: p.artist_uri.clone(),
            image: p.image.clone(),
            ..Default::default()
        });
        a.plays += 1;
        a.ms += p.ms as u64;
        if !p.album.is_empty() {
            let al_key = p
                .album_uri
                .clone()
                .unwrap_or_else(|| p.album.to_lowercase());
            let al = albums.entry(al_key).or_insert_with(|| TopGroup {
                name: p.album.clone(),
                uri: p.album_uri.clone(),
                image: p.image.clone(),
                detail: p.primary_artist().to_string(),
                ..Default::default()
            });
            al.plays += 1;
            al.ms += p.ms as u64;
        }
        let d = local(p.ts);
        s.by_hour[d.hour() as usize] += p.ms as u64;
        s.by_weekday[d.weekday().num_days_from_monday() as usize] += p.ms as u64;
    }
    s.tracks = tracks.len();
    s.artists = artists.len();
    let mut tt: Vec<TopTrack> = tracks.into_values().collect();
    tt.sort_by(|a, b| b.plays.cmp(&a.plays).then(b.ms.cmp(&a.ms)));
    tt.truncate(50);
    s.top_tracks = tt;
    let rank = |m: HashMap<String, TopGroup>| {
        let mut v: Vec<TopGroup> = m.into_values().collect();
        v.sort_by(|a, b| b.ms.cmp(&a.ms).then(b.plays.cmp(&a.plays)));
        v.truncate(20);
        v
    };
    s.top_artists = rank(artists);
    s.top_albums = rank(albums);

    // Last 30 days (all plays, independent of the range) and the streak.
    let today = local(now).date_naive();
    let mut per_day: HashMap<chrono::NaiveDate, u64> = HashMap::new();
    for p in all {
        *per_day.entry(local(p.ts).date_naive()).or_default() += p.ms as u64;
    }
    s.last_30_days = (0..30)
        .rev()
        .map(|i| {
            let d = today - chrono::Duration::days(i);
            per_day.get(&d).copied().unwrap_or(0)
        })
        .collect();
    let mut day = today;
    if !per_day.contains_key(&day) {
        day -= chrono::Duration::days(1);
    }
    while per_day.contains_key(&day) {
        s.streak += 1;
        day -= chrono::Duration::days(1);
    }
    s
}

/// What Home builds from your own listening: works with or without the Web API.
#[derive(Clone, Debug, Default)]
pub struct HomeMix {
    /// Distinct songs, newest first.
    pub recent: Vec<crate::model::TrackRef>,
    /// Distinct albums you played lately, newest first ("Jump back in").
    pub albums: Vec<crate::model::Collection>,
    /// Most played artists in the last four weeks.
    pub artists: Vec<crate::model::Collection>,
    /// Most played songs in the last four weeks ("On repeat").
    pub repeat: Vec<crate::model::TrackRef>,
}

pub fn home_mix(all: &[Play], now: i64) -> HomeMix {
    use crate::model::{Collection, CollectionKind};
    let mut mix = HomeMix::default();
    let mut seen = std::collections::HashSet::new();
    let mut seen_album = std::collections::HashSet::new();
    for p in all.iter().rev() {
        if mix.recent.len() < 20 && seen.insert(p.uri.as_str()) {
            mix.recent.push(p.to_track());
        }
        let album_uri = p.album_uri.as_deref().unwrap_or("");
        if mix.albums.len() < 12 && !album_uri.is_empty() && seen_album.insert(album_uri) {
            mix.albums.push(Collection {
                kind: CollectionKind::Album,
                uri: album_uri.to_string(),
                name: p.album.clone(),
                subtitle: p.artists.clone(),
                image: p.image.clone(),
                total_tracks: 0,
                owned: false,
            });
        }
        if mix.recent.len() >= 20 && mix.albums.len() >= 12 {
            break;
        }
    }
    let stats = compute(all, Range::Month, now);
    mix.repeat = stats
        .top_tracks
        .iter()
        .take(20)
        .map(|t| t.play.to_track())
        .collect();
    mix.artists = stats
        .top_artists
        .iter()
        .filter_map(|a| {
            Some(Collection {
                kind: CollectionKind::Artist,
                uri: a.uri.clone()?,
                name: a.name.clone(),
                subtitle: "Artist".into(),
                image: a.image.clone(),
                total_tracks: 0,
                owned: false,
            })
        })
        .take(12)
        .collect();
    mix
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(ts: i64, uri: &str, artist: &str, ms: u32) -> Play {
        Play {
            ts,
            ms,
            uri: uri.into(),
            name: uri.into(),
            artists: artist.into(),
            artist_uri: Some(format!(
                "spotify:artist:{}",
                artist.split(", ").next().unwrap()
            )),
            album: format!("{artist} album"),
            album_uri: None,
            image: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn top_lists_and_ranges() {
        let now = 1_800_000_000;
        let v = vec![
            play(now - 100, "a", "X", 200_000),
            play(now - 200, "a", "X", 200_000),
            play(now - 300, "b", "Y", 180_000),
            play(now - 400, "c", "X, Z", 120_000),
            play(now - 100 * 86_400, "old", "Old", 100_000),
        ];
        let m = compute(&v, Range::Month, now);
        assert_eq!(m.plays, 4);
        assert_eq!(m.top_tracks[0].play.uri, "a");
        assert_eq!(m.top_tracks[0].plays, 2);
        assert_eq!(m.top_artists[0].name, "X");
        assert_eq!(m.top_artists[0].plays, 3);
        assert_eq!(m.artists, 2);
        assert_eq!(m.ms, 700_000);
        let all = compute(&v, Range::All, now);
        assert_eq!(all.plays, 5);
        assert_eq!(m.last_30_days.len(), 30);
        assert!(m.streak >= 1);
        assert_eq!(m.by_hour.iter().sum::<u64>(), 700_000);
        assert_eq!(m.by_weekday.iter().sum::<u64>(), 700_000);
    }

    #[test]
    fn file_roundtrip_skips_bad_lines() {
        let dir = std::env::temp_dir().join(format!("lumen-hist-{}", std::process::id()));
        let f = path(&dir);
        let p = play(1, "u", "A", 40_000);
        append(&f, &p).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&f)
            .unwrap()
            .write_all(b"garbage\n")
            .unwrap();
        append(&f, &p).unwrap();
        assert_eq!(load(&f), vec![p.clone(), p]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn home_mix_is_newest_first_and_distinct() {
        let now = 1_700_000_000;
        let mut a = play(now - 300, "t:a", "X", 60_000);
        a.album_uri = Some("spotify:album:1".into());
        let mut b = play(now - 200, "t:b", "Y", 60_000);
        b.album_uri = Some("spotify:album:2".into());
        let mut c = play(now - 100, "t:a", "X", 60_000);
        c.album_uri = Some("spotify:album:1".into());
        let mix = home_mix(&[a, b, c], now);
        let recent: Vec<_> = mix.recent.iter().map(|t| t.uri.as_str()).collect();
        assert_eq!(recent, ["t:a", "t:b"]);
        let albums: Vec<_> = mix.albums.iter().map(|c| c.uri.as_str()).collect();
        assert_eq!(albums, ["spotify:album:1", "spotify:album:2"]);
        assert_eq!(mix.repeat[0].uri, "t:a");
        assert_eq!(mix.artists[0].uri, "spotify:artist:X");
    }
}
