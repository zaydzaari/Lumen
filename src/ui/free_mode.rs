use super::{App, Screen, TrackList, View};
use crate::{
    events::{AppEvent, ListKey, ToastKind},
    model::SearchResults,
    player::PlayerCmd,
};
use std::sync::Arc;

impl App {
    pub fn start_account_login(&mut self, interactive: bool) {
        if self.login.busy {
            return;
        }
        if interactive && self.free_importing {
            self.toast(
                ToastKind::Info,
                "Wait for the current playlist import to finish",
            );
            return;
        }
        self.account_generation = self.account_generation.wrapping_add(1);
        let generation = self.account_generation;
        self.login.busy = true;
        self.login.error = None;
        self.login.url = None;
        self.login.status = "Connecting your Spotify library…".into();
        if interactive {
            self.screen = Screen::Connecting;
        }
        let shared = self.shared.clone();
        let config = self.config.clone();
        let ui = self.ui_tx.clone();
        let current = self.free_library.clone();
        self.account_task = Some(self.shared.rt.spawn(async move {
            let result = async {
                let mut connected = crate::spotify_library::connect(
                    shared.clone(),
                    config,
                    ui.clone(),
                    interactive,
                )
                .await?;
                let mut merged: Vec<_> = current
                    .into_iter()
                    .filter(|p| {
                        !p.id.starts_with("lumen:playlist:account:")
                            || (connected.restricted > 0
                                && connected.library.playlists.iter().any(|n| {
                                    n.id.rsplit_once(':')
                                        .map(|(prefix, _)| p.id.starts_with(prefix))
                                        .unwrap_or(false)
                                }))
                    })
                    .collect();
                for playlist in connected.library.playlists {
                    merged.retain(|p| p.id != playlist.id);
                    merged.push(playlist);
                }
                connected.library.playlists = merged;
                let file = shared.paths.data_dir.join("free-library.json");
                crate::free_library::save(&file, &connected.library.playlists)?;
                let profile = shared.paths.data_dir.join("account-profile.json");
                let tmp = profile.with_extension("json.tmp");
                std::fs::write(&tmp, serde_json::to_vec(&connected.profile)?)?;
                std::fs::rename(&tmp, &profile)?;
                Ok::<_, anyhow::Error>(connected)
            }
            .await
            .map_err(|e| format!("{e:#}"));
            ui.send(AppEvent::AccountLibrary { generation, result });
        }));
    }
    pub fn cancel_account_login(&mut self) {
        self.account_generation = self.account_generation.wrapping_add(1);
        if let Some(task) = self.account_task.take() {
            task.abort();
        }
        self.login.busy = false;
        self.login.url = None;
        self.screen = Screen::Login;
    }
    pub fn show_update_notice(&mut self, ctx: &egui::Context) {
        let state = self.updater.snapshot();
        let Some(version) = state.version.as_ref() else {
            return;
        };
        if self.updater.dismissed.as_ref() == Some(version) {
            return;
        }
        let mut open = true;
        egui::Window::new("Lumen update").open(&mut open).resizable(false).collapsible(false).default_width(360.0).show(ctx, |ui| {
            ui.label(&state.status);
            if let Some(path) = state.ready.as_ref() {
                let archive = crate::updates::Kind::detect() == crate::updates::Kind::Archive;
                ui.label(if archive { "Extract the downloaded archive to update your portable installation." } else { "Your settings and saved music stay on this device. The system installer will finish the update." });
                if ui.button(if archive { "Open downloaded archive" } else { "Install update" }).clicked() {
                    match open::that_detached(path) {
                        Ok(()) if !archive => self.quit(ctx),
                        Ok(()) => self.updater.dismissed = Some(version.clone()),
                        Err(error) => self.toast(ToastKind::Error, format!("Open {} to install the update: {error}", path.display())),
                    }
                }
            } else if ui.add_enabled(!state.busy, egui::Button::new("Download update")).clicked() {
                self.updater.check(&self.shared, ctx, true);
            }
            if ui.button("Later").clicked() { self.updater.dismissed = Some(version.clone()); }
        });
        if !open {
            self.updater.dismissed = Some(version.clone());
        }
    }

    pub fn enter_free_mode(&mut self) {
        // Never open the Spotify streaming session for this mode: librespot exits on Free accounts.
        if let Some(backend) = self.backend.take() {
            backend.shutdown();
        }
        self.config.downloads.independent = true;
        self.send(PlayerCmd::Independent(true));
        self.save_config();
        self.attach_free_playlists();
        self.screen = Screen::Main;
        self.navigate(View::Home);
    }

    pub(super) fn attach_free_playlists(&mut self) {
        self.playlists
            .items
            .retain(|p| !p.uri.starts_with("lumen:playlist:"));
        self.playlists
            .items
            .extend(self.free_library.iter().map(|p| p.collection()));
        self.playlists.total = self.playlists.total.max(self.playlists.items.len() as u32);
        if self.backend.is_none() {
            self.liked.clear();
            if let Some(liked) = self
                .free_library
                .iter()
                .find(|p| p.id.starts_with("lumen:playlist:account:") && p.id.ends_with(":liked"))
            {
                self.liked = liked.tracks.iter().map(|t| t.uri.clone()).collect();
            }
        }
    }

    pub(super) fn open_free_list(&mut self, key: &ListKey) -> bool {
        let playlist = match key {
            ListKey::Collection(uri) => self.free_library.iter().find(|p| &p.id == uri),
            ListKey::Liked => self
                .free_library
                .iter()
                .find(|p| p.id.starts_with("lumen:playlist:account:") && p.id.ends_with(":liked")),
            _ => None,
        };
        let Some(playlist) = playlist else {
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
        if self.free_importing || self.login.busy {
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
