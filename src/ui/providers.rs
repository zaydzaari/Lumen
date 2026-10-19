use super::{App, Screen, View};
use crate::{
    events::AppEvent,
    player::PlayerCmd,
    providers::{Kind, UiEvent},
};
use std::{sync::Arc, time::Duration};

#[derive(Default)]
pub struct State {
    input: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    prompt: Option<sonora_music::SignInPrompt>,
    pub connected: Option<Arc<crate::providers::Connected>>,
    pub busy: bool,
    pub generation: u64,
    pub secret: String,
    pub status: String,
    task: Option<tokio::task::JoinHandle<()>>,
    login: Option<(Kind, crate::provider_browser::Browser)>,
    token: Option<(String, crate::provider_browser::Browser)>,
    pub cdm_offer: Option<sonora_music::drm::Offer>,
    pub cdm_busy: bool,
    pub cdm_accepted: bool,
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.login.take();
        self.token.take();
    }
}
impl App {
    pub fn connect_native(&mut self, restore: bool) {
        if self.native.busy {
            return;
        }
        let settings = self.config.provider.clone();
        if settings.kind == Kind::Downloads {
            self.stop_native(false);
            return;
        }
        self.native.generation = self.native.generation.wrapping_add(1);
        let generation = self.native.generation;
        self.native.busy = true;
        self.native.status = format!("Connecting {}…", settings.kind.label());
        let secret = std::mem::take(&mut self.native.secret);
        let ui = self.ui_tx.clone();
        let (input_tx, input) = tokio::sync::mpsc::unbounded_channel();
        if !secret.is_empty() {
            let _ = input_tx.send(secret.clone());
        }
        self.native.input = Some(input_tx);
        self.native.prompt = None;
        let prompt_ui = ui.clone();
        let prompt = Arc::new(move |prompt| {
            prompt_ui.send(AppEvent::Provider(UiEvent::Prompt { generation, prompt }))
        });
        self.native.task = Some(self.shared.rt.spawn(async move {
            let result = tokio::time::timeout(Duration::from_secs(180), crate::providers::connect(settings, secret, restore, prompt, input)).await
                .map_err(|_| "Provider connection timed out".to_owned()).and_then(|r| r.map(Arc::new).map_err(|_| "Could not connect. Check the account credential, server address and network.".to_owned()));
            ui.send(AppEvent::Provider(UiEvent::Connected { generation, result }));
        }));
    }
    pub fn stop_native(&mut self, forget: bool) {
        self.native.generation = self.native.generation.wrapping_add(1);
        if let Some(task) = self.native.task.take() {
            task.abort();
        }
        self.native.login.take();
        self.native.token.take();
        self.native.secret.clear();
        self.native.input.take();
        self.native.prompt.take();
        self.native.busy = false;
        if forget {
            let kind = self
                .native
                .connected
                .as_ref()
                .map(|c| c.kind)
                .unwrap_or(self.config.provider.kind);
            if let Some(provider) = kind.provider() {
                provider.sign_out();
            }
        }
        self.send(PlayerCmd::NativeProvider(None));
        self.native.connected = None;
        self.native.status = "Not connected".into();
        if forget {
            self.config.provider.kind = Kind::Downloads;
            self.save_config();
        }
    }
    pub fn open_native_login(&mut self) {
        if self.native.busy {
            return;
        }
        let kind = self.config.provider.kind;
        match crate::provider_browser::Browser::open(crate::provider_browser::Request {
            kind,
            binding: None,
        }) {
            Ok(browser) => {
                self.native.login = Some((kind, browser));
                self.native.busy = true;
                self.native.status = "Complete sign-in in the provider window".into();
            }
            Err(_) => {
                self.native.status = "Could not open the provider window. You can enter the account credential locally below.".into();
            }
        }
    }
    pub fn tick_provider_browser(&mut self, ctx: &egui::Context) {
        if let Some((kind, browser)) = &mut self.native.login {
            let kind = *kind;
            if kind != self.config.provider.kind {
                self.native.login.take();
                self.native.busy = false;
            } else if let Some(answer) = browser.poll() {
                self.native.login.take();
                self.native.busy = false;
                match answer {
                    Some(secret) => {
                        self.native.secret = secret;
                        self.connect_native(false);
                    }
                    None => {
                        self.native.status =
                            "Sign-in was cancelled or the browser could not complete it".into()
                    }
                }
            }
        }
        if let Some((binding, browser)) = &mut self.native.token {
            if let Some(answer) = browser.poll() {
                let binding = binding.clone();
                self.native.token.take();
                let token = answer
                    .as_deref()
                    .and_then(|header| {
                        header
                            .split(';')
                            .find_map(|p| p.trim().strip_prefix("SONORA_POT="))
                    })
                    .filter(|t| !t.is_empty() && !t.starts_with('!'));
                if let Some(token) = token {
                    sonora_music::potoken::give(&binding, token.to_owned());
                } else {
                    sonora_music::potoken::failed(&binding);
                }
            }
        }
        if self.native.token.is_none()
            && self
                .native
                .connected
                .as_ref()
                .is_some_and(|c| c.kind == Kind::Youtube)
        {
            if let Some(binding) = sonora_music::potoken::wanted() {
                match crate::provider_browser::Browser::open(crate::provider_browser::Request {
                    kind: Kind::Youtube,
                    binding: Some(binding.clone()),
                }) {
                    Ok(browser) => self.native.token = Some((binding, browser)),
                    Err(_) => sonora_music::potoken::failed(&binding),
                }
            }
        }
        if self.native.busy
            || self.native.token.is_some()
            || (self.native.connected.is_some() && self.snapshot.loading)
        {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
    pub fn search_native(&mut self) {
        let Some(connected) = self.native.connected.clone() else {
            return;
        };
        let generation = self.native.generation;
        let query = self.search.sent.clone();
        if query.trim().is_empty() {
            return;
        }
        self.search.loading = true;
        let ui = self.ui_tx.clone();
        self.shared.rt.spawn(async move {
            let result = tokio::time::timeout(Duration::from_secs(25), connected.session.api.search(&query)).await
                .map_err(|_| "Provider search timed out".to_owned()).and_then(|r| r.map_err(|_| "The provider could not complete the search. Check the connection in Settings > Storage.".to_owned()))
                .map(|tracks| tracks.into_iter().take(100).filter_map(|t| crate::providers::track(connected.kind, t)).collect());
            ui.send(AppEvent::Provider(UiEvent::Search { generation, query, result }));
        });
    }
    fn import_native_library(&mut self) {
        let Some(connected) = self.native.connected.clone() else {
            return;
        };
        let generation = self.native.generation;
        let ui = self.ui_tx.clone();
        self.native.status = "Importing provider songs…".into();
        self.shared.rt.spawn(async move {
            let result = async {
                let api = &connected.session.api;
                let mut pages = if connected.session.shape == sonora_music::Shape::Catalog {
                    api.all_tracks_paged().await?
                } else {
                    api.saved_tracks_paged().await?
                };
                let mut tracks = Vec::new();
                while let Some(page) = pages.recv().await {
                    let page = page?;
                    anyhow::ensure!(
                        tracks.len() + page.items.len() <= 50_000,
                        "Provider library is too large"
                    );
                    tracks.extend(
                        page.items
                            .into_iter()
                            .filter_map(|t| crate::providers::track(connected.kind, t))
                            .map(|t| (*t).clone()),
                    );
                }
                Ok::<_, anyhow::Error>(crate::free_library::Playlist {
                    id: format!("lumen:playlist:native:{}:songs", connected.kind.slug()),
                    name: format!("{} · Songs", connected.kind.label()),
                    tracks,
                })
            };
            let result = tokio::time::timeout(Duration::from_secs(90), result)
                .await
                .ok()
                .and_then(Result::ok)
                .ok_or_else(|| {
                    "Could not import this provider's songs. Check the account and connection."
                        .to_owned()
                });
            ui.send(AppEvent::Provider(UiEvent::Library { generation, result }));
        });
    }
    pub fn handle_provider_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Prompt { generation, prompt } if generation == self.native.generation => {
                self.native.prompt = Some(prompt)
            }
            UiEvent::Connected { generation, result } if generation == self.native.generation => {
                self.native.busy = false;
                self.native.task.take();
                self.native.input.take();
                self.native.prompt.take();
                match result {
                    Ok(connected) => {
                        self.native.status = format!(
                            "{} connected · {}",
                            connected.kind.label(),
                            connected.session.profile.display_name
                        );
                        self.config.downloads.independent = true;
                        if let Some(backend) = self.backend.take() {
                            backend.shutdown();
                        }
                        self.send(PlayerCmd::Independent(true));
                        self.send(PlayerCmd::NativeProvider(Some(connected.clone())));
                        self.native.connected = Some(connected);
                        self.save_config();
                        if self.screen == Screen::Login {
                            self.screen = Screen::Main;
                            self.navigate(View::Home);
                        }
                    }
                    Err(error) => {
                        self.native.status = error;
                    }
                }
            }
            UiEvent::Search {
                generation,
                query,
                result,
            } if generation == self.native.generation && query == self.search.sent => {
                self.search.loading = false;
                match result {
                    Ok(tracks) => {
                        if let Some(results) = &mut self.search.results {
                            results.tracks.extend(tracks);
                            results.tracks.truncate(600);
                        }
                    }
                    Err(error) => self.search.error = Some(error),
                }
            }
            UiEvent::Library { generation, result } if generation == self.native.generation => {
                match result {
                    Ok(playlist) => {
                        self.free_library.retain(|p| p.id != playlist.id);
                        self.free_library.push(playlist);
                        match crate::free_library::save(
                            &self.shared.paths.data_dir.join("free-library.json"),
                            &self.free_library,
                        ) {
                            Ok(()) => {
                                self.attach_free_playlists();
                                self.native.status = "Provider songs imported".into();
                            }
                            Err(_) => {
                                self.native.status = "Provider songs could not be saved".into()
                            }
                        }
                    }
                    Err(error) => self.native.status = error,
                }
            }
            UiEvent::CdmOffer(result) => {
                self.native.cdm_busy = false;
                self.native.cdm_accepted = false;
                match result {
                    Ok(offer) => self.native.cdm_offer = Some(offer),
                    Err(error) => self.native.status = error,
                }
            }
            UiEvent::CdmInstalled(success) => {
                self.native.cdm_busy = false;
                self.native.status = if success {
                    "Widevine installed. Apple Music playback is ready for an authorized account."
                } else {
                    "Could not install the Widevine module"
                }
                .into();
            }
            _ => {}
        }
    }
}

pub fn settings(app: &mut App, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Music providers").default_open(true).show(ui, |ui| {
        ui.label("Native streaming engines adapted from Sonora (GPL-3.0). Spotify library sign-in stays separate.");
        let before = app.config.provider.kind;
        egui::ComboBox::from_id_salt("native-provider").selected_text(before.label()).show_ui(ui, |ui| {
            for kind in Kind::ALL { ui.selectable_value(&mut app.config.provider.kind, kind, kind.label()); }
        });
        if before != app.config.provider.kind { app.stop_native(false); app.save_config(); }
        let kind = app.config.provider.kind;
        if kind == Kind::Downloads { ui.label("Uses the existing permanent-download connection and saved files."); return; }
        let description = match kind {
            Kind::Youtube => "Guest playback needs no YouTube Premium. Sign-in is optional; the provider can still require browser verification.",
            Kind::Deezer => "Sign in to Deezer. Playback quality and access depend on the formats licensed to that account.",
            Kind::Apple => "Requires an Apple Music account with playback access and a Widevine module. Protected audio is streamed without exporting decrypted files.",
            Kind::Subsonic => "Enter your own Subsonic or Navidrome server address and account.",
            Kind::Downloads => unreachable!(),
        };
        ui.label(description);
        ui.label("Saved local files take priority. This provider mode streams; it does not run the separate permanent-download worker.");
        if kind == Kind::Subsonic {
            let server = ui.add(egui::TextEdit::singleline(&mut app.config.provider.server).hint_text("https://music.example.com")).changed();
            let username = ui.add(egui::TextEdit::singleline(&mut app.config.provider.username).hint_text("Username")).changed();
            if server || username { app.save_config(); }
        }
        ui.horizontal_wrapped(|ui| {
            if kind != Kind::Subsonic && ui.add_enabled(!app.native.busy, egui::Button::new("Sign in in provider window")).clicked() { app.open_native_login(); }
            if kind == Kind::Youtube && ui.add_enabled(!app.native.busy, egui::Button::new("Use guest playback")).clicked() { app.native.secret.clear(); app.connect_native(false); }
            if app.native.connected.is_some() && ui.button("Disconnect").clicked() { app.stop_native(true); }
            if app.native.busy && ui.button("Cancel").clicked() { app.stop_native(false); }
        });
        egui::CollapsingHeader::new("Manual credential (optional)").show(ui, |ui| {
            let hint = match kind { Kind::Apple => "media-user-token", Kind::Deezer => "arl cookie", Kind::Youtube => "YouTube-only cookie header", _ => "Password" };
            ui.add(egui::TextEdit::singleline(&mut app.native.secret).password(true).hint_text(hint));
            ui.label("Kept locally in Lumen's private provider files, never in config.toml or the repository.");
            if ui.add_enabled(!app.native.busy, egui::Button::new("Connect with credential")).clicked() { app.connect_native(false); }
        });
        if let Some(connected) = &app.native.connected {
            if (connected.session.capabilities.library || connected.session.shape == sonora_music::Shape::Catalog) && ui.button("Import provider songs").clicked() { app.import_native_library(); }
        }
        ui.label(&app.native.status);
        if let Some(prompt) = app.native.prompt.clone() {
            match prompt {
                sonora_music::SignInPrompt::Accounts(accounts) => {
                    ui.label("Choose the account to use:");
                    for account in accounts {
                        if ui.button(&account.name).clicked() {
                            if let Some(input) = &app.native.input { let _ = input.send(account.id); }
                            app.native.prompt = None;
                        }
                    }
                }
                sonora_music::SignInPrompt::Code { code, url } => { ui.label(code); ui.hyperlink(url); }
                sonora_music::SignInPrompt::Url(url) => { ui.hyperlink(url); }
                sonora_music::SignInPrompt::Secret => {} // supplied by the sign-in browser or local field
            }
        }
        if kind == Kind::Apple {
            ui.label(if sonora_music::drm::available() { "Widevine module available on this PC" } else { "Widevine module not found. It is not bundled with Lumen." });
            if ui.add_enabled(!app.native.cdm_busy, egui::Button::new("Get Widevine module and review terms")).clicked() {
                app.native.cdm_busy = true; let ui_tx = app.ui_tx.clone();
                app.shared.rt.spawn(async move { let result = sonora_music::drm::offer().await.map_err(|_| "Could not retrieve a verified Widevine module".to_owned()); ui_tx.send(AppEvent::Provider(UiEvent::CdmOffer(result))); });
            }
            if let Some(offer) = &app.native.cdm_offer {
                ui.label(format!("Widevine {} · Google terms", offer.version()));
                egui::ScrollArea::vertical().id_salt("widevine-terms").max_height(180.).show(ui, |ui| { ui.label(offer.license()); });
                ui.checkbox(&mut app.native.cdm_accepted, "I accept the terms shown above");
                if ui.add_enabled(app.native.cdm_accepted && !app.native.cdm_busy, egui::Button::new("Install Widevine module")).clicked() {
                    let offer = app.native.cdm_offer.take().unwrap(); app.native.cdm_busy = true;
                    let ui_tx = app.ui_tx.clone();
                    app.shared.rt.spawn(async move { let success = tokio::task::spawn_blocking(move || offer.install().is_ok()).await.unwrap_or(false); ui_tx.send(AppEvent::Provider(UiEvent::CdmInstalled(success))); });
                }
            }
        }
    });
}
