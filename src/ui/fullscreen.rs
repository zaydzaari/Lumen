//! Full-screen "Now playing" mode (Spotify's full-screen view): blurred
//! artwork backdrop, big cover and title, the current lyric line, and the
//! transport controls along the bottom. Esc or F11 leaves it.

use egui::{Align, Align2, Color32, Layout, Rect, Sense, UiBuilder, Vec2};

use super::art::{LARGE_PX, THUMB_PX};
use super::widgets::{self, Icon, Weight};
use super::{App, LyricsState};
use crate::lyrics::{self, Lyrics};
use crate::model::fmt_ms;
use crate::player::{PlayerCmd, Repeat};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let full = ui.max_rect();
    let snap = app.snapshot.clone();
    let painter = ui.painter().clone();
    let k = app
        .fullscreen_at
        .map(|t| (t.elapsed().as_secs_f32() / 0.35).min(1.0))
        .unwrap_or(1.0);
    if k < 1.0 {
        ui.ctx().request_repaint();
    }
    let e = 1.0 - (1.0 - k).powi(3);

    // ------------------------------------------------ backdrop
    painter.rect_filled(full, 0.0, Color32::BLACK);
    let track = snap.track.clone();
    let url = track
        .as_ref()
        .and_then(|t| t.image_large.clone().or(t.image.clone()));
    let color = super::panels::current_color(app).unwrap_or(Color32::from_rgb(0x53, 0x53, 0x53));
    if let Some(u) = &url {
        // A 96 px thumbnail stretched over the screen is naturally soft — a
        // free "blur" without any shader work.
        let tex = app.art.get_sized(u, THUMB_PX);
        let bg = full.expand(80.0 * (1.0 - e));
        widgets::image_cover(&painter, bg, tex, 0.0, color);
    }
    widgets::gradient(
        &painter,
        full,
        0.0,
        widgets::with_alpha(widgets::mix(color, Color32::BLACK, 0.35), 0.72),
        Color32::from_black_alpha(235),
    );
    ui.multiply_opacity(e);

    // ------------------------------------------------ top bar
    let top = full.top() + 36.0;
    painter.circle_filled(
        egui::pos2(full.left() + 60.0, top),
        14.0,
        app.palette.accent,
    );
    widgets::draw(
        ui,
        Rect::from_center_size(egui::pos2(full.left() + 61.0, top), Vec2::splat(13.0)),
        Icon::Play,
        app.palette.on_accent,
    );
    if !snap.context_name.is_empty() {
        widgets::text(
            ui,
            egui::pos2(full.left() + 86.0, top - 8.0),
            Align2::LEFT_CENTER,
            "PLAYING FROM",
            11.0,
            Weight::Bold,
            widgets::with_alpha(Color32::WHITE, 0.7),
            400.0,
        );
        widgets::text(
            ui,
            egui::pos2(full.left() + 86.0, top + 9.0),
            Align2::LEFT_CENTER,
            &snap.context_name,
            14.0,
            Weight::Bold,
            Color32::WHITE,
            full.width() * 0.5,
        );
    }
    let close = Rect::from_center_size(egui::pos2(full.right() - 56.0, top), Vec2::splat(40.0));
    let cr = ui.interact(close, ui.id().with("fs-close"), Sense::click());
    painter.circle_filled(
        close.center(),
        20.0,
        widgets::with_alpha(Color32::WHITE, if cr.hovered() { 0.2 } else { 0.1 }),
    );
    widgets::draw(
        ui,
        Rect::from_center_size(close.center(), Vec2::splat(16.0)),
        Icon::Collapse,
        Color32::WHITE,
    );
    if cr.on_hover_text("Exit full screen (Esc)").clicked() {
        app.set_fullscreen(false);
        return;
    }

    // ------------------------------------------------ cover + title
    let margin = 60.0;
    let controls_h = 150.0;
    let cover = (full.height() * 0.42)
        .min(full.width() * 0.3)
        .clamp(160.0, 440.0);
    let cover_rect = Rect::from_min_size(
        egui::pos2(
            full.left() + margin,
            full.bottom() - controls_h - cover - 20.0 * (1.0 - e),
        ),
        Vec2::splat(cover),
    );
    widgets::shadow(&painter, cover_rect, 8.0);
    let tex = url.as_deref().and_then(|u| app.art.get(u, LARGE_PX * 2));
    widgets::image(
        &painter,
        cover_rect,
        tex,
        8.0,
        app.palette.panel,
        app.palette.weak,
    );
    let tx = cover_rect.right() + 36.0;
    let tw = full.right() - margin - tx;
    if let Some(t) = &track {
        let title = t.display_name();
        let size = fit(ui, title, tw, &[72.0, 60.0, 48.0, 38.0, 30.0]);
        widgets::text(
            ui,
            egui::pos2(tx, cover_rect.bottom() - 34.0),
            Align2::LEFT_BOTTOM,
            title,
            size,
            Weight::Black,
            Color32::WHITE,
            tw,
        );
        widgets::text(
            ui,
            egui::pos2(tx, cover_rect.bottom() - 4.0),
            Align2::LEFT_BOTTOM,
            &t.artists,
            22.0,
            Weight::Semibold,
            widgets::with_alpha(Color32::WHITE, 0.75),
            tw,
        );
    }

    // ------------------------------------------------ current lyric line
    if let LyricsState::Ready(Lyrics::Synced(lines)) = &app.lyrics.state {
        if let Some(i) = lyrics::current_line(lines, snap.position_now()) {
            let area_top = top + 60.0;
            let area_bottom = cover_rect.top() - 30.0;
            if area_bottom - area_top > 80.0 {
                let w = full.width() - 2.0 * margin;
                let cur = lines[i].text.trim();
                let cur = if cur.is_empty() { "♪" } else { cur };
                let g = widgets::wrapped(ui, cur, 40.0, Weight::Bold, Color32::WHITE, w);
                let y = area_bottom - g.size().y - 44.0;
                if y > area_top {
                    painter.galley(egui::pos2(full.left() + margin, y), g, Color32::WHITE);
                    if let Some(next) = lines.get(i + 1) {
                        widgets::text(
                            ui,
                            egui::pos2(full.left() + margin, area_bottom - 16.0),
                            Align2::LEFT_CENTER,
                            next.text.trim(),
                            24.0,
                            Weight::Bold,
                            widgets::with_alpha(Color32::WHITE, 0.45),
                            w,
                        );
                    }
                }
            }
        }
    } else if app.lyrics.uri.as_deref() != track.as_ref().map(|t| t.uri.as_str()) {
        app.request_lyrics();
    }

    // ------------------------------------------------ progress
    let dur = track.as_ref().map_or(0, |t| t.duration_ms);
    let pos = app
        .seek_drag
        .map(|v| v as u32)
        .unwrap_or_else(|| snap.position_now());
    let bar_y = full.bottom() - controls_h + 34.0;
    let bar = Rect::from_min_max(
        egui::pos2(full.left() + margin + 56.0, bar_y - 9.0),
        egui::pos2(full.right() - margin - 56.0, bar_y + 9.0),
    );
    widgets::text(
        ui,
        egui::pos2(bar.left() - 14.0, bar_y),
        Align2::RIGHT_CENTER,
        &fmt_ms(pos),
        13.0,
        Weight::Regular,
        Color32::WHITE,
        60.0,
    );
    widgets::text(
        ui,
        egui::pos2(bar.right() + 14.0, bar_y),
        Align2::LEFT_CENTER,
        &fmt_ms(dur),
        13.0,
        Weight::Regular,
        Color32::WHITE,
        60.0,
    );
    let mut value = app.seek_drag.unwrap_or(pos as f32);
    let r = ui
        .scope_builder(UiBuilder::new().max_rect(bar), |ui| {
            ui.visuals_mut().override_text_color = Some(Color32::WHITE);
            super::player_bar::progress(
                ui,
                &mut value,
                dur.max(1) as f32,
                bar.width(),
                Color32::WHITE,
                "Seek",
            )
        })
        .inner;
    if r.dragged() {
        app.seek_drag = Some(value);
    }
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        app.seek_drag = None;
        app.send(PlayerCmd::Seek(value as u32));
    }

    // ------------------------------------------------ transport
    let row = Rect::from_center_size(
        egui::pos2(full.center().x, full.bottom() - controls_h + 92.0),
        Vec2::new(360.0, 64.0),
    );
    let white = Color32::WHITE;
    let dim = widgets::with_alpha(Color32::WHITE, 0.7);
    let accent = app.palette.accent;
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 22.0;
            ui.add_space(14.0);
            if widgets::toggle_icon(
                ui,
                Icon::Shuffle,
                22.0,
                "Shuffle",
                snap.shuffle,
                accent,
                dim,
                white,
            )
            .clicked()
            {
                app.send(PlayerCmd::SetShuffle(!snap.shuffle));
            }
            if widgets::icon_button(ui, Icon::Prev, 24.0, "Previous", white, white).clicked() {
                app.send(PlayerCmd::Previous);
            }
            if widgets::round_play(
                ui,
                60.0,
                snap.playing,
                white,
                Color32::BLACK,
                if snap.playing { "Pause" } else { "Play" },
            )
            .clicked()
            {
                app.send(PlayerCmd::TogglePlay);
            }
            if widgets::icon_button(ui, Icon::Next, 24.0, "Next", white, white).clicked() {
                app.send(PlayerCmd::Next);
            }
            let icon = if snap.repeat == Repeat::One {
                Icon::RepeatOne
            } else {
                Icon::Repeat
            };
            if widgets::toggle_icon(
                ui,
                icon,
                22.0,
                "Repeat",
                snap.repeat != Repeat::Off,
                accent,
                dim,
                white,
            )
            .clicked()
            {
                app.send(PlayerCmd::SetRepeat(snap.repeat.cycle()));
            }
        },
    );
    // Save button bottom-left, like Spotify.
    if let Some(t) = &track {
        if !t.uri.starts_with("local:") {
            let saved = app.is_liked(&t.uri);
            let c = egui::pos2(full.left() + margin + 12.0, row.center().y);
            if widgets::like_button(
                ui,
                c,
                24.0,
                ui.id().with("fs-like"),
                saved,
                accent,
                dim,
                white,
            )
            .clicked()
            {
                let uri = t.uri.clone();
                app.toggle_liked(&uri);
            }
        }
    }

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.set_fullscreen(false);
    }
}

/// Largest of `sizes` at which `text` fits on one line in `width`.
fn fit(ui: &egui::Ui, text: &str, width: f32, sizes: &[f32]) -> f32 {
    let w20 = widgets::galley(ui, text, 20.0, Weight::Black, Color32::WHITE, f32::INFINITY)
        .size()
        .x;
    for &s in sizes {
        if w20 * s / 20.0 <= width {
            return s;
        }
    }
    *sizes.last().unwrap_or(&30.0)
}
