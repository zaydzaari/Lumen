//! Profile page, like Spotify's: round photo on a gradient in the photo's
//! colour, "Profile" label, big name, counts, then public playlists and
//! recently played artists.

use egui::{Align2, Color32, Rect, Sense, Vec2};

use super::art::LARGE_PX;
use super::views::{shelf, title_size, CardKind};
use super::widgets::{self, Icon, Weight};
use super::App;

const PAD: f32 = 24.0;

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn show(app: &mut App, ui: &mut egui::Ui, username: &str) {
    let pal = app.palette;
    let state = app.profiles.get(username).cloned().flatten();
    let profile = match state {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("Couldn't load this profile")
                        .font(widgets::font(22.0, Weight::Bold)),
                );
                ui.label(egui::RichText::new(e).color(pal.weak));
                ui.add_space(8.0);
                if ui.button("Try again").clicked() {
                    app.load_profile(username, true);
                }
            });
            return;
        }
        None => {
            app.load_profile(username, false);
            ui.add_space(60.0);
            ui.vertical_centered(|ui| ui.spinner());
            return;
        }
    };
    let is_me = app.user_id.as_deref() == Some(username);
    let color = profile
        .image
        .as_deref()
        .and_then(|u| {
            let _ = app.art.get(u, LARGE_PX);
            app.art.color(u)
        })
        .unwrap_or(Color32::from_rgb(0x53, 0x53, 0x53));

    egui::ScrollArea::vertical()
        .id_salt(("profile", username))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let w = ui.available_width();
            let (head, _) = ui.allocate_exact_size(Vec2::new(w, 300.0), Sense::hover());
            widgets::gradient(
                ui.painter(),
                head,
                8.0,
                color,
                widgets::mix(color, pal.bg, 0.65),
            );
            let pic = Rect::from_min_size(
                egui::pos2(head.left() + PAD, head.bottom() - PAD - 232.0),
                Vec2::splat(232.0),
            );
            widgets::shadow(ui.painter(), pic, 116.0);
            let tex = profile
                .image
                .as_deref()
                .and_then(|u| app.art.get(u, LARGE_PX));
            if tex.is_some() {
                widgets::image(ui.painter(), pic, tex, 116.0, pal.panel, pal.weak);
            } else {
                ui.painter().circle_filled(pic.center(), 116.0, pal.panel);
                widgets::draw(
                    ui,
                    Rect::from_center_size(pic.center(), Vec2::splat(96.0)),
                    Icon::User,
                    pal.weak,
                );
            }
            let tx = pic.right() + 24.0;
            let tw = head.right() - tx - PAD;
            widgets::text(
                ui,
                egui::pos2(tx, pic.center().y - 40.0),
                Align2::LEFT_CENTER,
                "Profile",
                14.0,
                Weight::Semibold,
                Color32::WHITE,
                tw,
            );
            let size = title_size(ui, &profile.name, tw);
            widgets::text(
                ui,
                egui::pos2(tx, pic.bottom() - 36.0),
                Align2::LEFT_BOTTOM,
                &profile.name,
                size,
                Weight::Black,
                Color32::WHITE,
                tw,
            );
            let meta = [
                plural(
                    profile.total_public_playlists,
                    "Public Playlist",
                    "Public Playlists",
                ),
                plural(profile.followers, "Follower", "Followers"),
                format!("{} Following", profile.following),
            ]
            .join(" • ");
            widgets::text(
                ui,
                egui::pos2(tx, pic.bottom() - 12.0),
                Align2::LEFT_CENTER,
                &meta,
                14.0,
                Weight::Semibold,
                Color32::WHITE,
                tw,
            );

            // Actions band
            ui.add_space(-ui.spacing().item_spacing.y);
            let (act, _) = ui.allocate_exact_size(Vec2::new(w, 88.0), Sense::hover());
            widgets::gradient(
                ui.painter(),
                act,
                0.0,
                widgets::mix(color, pal.bg, 0.65),
                pal.bg,
            );
            let more = Rect::from_center_size(
                egui::pos2(act.left() + PAD + 16.0, act.center().y),
                Vec2::splat(32.0),
            );
            let mr = ui.interact(more, ui.id().with("profile-more"), Sense::click());
            widgets::draw(
                ui,
                Rect::from_center_size(more.center(), Vec2::splat(26.0)),
                Icon::More,
                if mr.hovered() { pal.text } else { pal.weak },
            );
            let mut open_stats = false;
            if is_me {
                // Spotify-style pill next to ⋯ on your own profile.
                let g = widgets::galley(ui, "Listening stats", 14.0, Weight::Bold, pal.text, 300.0);
                let b = Rect::from_min_size(
                    egui::pos2(more.right() + 16.0, act.center().y - 16.0),
                    Vec2::new(g.size().x + 32.0, 32.0),
                );
                let br = ui.interact(b, ui.id().with("profile-stats"), Sense::click());
                let h = ui.ctx().animate_bool_with_time(br.id, br.hovered(), 0.12);
                ui.painter().rect_stroke(
                    b.expand(h),
                    16.0,
                    egui::Stroke::new(1.0, widgets::with_alpha(pal.weak, 0.6 + 0.4 * h)),
                    egui::StrokeKind::Inside,
                );
                ui.painter()
                    .galley(b.center() - g.size() / 2.0, g, pal.text);
                open_stats = br.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
            }
            if open_stats {
                app.navigate(super::View::Stats);
            }
            egui::Popup::menu(&mr)
                .id(ui.id().with("profile-menu"))
                .show(|ui| {
                    widgets::menu_style(ui, 220.0);
                    if ui.button("Copy link to profile").clicked() {
                        ui.ctx()
                            .copy_text(format!("https://open.spotify.com/user/{username}"));
                        ui.close();
                    }
                    if ui.button("Open in Spotify").clicked() {
                        let _ = open::that_detached(format!("spotify:user:{username}"));
                        ui.close();
                    }
                    if is_me && ui.button("Edit profile on spotify.com").clicked() {
                        let _ = open::that_detached("https://www.spotify.com/account/profile/");
                        ui.close();
                    }
                });

            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(PAD as i8, 0))
                .show(ui, |ui| {
                    if !profile.recent_artists.is_empty() {
                        let cards = profile
                            .recent_artists
                            .iter()
                            .cloned()
                            .map(CardKind::Coll)
                            .collect();
                        shelf(
                            app,
                            ui,
                            if is_me {
                                "Recently played artists"
                            } else {
                                "Artists"
                            },
                            cards,
                            None,
                        );
                        ui.add_space(16.0);
                    }
                    if !profile.public_playlists.is_empty() {
                        let cards = profile
                            .public_playlists
                            .iter()
                            .cloned()
                            .map(CardKind::Coll)
                            .collect();
                        shelf(app, ui, "Public Playlists", cards, None);
                    } else if profile.recent_artists.is_empty() {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Nothing public here yet.").color(pal.weak));
                    }
                    ui.add_space(32.0);
                });
        });
}
