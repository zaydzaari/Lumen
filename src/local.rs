//! User-owned music only. The index stores tags and file stamps, never audio or covers.
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::{Context as _, Result};
use lofty::config::ParseOptions;
use lofty::file::{AudioFile as _, TaggedFileExt as _};
use lofty::prelude::Accessor as _;
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::model::{Track, TrackRef};

pub const MAX_FILES: usize = 20_000;
const MAX_INDEX_BYTES: u64 = 64 << 20;
const MAX_COVER_BYTES: usize = 4 << 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    path: PathBuf,
    modified_ns: u128,
    size: u64,
    track: Track,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Index {
    version: u8,
    entries: Vec<Entry>,
}

pub struct Scan {
    pub index: Index,
    pub parsed: usize,
    pub reused: usize,
    pub warnings: Vec<String>,
}

pub fn uri(path: &Path) -> Option<String> {
    path.to_str().map(|p| format!("local:{p}"))
}

pub fn path(uri: &str) -> Option<&Path> {
    uri.strip_prefix("local:").map(Path::new)
}

fn supported(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "mp3" | "flac" | "ogg" | "opus" | "m4a" | "wav"
        )
    })
}

impl Index {
    pub fn load(file: &Path) -> Result<Self> {
        let f = File::open(file)?;
        anyhow::ensure!(
            f.metadata()?.len() <= MAX_INDEX_BYTES,
            "local index exceeds size limit"
        );
        let index: Self = serde_json::from_reader(BufReader::new(f))?;
        anyhow::ensure!(
            index.version == 1 && index.entries.len() <= MAX_FILES,
            "invalid local index"
        );
        Ok(index)
    }

    pub fn save(&self, file: &Path) -> Result<()> {
        let tmp = file.with_extension("json.tmp");
        {
            let f = File::create(&tmp)?;
            let mut writer = BufWriter::new(f);
            serde_json::to_writer(&mut writer, self)?;
            use std::io::Write as _;
            writer.flush()?;
            writer.get_ref().sync_all()?;
            anyhow::ensure!(
                writer.get_ref().metadata()?.len() <= MAX_INDEX_BYTES,
                "local index exceeds size limit"
            );
        }
        fs::rename(tmp, file)?;
        Ok(())
    }

    pub fn tracks(&self) -> Vec<TrackRef> {
        self.entries
            .iter()
            .map(|e| Arc::new(e.track.clone()))
            .collect()
    }

    pub fn retain_folders(&mut self, folders: &[PathBuf]) {
        let roots: Vec<_> = folders
            .iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        self.entries
            .retain(|e| roots.iter().any(|r| e.path.starts_with(r)));
    }
}

fn read_track(path: &Path) -> Result<Track> {
    lofty::config::apply_global_options(
        lofty::config::GlobalOptions::new()
            .allocation_limit(MAX_COVER_BYTES)
            .preserve_format_specific_items(false),
    );
    let tagged = Probe::open(path)?
        .options(ParseOptions::new().read_cover_art(false))
        .read()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let text = |get: fn(&lofty::tag::Tag) -> Option<std::borrow::Cow<'_, str>>| {
        tag.and_then(get)
            .map(|s| s.chars().take(1024).collect::<String>())
            .unwrap_or_default()
    };
    let name = text(|t| t.title());
    let image = uri(path).context("local path is not Unicode")?;
    Ok(Track {
        uri: image.clone(),
        name: if name.is_empty() {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            name
        },
        artists: text(|t| t.artist()),
        album: text(|t| t.album()),
        duration_ms: tagged
            .properties()
            .duration()
            .as_millis()
            .min(u32::MAX as u128) as u32,
        image: Some(image.clone()),
        image_large: Some(image),
        ..Track::default()
    })
}

/// Called on a blocking worker. Symlinks are not followed; overlapping roots are deduplicated.
pub fn scan(folders: &[PathBuf], previous: Index) -> Scan {
    let old: HashMap<_, _> = previous
        .entries
        .into_iter()
        .map(|e| (e.path.clone(), e))
        .collect();
    let mut result = Scan {
        index: Index {
            version: 1,
            entries: Vec::new(),
        },
        parsed: 0,
        reused: 0,
        warnings: Vec::new(),
    };
    let mut seen = HashSet::new();
    let warn = |result: &mut Scan, message: String| {
        if result.warnings.len() < 5 {
            result.warnings.push(message);
        }
    };
    'folders: for root in folders {
        if !root.is_dir() {
            warn(
                &mut result,
                format!("Folder unavailable: {}", root.display()),
            );
            continue;
        }
        for entry in WalkDir::new(root).follow_links(false).max_open(8) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn(&mut result, e.to_string());
                    continue;
                }
            };
            if !entry.file_type().is_file() || !supported(entry.path()) {
                continue;
            }
            if entry.path().canonicalize().is_ok_and(|p| seen.contains(&p)) {
                continue;
            }
            if seen.len() >= MAX_FILES {
                warn(
                    &mut result,
                    format!("Local library limited to {MAX_FILES} files"),
                );
                break 'folders;
            }
            let parsed = (|| -> Result<Entry> {
                let p = entry.path().canonicalize()?;
                let metadata = fs::metadata(&p)?;
                let modified_ns = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
                let size = metadata.len();
                if let Some(e) = old
                    .get(&p)
                    .filter(|e| e.modified_ns == modified_ns && e.size == size)
                {
                    return Ok(e.clone());
                }
                Ok(Entry {
                    track: read_track(&p)?,
                    path: p,
                    modified_ns,
                    size,
                })
            })();
            match parsed {
                Ok(e) if seen.insert(e.path.clone()) => {
                    if old
                        .get(&e.path)
                        .is_some_and(|old| old.modified_ns == e.modified_ns && old.size == e.size)
                    {
                        result.reused += 1;
                    } else {
                        result.parsed += 1;
                    }
                    result.index.entries.push(e);
                }
                Ok(_) => {}
                Err(e) => warn(&mut result, format!("{}: {e}", entry.path().display())),
            }
        }
    }
    result.index.entries.sort_by(|a, b| {
        a.track
            .artists
            .cmp(&b.track.artists)
            .then(a.track.album.cmp(&b.track.album))
            .then(a.track.name.cmp(&b.track.name))
    });
    result
}

pub fn cover(path: &Path) -> Result<Vec<u8>> {
    lofty::config::apply_global_options(
        lofty::config::GlobalOptions::new()
            .allocation_limit(MAX_COVER_BYTES)
            .preserve_format_specific_items(false),
    );
    let tagged = Probe::open(path)?
        .options(ParseOptions::new().read_properties(false))
        .read()?;
    let picture = tagged
        .tags()
        .iter()
        .flat_map(|t| t.pictures())
        .find(|p| p.data().len() <= MAX_COVER_BYTES)
        .context("no suitably sized embedded cover")?;
    Ok(picture.data().to_vec())
}

pub fn sibling_lyrics(track: &Track) -> Result<Option<crate::lyrics::Lyrics>> {
    let Some(path) = path(&track.uri) else {
        return Ok(None);
    };
    let file = path.with_extension("lrc");
    if !file.exists() {
        return Ok(None);
    }
    let mut text = String::new();
    File::open(file)?.take(1 << 20).read_to_string(&mut text)?;
    let lines = crate::lyrics::parse_lrc(&text);
    Ok(Some(if lines.is_empty() {
        crate::lyrics::Lyrics::Plain(text)
    } else {
        crate::lyrics::Lyrics::Synced(lines)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wav(path: &Path, samples: u32) {
        let data_len = samples * 2;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + data_len).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(8000u32.to_le_bytes());
        bytes.extend(16000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data_len.to_le_bytes());
        bytes.resize(44 + data_len as usize, 0);
        fs::write(path, bytes).unwrap();
    }
    #[test]
    fn index_roundtrip_changed_deleted_and_overlapping_files() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("own music.WAV");
        wav(&file, 8000);
        let folders = vec![temp.path().to_path_buf(), temp.path().to_path_buf()];
        let first = scan(&folders, Index::default());
        assert_eq!(first.parsed, 1);
        assert_eq!(first.index.tracks()[0].name, "own music");
        assert_eq!(first.index.tracks()[0].duration_ms, 1000);
        let index_path = temp.path().join("local-index.json");
        first.index.save(&index_path).unwrap();
        let second = scan(&folders, Index::load(&index_path).unwrap());
        assert_eq!((second.parsed, second.reused), (0, 1));
        wav(&file, 16000);
        let third = scan(&folders, second.index);
        assert_eq!((third.parsed, third.reused), (1, 0));
        fs::remove_file(file).unwrap();
        assert!(scan(&folders, third.index).index.tracks().is_empty());
    }
    #[test]
    fn local_paths_and_lyrics_do_not_need_network() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("你好 music.wav");
        let track = Track {
            uri: uri(&file).unwrap(),
            ..Track::default()
        };
        assert_eq!(path(&track.uri), Some(file.as_path()));
        fs::write(file.with_extension("lrc"), "[00:01.00]Owned song").unwrap();
        assert!(matches!(
            sibling_lyrics(&track).unwrap(),
            Some(crate::lyrics::Lyrics::Synced(_))
        ));
        assert!(path("spotify:track:abc").is_none());
    }
    #[test]
    fn reads_tags_and_durations_in_all_six_formats() {
        for extension in ["wav", "flac", "mp3", "ogg", "opus", "m4a"] {
            let file = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/tone.{extension}"));
            let track = read_track(&file).unwrap_or_else(|e| panic!("{extension}: {e:#}"));
            assert_eq!(track.name, "Local fixture", "{extension}");
            assert_eq!(track.artists, "Lumen Tests", "{extension}");
            assert_eq!(track.album, "Owned audio", "{extension}");
            assert!(
                (900..=1200).contains(&track.duration_ms),
                "{extension}: {}",
                track.duration_ms
            );
        }
    }
    #[test]
    fn cover_is_read_on_demand_and_not_stored_in_index() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cover.mp3");
        let bytes = cover(&file).unwrap();
        assert_eq!(crate::ui::art::decode(&bytes, 96).unwrap().size, [32, 32]);
        let track = read_track(&file).unwrap();
        assert!(track.image.unwrap().starts_with("local:"));
    }
    #[test]
    fn same_size_modified_file_is_reparsed() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("own.wav");
        wav(&file, 8000);
        let folders = vec![temp.path().to_path_buf()];
        let first = scan(&folders, Index::default());
        fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(42))
            .unwrap();
        let second = scan(&folders, first.index);
        assert_eq!((second.parsed, second.reused), (1, 0));
    }
    #[test]
    fn malformed_index_and_invalid_audio_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("local-index.json");
        fs::write(&file, "{broken").unwrap();
        assert!(Index::load(&file).is_err());
        fs::write(temp.path().join("bad.mp3"), "not music").unwrap();
        let scan = scan(&[temp.path().to_path_buf()], Index::default());
        assert_eq!(scan.warnings.len(), 1);
        assert!(scan.index.tracks().is_empty());
    }
}
