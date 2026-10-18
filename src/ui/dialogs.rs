//! Modal dialogs: Spotify's "Edit details" for playlists and the delete
//! confirmation.

use egui::{Align2, Color32, Rect, Sense, Vec2};

use super::art::LARGE_PX;
use super::widgets::{self, Icon, Weight};
use super::App;

fn frame(app: &App) -> egui::Frame {
    egui::Frame::new()
        .fill(widgets::mix(app.palette.bg, app.palette.text, 0.06))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(24))
        .shadow(egui::epaint::Shadow {
            offset: [0, 16],
            blur: 48,
            spread: 0,
            color: Color32::from_black_alpha(170),
        })
}

fn pill(
    ui: &mut egui::Ui,
    label: &str,
    fill: Color32,
    fg: Color32,
    enabled: bool,
) -> egui::Response {
    let g = widgets::galley(ui, label, 15.0, Weight::Bold, fg, f32::INFINITY);
    let (rect, r) = ui.allocate_exact_size(
        Vec2::new(g.size().x + 64.0, 48.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let h = ui
        .ctx()
        .animate_bool_with_time(r.id, r.hovered() && enabled, 0.12);
    let fill = if enabled {
        fill
    } else {
        widgets::with_alpha(fill, 0.5)
    };
    ui.painter().rect_filled(rect.expand(h * 1.5), 24.0, fill);
    ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
    r.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    })
}

/// Styled text field: filled box, label floating on the border when focused.
fn field(
    ui: &mut egui::Ui,
    label: &str,
    text: &mut String,
    multiline: bool,
    height: f32,
) -> egui::Response {
    let pal = ui.visuals().clone();
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let id = ui.id().with(("field", label));
    let focused = ui.memory(|m| m.has_focus(id));
    let fill = widgets::with_alpha(pal.text_color(), 0.1);
    ui.painter().rect_filled(rect, 4.0, fill);
    if focused {
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, widgets::with_alpha(pal.text_color(), 0.6)),
            egui::StrokeKind::Inside,
        );
    }
    let inner = rect.shrink2(Vec2::new(12.0, 10.0));
    let edit = if multiline {
        egui::TextEdit::multiline(text).desired_rows(4)
    } else {
        egui::TextEdit::singleline(text)
    }
    .id(id)
    .frame(egui::Frame::NONE)
    .font(widgets::font(14.0, Weight::Regular))
    .hint_text(egui::RichText::new(label).color(widgets::with_alpha(pal.text_color(), 0.5)));
    let r = ui.put(inner, edit);
    if focused || !text.is_empty() {
        // Small label on the top edge, like Spotify's dialog.
        let g = widgets::galley(ui, label, 11.0, Weight::Bold, pal.text_color(), 300.0);
        let lr = Rect::from_min_size(
            rect.min + Vec2::new(8.0, -g.size().y / 2.0),
            g.size() + Vec2::new(8.0, 0.0),
        );
        ui.painter().rect_filled(
            lr,
            2.0,
            widgets::mix(ui.visuals().panel_fill, pal.text_color(), 0.06),
        );
        ui.painter()
            .galley(lr.min + Vec2::new(4.0, 0.0), g, pal.text_color());
    }
    r
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    edit_details(app, ctx);
    confirm_delete(app, ctx);
    about(app, ctx);
    credits(app, ctx);
    shortcuts(app, ctx);
}

/// Spotify's "Keyboard shortcuts" overview (Ctrl+/).
fn shortcuts(app: &mut App, ctx: &egui::Context) {
    if !app.shortcuts_open {
        return;
    }
    let pal = app.palette;
    let sc = app.config.shortcuts.clone();
    let groups: [(&str, Vec<(&str, String)>); 3] = [
        (
            "Playback",
            vec![
                ("Play / pause", sc.play_pause.clone()),
                ("Next song", sc.next.clone()),
                ("Previous song", sc.previous.clone()),
                ("Volume up", sc.volume_up.clone()),
                ("Volume down", sc.volume_down.clone()),
                ("Like the current song", "Alt+Shift+B".into()),
            ],
        ),
        (
            "Navigation",
            vec![
                ("Search (or Ctrl+K)", sc.search.clone()),
                ("Go back", "Alt+Left".into()),
                ("Go forward", "Alt+Right".into()),
                ("Find in playlist", "Ctrl+F".into()),
                ("Full screen", "F11".into()),
                ("Mini player", "Ctrl+Shift+M".into()),
            ],
        ),
        (
            "Song lists",
            vec![
                ("Select a song", "Up / Down".into()),
                ("Play the selected song", "Enter".into()),
                ("Select all", "Ctrl+A".into()),
                ("Remove from your playlist", "Delete".into()),
                ("Clear the selection", "Esc".into()),
                ("This overview", "Ctrl+/".into()),
            ],
        ),
    ];
    let mut close = false;
    let resp = egui::Modal::new(egui::Id::new("shortcuts"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame(app))
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Keyboard shortcuts")
                        .font(widgets::font(24.0, Weight::Bold)),
                );
                ui.add_space(ui.available_width() - 32.0);
                if widgets::icon_button(ui, Icon::Close, 16.0, "Close", pal.weak, pal.text)
                    .clicked()
                {
                    close = true;
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(480.0)
                .show(ui, |ui| {
                    for (title, rows) in &groups {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(*title).font(widgets::font(15.0, Weight::Bold)),
                        );
                        ui.add_space(4.0);
                        for (label, keys) in rows {
                            ui.horizontal(|ui| {
                                ui.set_min_height(32.0);
                                ui.label(egui::RichText::new(*label).color(pal.weak));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        super::settings::keycaps(ui, keys, &pal);
                                    },
                                );
                            });
                        }
                    }
                });
        });
    if close || resp.should_close() {
        app.shortcuts_open = false;
    }
}

fn credits(app: &mut App, ctx: &egui::Context) {
    let Some((track, state)) = app.credits.clone() else {
        return;
    };
    let pal = app.palette;
    let mut close = false;
    let mut open_artist: Option<(String, String)> = None;
    let resp = egui::Modal::new(egui::Id::new("song-credits"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame(app))
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Credits").font(widgets::font(24.0, Weight::Bold)));
                ui.add_space(ui.available_width() - 32.0);
                if widgets::icon_button(ui, Icon::Close, 16.0, "Close", pal.weak, pal.text)
                    .clicked()
                {
                    close = true;
                }
            });
            ui.label(
                egui::RichText::new(&track.name)
                    .font(widgets::font(16.0, Weight::Bold))
                    .color(pal.text),
            );
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(440.0)
                .show(ui, |ui| match &state {
                    None => {
                        ui.spinner();
                    }
                    Some(Err(_)) => {
                        ui.label(
                            egui::RichText::new("Spotify has no credits for this song.")
                                .color(pal.weak),
                        );
                    }
                    Some(Ok(c)) => {
                        for (role, people) in &c.roles {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(role).font(widgets::font(15.0, Weight::Bold)),
                            );
                            for p in people {
                                ui.add_space(4.0);
                                let name = egui::RichText::new(&p.name)
                                    .font(widgets::font(15.0, Weight::Regular));
                                if p.uri.is_some() {
                                    let r =
                                        ui.add(egui::Label::new(name).sense(egui::Sense::click()));
                                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        open_artist = Some((
                                            p.uri.clone().unwrap_or_default(),
                                            p.name.clone(),
                                        ));
                                    }
                                } else {
                                    ui.label(name);
                                }
                                if !p.roles.is_empty() {
                                    ui.label(egui::RichText::new(&p.roles).small().color(pal.weak));
                                }
                            }
                        }
                        if !c.sources.is_empty() {
                            ui.add_space(14.0);
                            ui.label(
                                egui::RichText::new(format!("Source: {}", c.sources.join(", ")))
                                    .small()
                                    .color(pal.weak),
                            );
                        }
                    }
                });
        });
    if let Some((uri, name)) = open_artist {
        app.credits = None;
        app.open_uri(crate::model::CollectionKind::Artist, &uri, &name);
        return;
    }
    if close || resp.should_close() {
        app.credits = None;
    }
}

fn about(app: &mut App, ctx: &egui::Context) {
    let Some((name, bio)) = app.about_open.clone() else {
        return;
    };
    let pal = app.palette;
    let mut close = false;
    let resp = egui::Modal::new(egui::Id::new("artist-about"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame(app))
        .show(ctx, |ui| {
            ui.set_width(620.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&name).font(widgets::font(24.0, Weight::Bold)));
                ui.add_space(ui.available_width() - 32.0);
                if widgets::icon_button(ui, Icon::Close, 16.0, "Close", pal.weak, pal.text)
                    .clicked()
                {
                    close = true;
                }
            });
            ui.add_space(10.0);
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&bio).font(widgets::font(15.0, Weight::Regular)));
                });
        });
    if close || resp.should_close() {
        app.about_open = None;
    }
}

fn edit_details(app: &mut App, ctx: &egui::Context) {
    if app.edit_playlist.is_none() {
        return;
    }
    let pal = app.palette;
    let mut save = false;
    let mut close = false;
    let image = app.edit_playlist.as_ref().and_then(|e| e.image.clone());
    let tex = image.as_deref().and_then(|u| app.art.get(u, LARGE_PX));
    let resp = egui::Modal::new(egui::Id::new("edit-playlist"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame(app))
        .show(ctx, |ui| {
            ui.set_width(524.0);
            let Some(e) = app.edit_playlist.as_mut() else {
                return;
            };
            ui.horizontal(|ui| {
                widgets::text(
                    ui,
                    ui.cursor().left_center() + Vec2::new(0.0, 14.0),
                    Align2::LEFT_CENTER,
                    "Edit details",
                    24.0,
                    Weight::Bold,
                    pal.text,
                    400.0,
                );
                ui.add_space(ui.available_width() - 32.0);
                if widgets::icon_button(ui, Icon::Close, 16.0, "Close", pal.weak, pal.text)
                    .clicked()
                {
                    close = true;
                }
            });
            ui.add_space(18.0);
            ui.horizontal_top(|ui| {
                let (cover, _) = ui.allocate_exact_size(Vec2::splat(180.0), Sense::hover());
                widgets::shadow(ui.painter(), cover, 4.0);
                if tex.is_some() {
                    widgets::image(ui.painter(), cover, tex, 4.0, pal.panel, pal.weak);
                } else {
                    ui.painter()
                        .rect_filled(cover, 4.0, widgets::mix(pal.bg, pal.text, 0.12));
                    widgets::draw(
                        ui,
                        Rect::from_center_size(cover.center(), Vec2::splat(56.0)),
                        Icon::Note,
                        pal.weak,
                    );
                }
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    let r = field(ui, "Name", &mut e.name, false, 40.0);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        save = true;
                    }
                    ui.add_space(12.0);
                    let before = e.description.clone();
                    field(
                        ui,
                        "Add an optional description",
                        &mut e.description,
                        true,
                        124.0,
                    );
                    if e.description != before {
                        e.description_dirty = true;
                    }
                });
            });
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() - 120.0);
                let label = if e.saving { "Saving…" } else { "Save" };
                if pill(
                    ui,
                    label,
                    pal.text,
                    super::theme::contrast_on(pal.text),
                    !e.saving,
                )
                .clicked()
                {
                    save = true;
                }
            });
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(
                    "Changes are saved to your Spotify account. Cover images can be changed in \
                     the Spotify app.",
                )
                .small()
                .color(pal.weak),
            );
        });
    if save {
        app.save_playlist_edit();
    } else if close || resp.should_close() {
        app.edit_playlist = None;
    }
}

fn confirm_delete(app: &mut App, ctx: &egui::Context) {
    let Some(c) = app.confirm_delete.clone() else {
        return;
    };
    let pal = app.palette;
    let mut decision: Option<bool> = None;
    let resp = egui::Modal::new(egui::Id::new("confirm-delete"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame(app).fill(Color32::WHITE))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                egui::RichText::new("Delete from Your Library?")
                    .font(widgets::font(22.0, Weight::Bold))
                    .color(Color32::BLACK),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!("This will delete {} from Your Library.", c.name))
                    .color(Color32::from_gray(0x3c)),
            );
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() - 220.0);
                let cancel =
                    widgets::galley(ui, "Cancel", 15.0, Weight::Bold, Color32::BLACK, 200.0);
                let (cr, r) =
                    ui.allocate_exact_size(Vec2::new(cancel.size().x + 32.0, 48.0), Sense::click());
                ui.painter()
                    .galley(cr.center() - cancel.size() / 2.0, cancel, Color32::BLACK);
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    decision = Some(false);
                }
                if pill(ui, "Delete", pal.accent, pal.on_accent, true).clicked() {
                    decision = Some(true);
                }
            });
        });
    match decision {
        Some(true) => {
            app.confirm_delete = None;
            app.delete_playlist(&c.uri);
        }
        Some(false) => app.confirm_delete = None,
        None if resp.should_close() => app.confirm_delete = None,
        None => {}
    }
}
