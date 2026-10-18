//! Login screen styled after Spotify's login page: centred card on a dark
//! gradient, green "Continue with Spotify" pill, local-files shortcut and an
//! "Advanced" section for the optional developer client ID.

use egui::{Align2, Color32, RichText, Sense, Vec2};

use super::widgets::{self, Icon, Weight};
use super::{App, Screen};

fn pill(
    ui: &mut egui::Ui,
    text: &str,
    fill: Option<Color32>,
    fg: Color32,
    enabled: bool,
) -> egui::Response {
    let w = ui.available_width();
    let (rect, r) = ui.allocate_exact_size(
        Vec2::new(w, 48.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let hovered = r.hovered() && enabled;
    match fill {
        Some(f) => {
            let f = if hovered {
                widgets::mix(f, Color32::WHITE, 0.12)
            } else {
                f
            };
            let f = if enabled {
                f
            } else {
                widgets::with_alpha(f, 0.5)
            };
            let grow = if hovered { 1.0 } else { 0.0 };
            ui.painter().rect_filled(rect.expand(grow), 24.0, f);
        }
        None => {
            let c = if hovered {
                Color32::WHITE
            } else {
                Color32::from_gray(0x87)
            };
            ui.painter().rect_stroke(
                rect,
                24.0,
                egui::Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
        }
    }
    widgets::text(
        ui,
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        16.0,
        Weight::Bold,
        fg,
        w - 32.0,
    );
    r.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    })
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, Color32::BLACK);
    widgets::gradient(
        ui.painter(),
        egui::Rect::from_min_size(full.min, Vec2::new(full.width(), full.height() * 0.6)),
        0.0,
        Color32::from_rgb(0x2a, 0x2a, 0x2a),
        Color32::BLACK,
    );
    if super::topbar::custom_chrome(app) {
        // Allow moving the undecorated window from the login screen too.
        let bar = egui::Rect::from_min_size(full.min, Vec2::new(full.width(), 40.0));
        let drag = ui.interact(bar, ui.id().with("login-drag"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let close = egui::Rect::from_min_size(
            egui::pos2(full.right() - 46.0, full.top()),
            Vec2::new(46.0, 32.0),
        );
        let cr = ui.interact(close, ui.id().with("login-close"), Sense::click());
        if cr.hovered() {
            ui.painter()
                .rect_filled(close, 0.0, Color32::from_rgb(0xc4, 0x2b, 0x1c));
        }
        widgets::draw(
            ui,
            egui::Rect::from_center_size(close.center(), Vec2::splat(14.0)),
            Icon::Close,
            Color32::WHITE,
        );
        if cr.clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    egui::ScrollArea::vertical()
        .id_salt("login")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let card_w = 460.0f32.min(full.width() - 32.0);
            ui.add_space((full.height() * 0.08).clamp(24.0, 80.0));
            ui.vertical_centered(|ui| {
                ui.set_max_width(card_w);
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x12, 0x12, 0x12))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(40, 36))
                    .show(ui, |ui| {
                        ui.set_width(card_w - 80.0);
                        // Logo
                        let (logo, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::hover());
                        ui.painter().circle_filled(logo.center(), 28.0, pal.accent);
                        widgets::draw(
                            ui,
                            egui::Rect::from_center_size(logo.center() + Vec2::new(2.0, 0.0), Vec2::splat(26.0)),
                            Icon::Play,
                            Color32::BLACK,
                        );
                        ui.add_space(18.0);
                        let (title, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
                        widgets::text(
                            ui,
                            title.center(),
                            Align2::CENTER_CENTER,
                            if app.screen == Screen::Connecting { "Signing you in" } else { "Log in to Lumen" },
                            30.0,
                            Weight::Black,
                            Color32::WHITE,
                            title.width(),
                        );
                        ui.add_space(24.0);

                        if app.screen == Screen::Connecting {
                            ui.vertical_centered(|ui| {
                                ui.spinner();
                                ui.add_space(10.0);
                                ui.label(RichText::new(&app.login.status).size(15.0).color(Color32::WHITE));
                            });
                            if let Some(url) = app.login.url.clone() {
                                ui.add_space(18.0);
                                ui.label(RichText::new("Browser didn't open?").color(pal.weak));
                                ui.add_space(6.0);
                                if pill(ui, "Open the login page", Some(pal.accent), Color32::BLACK, true).clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(&url));
                                }
                                ui.add_space(8.0);
                                if pill(ui, "Copy link", None, Color32::WHITE, true).clicked() {
                                    ui.ctx().copy_text(url.clone());
                                    app.toast(crate::events::ToastKind::Info, "Link copied");
                                }
                            }
                            if app.account_task.is_some() {
                                ui.add_space(12.0);
                                if pill(ui, "Cancel", None, Color32::WHITE, true).clicked() { app.cancel_account_login(); }
                            }
                            return;
                        }

                        if let Some(err) = app.login.error.clone() {
                            egui::Frame::new()
                                .fill(Color32::from_rgb(0xe9, 0x14, 0x29))
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(14, 10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.add(egui::Label::new(RichText::new(err).color(Color32::WHITE)).wrap());
                                });
                            ui.add_space(16.0);
                        }

                        if pill(ui, "Continue with Spotify", Some(pal.accent), Color32::BLACK, !app.login.busy).clicked() {
                            app.config.client_id = app.login.client_id.trim().to_string();
                            app.save_config();
                            app.start_account_login(true);
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new("Connect your playlists and liked songs. Lumen handles playback separately; Spotify controls account access.").small().color(pal.weak));
                        ui.add_space(10.0);
                        if pill(ui, "Use Spotify Premium streaming", None, Color32::WHITE, !app.login.busy).clicked() {
                            app.config.client_id = app.login.client_id.trim().to_string();
                            app.save_config();
                            app.start_login(true);
                        }
                        ui.add_space(10.0);
                        if pill(ui, "Continue without signing in", None, Color32::WHITE, true).clicked() {
                            app.enter_free_mode();
                        }
                        ui.add_space(10.0);
                        if pill(ui, "Listen to local files", None, Color32::WHITE, true).clicked() {
                            app.show_local();
                        }
                        ui.add_space(22.0);
                        let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                        ui.painter().hline(line.x_range(), line.center().y, egui::Stroke::new(1.0, Color32::from_gray(0x29)));
                        ui.add_space(14.0);

                        egui::CollapsingHeader::new(RichText::new("Advanced: your own developer app (optional)").color(pal.weak))
                            .default_open(app.login.client_id != crate::spotify_library::DEFAULT_CLIENT_ID)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(
                                        "Lumen includes its registered public Client ID. Development-mode accounts must be allowed in the app dashboard. You can use a different registered app below.",
                                    )
                                    .color(pal.weak),
                                );
                                ui.add_space(6.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut app.login.client_id)
                                        .hint_text("Client ID (32 characters)")
                                        .desired_width(f32::INFINITY)
                                        .margin(Vec2::new(10.0, 8.0)),
                                );
                                ui.add_space(6.0);
                                let redirect = app.config.redirect_uri();
                                ui.label("1. Open the Spotify developer dashboard:");
                                ui.hyperlink("https://developer.spotify.com/dashboard");
                                ui.label("2. Create app (any name). Tick \"Web API\".");
                                ui.horizontal_wrapped(|ui| {
                                    ui.label("3. Redirect URI:");
                                    ui.code(&redirect);
                                    if ui.small_button("Copy").clicked() {
                                        ui.ctx().copy_text(redirect.clone());
                                    }
                                });
                                ui.label("4. Copy the Client ID from the app's settings into the box above.");
                            });
                        ui.add_space(18.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new(
                                    "Spotify streaming requires Premium. Library access depends on Spotify app permissions. Local files work without signing in.",
                                )
                                .small()
                                .color(pal.weak),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Lumen is unofficial and not affiliated with Spotify.")
                                    .small()
                                    .color(Color32::from_gray(0x6a)),
                            );
                        });
                    });
            });
            ui.add_space(40.0);
        });
}
