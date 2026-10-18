use super::{App, Screen, TrackList, View};
use crate::{
    events::{AppEvent, ListKey, ToastKind},
    model::SearchResults,
    player::PlayerCmd,
};
use std::sync::Arc;

impl App {
    pub fn enter_free_mode(&mut self) {
        // Never open the Spotify streaming session for this mode: librespot exits on Free accounts.
        if let Some(backend) = self.backend.take() {
            backend.shutdown();
        }
        self.config.downloads.independent = true;
        self.send(PlayerCmd::Independent(true));
        self.save_config();
        self.load_cached_library();
        self.attach_free_playlists();
        self.screen = Screen::Main;
        self.navigate(if self.playlists.items.is_empty() {
            View::Settings
        } else {
            View::Playlists
        });
        self.settings_scroll_to = Some("storage".into());
    }

    pub(super) fn attach_free_playlists(&mut self) {
        self.playlists
            .items
            .retain(|p| !p.uri.starts_with("lumen:playlist:"));
        self.playlists
            .items
            .extend(self.free_library.iter().map(|p| p.collection()));
        self.playlists.total = self.playlists.total.max(self.playlists.items.len() as u32);
    }

    pub(super) fn open_free_list(&mut self, key: &ListKey) -> bool {
        let ListKey::Collection(uri) = key else {
            return false;
        };
        let Some(playlist) = self.free_library.iter().find(|p| &p.id == uri) else {
            return false;
        };
        let collection = playlist.collection();
        let mut list = TrackList::new(playlist.name.clone());
        list.subtitle = collection.subtitle.clone();
        list.total = playlist.tracks.len() as u32;
        list.tracks = playlist.tracks.iter().cloned().map(Arc::new).collect();
        list.next = None;
        list.collection = Some(collection);
        self.lists.put(key.clone(), list);
        true
    }

    pub fn import_free_library(&mut self) {
        if self.free_importing {
            return;
        }
        let path = std::path::PathBuf::from(self.free_import_path.trim().trim_matches('"'));
        if path.as_os_str().is_empty() {
            self.toast(
                ToastKind::Error,
                "Choose an extracted Spotify playlist/library JSON file",
            );
            return;
        }
        self.free_importing = true;
        let mut current = self.free_library.clone();
        let destination = self.shared.paths.data_dir.join("free-library.json");
        let ui = self.ui_tx.clone();
        let input = self.free_import_path.trim().to_string();
        let shared = self.shared.clone();
        let connection = self.download_connection.clone();
        self.shared.rt.spawn(async move {
            let result = async {
                let imported = if crate::provider_catalog::reference(&input).is_some() {
                    crate::provider_catalog::import(&shared, connection, ui.clone(), &input).await?
                } else {
                    tokio::task::spawn_blocking(move || crate::free_library::read(&path)).await??
                };
                for playlist in imported.playlists {
                    current.retain(|p| p.id != playlist.id);
                    current.push(playlist);
                }
                tokio::task::spawn_blocking(move || {
                    crate::free_library::save(&destination, &current)?;
                    Ok::<_, anyhow::Error>(crate::free_library::Import {
                        playlists: current,
                        skipped: imported.skipped,
                    })
                })
                .await?
            }
            .await
            .map_err(|e: anyhow::Error| format!("{e:#}"));
            ui.send(AppEvent::FreeLibraryImported {
                result,
                activate: true,
            });
        });
    }

    pub(super) fn search_free_library(&mut self, query: String) {
        let extra = self
            .local_tracks
            .iter()
            .chain(self.download_tracks.iter())
            .cloned();
        let tracks = crate::free_library::search(&self.free_library, extra, &query);
        let lower = query.to_lowercase();
        let playlists = self
            .free_library
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&lower))
            .map(|p| p.collection())
            .collect();
        self.search.sent = query.clone();
        self.search.loading = false;
        self.search.error = None;
        self.search.results = Some(SearchResults {
            query,
            tracks,
            playlists,
            ..Default::default()
        });
    }
}
