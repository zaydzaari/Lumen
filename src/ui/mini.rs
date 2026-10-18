//! Mini player: the window shrinks to a small always-on-top card with the
//! cover, title, transport and a thin progress line (like Spotify's
//! "Open Miniplayer"). The full window comes back with the expand button,
//! Esc, or a double click on the cover.

use egui::{Align, Align2, Color32, Layout, Rect, Sense, UiBuilder, Vec2};

use super::art::LARGE_PX;
use super::widgets::{self, Icon, Weight};
use super::App;
use crate::player::PlayerCmd;

pub const SIZE: Vec2 = Vec2::new(340.0, 340.0);

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let full = ui.max_rect();
    let snap = app.snapshot.clone();
    let painter = ui.painter().clone();
    let track = snap.track.clone();
    let url = track
        .as_ref()
        .and_then(|t| t.image_large.clone().or(t.image.clone()));
    let color = super::panels::current_color(app).unwrap_or(Color32::from_rgb(0x53, 0x53, 0x53));
    painter.rect_filled(full, 0.0, Color32::BLACK);
    let tex = url.as_deref().and_then(|u| app.art.get_sized(u, LARGE_PX));
    widgets::image_cover(&painter, full, tex, 0.0, color);

    // The whole window drags (no title bar in this mode).
    let bg = ui.interact(full, ui.id().with("mini-bg"), Sense::click_and_drag());
    if bg.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if bg.double_clicked() {
        app.set_mini(false);
        return;
    }
    let hovered = ui.rect_contains_pointer(full);
    let h = ui
        .ctx()
        .animate_bool_with_time(ui.id().with("mini-h"), hovered, 0.18);
    // Controls fade in over a dark gradient while the pointer is on the window.
    let shade = 0.25 + 0.6 * h;
    widgets::gradient(
        &painter,
        full,
        0.0,
        Color32::from_black_alpha((90.0 * shade) as u8),
        Color32::from_black_alpha((250.0 * shade) as u8),
    );

    let white = Color32::WHITE;
    let dim = widgets::with_alpha(white, 0.75);
    // Top right: back to the full window.
    let exp = Rect::from_center_size(full.right_top() + Vec2::new(-24.0, 24.0), Vec2::splat(32.0));
    ui.scope_builder(UiBuilder::new().max_rect(exp), |ui| {
        ui.multiply_opacity(h);
        if widgets::icon_button(ui, Icon::Expand, 16.0, "Back to full window", dim, white).clicked()
        {
            app.set_mini(false);
        }
    });

    // Title / artist.
    let left = full.left() + 18.0;
    let w = full.width() - 36.0;
    if let Some(t) = &track {
        widgets::text(
            ui,
            egui::pos2(left, full.bottom() - 104.0),
            Align2::LEFT_CENTER,
            &t.name,
            17.0,
            Weight::Bold,
            white,
            w - 34.0,
        );
        widgets::text(
            ui,
            egui::pos2(left, full.bottom() - 82.0),
            Align2::LEFT_CENTER,
            &t.artists,
            13.0,
            Weight::Regular,
            dim,
            w - 34.0,
        );
        if !t.uri.starts_with("local:") {
            let saved = app.is_liked(&t.uri);
            let c = egui::pos2(full.right() - 28.0, full.bottom() - 94.0);
            if widgets::like_button(
                ui,
                c,
                20.0,
                ui.id().with("mini-like"),
                saved,
                app.palette.accent,
                dim,
                white,
            )
            .clicked()
            {
                let uri = t.uri.clone();
                app.toggle_liked(&uri);
            }
        }
    } else {
        widgets::text(
            ui,
            egui::pos2(left, full.bottom() - 94.0),
            Align2::LEFT_CENTER,
            "Nothing playing",
            16.0,
            Weight::Bold,
            dim,
            w,
        );
    }

    // Transport.
    let row = Rect::from_center_size(
        egui::pos2(full.center().x, full.bottom() - 42.0),
        Vec2::new(200.0, 48.0),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            ui.add_space(22.0);
            if widgets::icon_button(ui, Icon::Prev, 20.0, "Previous", dim, white).clicked() {
                app.send(PlayerCmd::Previous);
            }
            if widgets::round_play(
                ui,
                44.0,
                snap.playing,
                white,
                Color32::BLACK,
                if snap.playing { "Pause" } else { "Play" },
            )
            .clicked()
            {
                app.send(PlayerCmd::TogglePlay);
            }
            if widgets::icon_button(ui, Icon::Next, 20.0, "Next", dim, white).clicked() {
                app.send(PlayerCmd::Next);
            }
        },
    );

    // Progress line along the bottom edge.
    let dur = track.as_ref().map_or(0, |t| t.duration_ms).max(1);
    let pos = snap.position_now().min(dur);
    let line = Rect::from_min_max(
        egui::pos2(full.left(), full.bottom() - 4.0),
        full.right_bottom(),
    );
    painter.rect_filled(line, 0.0, widgets::with_alpha(white, 0.25));
    let mut done = line;
    done.set_right(line.left() + line.width() * pos as f32 / dur as f32);
    painter.rect_filled(done, 0.0, white);
    let seek = ui.interact(
        line.expand2(Vec2::new(0.0, 4.0)),
        ui.id().with("mini-seek"),
        Sense::click(),
    );
    if seek.clicked() {
        if let Some(p) = seek.interact_pointer_pos() {
            let f = ((p.x - line.left()) / line.width()).clamp(0.0, 1.0);
            app.send(PlayerCmd::Seek((f * dur as f32) as u32));
        }
    }

    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.set_mini(false);
    }
    if snap.playing {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
}
