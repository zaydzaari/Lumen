//! Right-hand panels (Now playing view, Queue, compact Lyrics) and the shared
//! lyrics renderer also used by the full-screen lyrics view.

use egui::{Align, Align2, Color32, Rect, Sense, Vec2};

use super::art::{LARGE_PX, THUMB_PX};
use super::widgets::{self, Icon, Weight};
use super::{App, LyricsState, RightPanel, View};
use crate::lyrics::{self, Lyrics};
use crate::model::{CollectionKind, TrackRef};
use crate::player::{Mode, PlayerCmd};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    match app.right {
        RightPanel::Queue => queue(app, ui),
        RightPanel::Lyrics => {
            let _ = panel_header(app, ui, "Lyrics");
            let color = current_color(app);
            lyrics_body(app, ui, false, color);
        }
        RightPanel::NowPlaying => now_playing(app, ui),
        RightPanel::Friends => friends(app, ui),
        RightPanel::None => {}
    }
}

/// Dominant colour of the current track's artwork (if loaded).
pub fn current_color(app: &mut App) -> Option<Color32> {
    let t = app.snapshot.track.clone()?;
    let url = t.image_large.as_deref().or(t.image.as_deref())?;
    // Make sure the large art gets loaded so its colour becomes available.
    let _ = app.art.get(url, LARGE_PX);
    app.art.color(url)
}

/// Panel title row with a close button; returns the title's response (clickable).
fn panel_header(app: &mut App, ui: &mut egui::Ui, title: &str) -> egui::Response {
    let pal = app.palette;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
    let tr = widgets::text(
        ui,
        rect.left_center(),
        Align2::LEFT_CENTER,
        title,
        16.0,
        Weight::Bold,
        pal.text,
        rect.width() - 40.0,
    );
    let title_resp = ui.interact(tr, ui.id().with(("title", title)), Sense::click());
    let close = Rect::from_center_size(
        rect.right_center() - Vec2::new(14.0, 0.0),
        Vec2::splat(28.0),
    );
    let r = ui.interact(close, ui.id().with(("close", title)), Sense::click());
    if r.hovered() {
        ui.painter().circle_filled(close.center(), 14.0, pal.panel);
    }
    widgets::draw(
        ui,
        Rect::from_center_size(close.center(), Vec2::splat(14.0)),
        Icon::Close,
        if r.hovered() { pal.text } else { pal.weak },
    );
    if r.on_hover_text("Close").clicked() {
        app.right = RightPanel::None;
    }
    ui.add_space(6.0);
    title_resp
}

/// Card with a rounded background.
fn card<R>(ui: &mut egui::Ui, fill: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn now_playing(app: &mut App, ui: &mut egui::Ui) {
    let snap = app.snapshot.clone();
    let pal = app.palette;
    let title = if snap.context_name.is_empty() {
        "Now playing".to_string()
    } else {
        snap.context_name.clone()
    };
    // "Playing from …": click to open that playlist / album, like Spotify.
    let tr = panel_header(app, ui, &title);
    if snap.context_key.is_some() {
        if tr.hovered() {
            ui.painter().hline(
                tr.rect.x_range(),
                tr.rect.bottom(),
                egui::Stroke::new(1.0, pal.text),
            );
        }
        if tr
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Go to what's playing")
            .clicked()
        {
            app.open_context();
        }
    }
    let Some(track) = snap.track.clone() else {
        ui.add_space(30.0);
        ui.label(
            egui::RichText::new("Play something you love").font(widgets::font(22.0, Weight::Bold)),
        );
        ui.label(
            egui::RichText::new("The song, its lyrics and what's next will show up here.")
                .color(pal.weak),
        );
        return;
    };
    let color = current_color(app);
    egui::ScrollArea::vertical()
        .id_salt("now-playing")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let w = ui.available_width();
            let (art, art_r) = ui.allocate_exact_size(Vec2::splat(w), Sense::click());
            super::views::track_context_menu(app, &art_r, &track);
            let tex = track
                .image_large
                .as_deref()
                .or(track.image.as_deref())
                .and_then(|u| app.art.get(u, LARGE_PX));
            widgets::image(ui.painter(), art, tex, 8.0, pal.panel, pal.weak);
            ui.add_space(14.0);
            let (row, _) = ui.allocate_exact_size(Vec2::new(w, 54.0), Sense::hover());
            let tr = widgets::text(
                ui,
                row.left_top() + Vec2::new(0.0, 14.0),
                Align2::LEFT_CENTER,
                track.display_name(),
                22.0,
                Weight::Bold,
                pal.text,
                w - 84.0,
            );
            let title_hit = ui.interact(tr, ui.id().with("np-panel-title"), Sense::click());
            super::views::track_context_menu(app, &title_hit, &track);
            if title_hit.clicked() {
                if let Some(a) = &track.album_uri {
                    app.open_uri(CollectionKind::Album, a, &track.album);
                }
            }
            let arr = widgets::text(
                ui,
                row.left_top() + Vec2::new(0.0, 42.0),
                Align2::LEFT_CENTER,
                &track.artists,
                15.0,
                Weight::Regular,
                pal.weak,
                w - 84.0,
            );
            let artist_hit = ui.interact(arr, ui.id().with("np-panel-artist"), Sense::click());
            super::views::track_context_menu(app, &artist_hit, &track);
            if artist_hit
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                if let Some(a) = &track.artist_uri {
                    app.open_uri(
                        CollectionKind::Artist,
                        a,
                        track.artists.split(", ").next().unwrap_or(""),
                    );
                }
            }
            if !track.uri.starts_with("local:") {
                let save = Rect::from_center_size(
                    row.right_top() + Vec2::new(-14.0, 26.0),
                    Vec2::splat(28.0),
                );
                let id = ui.id().with("np-panel-save");
                super::views::save_button(app, ui, save.center(), 20.0, id, &track);
            }
            // ⋯ with the whole song menu (copy link, add to playlist, credits…).
            let more = Rect::from_center_size(
                row.right_top()
                    + Vec2::new(
                        if track.uri.starts_with("local:") {
                            -14.0
                        } else {
                            -50.0
                        },
                        26.0,
                    ),
                Vec2::splat(30.0),
            );
            super::views::track_more_button(app, ui, more, &track);
            ui.add_space(14.0);

            // Lyrics preview card in the artwork colour.
            if app.lyrics.uri.as_deref() != Some(track.uri.as_str()) {
                app.request_lyrics();
            }
            let card_fill = color.unwrap_or(pal.panel);
            card(ui, card_fill, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Lyrics")
                            .font(widgets::font(16.0, Weight::Bold))
                            .color(Color32::WHITE),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let b = egui::Button::new(
                            egui::RichText::new("Show lyrics")
                                .font(widgets::font(13.0, Weight::Bold))
                                .color(Color32::BLACK),
                        )
                        .fill(Color32::WHITE)
                        .corner_radius(14);
                        if ui.add(b).clicked() {
                            app.navigate(View::Lyrics);
                        }
                    });
                });
                ui.add_space(8.0);
                lyrics_preview(app, ui, snap.position_now());
            });
            ui.add_space(12.0);

            // About the album.
            about_artist(app, ui, &track);
            credits_card(app, ui, &track);

            // Next in queue.
            card(ui, pal.panel, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Next in queue")
                            .font(widgets::font(16.0, Weight::Bold)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.link("Open queue").clicked() {
                            app.right = RightPanel::Queue;
                        }
                    });
                });
                ui.add_space(6.0);
                let next = snap
                    .queue
                    .first()
                    .map(|t| (t.clone(), true))
                    .or_else(|| snap.upcoming.first().map(|t| (t.clone(), false)));
                match next {
                    Some((t, from_queue)) => {
                        if track_line(app, ui, &t, false, None).clicked {
                            app.send(if from_queue {
                                PlayerCmd::PlayQueued(0)
                            } else {
                                PlayerCmd::PlayUpcoming(0)
                            });
                        }
                    }
                    None => {
                        ui.label(egui::RichText::new("Nothing queued").color(pal.weak));
                    }
                }
            });
            ui.add_space(8.0);
        });
}

fn lyrics_preview(app: &mut App, ui: &mut egui::Ui, pos: u32) {
    let w = ui.available_width();
    let font_size = 18.0;
    match &app.lyrics.state {
        LyricsState::Ready(Lyrics::Synced(lines)) => {
            let cur = lyrics::current_line(lines, pos).unwrap_or(0);
            let start = cur.saturating_sub(1);
            for (i, l) in lines.iter().enumerate().skip(start).take(4) {
                let text = if l.text.is_empty() {
                    "♪"
                } else {
                    l.text.as_str()
                };
                let color = if i == cur {
                    Color32::WHITE
                } else {
                    Color32::from_black_alpha(170)
                };
                let g = widgets::wrapped(ui, text, font_size, Weight::Bold, color, w);
                let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
                ui.painter().galley(r.min, g, color);
            }
        }
        LyricsState::Ready(Lyrics::Plain(text)) => {
            let first: String = text.lines().take(4).collect::<Vec<_>>().join("\n");
            let g = widgets::wrapped(ui, &first, font_size, Weight::Bold, Color32::WHITE, w);
            let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(r.min, g, Color32::WHITE);
        }
        LyricsState::Ready(Lyrics::NotFound) => {
            ui.label(egui::RichText::new("No lyrics for this one.").color(Color32::WHITE));
        }
        LyricsState::Error(_) => {
            ui.label(egui::RichText::new("Couldn't load lyrics.").color(Color32::WHITE));
        }
        LyricsState::Idle | LyricsState::Loading => {
            ui.spinner();
        }
    }
}

/// Result of a compact track line.
pub struct LineResult {
    pub clicked: bool,
    pub action: Option<QAction>,
}

#[derive(Clone, Copy)]
pub enum QAction {
    Play(bool, usize),
    Up(bool, usize),
    Down(bool, usize),
    Remove(bool, usize),
    /// Drag-and-drop move within the queue (true) or upcoming list.
    Move(bool, usize, usize),
}

/// Payload while dragging a queue row.
#[derive(Clone, Copy)]
struct QueueDrag {
    is_queue: bool,
    from: usize,
}

/// Compact row: 48px art, title, artist; optional hover actions for queue items.
fn track_line(
    app: &mut App,
    ui: &mut egui::Ui,
    t: &TrackRef,
    current: bool,
    queue_ctx: Option<(bool, usize, usize)>,
) -> LineResult {
    let pal = app.palette;
    let sense = if queue_ctx.is_some() {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), sense);
    // Right-click: the full song menu.
    super::views::track_context_menu(app, &r, t);
    let mut drop: Option<QAction> = None;
    if let Some((is_queue, i, _)) = queue_ctx {
        if r.drag_started() {
            r.dnd_set_drag_payload(QueueDrag { is_queue, from: i });
        }
        if let Some(p) = r.dnd_hover_payload::<QueueDrag>() {
            if p.is_queue == is_queue && p.from != i {
                let below = ui
                    .input(|q| q.pointer.hover_pos())
                    .is_some_and(|q| q.y > rect.center().y);
                let y = if below { rect.bottom() } else { rect.top() };
                ui.painter().hline(
                    rect.x_range(),
                    y,
                    egui::Stroke::new(2.0, app.palette.accent),
                );
            }
        }
        if let Some(p) = r.dnd_release_payload::<QueueDrag>() {
            if p.is_queue == is_queue && p.from != i {
                let below = ui
                    .input(|q| q.pointer.interact_pos())
                    .is_some_and(|q| q.y > rect.center().y);
                let mut to = if below { i + 1 } else { i };
                if to > p.from {
                    to -= 1;
                }
                drop = Some(QAction::Move(is_queue, p.from, to));
            }
        }
    }
    let hovered = r.hovered() || ui.rect_contains_pointer(rect);
    if hovered {
        ui.painter()
            .rect_filled(rect, 6.0, widgets::with_alpha(pal.text, 0.07));
    }
    let art = Rect::from_min_size(
        egui::pos2(rect.left() + 4.0, rect.center().y - 24.0),
        Vec2::splat(48.0),
    );
    let tex = t.image.as_deref().and_then(|u| app.art.get(u, THUMB_PX));
    widgets::image(ui.painter(), art, tex, 4.0, pal.hover, pal.weak);
    let actions_w = if queue_ctx.is_some() && hovered {
        84.0
    } else {
        0.0
    };
    let x = art.right() + 12.0;
    let w = rect.right() - x - actions_w - 6.0;
    widgets::text(
        ui,
        egui::pos2(x, rect.center().y - 9.0),
        Align2::LEFT_CENTER,
        t.display_name(),
        15.0,
        Weight::Semibold,
        if current { pal.accent } else { pal.text },
        w,
    );
    widgets::text(
        ui,
        egui::pos2(x, rect.center().y + 11.0),
        Align2::LEFT_CENTER,
        &t.artists,
        13.0,
        Weight::Regular,
        pal.weak,
        w,
    );
    let mut action = None;
    if let (Some((is_queue, i, len)), true) = (queue_ctx, hovered) {
        let mut x = rect.right() - 16.0;
        for (icon, act, enabled, tip) in [
            (
                Icon::Close,
                QAction::Remove(is_queue, i),
                true,
                "Remove from queue",
            ),
            (
                Icon::Back,
                QAction::Down(is_queue, i),
                i + 1 < len,
                "Move down",
            ),
            (Icon::Forward, QAction::Up(is_queue, i), i > 0, "Move up"),
        ] {
            let b = Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::splat(26.0));
            x -= 28.0;
            if !enabled {
                continue;
            }
            let br = ui.interact(b, ui.id().with((tip, is_queue, i)), Sense::click());
            let c = if br.hovered() { pal.text } else { pal.weak };
            // Up/down use rotated chevrons.
            let icon_rect = Rect::from_center_size(b.center(), Vec2::splat(14.0));
            match icon {
                Icon::Back => draw_chevron(ui, icon_rect, false, c),
                Icon::Forward => draw_chevron(ui, icon_rect, true, c),
                _ => widgets::draw(ui, icon_rect, icon, c),
            }
            if br.on_hover_text(tip).clicked() {
                action = Some(act);
            }
        }
    }
    let r = r.on_hover_text("Double-click to play · drag to reorder");
    LineResult {
        clicked: r.double_clicked() || (r.clicked() && queue_ctx.is_none()),
        action: drop.or(action),
    }
}

fn draw_chevron(ui: &egui::Ui, rect: Rect, up: bool, color: Color32) {
    let c = rect.center();
    let s = rect.width() / 2.0;
    let dy = if up { -s * 0.4 } else { s * 0.4 };
    ui.painter().add(egui::Shape::line(
        vec![
            c + Vec2::new(-s * 0.7, -dy),
            c + Vec2::new(0.0, dy),
            c + Vec2::new(s * 0.7, -dy),
        ],
        egui::Stroke::new(1.8, color),
    ));
}

fn queue(app: &mut App, ui: &mut egui::Ui) {
    let snap = app.snapshot.clone();
    let pal = app.palette;
    let _ = panel_header(app, ui, "Queue");
    if snap.mode == Mode::Connect {
        ui.label(
            egui::RichText::new(
                "Playback is controlled via Spotify Connect; the queue lives on the controlling device.",
            )
            .color(pal.weak),
        );
    }
    let mut action: Option<QAction> = None;
    egui::ScrollArea::vertical()
        .id_salt("queue-panel")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let section = |ui: &mut egui::Ui, s: &str| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(s).font(widgets::font(16.0, Weight::Bold)));
                ui.add_space(4.0);
            };
            section(ui, "Now playing");
            match &snap.track {
                Some(t) => {
                    track_line(app, ui, t, true, None);
                }
                None => {
                    ui.label(egui::RichText::new("Nothing playing").color(pal.weak));
                }
            }
            if !snap.queue.is_empty() {
                section(ui, "Next in queue");
                let head = ui.min_rect();
                let link = egui::Rect::from_min_max(
                    egui::pos2(head.right() - 90.0, ui.cursor().top() - 28.0),
                    egui::pos2(head.right(), ui.cursor().top() - 6.0),
                );
                if ui
                    .put(
                        link,
                        egui::Link::new(egui::RichText::new("Clear queue").color(pal.weak)),
                    )
                    .clicked()
                {
                    app.send(PlayerCmd::ClearQueue);
                }
                for (i, t) in snap.queue.iter().enumerate() {
                    let r = track_line(app, ui, t, false, Some((true, i, snap.queue.len())));
                    if r.clicked {
                        action = Some(QAction::Play(true, i));
                    }
                    if r.action.is_some() {
                        action = r.action;
                    }
                }
            }
            if !snap.upcoming.is_empty() {
                section(
                    ui,
                    &format!(
                        "Next from: {}",
                        if snap.context_name.is_empty() {
                            "your music"
                        } else {
                            &snap.context_name
                        }
                    ),
                );
                for (i, t) in snap.upcoming.iter().enumerate() {
                    let r = track_line(app, ui, t, false, Some((false, i, snap.upcoming_total)));
                    if r.clicked {
                        action = Some(QAction::Play(false, i));
                    }
                    if r.action.is_some() {
                        action = r.action;
                    }
                }
                if snap.upcoming_total > snap.upcoming.len() {
                    ui.label(
                        egui::RichText::new(format!(
                            "…and {} more",
                            snap.upcoming_total - snap.upcoming.len()
                        ))
                        .color(pal.weak),
                    );
                }
            }
            if snap.queue.is_empty() && snap.upcoming.is_empty() {
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new("Right-click any song and choose \"Add to queue\".")
                        .color(pal.weak),
                );
            }
        });
    if let Some(a) = action {
        let cmd = match a {
            QAction::Play(true, i) => PlayerCmd::PlayQueued(i),
            QAction::Play(false, i) => PlayerCmd::PlayUpcoming(i),
            QAction::Up(true, i) => PlayerCmd::QueueMove { from: i, to: i - 1 },
            QAction::Up(false, i) => PlayerCmd::UpcomingMove { from: i, to: i - 1 },
            QAction::Down(true, i) => PlayerCmd::QueueMove { from: i, to: i + 1 },
            QAction::Down(false, i) => PlayerCmd::UpcomingMove { from: i, to: i + 1 },
            QAction::Remove(true, i) => PlayerCmd::QueueRemove(i),
            QAction::Remove(false, i) => PlayerCmd::UpcomingRemove(i),
            QAction::Move(true, from, to) => PlayerCmd::QueueMove { from, to },
            QAction::Move(false, from, to) => PlayerCmd::UpcomingMove { from, to },
        };
        app.send(cmd);
    }
}

/// Lyrics body: big bold lines; sung lines dim, the current line white, upcoming
/// lines darker. Click a line to seek there. `full` = full-screen size.
pub fn lyrics_body(app: &mut App, ui: &mut egui::Ui, full: bool, bg: Option<Color32>) {
    let snap = app.snapshot.clone();
    let pal = app.palette;
    let Some(track) = snap.track.clone() else {
        ui.label(egui::RichText::new("Play something to see its lyrics.").color(pal.weak));
        return;
    };
    if app.lyrics.uri.as_deref() != Some(track.uri.as_str()) {
        app.request_lyrics();
    }
    let size = if full { 30.0 } else { 19.0 };
    let gap = if full { 14.0 } else { 8.0 };
    let colored = bg.is_some();
    let cur_col = Color32::WHITE;
    let past_col = if colored {
        widgets::with_alpha(Color32::WHITE, 0.62)
    } else {
        pal.weak
    };
    let next_col = if colored {
        Color32::from_black_alpha(150)
    } else {
        widgets::with_alpha(pal.text, 0.45)
    };
    let pos = snap.position_now();
    let mut seek_to = None;
    match &app.lyrics.state {
        LyricsState::Idle | LyricsState::Loading => {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.spinner());
        }
        LyricsState::Error(e) => {
            ui.add_space(30.0);
            ui.label(
                egui::RichText::new("Couldn't load lyrics")
                    .font(widgets::font(size, Weight::Bold))
                    .color(cur_col),
            );
            ui.label(egui::RichText::new(e).color(past_col));
            if ui.button("Try again").clicked() {
                app.lyrics.uri = None;
                app.request_lyrics();
            }
        }
        LyricsState::Ready(Lyrics::NotFound) => {
            ui.add_space(40.0);
            ui.label(
                egui::RichText::new("Hmm. We don't know the lyrics for this one.")
                    .font(widgets::font(if full { 28.0 } else { 18.0 }, Weight::Bold))
                    .color(cur_col),
            );
            ui.label(
                egui::RichText::new("We looked on Spotify and in the LRCLIB community database.")
                    .color(past_col),
            );
        }
        LyricsState::Ready(Lyrics::Plain(text)) => {
            ui.label(
                egui::RichText::new("These lyrics aren't synced to the song yet.").color(past_col),
            );
            ui.add_space(gap);
            egui::ScrollArea::vertical()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .id_salt(("lyrics-plain", full))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let w = ui
                        .available_width()
                        .min(if full { 900.0 } else { f32::INFINITY });
                    for line in text.lines() {
                        let s = if line.trim().is_empty() { " " } else { line };
                        let g = widgets::wrapped(ui, s, size, Weight::Bold, cur_col, w);
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::new(w, g.size().y), Sense::hover());
                        ui.painter().galley(r.min, g, cur_col);
                        ui.add_space(gap * 0.5);
                    }
                    ui.add_space(80.0);
                });
        }
        LyricsState::Ready(Lyrics::Synced(lines)) => {
            let current = lyrics::current_line(lines, pos);
            let scroll_needed = current != app.lyrics.last_line;
            let lines = lines.clone();
            egui::ScrollArea::vertical()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .id_salt(("lyrics-synced", full))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let w = ui
                        .available_width()
                        .min(if full { 900.0 } else { f32::INFINITY });
                    ui.add_space(ui.available_height() * 0.25);
                    for (i, line) in lines.iter().enumerate() {
                        let is_cur = Some(i) == current;
                        let past = current.is_some_and(|c| i < c);
                        let text = if line.text.is_empty() {
                            "♪"
                        } else {
                            line.text.as_str()
                        };
                        // Placeholder colour: one cached layout per line, tinted at paint time.
                        let g =
                            widgets::wrapped(ui, text, size, Weight::Bold, Color32::PLACEHOLDER, w);
                        let (r, resp) =
                            ui.allocate_exact_size(Vec2::new(w, g.size().y + gap), Sense::click());
                        let hover = resp.hovered();
                        // The active line brightens (and the old one dims) smoothly.
                        let k = if app.config.ui.animations {
                            ui.ctx().animate_bool_with_time(
                                egui::Id::new(("lyr", full, i)),
                                is_cur,
                                0.3,
                            )
                        } else if is_cur {
                            1.0
                        } else {
                            0.0
                        };
                        let base_col = if past || is_cur { past_col } else { next_col };
                        let mut paint_col = widgets::mix(base_col, cur_col, k);
                        if hover && !is_cur {
                            paint_col = cur_col;
                        }
                        ui.painter().galley(r.min, g, paint_col);
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            seek_to = Some(line.time_ms);
                        }
                        if is_cur && scroll_needed {
                            ui.scroll_to_rect_animation(
                                r,
                                Some(Align::Center),
                                egui::style::ScrollAnimation::duration(0.45),
                            );
                        }
                    }
                    ui.add_space(ui.available_height().max(240.0));
                });
            app.lyrics.last_line = current;
        }
    }
    if let Some(ms) = seek_to {
        app.send(PlayerCmd::Seek(ms));
    }
}

/// Spotify's "About the artist" card: picture with the label on it, name,
/// a few lines of biography and Follow.
fn about_artist(app: &mut App, ui: &mut egui::Ui, track: &crate::model::TrackRef) {
    let pal = app.palette;
    let Some(uri) = track.artist_uri.clone() else {
        return;
    };
    let name = track.artists.split(", ").next().unwrap_or("").to_string();
    app.load_artist_info(&uri);
    let info = app
        .artist_info
        .get(&uri)
        .cloned()
        .flatten()
        .and_then(|r| r.ok());
    let w = ui.available_width();
    let pic_h = (w * 0.62).round();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, pic_h), Sense::click());
    let url = info
        .as_ref()
        .and_then(|i| i.portrait.clone())
        .or_else(|| track.image_large.clone().or(track.image.clone()));
    let tex = url.as_deref().and_then(|u| app.art.get_sized(u, LARGE_PX));
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        },
        pal.panel,
    );
    let clip = ui.painter().with_clip_rect(rect);
    widgets::image_cover(&clip, rect, tex, 8.0, pal.panel);
    widgets::gradient(
        ui.painter(),
        rect,
        8.0,
        Color32::from_black_alpha(140),
        Color32::from_black_alpha(0),
    );
    widgets::text(
        ui,
        rect.left_top() + Vec2::new(14.0, 20.0),
        Align2::LEFT_CENTER,
        "About the artist",
        15.0,
        Weight::Bold,
        Color32::WHITE,
        w - 28.0,
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        app.open_uri(CollectionKind::Artist, &uri, &name);
    }
    egui::Frame::new()
        .fill(pal.panel)
        .corner_radius(egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: 8,
            se: 8,
        })
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(w - 28.0);
            ui.horizontal(|ui| {
                let n = ui.add(
                    egui::Label::new(
                        egui::RichText::new(&name).font(widgets::font(16.0, Weight::Bold)),
                    )
                    .truncate()
                    .sense(Sense::click()),
                );
                if n.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    app.open_uri(CollectionKind::Artist, &uri, &name);
                }
                if app.backend.is_some() || app.demo {
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let following = app.is_following(&uri);
                        let b = egui::Button::new(
                            egui::RichText::new(if following { "Unfollow" } else { "Follow" })
                                .font(widgets::font(13.0, Weight::Bold))
                                .color(pal.text),
                        )
                        .fill(Color32::TRANSPARENT)
                        .stroke(egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.5)))
                        .corner_radius(16);
                        if ui.add(b).clicked() {
                            app.toggle_follow(&uri);
                        }
                    });
                }
            });
            if let Some(i) = &info {
                if i.popularity > 0 {
                    ui.label(
                        egui::RichText::new(format!("Popularity {} / 100", i.popularity))
                            .small()
                            .color(pal.weak),
                    );
                }
                if let Some(bio) = &i.bio {
                    ui.add_space(4.0);
                    let short: String = bio.chars().take(170).collect();
                    let more = bio.chars().count() > 170;
                    let r = ui.add(
                        egui::Label::new(
                            egui::RichText::new(if more { format!("{short}…") } else { short })
                                .color(pal.weak),
                        )
                        .sense(Sense::click()),
                    );
                    if more
                        && r.on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Read more")
                            .clicked()
                    {
                        app.about_open = Some((name.clone(), bio.clone()));
                    }
                }
            }
        });
    ui.add_space(12.0);
}

/// "Credits": the first few people, and Show all for the full dialog.
fn credits_card(app: &mut App, ui: &mut egui::Ui, track: &crate::model::TrackRef) {
    if track.uri.starts_with("local:") {
        return;
    }
    let pal = app.palette;
    app.load_track_credits(&track.uri);
    let credits = app.track_credits.get(&track.uri).cloned().flatten();
    card(ui, pal.panel, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Credits").font(widgets::font(16.0, Weight::Bold)));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if ui.link("Show all").clicked() {
                    app.show_credits(track);
                }
            });
        });
        ui.add_space(4.0);
        let people: Vec<(String, String)> = match &credits {
            Some(Ok(c)) => c
                .roles
                .iter()
                .flat_map(|(role, ps)| {
                    ps.iter().map(move |p| {
                        let r = if p.roles.is_empty() {
                            role.clone()
                        } else {
                            p.roles.clone()
                        };
                        (p.name.clone(), r)
                    })
                })
                .take(3)
                .collect(),
            _ => track
                .artists
                .split(", ")
                .take(3)
                .map(|a| (a.to_string(), "Main Artist".to_string()))
                .collect(),
        };
        for (name, role) in people {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(name).font(widgets::font(15.0, Weight::Semibold)));
            ui.label(egui::RichText::new(role).small().color(pal.weak));
        }
    });
    ui.add_space(12.0);
}

/// "x min" / "x hr" / "x d" since a Unix-ms timestamp (Spotify's friend times).
fn ago(ts: i64) -> String {
    let mins = ((chrono::Local::now().timestamp_millis() - ts) / 60_000).max(0);
    match mins {
        0..=2 => String::new(),
        3..=59 => format!("{mins} min"),
        60..=1439 => format!("{} hr", mins / 60),
        _ => format!("{} d", mins / 1440),
    }
}

/// Spotify's Friend Activity panel.
fn friends(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    let _ = panel_header(app, ui, "Friend Activity");
    app.load_friends(false);
    let list = match app.friends.clone() {
        None => {
            widgets::skeleton_rows(ui, 4, pal.text);
            return;
        }
        Some(Err(_)) => {
            ui.add_space(16.0);
            ui.label(
                egui::RichText::new("Friend Activity isn't available")
                    .font(widgets::font(16.0, Weight::Bold)),
            );
            ui.label(
                egui::RichText::new(
                    "Spotify didn't share it with Lumen for this login. It shows in the official app when you follow friends who share what they play.",
                )
                .color(pal.weak),
            );
            if ui.button("Try again").clicked() {
                app.load_friends(true);
            }
            return;
        }
        Some(Ok(v)) => v,
    };
    if list.is_empty() {
        ui.add_space(16.0);
        ui.label(
            egui::RichText::new(
                "Let friends and followers on Spotify see what you're listening to.",
            )
            .color(pal.weak),
        );
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Follow friends in Spotify and turn on \"Share my listening activity\" there; their music shows up here.").small().color(pal.weak));
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("friends")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in &list {
                let w = ui.available_width();
                let (row, r) = ui.allocate_exact_size(Vec2::new(w, 76.0), Sense::click());
                if r.hovered() {
                    ui.painter()
                        .rect_filled(row, 6.0, widgets::with_alpha(pal.text, 0.06));
                }
                let pic = Rect::from_min_size(row.min + Vec2::new(6.0, 10.0), Vec2::splat(42.0));
                let tex = f
                    .image
                    .as_deref()
                    .and_then(|u| app.art.get(u, super::art::THUMB_PX));
                if tex.is_some() {
                    widgets::image(ui.painter(), pic, tex, 21.0, pal.panel, pal.weak);
                } else {
                    ui.painter().circle_filled(pic.center(), 21.0, pal.hover);
                    widgets::draw(
                        ui,
                        Rect::from_center_size(pic.center(), Vec2::splat(20.0)),
                        Icon::User,
                        pal.weak,
                    );
                }
                let when = ago(f.ts);
                let live = when.is_empty();
                if live {
                    // Playing now: Spotify's little equaliser instead of a time.
                    widgets::eq_bars(
                        ui,
                        Rect::from_center_size(
                            egui::pos2(row.right() - 18.0, row.top() + 22.0),
                            Vec2::splat(14.0),
                        ),
                        pal.accent,
                        false,
                    );
                } else {
                    widgets::text(
                        ui,
                        egui::pos2(row.right() - 8.0, row.top() + 22.0),
                        Align2::RIGHT_CENTER,
                        &when,
                        12.0,
                        Weight::Regular,
                        pal.weak,
                        60.0,
                    );
                }
                let x = pic.right() + 12.0;
                let tw = row.right() - x - 52.0;
                widgets::text(
                    ui,
                    egui::pos2(x, row.top() + 20.0),
                    Align2::LEFT_CENTER,
                    &f.name,
                    14.0,
                    Weight::Bold,
                    pal.text,
                    tw,
                );
                widgets::text(
                    ui,
                    egui::pos2(x, row.top() + 40.0),
                    Align2::LEFT_CENTER,
                    &format!("{} • {}", f.track, f.artist),
                    13.0,
                    Weight::Regular,
                    pal.weak,
                    row.right() - x - 8.0,
                );
                if !f.context.is_empty() {
                    widgets::draw(
                        ui,
                        Rect::from_center_size(
                            egui::pos2(x + 6.0, row.top() + 59.0),
                            Vec2::splat(12.0),
                        ),
                        Icon::Note,
                        pal.weak,
                    );
                    widgets::text(
                        ui,
                        egui::pos2(x + 16.0, row.top() + 59.0),
                        Align2::LEFT_CENTER,
                        &f.context,
                        12.0,
                        Weight::Regular,
                        pal.weak,
                        row.right() - x - 24.0,
                    );
                }
                let r = r
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Play this song");
                if r.double_clicked() || r.clicked() {
                    let t = std::sync::Arc::new(crate::model::Track {
                        uri: f.track_uri.clone(),
                        name: f.track.clone(),
                        artists: f.artist.clone(),
                        artist_uri: f.artist_uri.clone(),
                        ..Default::default()
                    });
                    app.play_tracks(&format!("{}'s music", f.name), vec![t], 0);
                }
            }
        });
}
