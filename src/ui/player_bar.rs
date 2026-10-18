//! Bottom player bar: now playing, transport controls, seek bar, volume, panel toggles.

use egui::{Align, Align2, Layout, Sense, Vec2};

use super::art::THUMB_PX;
use super::widgets::{self, Icon, Weight};
use super::{App, RightPanel};
use crate::model::{fmt_ms, CollectionKind};
use crate::player::{Mode, PlayerCmd, Repeat};

/// Four-pixel rail with a large interaction target and keyboard support.
pub(super) fn progress(
    ui: &mut egui::Ui,
    value: &mut f32,
    max: f32,
    width: f32,
    accent: egui::Color32,
    label: &str,
) -> egui::Response {
    let (rect, mut r) =
        ui.allocate_exact_size(egui::vec2(width.max(16.), 18.), Sense::click_and_drag());
    r.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), *value as f64, label));
    let old = *value;
    if r.clicked() || r.drag_started() {
        r.request_focus();
    }
    if r.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                r.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    ..Default::default()
                },
            )
        });
    }
    if ui.is_enabled() {
        if r.clicked() || r.dragged() || r.drag_stopped() {
            if let Some(pos) = r.interact_pointer_pos() {
                *value = ((pos.x - rect.left()) / rect.width()).clamp(0., 1.) * max;
            }
        }
        if r.has_focus() {
            let delta = ui.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    max / 100.
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    -max / 100.
                } else {
                    0.
                }
            });
            *value = (*value + delta).clamp(0., max);
            if ui.input(|i| i.key_pressed(egui::Key::Home)) {
                *value = 0.;
            }
            if ui.input(|i| i.key_pressed(egui::Key::End)) {
                *value = max;
            }
        }
    }
    if *value != old {
        r.mark_changed();
    }
    let rail = egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 4.));
    ui.painter().rect_filled(
        rail,
        2.,
        widgets::with_alpha(ui.visuals().text_color(), 0.3),
    );
    let x = rail.left() + rail.width() * (*value / max.max(1.)).clamp(0., 1.);
    // Spotify: the fill turns accent and the knob grows in on hover.
    let active = r.hovered() || r.dragged() || r.has_focus();
    let h = ui
        .ctx()
        .animate_bool_with_time(r.id.with("hover"), active, 0.12);
    ui.painter().rect_filled(
        egui::Rect::from_min_max(rail.min, egui::pos2(x, rail.bottom())),
        2.,
        widgets::mix(ui.visuals().text_color(), accent, h),
    );
    if h > 0.0 {
        ui.painter()
            .circle_filled(egui::pos2(x, rail.center().y), 6. * h, egui::Color32::WHITE);
    }
    if label == "Seek" && max > 1000.0 {
        // Spotify-style: show the time under the pointer while hovering.
        if let Some(p) = r.hover_pos() {
            let ms = ((p.x - rail.left()) / rail.width()).clamp(0., 1.) * max;
            let s = (ms / 1000.0) as u64;
            return r.on_hover_text_at_pointer(format!("{}:{:02}", s / 60, s % 60));
        }
    }
    r.on_hover_text(label)
}

fn regions(rect: egui::Rect) -> [egui::Rect; 3] {
    let side = (rect.width() * 0.3).clamp(160., 340.);
    [
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + side, rect.bottom())),
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + side + 12., rect.top()),
            egui::pos2(rect.right() - side - 12., rect.bottom()),
        ),
        egui::Rect::from_min_max(egui::pos2(rect.right() - side, rect.top()), rect.max),
    ]
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let snap = app.snapshot.clone();
    let pal = app.palette;
    let [left, center, right] = regions(ui.available_rect_before_wrap());

    // ---------------------------------------------------- now playing
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(left)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.set_clip_rect(left);
            let Some(t) = &snap.track else {
                return;
            };
            let art = egui::Rect::from_min_size(
                egui::pos2(left.left() + 8.0, left.center().y - 28.0),
                Vec2::splat(56.0),
            );
            let ar = ui.interact(art, ui.id().with("np-art"), Sense::click());
            // Right-click the cover, title or artist for the full song menu.
            super::views::track_context_menu(app, &ar, t);
            let tex = t.image.as_deref().and_then(|u| app.art.get(u, THUMB_PX));
            widgets::image(ui.painter(), art, tex, 4.0, pal.panel, pal.weak);
            if ar
                .on_hover_text("Now playing view")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app.right = if app.right == RightPanel::NowPlaying {
                    RightPanel::None
                } else {
                    RightPanel::NowPlaying
                };
            }
            let x = art.right() + 14.0;
            let text_w = (left.right() - x - 44.0).max(40.0);
            let title_r = widgets::text(
                ui,
                egui::pos2(x, art.center().y - 10.0),
                Align2::LEFT_CENTER,
                t.display_name(),
                14.0,
                Weight::Semibold,
                pal.text,
                text_w,
            );
            let tr = ui.interact(title_r, ui.id().with("np-title"), Sense::click());
            super::views::track_context_menu(app, &tr, t);
            if tr.hovered() {
                ui.painter().hline(
                    title_r.x_range(),
                    title_r.bottom(),
                    egui::Stroke::new(1.0, pal.text),
                );
            }
            if tr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                if let Some(a) = &t.album_uri {
                    app.open_uri(CollectionKind::Album, a, &t.album);
                }
            }
            let artist_text = if snap.mode == Mode::Connect {
                format!(
                    "{} • on {}",
                    t.artists,
                    snap.remote_client.as_deref().unwrap_or("Spotify Connect")
                )
            } else {
                t.artists.clone()
            };
            let artist_r = widgets::text(
                ui,
                egui::pos2(x, art.center().y + 10.0),
                Align2::LEFT_CENTER,
                &artist_text,
                12.0,
                Weight::Regular,
                pal.weak,
                text_w,
            );
            let arr = ui.interact(artist_r, ui.id().with("np-artist"), Sense::click());
            super::views::track_context_menu(app, &arr, t);
            if arr.hovered() {
                ui.painter().hline(
                    artist_r.x_range(),
                    artist_r.bottom(),
                    egui::Stroke::new(1.0, pal.text),
                );
            }
            if arr
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                if let Some(a) = &t.artist_uri {
                    app.open_uri(
                        CollectionKind::Artist,
                        a,
                        t.artists.split(", ").next().unwrap_or(""),
                    );
                }
            }
            if !t.uri.starts_with("local:") {
                let save = egui::Rect::from_center_size(
                    egui::pos2(title_r.right().max(artist_r.right()) + 22.0, art.center().y),
                    Vec2::splat(28.0),
                );
                if save.right() <= left.right() {
                    let id = ui.id().with("np-save");
                    super::views::save_button(app, ui, save.center(), 16.0, id, t);
                }
            }
        },
    );

    // ---------------------------------------------------- transport + seek
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(center)
            .layout(Layout::top_down(Align::Center)),
        |ui| {
            ui.set_clip_rect(center);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let row_w = 4.0 * 28.0 + 32.0 + 4.0 * 8.0 + 8.0;
                ui.add_space(((center.width() - row_w) / 2.0).max(0.0));
                if widgets::toggle_icon(
                    ui,
                    Icon::Shuffle,
                    16.0,
                    if snap.shuffle {
                        "Disable shuffle"
                    } else {
                        "Enable shuffle"
                    },
                    snap.shuffle,
                    pal.accent,
                    pal.weak,
                    pal.text,
                )
                .clicked()
                {
                    app.send(PlayerCmd::SetShuffle(!snap.shuffle));
                }
                if widgets::icon_button(ui, Icon::Prev, 16.0, "Previous", pal.weak, pal.text)
                    .clicked()
                {
                    app.send(PlayerCmd::Previous);
                }
                if widgets::round_play(
                    ui,
                    32.0,
                    snap.playing,
                    pal.text,
                    pal.base,
                    if snap.playing { "Pause" } else { "Play" },
                )
                .clicked()
                {
                    app.send(PlayerCmd::TogglePlay);
                }
                if widgets::icon_button(ui, Icon::Next, 16.0, "Next", pal.weak, pal.text).clicked()
                {
                    app.send(PlayerCmd::Next);
                }
                let (icon, tip) = match snap.repeat {
                    Repeat::Off => (Icon::Repeat, "Enable repeat"),
                    Repeat::All => (Icon::Repeat, "Enable repeat one"),
                    Repeat::One => (Icon::RepeatOne, "Disable repeat"),
                };
                if widgets::toggle_icon(
                    ui,
                    icon,
                    16.0,
                    tip,
                    snap.repeat != Repeat::Off,
                    pal.accent,
                    pal.weak,
                    pal.text,
                )
                .clicked()
                {
                    app.send(PlayerCmd::SetRepeat(snap.repeat.cycle()));
                }
            });
            ui.horizontal(|ui| {
                let dur = snap.track.as_ref().map_or(0, |t| t.duration_ms);
                let pos = app
                    .seek_drag
                    .map(|v| v as u32)
                    .unwrap_or_else(|| snap.position_now());
                let time = |ui: &mut egui::Ui, s: String, align: Align2| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(40.0, 18.0), Sense::hover());
                    let x = if align == Align2::RIGHT_CENTER {
                        r.right()
                    } else {
                        r.left()
                    };
                    widgets::text(
                        ui,
                        egui::pos2(x, r.center().y),
                        align,
                        &s,
                        11.5,
                        Weight::Regular,
                        pal.weak,
                        40.0,
                    );
                };
                time(ui, fmt_ms(pos), Align2::RIGHT_CENTER);
                let mut value = app.seek_drag.unwrap_or(pos as f32);
                let width = (center.width() - 100.0).max(40.0);
                let r = ui
                    .add_enabled_ui(dur > 0, |ui| {
                        progress(ui, &mut value, dur.max(1) as f32, width, pal.accent, "Seek")
                    })
                    .inner;
                if r.dragged() {
                    app.seek_drag = Some(value);
                }
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    app.seek_drag = None;
                    app.send(PlayerCmd::Seek(value as u32));
                }
                time(ui, fmt_ms(dur), Align2::LEFT_CENTER);
            });
        },
    );

    // ---------------------------------------------------- right controls
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(right)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.set_clip_rect(right);
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(8.0);
            if right.width() >= 250.0
                && widgets::icon_button(
                    ui,
                    Icon::Expand,
                    15.0,
                    "Full screen (F11)",
                    pal.weak,
                    pal.text,
                )
                .clicked()
            {
                app.set_fullscreen(true);
            }
            if right.width() >= 280.0
                && widgets::icon_button(
                    ui,
                    Icon::MiniPlayer,
                    16.0,
                    "Open Miniplayer",
                    pal.weak,
                    pal.text,
                )
                .clicked()
            {
                app.set_mini(true);
            }
            let mut vol = app.config.audio.volume as f32;
            let r = progress(
                ui,
                &mut vol,
                100.0,
                if right.width() < 230.0 { 60.0 } else { 93.0 },
                pal.accent,
                "Volume",
            );
            if r.changed() {
                app.set_volume(vol.round() as u8);
            }
            // Mouse wheel over the volume bar nudges it, 5% per notch.
            if r.hovered() {
                let dy: f32 = ui.input(|i| {
                    i.raw
                        .events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                            _ => None,
                        })
                        .sum()
                });
                if dy != 0.0 {
                    let step = if dy > 0.0 { 5 } else { -5 };
                    let v = (app.config.audio.volume as i32 + step).clamp(0, 100) as u8;
                    app.set_volume(v);
                    app.save_config();
                }
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                app.save_config();
            }
            let level = match app.config.audio.volume {
                0 => 0,
                1..=50 => 1,
                _ => 2,
            };
            if widgets::icon_button(
                ui,
                Icon::Volume(level),
                16.0,
                if level == 0 { "Unmute" } else { "Mute" },
                pal.weak,
                pal.text,
            )
            .clicked()
            {
                if vol > 0.0 {
                    app.volume_before_mute = vol.round() as u8;
                    app.set_volume(0);
                } else {
                    app.set_volume(app.volume_before_mute.max(10));
                }
                app.save_config();
            }
            let devices_tip = if snap.mode == Mode::Connect {
                format!(
                    "Controlled from {}",
                    snap.remote_client.as_deref().unwrap_or("Spotify Connect")
                )
            } else if app.connect_enabled {
                format!(
                    "Listening on this computer — visible as \"{}\" in Spotify Connect",
                    app.config.device_name
                )
            } else {
                "Listening on this computer".to_string()
            };
            let me = app.backend.as_ref().map(|b| b.api.device_id());
            let elsewhere = app
                .devices
                .iter()
                .any(|d| d.active && Some(&d.id) != me.as_ref());
            let dr = widgets::toggle_icon(
                ui,
                Icon::Devices,
                16.0,
                &devices_tip,
                snap.mode == Mode::Connect || elsewhere,
                pal.accent,
                pal.weak,
                pal.text,
            );
            egui::Popup::menu(&dr)
                .id(ui.id().with("devices-menu"))
                .align(egui::RectAlign::TOP_END)
                .gap(8.0)
                .show(|ui| device_picker(app, ui, me.as_deref()));
            let q = app.right == RightPanel::Queue;
            if widgets::toggle_icon(
                ui,
                Icon::Queue,
                16.0,
                "Queue",
                q,
                pal.accent,
                pal.weak,
                pal.text,
            )
            .clicked()
            {
                app.right = if q {
                    RightPanel::None
                } else {
                    RightPanel::Queue
                };
            }
            let l = app.view == super::View::Lyrics;
            if widgets::toggle_icon(
                ui,
                Icon::Mic,
                16.0,
                "Lyrics",
                l,
                pal.accent,
                pal.weak,
                pal.text,
            )
            .clicked()
            {
                if l {
                    app.go_back();
                } else {
                    app.navigate(super::View::Lyrics);
                    app.request_lyrics();
                }
            }
            if right.width() >= 210.0 {
                let np = app.right == RightPanel::NowPlaying;
                if widgets::toggle_icon(
                    ui,
                    Icon::Playing,
                    16.0,
                    "Now playing view",
                    np,
                    pal.accent,
                    pal.weak,
                    pal.text,
                )
                .clicked()
                {
                    app.right = if np {
                        RightPanel::None
                    } else {
                        RightPanel::NowPlaying
                    };
                }
            }
        },
    );
}

fn device_icon(kind: &str) -> Icon {
    match kind {
        "smartphone" | "tablet" => Icon::Phone,
        "speaker" | "avr" | "stb" | "audio_dongle" | "car" | "tv" | "cast_audio" | "cast_video" => {
            Icon::Volume(2)
        }
        _ => Icon::Devices,
    }
}

/// Spotify's "Connect to a device" popup.
fn device_picker(app: &mut App, ui: &mut egui::Ui, me: Option<&str>) {
    let pal = app.palette;
    widgets::menu_style(ui, 300.0);
    ui.set_max_width(320.0);
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Connect to a device").font(widgets::font(16.0, Weight::Bold)));
    ui.add_space(8.0);
    let devices = app.devices.clone();
    let active = devices.iter().find(|d| d.active).cloned();
    // Current device box.
    let (cur_name, cur_kind) = match &active {
        Some(d) if Some(d.id.as_str()) != me => (d.name.clone(), d.kind.clone()),
        _ => ("This computer".to_string(), "computer".to_string()),
    };
    egui::Frame::new()
        .fill(widgets::with_alpha(pal.accent, 0.12))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                widgets::draw(ui, r, device_icon(&cur_kind), pal.accent);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Current device")
                            .small()
                            .color(pal.weak),
                    );
                    ui.label(
                        egui::RichText::new(&cur_name)
                            .font(widgets::font(15.0, Weight::Bold))
                            .color(pal.accent),
                    );
                });
            });
        });
    ui.add_space(8.0);
    let others: Vec<_> = devices
        .iter()
        .filter(|d| !d.active && !(active.is_none() && Some(d.id.as_str()) == me))
        .cloned()
        .collect();
    if others.is_empty() {
        // Not in the menu's justified layout: that stretches the word spacing.
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.label(
                egui::RichText::new(
                    "No other devices found. Open Spotify on your phone, TV or speaker with the same account — it shows up here.",
                )
                .color(pal.weak),
            );
        });
    } else {
        ui.label(
            egui::RichText::new("Select another device")
                .small()
                .color(pal.weak),
        );
        for d in others {
            let label = if Some(d.id.as_str()) == me {
                "This computer (Lumen)".to_string()
            } else {
                d.name.clone()
            };
            let (rect, r) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
            if r.hovered() {
                ui.painter().rect_filled(rect, 4.0, pal.hover);
            }
            widgets::draw(
                ui,
                egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 20.0, rect.center().y),
                    Vec2::splat(20.0),
                ),
                device_icon(&d.kind),
                pal.text,
            );
            widgets::text(
                ui,
                egui::pos2(rect.left() + 44.0, rect.center().y),
                Align2::LEFT_CENTER,
                &label,
                14.0,
                Weight::Regular,
                pal.text,
                rect.width() - 52.0,
            );
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.transfer_to(&d);
                ui.close();
            }
        }
    }
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_accepts_pointer_keyboard_and_disabled_input() {
        let ctx = egui::Context::default();
        let mut value = 25.;
        let frame = |events: Vec<egui::Event>, enabled: bool, value: &mut f32| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320., 100.),
                )),
                events,
                ..Default::default()
            };
            let mut result = None;
            let mut output = ctx.run_ui(raw, |ui| {
                result = Some(
                    ui.add_enabled_ui(enabled, |ui| {
                        progress(ui, value, 100., 200., egui::Color32::GREEN, "Volume")
                    })
                    .inner,
                );
            });
            output.textures_delta.clear();
            result.expect("slider is rendered")
        };
        let initial = frame(vec![], true, &mut value);
        let point = egui::pos2(
            initial.rect.left() + initial.rect.width() * 0.75,
            initial.rect.center().y,
        );
        let button = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            vec![egui::Event::PointerMoved(point), button(true)],
            true,
            &mut value,
        );
        frame(vec![button(false)], true, &mut value);
        assert!((value - 75.).abs() < 0.01);
        ctx.memory_mut(|m| m.request_focus(initial.id));
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let response = frame(vec![key(egui::Key::ArrowRight)], true, &mut value);
        assert!(response.changed());
        assert!((value - 76.).abs() < 0.01);
        frame(vec![key(egui::Key::End)], true, &mut value);
        assert_eq!(value, 100.);
        frame(vec![key(egui::Key::Home)], false, &mut value);
        assert_eq!(value, 100.);
    }
    #[test]
    fn player_regions_are_centered_and_do_not_overlap() {
        for width in [696., 800., 1100., 1600.] {
            let bounds = egui::Rect::from_min_size(egui::pos2(12., 20.), egui::vec2(width, 64.));
            let [left, center, right] = regions(bounds);
            for r in [left, center, right] {
                assert!(bounds.contains_rect(r));
            }
            assert!(left.right() < center.left() && center.right() < right.left());
            assert_eq!(bounds.center(), center.center());
            assert!(center.width() >= 240.);
        }
    }
}
