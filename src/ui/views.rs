//! Central views in a Spotify-style layout: home (shortcuts + shelves), playlist /
//! album pages with a colour-gradient header, artist pages, collection grids,
//! search, local files and the full-screen lyrics view.

use std::collections::HashSet;

use egui::{Align2, Color32, Rect, Sense, Vec2};

use super::art::{LARGE_PX, THUMB_PX};
use super::widgets::{self, Icon, Weight};
use super::{App, View};
use crate::events::ListKey;
use crate::model::{fmt_ms, Collection, CollectionKind, Track, TrackRef};
use crate::player::PlayerCmd;

pub const ROW_H: f32 = 56.0;
const PAD: f32 = 24.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    match app.view.clone() {
        View::Home => home(app, ui),
        View::Search => search(app, ui),
        View::Liked => track_page(app, ui, Source::List(ListKey::Liked)),
        View::Playlists => grid_page(app, ui, CollectionKind::Playlist),
        View::Albums => grid_page(app, ui, CollectionKind::Album),
        View::Artists => grid_page(app, ui, CollectionKind::Artist),
        View::Collection(c) if c.kind == CollectionKind::Artist => artist_page(app, ui, &c),
        View::Collection(c) => {
            track_page(app, ui, Source::List(ListKey::Collection(c.uri.clone())))
        }
        View::Settings => {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 32,
                    right: 32,
                    top: 20,
                    bottom: 0,
                })
                .show(ui, |ui| super::settings::show(app, ui));
        }
        View::Local => track_page(app, ui, Source::Local),
        View::Lyrics => lyrics_page(app, ui),
        View::Themes => super::themes_view::show(app, ui),
        View::Profile(u) => super::profile_view::show(app, ui, &u),
        View::Stats => super::stats_view::show(app, ui),
    }
}

// ====================================================================== rows

/// What the user asked for on a track row.
#[derive(Clone)]
pub enum RowAction {
    Play,
    Queue,
    Album,
    Artist,
    Save,
    OpenSpotify,
    /// Link already copied by the menu; just confirm with a toast.
    Copied,
    /// Add to an existing playlist (uri, name).
    AddTo(String, String),
    /// Create a new playlist containing this song.
    NewPlaylist,
    /// Remove from the owned playlist being shown (its uri).
    RemoveFrom(String),
    /// Spotify radio based on this song.
    Radio,
    /// Drag-and-drop reorder within the owned playlist (uri, from, to).
    Move(String, usize, usize),
    /// Songwriters, producers and performers.
    Credits,
}

fn row_menu_items(app: &App, ui: &mut egui::Ui, t: &Track, action: &mut Option<RowAction>) {
    track_menu_items(app, ui, t, action, app.row_playlist.as_deref(), true);
}

/// Spotify's song menu. `playlist` is your own playlist the song is shown in
/// (enables "Remove from this playlist"); `play` adds a Play item.
pub(super) fn track_menu_items(
    app: &App,
    ui: &mut egui::Ui,
    t: &Track,
    action: &mut Option<RowAction>,
    playlist: Option<&str>,
    play: bool,
) {
    widgets::menu_style(ui, 230.0);
    // Acting on a multi-selection: the actions below apply to every selected song.
    let multi = if app.row_hl {
        app.multi_tracks.len()
    } else {
        0
    };
    if multi > 1 {
        ui.label(
            egui::RichText::new(format!("{multi} songs selected"))
                .small()
                .color(app.palette.weak),
        );
    }
    if play {
        pick(ui, "Play", RowAction::Play, action);
    }
    let spotify = !t.uri.starts_with("local:");
    if spotify {
        ui.menu_button("Add to playlist", |ui| {
            widgets::menu_style(ui, 230.0);
            if ui.button("+  New playlist").clicked() {
                *action = Some(RowAction::NewPlaylist);
                ui.close();
            }
            let mine = app.owned_playlists();
            if !mine.is_empty() {
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for c in mine {
                            if ui.button(&c.name).clicked() {
                                *action = Some(RowAction::AddTo(c.uri.clone(), c.name.clone()));
                                ui.close();
                            }
                        }
                    });
            }
        });
        if let Some(pl) = playlist {
            if ui.button("Remove from this playlist").clicked() {
                *action = Some(RowAction::RemoveFrom(pl.to_string()));
                ui.close();
            }
        }
        let can_save = app.demo || app.backend.is_some();
        let label = if multi > 1 {
            "Save to / remove from Liked Songs"
        } else if app.is_liked(&t.uri) {
            "Remove from your Liked Songs"
        } else {
            "Save to your Liked Songs"
        };
        if ui
            .add_enabled(can_save, egui::Button::new(label))
            .on_disabled_hover_text("Log in to Spotify to save songs")
            .clicked()
        {
            *action = Some(RowAction::Save);
            ui.close();
        }
    }
    pick(ui, "Add to queue", RowAction::Queue, action);
    ui.separator();
    if t.uri.starts_with("spotify:track:") {
        pick(ui, "Go to song radio", RowAction::Radio, action);
    }
    if t.artist_uri.is_some() {
        pick(ui, "Go to artist", RowAction::Artist, action);
    }
    if t.album_uri.is_some() {
        pick(ui, "Go to album", RowAction::Album, action);
    }
    if t.uri.starts_with("spotify:track:") {
        pick(ui, "View credits", RowAction::Credits, action);
    }
    if spotify {
        ui.menu_button("Share", |ui| {
            widgets::menu_style(ui, 230.0);
            let link = crate::discord::open_url(&t.uri);
            if ui.button("Copy Song Link").clicked() {
                if let Some(u) = &link {
                    ui.ctx().copy_text(u.clone());
                }
                *action = Some(RowAction::Copied);
                ui.close();
            }
            if ui.button("Copy Spotify URI").clicked() {
                ui.ctx().copy_text(t.uri.clone());
                *action = Some(RowAction::Copied);
                ui.close();
            }
            if ui.button("Embed track").clicked() {
                let id = crate::model::uri_id(&t.uri);
                ui.ctx().copy_text(format!(
                    "<iframe style=\"border-radius:12px\" src=\"https://open.spotify.com/embed/track/{id}\" width=\"100%\" height=\"152\" frameBorder=\"0\" allowfullscreen=\"\" allow=\"autoplay; clipboard-write; encrypted-media; fullscreen; picture-in-picture\" loading=\"lazy\"></iframe>"
                ));
                *action = Some(RowAction::Copied);
                ui.close();
            }
        });
        ui.separator();
        pick(ui, "Open in Spotify app", RowAction::OpenSpotify, action);
    }
}

fn pick(ui: &mut egui::Ui, label: &str, a: RowAction, action: &mut Option<RowAction>) {
    if ui.button(label).clicked() {
        *action = Some(a);
        ui.close();
    }
}

fn row_context_menu(app: &App, resp: &egui::Response, t: &Track, action: &mut Option<RowAction>) {
    resp.context_menu(|ui| row_menu_items(app, ui, t, action));
}

/// Spotify's save button: "+" saves to Liked Songs; on a saved song the ✓
/// opens a popover to tick Liked Songs and your playlists, applied with Done.
pub(super) fn save_button(
    app: &mut App,
    ui: &mut egui::Ui,
    center: egui::Pos2,
    d: f32,
    id: egui::Id,
    t: &TrackRef,
) {
    let pal = app.palette;
    let saved = app.is_liked(&t.uri);
    let r = widgets::like_button(ui, center, d, id, saved, pal.accent, pal.weak, pal.text);
    let r = r.on_hover_text(if saved {
        "Add to playlist"
    } else {
        "Save to Liked Songs"
    });
    if r.clicked() {
        if !saved {
            app.toggle_liked(&t.uri);
        } else if app.save_pop.as_ref().is_some_and(|p| p.id == id) {
            app.save_pop = None;
        } else {
            app.open_save_pop(id, t);
        }
    }
    if !app.save_pop.as_ref().is_some_and(|p| p.id == id) {
        return;
    }
    let mine = app.owned_playlists();
    let mut done = false;
    let mut cancel = false;
    let mut new_playlist = false;
    let shown = egui::Popup::from_response(&r)
        .id(id.with("save-pop"))
        .open(true)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .align(egui::RectAlign::TOP_START)
        .align_alternatives(&[
            egui::RectAlign::BOTTOM_START,
            egui::RectAlign::TOP_END,
            egui::RectAlign::BOTTOM_END,
        ])
        .gap(6.0)
        .width(300.0)
        .show(|ui| {
            widgets::menu_style(ui, 300.0);
            let Some(pop) = app.save_pop.as_mut() else {
                return;
            };
            ui.label(
                egui::RichText::new("Add to playlist").font(widgets::font(15.0, Weight::Bold)),
            );
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(widgets::with_alpha(pal.text, 0.1))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut pop.query)
                            .frame(egui::Frame::NONE)
                            .hint_text("Find a playlist")
                            .desired_width(f32::INFINITY),
                    );
                });
            ui.add_space(4.0);
            if ui.button("+  New playlist").clicked() {
                new_playlist = true;
            }
            ui.separator();
            let tick = |ui: &mut egui::Ui, label: &str, on: bool| -> bool {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                if resp.hovered() {
                    ui.painter()
                        .rect_filled(rect, 4.0, widgets::with_alpha(pal.text, 0.08));
                }
                widgets::text(
                    ui,
                    rect.left_center() + Vec2::new(8.0, 0.0),
                    Align2::LEFT_CENTER,
                    label,
                    14.0,
                    Weight::Regular,
                    pal.text,
                    rect.width() - 50.0,
                );
                let c = egui::pos2(rect.right() - 18.0, rect.center().y);
                if on {
                    widgets::draw(
                        ui,
                        Rect::from_center_size(c, Vec2::splat(18.0)),
                        Icon::Saved,
                        pal.accent,
                    );
                } else {
                    ui.painter()
                        .circle_stroke(c, 8.0, egui::Stroke::new(1.5, pal.weak));
                }
                resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            };
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    let q = pop.query.trim().to_lowercase();
                    if (q.is_empty() || "liked songs".contains(&q))
                        && tick(ui, "Liked Songs", pop.liked)
                    {
                        pop.liked = !pop.liked;
                    }
                    for c in mine
                        .iter()
                        .filter(|c| q.is_empty() || c.name.to_lowercase().contains(&q))
                    {
                        let on = pop.checked.contains(&c.uri);
                        if tick(ui, &c.name, on) {
                            if on {
                                pop.checked.remove(&c.uri);
                            } else {
                                pop.checked.insert(c.uri.clone());
                            }
                        }
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() - 150.0);
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                let done_btn =
                    egui::Button::new(egui::RichText::new("Done").color(Color32::BLACK).strong())
                        .fill(Color32::WHITE)
                        .corner_radius(16);
                if ui.add(done_btn).clicked() {
                    done = true;
                }
            });
        });
    // Clicking elsewhere or Esc cancels, like Spotify.
    if let Some(inner) = &shown {
        let outside = ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !inner.response.rect.contains(p) && !r.rect.contains(p))
        });
        if outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
    }
    if new_playlist {
        app.save_pop = None;
        app.create_playlist(vec![t.uri.clone()]);
    } else if done {
        app.apply_save_pop();
    } else if cancel {
        app.save_pop = None;
    }
}

/// A "⋯" button in `rect` that opens the song menu (Now playing panel, etc.).
pub(super) fn track_more_button(app: &mut App, ui: &mut egui::Ui, rect: Rect, t: &TrackRef) {
    let pal = app.palette;
    let r = ui.interact(rect, ui.id().with(("more", &t.uri)), Sense::click());
    widgets::draw(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(22.0)),
        Icon::More,
        if r.hovered() { pal.text } else { pal.weak },
    );
    let r = r.on_hover_text(format!("More options for {}", t.name));
    let mut action = None;
    egui::Popup::menu(&r)
        .id(ui.id().with(("more-menu", &t.uri)))
        .align(egui::RectAlign::BOTTOM_END)
        .gap(4.0)
        .show(|ui| track_menu_items(app, ui, t, &mut action, None, false));
    track_context_menu(app, &r, t);
    if let Some(a) = action {
        let t2 = t.clone();
        apply_row_action(app, a, t, move |app| {
            app.play_tracks(&t2.name, vec![t2.clone()], 0)
        });
    }
}

/// The song menu on things that aren't list rows (player bar, Now playing,
/// queue): right-click `resp` to open it. Play plays just this song.
pub(super) fn track_context_menu(app: &mut App, resp: &egui::Response, t: &TrackRef) {
    let mut action = None;
    resp.context_menu(|ui| track_menu_items(app, ui, t, &mut action, None, false));
    if let Some(a) = action {
        let t2 = t.clone();
        apply_row_action(app, a, t, move |app| {
            app.play_tracks(&t2.name, vec![t2.clone()], 0)
        });
    }
}

/// Column layout of the track table.
struct Cols {
    idx: Rect,
    title: Rect,
    album: Option<Rect>,
    dur: Rect,
}

fn columns(row: Rect, show_album: bool) -> Cols {
    let idx = Rect::from_min_size(row.min, Vec2::new(48.0, row.height()));
    let dur = Rect::from_min_max(egui::pos2(row.right() - 116.0, row.top()), row.max);
    let rest = Rect::from_min_max(
        egui::pos2(idx.right(), row.top()),
        egui::pos2(dur.left(), row.bottom()),
    );
    if show_album && rest.width() > 360.0 {
        let tw = rest.width() * 0.58;
        Cols {
            idx,
            title: Rect::from_min_size(rest.min, Vec2::new(tw, row.height())),
            album: Some(Rect::from_min_max(
                egui::pos2(rest.left() + tw + 16.0, row.top()),
                egui::pos2(rest.right() - 8.0, row.bottom()),
            )),
            dur,
        }
    } else {
        Cols {
            idx,
            title: rest,
            album: None,
            dur,
        }
    }
}

/// One track row painted at `rect` (works inside virtualised viewports).
fn track_row_at(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: Rect,
    t: &Track,
    index: usize,
    show_album: bool,
    id: egui::Id,
) -> Option<RowAction> {
    let pal = app.palette;
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    // Drag songs onto playlists in Your Library, or within your own playlist.
    if resp.drag_started() {
        let many = app.row_sel_page && app.row_hl && app.multi_tracks.len() > 1;
        resp.dnd_set_drag_payload(super::DragItem::Tracks {
            tracks: if many {
                app.multi_tracks.clone()
            } else {
                vec![std::sync::Arc::new(t.clone())]
            },
            from: if many {
                None
            } else {
                app.row_source.clone().map(|k| (k, index))
            },
        });
    }
    let mut drop_action: Option<RowAction> = None;
    if let (Some(pl), Some(src)) = (app.row_playlist.clone(), app.row_source.clone()) {
        let same_list = |p: &super::DragItem| matches!(p, super::DragItem::Tracks { from: Some((k, _)), .. } if *k == src);
        if let Some(p) = resp.dnd_hover_payload::<super::DragItem>() {
            if same_list(&p) {
                let below = ui
                    .input(|i| i.pointer.hover_pos())
                    .is_some_and(|q| q.y > rect.center().y);
                let y = if below { rect.bottom() } else { rect.top() };
                ui.painter()
                    .hline(rect.x_range(), y, egui::Stroke::new(2.0, pal.accent));
            }
        }
        if let Some(p) = resp.dnd_release_payload::<super::DragItem>() {
            if let super::DragItem::Tracks {
                from: Some((k, from)),
                ..
            } = &*p
            {
                if *k == src {
                    let below = ui
                        .input(|i| i.pointer.interact_pos())
                        .is_some_and(|q| q.y > rect.center().y);
                    let mut to = if below { index + 1 } else { index };
                    if to > *from {
                        to -= 1;
                    }
                    drop_action = Some(RowAction::Move(pl, *from, to));
                }
            }
        }
    }
    let hovered = resp.hovered()
        || resp.context_menu_opened()
        || egui::Popup::is_id_open(ui.ctx(), id.with("more-menu"))
        || (ui.rect_contains_pointer(rect) && !ui.ctx().is_being_dragged(id));
    let is_current = app.snapshot.track.as_ref().is_some_and(|c| c.uri == t.uri);
    if app.row_sel_page && resp.clicked() {
        app.row_click = Some((index, ui.input(|i| i.modifiers)));
    }
    // Right-clicking a row outside the selection selects it (the menu acts on it).
    if app.row_sel_page && resp.secondary_clicked() && !app.row_hl {
        app.row_click = Some((index, egui::Modifiers::NONE));
    }
    let selected = app.row_sel_page && app.row_hl;
    if selected {
        ui.painter()
            .rect_filled(rect, 4.0, widgets::with_alpha(pal.text, 0.2));
    }
    let hv = ui
        .ctx()
        .animate_bool_with_time(id.with("hv"), hovered && !selected, 0.1);
    if hv > 0.0 {
        ui.painter()
            .rect_filled(rect, 4.0, widgets::with_alpha(pal.text, 0.08 * hv));
    }
    let c = columns(rect.shrink2(Vec2::new(8.0, 0.0)), show_album);
    // # / play / equaliser
    let mut action = None;
    if hovered {
        let play_rect = Rect::from_center_size(c.idx.center(), Vec2::splat(14.0));
        widgets::draw(
            ui,
            play_rect,
            if is_current && app.snapshot.playing {
                Icon::Pause
            } else {
                Icon::Play
            },
            pal.text,
        );
        let pr = ui.interact(
            Rect::from_center_size(c.idx.center(), Vec2::splat(32.0)),
            id.with("play"),
            Sense::click(),
        );
        if pr.clicked() {
            if is_current {
                app.send(PlayerCmd::TogglePlay);
            } else {
                action = Some(RowAction::Play);
            }
        }
    } else if is_current && app.snapshot.playing {
        widgets::eq_bars(
            ui,
            Rect::from_center_size(c.idx.center(), Vec2::splat(16.0)),
            pal.accent,
            app.config.ui.animations,
        );
    } else {
        widgets::text(
            ui,
            c.idx.center(),
            Align2::CENTER_CENTER,
            &(index + 1).to_string(),
            15.0,
            Weight::Regular,
            if is_current { pal.accent } else { pal.weak },
            40.0,
        );
    }
    // art + title + artist
    let art = Rect::from_min_size(
        egui::pos2(c.title.left(), rect.center().y - 20.0),
        Vec2::splat(40.0),
    );
    if ui.is_rect_visible(art) {
        let tex = t.image.as_deref().and_then(|u| app.art.get(u, THUMB_PX));
        widgets::image(ui.painter(), art, tex, 4.0, pal.panel, pal.weak);
    }
    let tx = art.right() + 12.0;
    let tw = c.title.right() - tx - 8.0;
    let title_rect = widgets::text(
        ui,
        egui::pos2(tx, rect.center().y - 10.0),
        Align2::LEFT_CENTER,
        t.display_name(),
        16.0,
        Weight::Regular,
        if is_current { pal.accent } else { pal.text },
        tw,
    );
    let mut ax = tx;
    if t.explicit {
        let e = Rect::from_min_size(egui::pos2(tx, rect.center().y + 4.0), Vec2::splat(14.0));
        ui.painter()
            .rect_filled(e, 2.0, widgets::with_alpha(pal.text, 0.6));
        widgets::text(
            ui,
            e.center(),
            Align2::CENTER_CENTER,
            "E",
            10.0,
            Weight::Bold,
            pal.bg,
            14.0,
        );
        ax = e.right() + 6.0;
    }
    let artist_rect = widgets::text(
        ui,
        egui::pos2(ax, rect.center().y + 11.0),
        Align2::LEFT_CENTER,
        &t.artists,
        14.0,
        Weight::Regular,
        if hovered { pal.text } else { pal.weak },
        c.title.right() - ax - 8.0,
    );
    if hovered {
        let ar = ui.interact(artist_rect, id.with("artist"), Sense::click());
        if ar.hovered() {
            ui.painter().hline(
                artist_rect.x_range(),
                artist_rect.bottom(),
                egui::Stroke::new(1.0, pal.text),
            );
        }
        if ar.clicked() && t.artist_uri.is_some() {
            action = Some(RowAction::Artist);
        }
        let tr = ui.interact(title_rect, id.with("title"), Sense::click());
        if tr.double_clicked() {
            action = Some(RowAction::Play);
        }
    }
    if let Some(album) = c.album {
        let ar = widgets::text(
            ui,
            egui::pos2(album.left(), rect.center().y),
            Align2::LEFT_CENTER,
            &t.album,
            14.0,
            Weight::Regular,
            if hovered { pal.text } else { pal.weak },
            album.width(),
        );
        if hovered && t.album_uri.is_some() {
            let r = ui.interact(ar, id.with("album"), Sense::click());
            if r.hovered() {
                ui.painter()
                    .hline(ar.x_range(), ar.bottom(), egui::Stroke::new(1.0, pal.text));
            }
            if r.clicked() {
                action = Some(RowAction::Album);
            }
        }
    }
    if t.duration_ms > 0 {
        widgets::text(
            ui,
            egui::pos2(c.dur.right() - 42.0, rect.center().y),
            Align2::RIGHT_CENTER,
            &fmt_ms(t.duration_ms),
            14.0,
            Weight::Regular,
            pal.weak,
            60.0,
        );
    }
    let saved = app.is_liked(&t.uri);
    let pop_here = app
        .save_pop
        .as_ref()
        .is_some_and(|p| p.id == id.with("add"));
    if (hovered || saved || pop_here) && !t.uri.starts_with("local:") {
        let tr = std::sync::Arc::new(t.clone());
        save_button(
            app,
            ui,
            egui::pos2(c.dur.left() + 14.0, rect.center().y),
            16.0,
            id.with("add"),
            &tr,
        );
    }
    // "⋯" button on hover, opening the same menu as a right-click.
    let more = Rect::from_center_size(
        egui::pos2(c.dur.right() - 18.0, rect.center().y),
        Vec2::new(28.0, 28.0),
    );
    let mr = ui.interact(more, id.with("more"), Sense::click());
    let menu_open = egui::Popup::is_id_open(ui.ctx(), id.with("more-menu"));
    if hovered || mr.hovered() || menu_open {
        widgets::draw(
            ui,
            Rect::from_center_size(more.center(), Vec2::splat(18.0)),
            Icon::More,
            if mr.hovered() { pal.text } else { pal.weak },
        );
    }
    // Open upwards for rows in the lower half so the menu never runs off-screen.
    let up = rect.center().y > ui.ctx().content_rect().center().y;
    egui::Popup::menu(&mr)
        .id(id.with("more-menu"))
        .align(if up {
            egui::RectAlign::TOP_END
        } else {
            egui::RectAlign::BOTTOM_END
        })
        .align_alternatives(&[])
        .gap(4.0)
        .show(|ui| row_menu_items(app, ui, t, &mut action));
    if resp.double_clicked() {
        action = Some(RowAction::Play);
    }
    row_context_menu(app, &resp, t, &mut action);
    drop_action.or(action)
}

/// Track row in normal (non-virtualised) layouts.
pub fn track_row(
    app: &mut App,
    ui: &mut egui::Ui,
    t: &Track,
    index: usize,
    show_album: bool,
) -> Option<RowAction> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
    let id = ui.id().with(("row", index, &t.uri));
    track_row_at(app, ui, rect, t, index, show_album, id)
}

/// Apply a row action for a track inside a list.
fn apply_row_action(app: &mut App, action: RowAction, t: &TrackRef, play: impl FnOnce(&mut App)) {
    match action {
        RowAction::Play => play(app),
        RowAction::Queue => app.send(PlayerCmd::AddToQueue(vec![t.clone()])),
        RowAction::Album => {
            if let Some(u) = &t.album_uri {
                app.open_uri(CollectionKind::Album, u, &t.album);
            }
        }
        RowAction::Artist => {
            if let Some(u) = &t.artist_uri {
                let name = t.artists.split(", ").next().unwrap_or("").to_string();
                app.open_uri(CollectionKind::Artist, u, &name);
            }
        }
        RowAction::Save => app.toggle_liked(&t.uri),
        RowAction::Copied => app.toast(crate::events::ToastKind::Info, "Copied to clipboard"),
        RowAction::AddTo(uri, name) => app.add_to_playlist(&uri, &name, vec![t.uri.clone()]),
        RowAction::NewPlaylist => app.create_playlist(vec![t.uri.clone()]),
        RowAction::RemoveFrom(uri) => app.remove_from_playlist(&uri, std::slice::from_ref(&t.uri)),
        RowAction::Radio => app.start_radio(&t.uri, &t.name),
        RowAction::Credits => app.show_credits(t),
        RowAction::Move(uri, from, to) => app.move_in_playlist(&uri, from, to),
        RowAction::OpenSpotify => {
            if let Err(e) = open::that_detached(&t.uri) {
                app.toast(
                    crate::events::ToastKind::Error,
                    format!("Could not open Spotify: {e}"),
                );
            }
        }
    }
}

// ====================================================================== track page

enum Source {
    List(ListKey),
    Local,
}

struct PageInfo {
    kind: &'static str,
    title: String,
    owner: String,
    image: Option<String>,
    total: usize,
    loaded: usize,
    duration_ms: u64,
    loading: bool,
    error: Option<String>,
    has_next: bool,
    color: Color32,
    show_album: bool,
}

fn page_info(app: &mut App, src: &Source) -> Option<PageInfo> {
    match src {
        Source::Local => Some(PageInfo {
            kind: "Local files",
            title: "Local Files".into(),
            owner: "On this computer".into(),
            image: None,
            total: app.local_filtered.len(),
            loaded: app.local_filtered.len(),
            duration_ms: app
                .local_filtered
                .iter()
                .map(|t| t.duration_ms as u64)
                .sum(),
            loading: app.local_scanning,
            error: None,
            has_next: false,
            color: Color32::from_rgb(0x1a, 0x7a, 0x4c),
            show_album: true,
        }),
        Source::List(key) => {
            let list = app.lists.get(key)?;
            let kind = match key {
                ListKey::Liked => "Playlist",
                ListKey::Recent | ListKey::Top => "Mix",
                ListKey::Collection(u) if u.starts_with("spotify:album:") => "Album",
                ListKey::Collection(u) if u.starts_with("spotify:artist:") => "Artist",
                _ => "Playlist",
            };
            let owner = match key {
                ListKey::Liked => app.username.clone(),
                _ => list.subtitle.trim_start_matches("by ").to_string(),
            };
            let image = list.image.clone();
            let info = PageInfo {
                kind,
                title: list.title.clone(),
                owner,
                image: image.clone(),
                total: (list.total as usize).max(list.tracks.len()),
                loaded: list.tracks.len(),
                duration_ms: list.tracks.iter().map(|t| t.duration_ms as u64).sum(),
                loading: list.loading,
                error: list.error.clone(),
                has_next: list.next.is_some(),
                color: Color32::from_rgb(0x53, 0x53, 0x53),
                show_album: kind != "Album",
            };
            let color = match key {
                ListKey::Liked => Some(Color32::from_rgb(0x50, 0x38, 0xa0)),
                _ => image.as_deref().and_then(|u| {
                    let _ = app.art.get(u, LARGE_PX);
                    app.art.color(u)
                }),
            };
            Some(PageInfo {
                color: color.unwrap_or(info.color),
                ..info
            })
        }
    }
}

fn fmt_total(ms: u64) -> String {
    let mins = ms / 60_000;
    if mins >= 60 {
        format!("about {} hr {} min", mins / 60, mins % 60)
    } else {
        format!("{mins} min {} sec", (ms / 1000) % 60)
    }
}

/// Largest title font that fits (Spotify scales the heading down for long names).
/// Measured once at a small size and scaled, so huge glyphs are only rasterised
/// for the size actually drawn (keeps the font atlas small).
pub(super) fn title_size(ui: &egui::Ui, text: &str, max_w: f32) -> f32 {
    let probe = widgets::galley(ui, text, 20.0, Weight::Black, Color32::WHITE, f32::INFINITY);
    let per_px = (probe.size().x / 20.0).max(0.01);
    for size in [72.0, 56.0, 44.0, 34.0] {
        if per_px * size <= max_w {
            return size;
        }
    }
    30.0
}

const HEADER_H: f32 = 300.0;
const ACTIONS_H: f32 = 96.0;
const TABLE_HEAD_H: f32 = 44.0;

fn track_page(app: &mut App, ui: &mut egui::Ui, src: Source) {
    let pal = app.palette;
    let Some(info) = page_info(app, &src) else {
        // Skeleton of the page while it loads: header block, then song rows.
        let text = pal.text;
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(PAD as i8, 24))
            .show(ui, |ui| {
                let (h, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 200.0), Sense::hover());
                let a = widgets::with_alpha(text, 0.07);
                ui.painter()
                    .rect_filled(Rect::from_min_size(h.min, Vec2::splat(190.0)), 6.0, a);
                ui.painter().rect_filled(
                    Rect::from_min_size(h.min + Vec2::new(214.0, 70.0), Vec2::new(320.0, 48.0)),
                    8.0,
                    a,
                );
                ui.painter().rect_filled(
                    Rect::from_min_size(h.min + Vec2::new(214.0, 134.0), Vec2::new(200.0, 12.0)),
                    5.0,
                    a,
                );
                ui.add_space(28.0);
                widgets::skeleton_rows(ui, 8, text);
            });
        return;
    };
    let key = match &src {
        Source::List(k) => Some(k.clone()),
        Source::Local => None,
    };
    let page_ctx = key
        .as_ref()
        .map(|k| k.context_key())
        .unwrap_or_else(|| "local".into());
    if app.list_find.ctx != page_ctx {
        app.list_find = super::ListFind {
            ctx: page_ctx.clone(),
            ..Default::default()
        };
    }
    // Find in playlist / sort: rows map to list indices through `order`.
    let order: Option<Vec<usize>> = key.as_ref().and_then(|k| app.list_order(k));
    let rows = order.as_ref().map_or(info.total, |o| o.len());
    let real = |i: usize| order.as_ref().map_or(Some(i), |o| o.get(i).copied());
    let mut clicked: Option<(usize, RowAction)> = None;
    let mut need_more = false;
    let mut need_meta: Vec<usize> = Vec::new();
    let mut header_action: Option<&'static str> = None;
    let mut header_lib: Option<Collection> = None;
    let mut delete_selected = false;
    // The selection belongs to this list as it is; a changed list drops it.
    let list_len = match &key {
        Some(k) => app.lists.peek(k).map_or(0, |l| l.tracks.len()),
        None => app.local_filtered.len(),
    };
    if list_len != app.row_multi_len {
        app.row_multi.clear();
        app.row_multi_len = list_len;
    }
    // The selected songs, in list order (used when more than one is selected).
    app.multi_tracks = if app.row_multi.len() > 1 {
        app.row_multi
            .iter()
            .filter_map(|&r| match &key {
                Some(k) => app.lists.peek(k).and_then(|l| l.tracks.get(r).cloned()),
                None => app.local_filtered.get(r).cloned(),
            })
            .collect()
    } else {
        Vec::new()
    };
    // The playlist as it appears in Your Library (owned → editable).
    let in_library: Option<Collection> = match &key {
        Some(ListKey::Collection(u)) if u.starts_with("spotify:playlist:") => {
            app.playlists.items.iter().find(|c| &c.uri == u).cloned()
        }
        _ => None,
    };
    let owned = in_library.as_ref().is_some_and(|c| c.owned);
    // ------------------------------------------------ footer (below the songs)
    // Albums: release date, label, copyright and "More by <artist>".
    // Your playlists: Spotify's "Recommended" songs to add.
    let page_w = ui.available_width() - 2.0 * PAD;
    let album_uri = match &key {
        Some(ListKey::Collection(u)) if u.starts_with("spotify:album:") => Some(u.clone()),
        _ => None,
    };
    let mut album_footer: Option<(crate::model::AlbumInfo, Vec<CardKind>)> = None;
    if let Some(u) = &album_uri {
        app.load_album_info(u);
        if let Some(Some(Ok(ai))) = app.album_info.get(u).cloned() {
            let more: Vec<CardKind> = match &ai.artist_uri {
                Some(a) => {
                    if !app.artist_albums.contains(a) {
                        app.load_artist_albums(a.clone());
                    }
                    app.artist_albums
                        .get(a)
                        .map(|v| {
                            v.iter()
                                .filter(|r| {
                                    r.group != crate::model::ReleaseGroup::AppearsOn
                                        && &r.coll.uri != u
                                })
                                .map(|r| CardKind::Coll(r.coll.clone()))
                                .collect()
                        })
                        .unwrap_or_default()
                }
                None => Vec::new(),
            };
            album_footer = Some((ai, more));
        }
    }
    let rec_for = in_library
        .as_ref()
        .filter(|c| c.owned && order.is_none() && !info.loading)
        .map(|c| c.uri.clone());
    if let Some(u) = &rec_for {
        app.load_recommended(u, false);
    }
    let recs: Vec<TrackRef> = rec_for
        .as_ref()
        .and_then(|u| app.recommended.get(u).cloned().flatten())
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let (_, shelf_cw) = card_width(page_w);
    let footer_h = match (&album_footer, &rec_for) {
        (Some((ai, more)), _) => {
            let text = 40.0 + 20.0 * (1 + ai.copyrights.len()) as f32;
            let shelf = if more.is_empty() {
                0.0
            } else {
                40.0 + shelf_cw + CARD_TEXT_H + 24.0
            };
            text + shelf
        }
        (None, Some(_)) => 110.0 + recs.len().max(1) as f32 * ROW_H + 40.0,
        _ => 0.0,
    };
    let total_h = HEADER_H + ACTIONS_H + TABLE_HEAD_H + rows as f32 * ROW_H + 80.0 + footer_h;
    let mut rec_add: Option<TrackRef> = None;
    let mut rec_refresh = false;
    app.row_playlist = in_library
        .as_ref()
        .filter(|c| c.owned)
        .map(|c| c.uri.clone());
    // Reordering by drag only makes sense in the list's own order.
    app.row_source = if order.is_some() { None } else { key.clone() };
    let panel = ui.max_rect();
    let mut scrolled = 0.0f32;
    let playing_this = key
        .as_ref()
        .map(|k| k.context_key())
        .or(Some("local".into()))
        == app.snapshot.context_key
        && app.snapshot.playing;
    egui::ScrollArea::vertical()
        .id_salt((
            "page",
            key.as_ref()
                .map(|k| k.context_key())
                .unwrap_or_else(|| "local".into()),
        ))
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, vp| {
            scrolled = vp.top();
            let origin = ui.max_rect().min;
            let width = ui.max_rect().width();
            let (content, _) = ui.allocate_exact_size(Vec2::new(width, total_h), Sense::hover());
            let abs = |y0: f32, h: f32| {
                Rect::from_min_size(egui::pos2(origin.x, origin.y + y0), Vec2::new(width, h))
            };

            // ------------------------------------------------ gradient + header
            let head = abs(0.0, HEADER_H);
            let top = widgets::mix(info.color, Color32::BLACK, 0.05);
            let mid = widgets::mix(info.color, pal.bg, 0.55);
            if pal.dark {
                widgets::gradient(ui.painter(), head, 8.0, top, mid);
                widgets::gradient(
                    ui.painter(),
                    abs(HEADER_H, ACTIONS_H + 140.0),
                    0.0,
                    widgets::mix(mid, pal.bg, 0.35),
                    pal.bg,
                );
            } else {
                widgets::gradient(
                    ui.painter(),
                    abs(0.0, HEADER_H + ACTIONS_H),
                    8.0,
                    widgets::mix(info.color, Color32::WHITE, 0.55),
                    pal.bg,
                );
            }
            let cover_size = (width * 0.3).clamp(128.0, 232.0);
            let cover = Rect::from_min_size(
                egui::pos2(head.left() + PAD, head.bottom() - PAD - cover_size),
                Vec2::splat(cover_size),
            );
            widgets::shadow(ui.painter(), cover, 6.0);
            match &src {
                Source::List(ListKey::Liked) => {
                    widgets::gradient(
                        ui.painter(),
                        cover,
                        6.0,
                        Color32::from_rgb(0x45, 0x0a, 0xf5),
                        Color32::from_rgb(0x8e, 0xb8, 0xe5),
                    );
                    widgets::draw(
                        ui,
                        Rect::from_center_size(cover.center(), Vec2::splat(cover_size * 0.4)),
                        Icon::Heart,
                        Color32::WHITE,
                    );
                }
                Source::Local => {
                    widgets::gradient(
                        ui.painter(),
                        cover,
                        6.0,
                        Color32::from_rgb(0x0f, 0x6b, 0x45),
                        Color32::from_rgb(0x1e, 0xd7, 0x60),
                    );
                    widgets::draw(
                        ui,
                        Rect::from_center_size(cover.center(), Vec2::splat(cover_size * 0.4)),
                        Icon::Local,
                        Color32::WHITE,
                    );
                }
                _ => {
                    let tex = info.image.as_deref().and_then(|u| app.art.get(u, LARGE_PX));
                    widgets::image(ui.painter(), cover, tex, 6.0, pal.panel, pal.weak);
                }
            }
            let tx = cover.right() + 24.0;
            let tw = head.right() - tx - PAD;
            let white = if pal.dark { Color32::WHITE } else { pal.text };
            widgets::text(
                ui,
                egui::pos2(tx, cover.top() + 52.0),
                Align2::LEFT_BOTTOM,
                info.kind,
                14.0,
                Weight::Semibold,
                white,
                tw,
            );
            let size = title_size(ui, &info.title, tw);
            let title_rect = widgets::text(
                ui,
                egui::pos2(tx, cover.bottom() - 34.0),
                Align2::LEFT_BOTTOM,
                &info.title,
                size,
                Weight::Black,
                white,
                tw,
            );
            if owned {
                // Like Spotify: click the title (or cover) to edit details.
                let tr = ui.interact(
                    title_rect.union(cover),
                    ui.id().with("edit-title"),
                    Sense::click(),
                );
                if tr
                    .on_hover_text("Edit details")
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    header_action = Some("edit");
                }
            }
            let mut meta = String::new();
            if !info.owner.is_empty() {
                meta.push_str(&info.owner);
                meta.push_str(" • ");
            }
            meta.push_str(&format!("{} songs", info.total));
            if info.duration_ms > 0 && info.loaded == info.total {
                meta.push_str(&format!(", {}", fmt_total(info.duration_ms)));
            }
            widgets::text(
                ui,
                egui::pos2(tx, cover.bottom() - 4.0),
                Align2::LEFT_BOTTOM,
                &meta,
                14.0,
                Weight::Semibold,
                widgets::with_alpha(white, 0.85),
                tw,
            );

            // ------------------------------------------------ actions
            let actions = abs(HEADER_H, ACTIONS_H);
            let play_rect = Rect::from_center_size(
                egui::pos2(actions.left() + PAD + 28.0, actions.center().y),
                Vec2::splat(56.0),
            );
            let playing_this = key
                .as_ref()
                .map(|k| k.context_key())
                .or(Some("local".into()))
                == app.snapshot.context_key
                && app.snapshot.playing;
            let pr = ui.interact(play_rect, ui.id().with("big-play"), Sense::click());
            widgets::paint_round_play(
                ui,
                play_rect,
                pr.hovered(),
                playing_this,
                pal.accent,
                pal.on_accent,
            );
            if pr
                .on_hover_text(if playing_this { "Pause" } else { "Play" })
                .clicked()
            {
                header_action = Some(if playing_this { "pause" } else { "play" });
            }
            let sh = Rect::from_center_size(
                egui::pos2(play_rect.right() + 36.0, play_rect.center().y),
                Vec2::splat(40.0),
            );
            let shr = ui.interact(sh, ui.id().with("page-shuffle"), Sense::click());
            let on = app.snapshot.shuffle;
            widgets::draw(
                ui,
                Rect::from_center_size(sh.center(), Vec2::splat(28.0)),
                Icon::Shuffle,
                if on {
                    pal.accent
                } else if shr.hovered() {
                    pal.text
                } else {
                    pal.weak
                },
            );
            if on {
                ui.painter().circle_filled(
                    sh.center_bottom() + Vec2::new(0.0, 2.0),
                    2.5,
                    pal.accent,
                );
            }
            if shr.on_hover_text("Shuffle play").clicked() {
                header_action = Some("shuffle");
            }
            let more = Rect::from_center_size(
                egui::pos2(sh.right() + 28.0, play_rect.center().y),
                Vec2::splat(40.0),
            );
            let mr = ui.interact(more, ui.id().with("page-more"), Sense::click());
            widgets::draw(
                ui,
                Rect::from_center_size(more.center(), Vec2::splat(28.0)),
                Icon::More,
                if mr.hovered() { pal.text } else { pal.weak },
            );
            let mr = mr.on_hover_text("More options");
            egui::Popup::menu(&mr).show(|ui| {
                super::widgets::menu_style(ui, 200.0);
                if ui.button("Add to queue").clicked() {
                    header_action = Some("queue");
                    ui.close();
                }
                if matches!(src, Source::Local) {
                    if ui.button("Rescan music folders").clicked() {
                        header_action = Some("rescan");
                        ui.close();
                    }
                    if ui.button("Music folders…").clicked() {
                        header_action = Some("folders");
                        ui.close();
                    }
                } else {
                    if ui.button("Reload").clicked() {
                        header_action = Some("reload");
                        ui.close();
                    }
                    if let Some(c) = &in_library {
                        if c.owned {
                            if ui.button("Edit details").clicked() {
                                header_action = Some("edit");
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                header_action = Some("delete");
                                ui.close();
                            }
                        }
                    }
                    if let Some(ListKey::Collection(u)) = &key {
                        let is_album = u.starts_with("spotify:album:");
                        // Save / follow when it isn't yours, like Spotify's menu.
                        let coll = app
                            .lists
                            .peek(&ListKey::Collection(u.clone()))
                            .and_then(|l| l.collection.clone())
                            .or_else(|| in_library.clone());
                        if let Some(c) = coll.filter(|c| !c.owned) {
                            let saved = app.in_library(&c);
                            if ui
                                .button(if saved { "Remove from Your Library" } else { "Add to Your Library" })
                                .clicked()
                            {
                                header_lib = Some(c);
                                ui.close();
                            }
                        }
                        if ui
                            .button(if is_album { "Go to album radio" } else { "Go to playlist radio" })
                            .clicked()
                        {
                            header_action = Some("radio");
                            ui.close();
                        }
                        let id = crate::model::uri_id(u);
                        let kind = if is_album { "album" } else { "playlist" };
                        ui.menu_button("Share", |ui| {
                            super::widgets::menu_style(ui, 200.0);
                            if ui.button(format!("Copy link to {kind}")).clicked() {
                                ui.ctx()
                                    .copy_text(format!("https://open.spotify.com/{kind}/{id}"));
                                ui.close();
                            }
                            if ui.button("Copy Spotify URI").clicked() {
                                ui.ctx().copy_text(u.clone());
                                ui.close();
                            }
                            if ui.button(format!("Embed {kind}")).clicked() {
                                ui.ctx().copy_text(format!(
                                    "<iframe style=\"border-radius:12px\" src=\"https://open.spotify.com/embed/{kind}/{id}\" width=\"100%\" height=\"352\" frameBorder=\"0\" allowfullscreen=\"\" allow=\"autoplay; clipboard-write; encrypted-media; fullscreen; picture-in-picture\" loading=\"lazy\"></iframe>"
                                ));
                                ui.close();
                            }
                        });
                        if ui.button("Open in Spotify app").clicked() {
                            let _ = open::that_detached(u);
                            ui.close();
                        }
                    }
                }
            });
            if let Some(k) = key.clone() {
                find_and_sort(app, ui, actions, &k);
            }
            if matches!(src, Source::Local) {
                // Filter box on the right of the action row.
                let fr = Rect::from_min_size(
                    egui::pos2(actions.right() - PAD - 240.0, actions.center().y - 16.0),
                    Vec2::new(240.0, 32.0),
                );
                ui.painter()
                    .rect_filled(fr, 4.0, widgets::with_alpha(pal.text, 0.1));
                widgets::draw(
                    ui,
                    Rect::from_center_size(
                        egui::pos2(fr.left() + 16.0, fr.center().y),
                        Vec2::splat(16.0),
                    ),
                    Icon::Search,
                    pal.weak,
                );
                let edit = ui.put(
                    Rect::from_min_max(fr.min + Vec2::new(32.0, 4.0), fr.max - Vec2::new(8.0, 4.0)),
                    egui::TextEdit::singleline(&mut app.local_filter)
                        .frame(egui::Frame::NONE)
                        .hint_text("Search in local files")
                        .vertical_align(egui::Align::Center),
                );
                if edit.changed() {
                    app.local_filter_changed = true;
                }
            }

            // ------------------------------------------------ table head
            let th = abs(HEADER_H + ACTIONS_H, TABLE_HEAD_H).shrink2(Vec2::new(PAD, 0.0));
            let c = columns(
                Rect::from_min_size(th.min, Vec2::new(th.width(), 36.0))
                    .shrink2(Vec2::new(8.0, 0.0)),
                info.show_album,
            );
            let hc = pal.weak;
            let sortable = key.is_some();
            // Click a column to sort by it; again to reverse; # goes back to the list order.
            let mut head = |ui: &mut egui::Ui,
                            rect: Rect,
                            label: &str,
                            sort: super::ListSort,
                            center: bool| {
                let hr = if sortable {
                    Some(ui.interact(rect, ui.id().with(("col", label)), Sense::click()))
                } else {
                    None
                };
                let on = sortable && app.list_find.sort == sort && sort != super::ListSort::Custom;
                let hov = hr.as_ref().is_some_and(|r| r.hovered());
                let color = if on || hov { pal.text } else { hc };
                let size = if label == "#" { 15.0 } else { 14.0 };
                let w = widgets::text(
                    ui,
                    if center {
                        rect.center()
                    } else {
                        rect.left_center()
                    },
                    if center {
                        Align2::CENTER_CENTER
                    } else {
                        Align2::LEFT_CENTER
                    },
                    label,
                    size,
                    Weight::Regular,
                    color,
                    rect.width(),
                );
                if on {
                    let x = if center {
                        rect.center().x + 10.0
                    } else {
                        rect.left() + w.width() + 6.0
                    };
                    let y = rect.center().y;
                    let (a, b) = if app.list_find.desc {
                        (-3.0, 3.0)
                    } else {
                        (3.0, -3.0)
                    };
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![
                            egui::pos2(x, y + a),
                            egui::pos2(x + 8.0, y + a),
                            egui::pos2(x + 4.0, y + b),
                        ],
                        pal.accent,
                        egui::Stroke::NONE,
                    ));
                }
                if let Some(r) = hr {
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        let f = &mut app.list_find;
                        if sort == super::ListSort::Custom {
                            f.sort = sort;
                            f.desc = false;
                        } else if f.sort == sort {
                            if f.desc {
                                f.sort = super::ListSort::Custom;
                                f.desc = false;
                            } else {
                                f.desc = true;
                            }
                        } else {
                            f.sort = sort;
                            f.desc = false;
                        }
                    }
                }
            };
            head(ui, c.idx, "#", super::ListSort::Custom, true);
            head(ui, c.title, "Title", super::ListSort::Title, false);
            if let Some(a) = c.album {
                head(ui, a, "Album", super::ListSort::Album, false);
            }
            let clock = Rect::from_center_size(
                egui::pos2(c.dur.right() - 56.0, c.dur.center().y),
                Vec2::splat(16.0),
            );
            let dur_on = sortable && app.list_find.sort == super::ListSort::Duration;
            let cr = sortable
                .then(|| ui.interact(clock.expand(6.0), ui.id().with("col-dur"), Sense::click()));
            let dur_hov = cr.as_ref().is_some_and(|r| r.hovered());
            widgets::draw(
                ui,
                clock,
                Icon::Clock,
                if dur_on || dur_hov { pal.text } else { hc },
            );
            if let Some(r) = cr {
                if r.on_hover_text("Sort by duration").clicked() {
                    let f = &mut app.list_find;
                    if f.sort != super::ListSort::Duration {
                        f.sort = super::ListSort::Duration;
                        f.desc = false;
                    } else if !f.desc {
                        f.desc = true;
                    } else {
                        f.sort = super::ListSort::Custom;
                        f.desc = false;
                    }
                }
            }
            ui.painter().hline(
                th.x_range(),
                th.top() + 37.0,
                egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.1)),
            );

            // ------------------------------------------------ rows (virtualised)
            let rows_top = HEADER_H + ACTIONS_H + TABLE_HEAD_H;
            if rows == 0 && order.is_some() && !app.list_find.query.trim().is_empty() {
                let r = abs(rows_top + 24.0, 60.0);
                widgets::text(
                    ui,
                    r.center_top(),
                    Align2::CENTER_TOP,
                    &format!("Couldn't find \"{}\"", app.list_find.query.trim()),
                    18.0,
                    Weight::Bold,
                    pal.text,
                    width - 2.0 * PAD,
                );
                widgets::text(
                    ui,
                    r.center_top() + Vec2::new(0.0, 30.0),
                    Align2::CENTER_TOP,
                    "Try different words, or check the spelling.",
                    14.0,
                    Weight::Regular,
                    pal.weak,
                    width - 2.0 * PAD,
                );
            }
            if rows == 0 && matches!(src, Source::Local) && !info.loading && info.error.is_none() {
                let filtering = !app.local_filter.trim().is_empty();
                let (title, body, btn) = if filtering {
                    (
                        "No matches",
                        "Nothing in your local files matches that search.",
                        None,
                    )
                } else {
                    (
                        "Play music from your computer",
                        "Add your music folders in Settings: MP3, FLAC, Ogg, Opus, M4A and WAV.",
                        Some("Add a folder"),
                    )
                };
                let top = abs(rows_top + 28.0, 1.0).center_top();
                if widgets::empty_state(
                    ui,
                    top,
                    width - 2.0 * PAD,
                    Icon::Local,
                    title,
                    body,
                    btn,
                    &pal,
                ) {
                    app.navigate(View::Settings);
                }
            } else if rows == 0 {
                let r = abs(rows_top + 20.0, 40.0);
                let msg = if info.loading {
                    "Loading…".to_string()
                } else if let Some(e) = &info.error {
                    e.clone()
                } else if matches!(src, Source::Local) {
                    "No local music yet — add your music folders in Settings.".into()
                } else {
                    "No songs here yet.".into()
                };
                widgets::text(
                    ui,
                    r.left_center() + Vec2::new(PAD + 8.0, 0.0),
                    Align2::LEFT_CENTER,
                    &msg,
                    15.0,
                    Weight::Regular,
                    pal.weak,
                    width - 2.0 * PAD,
                );
            }
            // Keyboard: ↑/↓ move the selected row, Enter plays it.
            if rows > 0 && !ui.ctx().egui_wants_keyboard_input() {
                let (up, down, enter, home, end) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowUp) && i.modifiers.is_none(),
                        i.key_pressed(egui::Key::ArrowDown) && i.modifiers.is_none(),
                        i.key_pressed(egui::Key::Enter),
                        i.key_pressed(egui::Key::Home),
                        i.key_pressed(egui::Key::End),
                    )
                });
                let before = app.row_sel;
                if down {
                    app.row_sel = Some(app.row_sel.map_or(0, |s| (s + 1).min(rows - 1)));
                }
                if up {
                    app.row_sel = Some(app.row_sel.map_or(0, |s| s.saturating_sub(1)));
                }
                if home {
                    app.row_sel = Some(0);
                }
                if end {
                    app.row_sel = Some(rows - 1);
                }
                if app.row_sel != before {
                    if let Some(r) = app.row_sel.and_then(real) {
                        app.row_multi = [r].into();
                    }
                }
                let (all, esc, del) = ui.input_mut(|i| {
                    (
                        i.consume_shortcut(&egui::KeyboardShortcut::new(
                            egui::Modifiers::COMMAND,
                            egui::Key::A,
                        )),
                        i.key_pressed(egui::Key::Escape),
                        i.key_pressed(egui::Key::Delete),
                    )
                });
                if all {
                    app.row_multi = (0..rows).filter_map(real).collect();
                }
                if esc {
                    app.row_multi.clear();
                    app.row_sel = None;
                }
                if del && !app.row_multi.is_empty() {
                    delete_selected = true;
                }
                if let Some(sel) = app.row_sel.filter(|s| *s < rows) {
                    if app.row_sel != before {
                        let r = abs(rows_top + sel as f32 * ROW_H, ROW_H);
                        // Keep it clear of the sticky header.
                        ui.scroll_to_rect(r.expand2(Vec2::new(0.0, ROW_H)), None);
                    }
                    if enter {
                        clicked = Some((sel, RowAction::Play));
                    }
                }
            }
            let first = ((vp.top() - rows_top) / ROW_H).floor().max(0.0) as usize;
            let last = (((vp.bottom() - rows_top) / ROW_H).ceil().max(0.0) as usize).min(rows);
            app.row_sel_page = true;
            for i in first..last {
                let rect = abs(rows_top + i as f32 * ROW_H, ROW_H).shrink2(Vec2::new(PAD, 0.0));
                let t = match &src {
                    Source::Local => app.local_filtered.get(i).cloned(),
                    Source::List(k) => real(i)
                        .and_then(|r| app.lists.get(k).and_then(|l| l.tracks.get(r).cloned())),
                };
                match t {
                    Some(t) => {
                        if t.is_stub() {
                            need_meta.push(real(i).unwrap_or(i));
                        }
                        let id = ui.id().with(("track-row", i));
                        app.row_hl = real(i).is_some_and(|r| app.row_multi.contains(&r));
                        if let Some(a) = track_row_at(app, ui, rect, &t, i, info.show_album, id) {
                            clicked = Some((i, a));
                        }
                        app.row_hl = false;
                    }
                    None => {
                        need_more = true;
                        let c = columns(rect.shrink2(Vec2::new(8.0, 0.0)), info.show_album);
                        widgets::text(
                            ui,
                            c.idx.center(),
                            Align2::CENTER_CENTER,
                            &(i + 1).to_string(),
                            15.0,
                            Weight::Regular,
                            pal.weak,
                            40.0,
                        );
                        let ph = Rect::from_min_size(
                            egui::pos2(c.title.left(), rect.center().y - 20.0),
                            Vec2::splat(40.0),
                        );
                        ui.painter().rect_filled(ph, 4.0, pal.panel);
                        ui.painter().rect_filled(
                            Rect::from_min_size(
                                egui::pos2(ph.right() + 12.0, rect.center().y - 12.0),
                                Vec2::new(160.0, 10.0),
                            ),
                            3.0,
                            pal.panel,
                        );
                        ui.painter().rect_filled(
                            Rect::from_min_size(
                                egui::pos2(ph.right() + 12.0, rect.center().y + 4.0),
                                Vec2::new(100.0, 8.0),
                            ),
                            3.0,
                            pal.panel,
                        );
                    }
                }
            }
            app.row_sel_page = false;
            if footer_h > 0.0 {
                let fr = abs(rows_top + rows as f32 * ROW_H + 24.0, footer_h).shrink2(Vec2::new(PAD, 0.0));
                if ui.is_rect_visible(fr) {
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(fr)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                        |ui| {
                            if let Some((ai, more)) = &album_footer {
                                album_footer_ui(app, ui, ai, more);
                            } else if rec_for.is_some() {
                                let (add, refresh) = recommended_ui(app, ui, &recs);
                                rec_add = add;
                                rec_refresh = refresh;
                            }
                        },
                    );
                }
            }
            let _ = content;
        });

    // ------------------------------------------------ sticky header (on scroll)
    let fade = ((scrolled - (HEADER_H - 140.0)) / 90.0).clamp(0.0, 1.0);
    if fade > 0.0 {
        let bar = Rect::from_min_size(panel.min, Vec2::new(panel.width(), 64.0));
        let base = widgets::mix(
            info.color,
            Color32::BLACK,
            if pal.dark { 0.45 } else { 0.1 },
        );
        let painter = ui.painter_at(panel);
        widgets::gradient(
            &painter,
            bar,
            8.0,
            widgets::with_alpha(base, fade),
            widgets::with_alpha(base, fade),
        );
        // Swallow clicks so rows underneath don't react.
        let _ = ui.interact(bar, ui.id().with("sticky-bar"), Sense::click());
        if fade > 0.6 {
            let a = (fade - 0.6) / 0.4;
            let pb = Rect::from_center_size(
                egui::pos2(bar.left() + PAD + 24.0, bar.center().y),
                Vec2::splat(48.0),
            );
            let pr = ui.interact(pb, ui.id().with("sticky-play"), Sense::click());
            widgets::paint_round_play(
                ui,
                pb,
                pr.hovered(),
                playing_this,
                widgets::with_alpha(pal.accent, a),
                widgets::with_alpha(pal.on_accent, a),
            );
            if pr
                .on_hover_text(if playing_this { "Pause" } else { "Play" })
                .clicked()
            {
                header_action = Some(if playing_this { "pause" } else { "play" });
            }
            widgets::text(
                ui,
                egui::pos2(pb.right() + 16.0, bar.center().y),
                Align2::LEFT_CENTER,
                &info.title,
                24.0,
                Weight::Bold,
                widgets::with_alpha(if pal.dark { Color32::WHITE } else { pal.text }, a),
                bar.width() - 120.0,
            );
        }
    }

    if let (Some(t), Some(pl)) = (rec_add, in_library.as_ref()) {
        app.add_to_playlist(&pl.uri, &pl.name, vec![t.uri.clone()]);
        if let Some(Some(Ok(v))) = app.recommended.get_mut(&pl.uri) {
            v.retain(|x| x.uri != t.uri);
        }
    }
    if rec_refresh {
        if let Some(u) = &rec_for {
            app.load_recommended(u, true);
        }
    }
    if let Some(c) = header_lib {
        app.toggle_in_library(&c);
    }
    // ------------------------------------------------ deferred actions
    let local_tracks = app.local_filtered.clone();
    let play_at = |app: &mut App, i: usize| match &key {
        Some(k) => app.play_list(k, i),
        None => app.send(PlayerCmd::PlayContext {
            name: "Local Files".into(),
            key: Some("local".into()),
            tracks: local_tracks.clone(),
            start: i,
        }),
    };
    app.row_playlist = None;
    app.row_source = None;
    match header_action {
        Some("edit") => {
            if let Some(c) = &in_library {
                app.edit_playlist_details(c);
            }
        }
        Some("delete") => {
            if let Some(c) = in_library.clone() {
                if c.owned {
                    app.confirm_delete = Some(c);
                } else {
                    app.delete_playlist(&c.uri);
                }
            }
        }
        Some("play") => {
            if app.snapshot.context_key
                == key
                    .as_ref()
                    .map(|k| k.context_key())
                    .or(Some("local".into()))
                && app.snapshot.track.is_some()
            {
                app.send(PlayerCmd::Play);
            } else {
                // Respects shuffle: a random first song when it's on.
                let shuffle = app.snapshot.shuffle;
                match &key {
                    Some(k) => app.play_whole(k, shuffle),
                    None => app.play_local_whole(shuffle),
                }
            }
        }
        Some("radio") => {
            if let Some(k) = &key {
                let title = app
                    .lists
                    .peek(k)
                    .map(|l| l.title.clone())
                    .unwrap_or_default();
                app.start_radio(&k.context_key(), &title);
            }
        }
        Some("pause") => app.send(PlayerCmd::Pause),
        Some("shuffle") => {
            let on = !app.snapshot.shuffle;
            app.send(PlayerCmd::SetShuffle(on));
            let here = app.snapshot.context_key
                == key
                    .as_ref()
                    .map(|k| k.context_key())
                    .or(Some("local".into()));
            if on && !(here && app.snapshot.track.is_some()) {
                // Turning shuffle on from a list that isn't playing starts it
                // shuffled, like Spotify.
                match &key {
                    Some(k) => app.play_whole(k, true),
                    None => app.play_local_whole(true),
                }
            } else if on && here && !app.snapshot.playing {
                app.send(PlayerCmd::Play);
            }
        }
        Some("queue") => {
            let tracks = match &key {
                Some(k) => app
                    .lists
                    .get(k)
                    .map(|l| l.tracks.clone())
                    .unwrap_or_default(),
                None => app.local_filtered.clone(),
            };
            if !tracks.is_empty() {
                app.send(PlayerCmd::AddToQueue(tracks));
            }
        }
        Some("rescan") => app.rescan_local(),
        Some("folders") => app.navigate(View::Settings),
        Some("reload") => {
            if let Some(k) = &key {
                app.lists.pop(k);
                if let View::Collection(c) = app.view.clone() {
                    app.navigate_replace(View::Collection(c));
                } else {
                    app.navigate_replace(app.view.clone());
                }
            }
        }
        _ => {}
    }
    if app.local_filter_changed {
        app.local_filter_changed = false;
        app.filter_local();
    }
    if let Some(k) = &key {
        // Finding or sorting needs the whole list: keep loading pages.
        if need_more || (info.has_next && (info.loaded < 50 || order.is_some())) {
            app.load_more(k);
        }
        if let Some(&first) = need_meta.first() {
            app.ensure_meta(k, first);
            if let Some(&last) = need_meta.last() {
                app.ensure_meta(k, last);
            }
        }
        if order.is_some() {
            // ...and the names of every song, a few chunks per frame.
            let stubs: Vec<usize> = app
                .lists
                .peek(k)
                .map(|l| {
                    l.tracks
                        .iter()
                        .enumerate()
                        .filter(|(i, t)| i % 50 == 0 || t.is_stub())
                        .filter(|(_, t)| t.is_stub())
                        .map(|(i, _)| i / 50 * 50)
                        .collect()
                })
                .unwrap_or_default();
            let mut seen = HashSet::new();
            for row in stubs.into_iter().filter(|r| seen.insert(*r)).take(3) {
                app.ensure_meta(k, row);
            }
        }
    }
    // Row clicks: plain selects one, Ctrl/Cmd toggles, Shift selects a range.
    if let Some((i, m)) = app.row_click.take() {
        select_click(
            &mut app.row_multi,
            &mut app.row_sel,
            i,
            m.command,
            m.shift,
            &real,
        );
    }
    let selected_tracks: Vec<TrackRef> = app
        .row_multi
        .iter()
        .filter_map(|&r| match &key {
            Some(k) => app.lists.peek(k).and_then(|l| l.tracks.get(r).cloned()),
            None => app.local_filtered.get(r).cloned(),
        })
        .collect();
    if delete_selected {
        if let Some(pl) = app.row_playlist.clone() {
            let uris: Vec<String> = selected_tracks.iter().map(|t| t.uri.clone()).collect();
            app.remove_from_playlist(&pl, &uris);
            app.row_multi.clear();
        }
    }
    // A menu action on a row inside a multi-selection applies to all of it.
    if let Some((i, action)) = &clicked {
        let on_selection =
            selected_tracks.len() > 1 && real(*i).is_some_and(|r| app.row_multi.contains(&r));
        if on_selection {
            let uris: Vec<String> = selected_tracks.iter().map(|t| t.uri.clone()).collect();
            let handled = match action.clone() {
                RowAction::Queue => {
                    app.send(PlayerCmd::AddToQueue(selected_tracks.clone()));
                    true
                }
                RowAction::AddTo(uri, name) => {
                    app.add_to_playlist(&uri, &name, uris);
                    true
                }
                RowAction::NewPlaylist => {
                    app.create_playlist(uris);
                    true
                }
                RowAction::RemoveFrom(pl) => {
                    app.remove_from_playlist(&pl, &uris);
                    app.row_multi.clear();
                    true
                }
                RowAction::Save => {
                    app.toggle_liked_many(uris);
                    true
                }
                _ => false,
            };
            if handled {
                clicked = None;
            }
        }
    }
    if let Some((i, action)) = clicked {
        match (&key, &order) {
            (Some(k), Some(o)) => {
                // Play in the order shown, like Spotify.
                let tracks: Vec<TrackRef> = app
                    .lists
                    .peek(k)
                    .map(|l| o.iter().filter_map(|&r| l.tracks.get(r).cloned()).collect())
                    .unwrap_or_default();
                let title = app
                    .lists
                    .peek(k)
                    .map(|l| l.title.clone())
                    .unwrap_or_default();
                if let Some(t) = tracks.get(i).cloned() {
                    let ctx = k.context_key();
                    apply_row_action(app, action, &t, move |app| {
                        app.send(PlayerCmd::PlayContext {
                            name: title,
                            key: Some(ctx),
                            tracks,
                            start: i,
                        })
                    });
                }
            }
            _ => {
                let t = match &key {
                    Some(k) => app.lists.get(k).and_then(|l| l.tracks.get(i).cloned()),
                    None => app.local_filtered.get(i).cloned(),
                };
                if let Some(t) = t {
                    apply_row_action(app, action, &t, move |app| play_at(app, i));
                }
            }
        }
    }
}

/// Row selection on click (`i` is the shown row, `real` maps it to the list
/// index): plain selects one, Ctrl/Cmd toggles, Shift selects from the anchor.
fn select_click(
    multi: &mut std::collections::BTreeSet<usize>,
    anchor: &mut Option<usize>,
    i: usize,
    toggle: bool,
    range: bool,
    real: &dyn Fn(usize) -> Option<usize>,
) {
    let Some(r) = real(i) else { return };
    if toggle {
        if !multi.remove(&r) {
            multi.insert(r);
        }
        *anchor = Some(i);
    } else if range {
        let a = anchor.unwrap_or(i);
        *multi = (a.min(i)..=a.max(i)).filter_map(real).collect();
    } else {
        *multi = [r].into();
        *anchor = Some(i);
    }
}

/// Spotify's right side of the playlist action row: a search icon that opens a
/// "Search in playlist" box, and the sort menu.
fn find_and_sort(app: &mut App, ui: &mut egui::Ui, actions: Rect, key: &ListKey) {
    let pal = app.palette;
    let y = actions.center().y;
    // Sort menu (rightmost).
    let label = app.list_find.sort.label();
    let g = widgets::galley(ui, label, 14.0, Weight::Regular, pal.weak, 200.0);
    let sort_rect = Rect::from_min_size(
        egui::pos2(actions.right() - PAD - g.size().x - 30.0, y - 16.0),
        Vec2::new(g.size().x + 30.0, 32.0),
    );
    let sr = ui.interact(sort_rect, ui.id().with("list-sort"), Sense::click());
    let col = if sr.hovered() { pal.text } else { pal.weak };
    ui.painter()
        .galley(egui::pos2(sort_rect.left(), y - g.size().y / 2.0), g, col);
    // Small list glyph after the label.
    for k in 0..3 {
        let yy = y - 4.0 + k as f32 * 4.0;
        let x0 = sort_rect.right() - 18.0;
        ui.painter().hline(
            x0..=x0 + 14.0 - k as f32 * 3.0,
            yy,
            egui::Stroke::new(1.5, col),
        );
    }
    egui::Popup::menu(&sr)
        .id(ui.id().with("list-sort-menu"))
        .align(egui::RectAlign::BOTTOM_END)
        .show(|ui| {
            widgets::menu_style(ui, 200.0);
            ui.label(egui::RichText::new("Sort by").small().color(pal.weak));
            for s in [
                super::ListSort::Custom,
                super::ListSort::Title,
                super::ListSort::Artist,
                super::ListSort::Album,
                super::ListSort::Duration,
            ] {
                let r = widgets::menu_item(ui, s.label(), false, &pal);
                if app.list_find.sort == s {
                    widgets::draw(
                        ui,
                        Rect::from_center_size(
                            egui::pos2(r.rect.right() - 18.0, r.rect.center().y),
                            Vec2::splat(14.0),
                        ),
                        Icon::Saved,
                        pal.accent,
                    );
                }
                if r.clicked() {
                    app.list_find.sort = s;
                    app.list_find.desc = false;
                    ui.close();
                }
            }
        });
    // Search: an icon that grows into a box.
    let open = app.list_find.open || !app.list_find.query.is_empty();
    let t = ui
        .ctx()
        .animate_bool_with_time(ui.id().with("find-open"), open, 0.15);
    let w = 36.0 + 184.0 * t;
    let fr = Rect::from_min_size(
        egui::pos2(sort_rect.left() - 12.0 - w, y - 16.0),
        Vec2::new(w, 32.0),
    );
    if t > 0.0 {
        ui.painter()
            .rect_filled(fr, 4.0, widgets::with_alpha(pal.text, 0.1 * t));
    }
    let icon = Rect::from_center_size(egui::pos2(fr.left() + 18.0, y), Vec2::splat(16.0));
    let ir = ui.interact(icon.expand(8.0), ui.id().with("find-icon"), Sense::click());
    widgets::draw(
        ui,
        icon,
        Icon::Search,
        if ir.hovered() || open {
            pal.text
        } else {
            pal.weak
        },
    );
    let edit_id = ui.id().with(("find-edit", key.context_key()));
    if ir.on_hover_text("Search in playlist").clicked() {
        app.list_find.open = true;
        app.list_find.focus = true;
    }
    if app.list_find.focus && fr.width() > 70.0 {
        // Focus once the box exists (it grows in over a few frames).
        app.list_find.focus = false;
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
    if open && fr.width() > 70.0 {
        let er = ui.put(
            Rect::from_min_max(fr.min + Vec2::new(34.0, 4.0), fr.max - Vec2::new(26.0, 4.0)),
            egui::TextEdit::singleline(&mut app.list_find.query)
                .id(edit_id)
                .frame(egui::Frame::NONE)
                .hint_text("Search in playlist")
                .vertical_align(egui::Align::Center),
        );
        if !app.list_find.query.is_empty() {
            let x = Rect::from_center_size(egui::pos2(fr.right() - 14.0, y), Vec2::splat(12.0));
            let xr = ui.interact(x.expand(6.0), ui.id().with("find-clear"), Sense::click());
            widgets::draw(
                ui,
                x,
                Icon::Close,
                if xr.hovered() { pal.text } else { pal.weak },
            );
            if xr.clicked() {
                app.list_find.query.clear();
                ui.memory_mut(|m| m.request_focus(edit_id));
            }
        }
        if er.lost_focus() && app.list_find.query.is_empty() {
            app.list_find.open = false;
        }
        if er.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.list_find.query.clear();
            app.list_find.open = false;
        }
    }
    // Ctrl+F opens it, like Spotify.
    if ui.input_mut(|i| {
        i.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND,
            egui::Key::F,
        ))
    }) {
        app.list_find.open = true;
        app.list_find.focus = true;
    }
}

// ====================================================================== cards

#[derive(Clone)]
pub(super) enum CardKind {
    Coll(Collection),
    Track(ListKey, usize, TrackRef),
    /// "<Artist> Radio": starts radio seeded by the artist.
    Radio(Collection),
}

struct CardResult {
    open: bool,
    play: bool,
}

const CARD_TEXT_H: f32 = 58.0;

/// A Spotify card: square art (round for artists), title, subtitle; on hover a
/// green play button rises from the artwork's corner.
fn card(app: &mut App, ui: &mut egui::Ui, rect: Rect, kind: &CardKind) -> CardResult {
    let pal = app.palette;
    let id = ui.id().with(match kind {
        CardKind::Coll(c) => c.uri.clone(),
        CardKind::Track(k, i, _) => format!("{}#{i}", k.context_key()),
        CardKind::Radio(c) => format!("radio:{}", c.uri),
    });
    let r = ui.interact(rect, id, Sense::click());
    let hovered = r.hovered() || ui.rect_contains_pointer(rect);
    let hv = ui
        .ctx()
        .animate_bool_with_time(id.with("hv"), hovered, 0.15);
    if hv > 0.0 {
        ui.painter()
            .rect_filled(rect, 8.0, widgets::with_alpha(pal.text, 0.08 * hv));
    }
    if hv >= 1.0 {
        if let CardKind::Coll(c) = kind {
            app.prefetch(c);
        }
    }
    let inner = rect.shrink(12.0);
    let art = Rect::from_min_size(inner.min, Vec2::splat(inner.width()));
    let (image, title, sub, round) = match kind {
        CardKind::Coll(c) => {
            let sub = match c.kind {
                CollectionKind::Artist => "Artist".to_string(),
                CollectionKind::Album => {
                    if c.subtitle.is_empty() {
                        "Album".into()
                    } else {
                        c.subtitle.clone()
                    }
                }
                CollectionKind::Playlist => {
                    let who = c.subtitle.trim_start_matches("by ").to_string();
                    if who.is_empty() {
                        "Playlist".into()
                    } else {
                        format!("By {who}")
                    }
                }
            };
            (
                c.image.clone(),
                c.name.clone(),
                sub,
                c.kind == CollectionKind::Artist,
            )
        }
        CardKind::Track(_, _, t) => (
            t.image_large.clone().or(t.image.clone()),
            t.name.clone(),
            t.artists.clone(),
            false,
        ),
        CardKind::Radio(c) => (
            c.image.clone(),
            format!("{} Radio", c.name),
            format!("With {} and more", c.name),
            false,
        ),
    };
    if ui.is_rect_visible(rect) {
        widgets::shadow(
            ui.painter(),
            art,
            if round { art.width() / 2.0 } else { 6.0 },
        );
        let tex = image.as_deref().and_then(|u| app.art.get(u, LARGE_PX));
        widgets::image(
            ui.painter(),
            art,
            tex,
            if round { art.width() / 2.0 } else { 6.0 },
            pal.panel,
            pal.weak,
        );
        if let CardKind::Radio(_) = kind {
            // Spotify's radio cover: the artist in a circle on a colour block, "RADIO" on top.
            let tint = image
                .as_deref()
                .and_then(|u| app.art.color(u))
                .unwrap_or(pal.accent);
            ui.painter()
                .rect_filled(art, 6.0, widgets::mix(tint, Color32::BLACK, 0.25));
            let c = Rect::from_center_size(art.center() + Vec2::new(0.0, 8.0), art.size() * 0.62);
            widgets::image(ui.painter(), c, tex, c.width() / 2.0, pal.panel, pal.weak);
            widgets::text(
                ui,
                art.left_top() + Vec2::new(12.0, 18.0),
                Align2::LEFT_CENTER,
                "RADIO",
                12.0,
                Weight::Bold,
                Color32::WHITE,
                art.width(),
            );
        }
        widgets::text(
            ui,
            egui::pos2(inner.left(), art.bottom() + 18.0),
            Align2::LEFT_CENTER,
            &title,
            15.0,
            Weight::Semibold,
            pal.text,
            inner.width(),
        );
        widgets::text(
            ui,
            egui::pos2(inner.left(), art.bottom() + 40.0),
            Align2::LEFT_CENTER,
            &sub,
            13.5,
            Weight::Regular,
            pal.weak,
            inner.width(),
        );
    }
    let mut play = false;
    let playing_here = match kind {
        CardKind::Coll(c) if c.kind != CollectionKind::Artist => {
            app.view_playing(&View::Collection(c.clone()))
        }
        CardKind::Track(_, _, t) => {
            app.snapshot.playing && app.snapshot.track.as_ref().is_some_and(|c| c.uri == t.uri)
        }
        _ => false,
    };
    // Spotify's card play button: slides up and fades in on hover, and stays
    // (as pause) while the card is what's playing.
    let t = ui
        .ctx()
        .animate_bool_with_time(id.with("lift"), hovered || playing_here, 0.18);
    if t > 0.0 {
        let e = 1.0 - (1.0 - t).powi(3);
        let pb = Rect::from_center_size(
            art.right_bottom() - Vec2::new(32.0, 32.0 - 10.0 * (1.0 - e)),
            Vec2::splat(48.0),
        );
        ui.painter().circle_filled(
            pb.center() + Vec2::new(0.0, 4.0),
            25.0,
            Color32::from_black_alpha((90.0 * e) as u8),
        );
        let pr = ui.interact(pb, id.with("play"), Sense::click());
        widgets::paint_round_play(
            ui,
            pb,
            pr.hovered(),
            playing_here,
            widgets::with_alpha(pal.accent, e),
            widgets::with_alpha(pal.on_accent, e),
        );
        if pr.clicked() && (hovered || playing_here) {
            play = true;
        }
    }
    // Right-click: the same menu as in Your Library (play, queue, add/remove…).
    let mut lib_act = None;
    match kind {
        CardKind::Coll(c) | CardKind::Radio(c) => {
            let c = c.clone();
            r.context_menu(|ui| {
                widgets::menu_style(ui, 240.0);
                super::sidebar::library_menu(app, ui, &c, &mut lib_act);
            });
        }
        CardKind::Track(_, _, t) => super::views::track_context_menu(app, &r, t),
    }
    if let Some(a) = lib_act {
        super::sidebar::apply_lib_action(app, a);
    }
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    CardResult {
        open: r.clicked() && !play,
        play,
    }
}

fn card_width(avail: f32) -> (usize, f32) {
    let min = 168.0;
    let n = ((avail + 8.0) / (min + 8.0)).floor().max(1.0) as usize;
    let n = n.min(8);
    (n, (avail - (n as f32 - 1.0) * 8.0) / n as f32)
}

/// One row of cards with a heading and "Show all".
pub(super) fn shelf(
    app: &mut App,
    ui: &mut egui::Ui,
    title: &str,
    items: Vec<CardKind>,
    show_all: Option<View>,
) {
    if items.is_empty() {
        return;
    }
    let pal = app.palette;
    let w = ui.available_width();
    let (head, _) = ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::hover());
    let tr = widgets::text(
        ui,
        head.left_center(),
        Align2::LEFT_CENTER,
        title,
        24.0,
        Weight::Bold,
        pal.text,
        w - 100.0,
    );
    if let Some(v) = show_all {
        let sr = widgets::galley(ui, "Show all", 14.0, Weight::Bold, pal.weak, 100.0);
        let rect = Align2::RIGHT_CENTER.anchor_size(head.right_center(), sr.size());
        let r = ui.interact(rect, ui.id().with(("show-all", title)), Sense::click());
        let col = if r.hovered() { pal.text } else { pal.weak };
        ui.painter().galley(rect.min, sr, col);
        if r.hovered() {
            ui.painter()
                .hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, col));
        }
        let tr_hit = ui.interact(tr, ui.id().with(("shelf-title", title)), Sense::click());
        if r.clicked() || tr_hit.clicked() {
            app.navigate(v);
        }
    }
    let (n, cw) = card_width(w);
    let ch = cw - 24.0 + 24.0 + CARD_TEXT_H;
    let (row, _) = ui.allocate_exact_size(Vec2::new(w, ch), Sense::hover());
    for (i, item) in items.iter().take(n).enumerate() {
        let rect = Rect::from_min_size(
            egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()),
            Vec2::new(cw, ch),
        );
        let res = card(app, ui, rect, item);
        handle_card(app, item, res);
    }
    ui.add_space(24.0);
}

fn handle_card(app: &mut App, item: &CardKind, res: CardResult) {
    // The play button on what's already playing pauses / resumes it.
    if res.play {
        let current = match item {
            CardKind::Coll(c) if c.kind != CollectionKind::Artist => app
                .snapshot
                .context_key
                .as_deref()
                .is_some_and(|k| k == c.uri),
            CardKind::Track(_, _, t) => app.snapshot.track.as_ref().is_some_and(|c| c.uri == t.uri),
            _ => false,
        };
        if current && app.snapshot.track.is_some() {
            app.send(if app.snapshot.playing {
                PlayerCmd::Pause
            } else {
                PlayerCmd::Play
            });
            return;
        }
    }
    match item {
        CardKind::Coll(c) => {
            if res.play && c.kind != CollectionKind::Artist {
                // Plays in place, like Spotify (no page change).
                app.play_collection(c);
            } else if res.open || res.play {
                if res.play {
                    app.autoplay = Some(ListKey::Collection(c.uri.clone()));
                }
                app.navigate(View::Collection(c.clone()));
                if res.play {
                    app.try_autoplay();
                }
            }
        }
        CardKind::Radio(c) => {
            if res.play || res.open {
                app.start_radio(&c.uri, &c.name);
            }
        }
        CardKind::Track(k, i, t) => {
            if res.play || res.open {
                if app.lists.contains(k) {
                    app.play_list(k, *i);
                } else {
                    app.play_tracks("Search", vec![t.clone()], 0);
                }
            }
        }
    }
}

// ====================================================================== home

fn greeting() -> &'static str {
    use chrono::Timelike;
    match chrono::Local::now().hour() {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

fn home(app: &mut App, ui: &mut egui::Ui) {
    app.load_home();
    let pal = app.palette;
    let full = ui.max_rect();
    let tint = app
        .home_hover_color
        .or_else(|| super::panels::current_color(app))
        .unwrap_or(Color32::from_rgb(0x3a, 0x3a, 0x6a));
    let shown = ui
        .ctx()
        .animate_value_with_time(egui::Id::new("home-tint"), 1.0, 0.3);
    widgets::gradient(
        ui.painter(),
        Rect::from_min_size(full.min, Vec2::new(full.width(), 320.0)),
        8.0,
        widgets::mix(pal.bg, tint, 0.45 * shown),
        pal.bg,
    );
    app.home_hover_color = None;
    egui::ScrollArea::vertical()
        .id_salt("home")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(PAD as i8, 16))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    ui.horizontal(|ui| {
                        for (label, view) in [
                            ("All", View::Home),
                            ("Playlists", View::Playlists),
                            ("Albums", View::Albums),
                            ("Artists", View::Artists),
                        ] {
                            if widgets::chip(ui, label, view == View::Home, &pal).clicked() {
                                app.navigate(view);
                            }
                        }
                    });
                    ui.add_space(8.0);
                    widgets::text(
                        ui,
                        ui.cursor().min + Vec2::new(0.0, 16.0),
                        Align2::LEFT_CENTER,
                        greeting(),
                        30.0,
                        Weight::Bold,
                        pal.text,
                        ui.available_width(),
                    );
                    ui.add_space(40.0);
                    shortcuts(app, ui);
                    ui.add_space(28.0);

                    let recent: Vec<CardKind> = app
                        .lists
                        .peek(&ListKey::Recent)
                        .map(|l| {
                            l.tracks
                                .iter()
                                .enumerate()
                                .take(12)
                                .map(|(i, t)| CardKind::Track(ListKey::Recent, i, t.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let jump: Vec<CardKind> = app
                        .home
                        .mix
                        .albums
                        .iter()
                        .cloned()
                        .map(CardKind::Coll)
                        .collect();
                    shelf(app, ui, "Jump back in", jump, None);
                    shelf(app, ui, "Recently played", recent, None);
                    let playlists: Vec<CardKind> = app
                        .playlists
                        .items
                        .iter()
                        .take(12)
                        .cloned()
                        .map(CardKind::Coll)
                        .collect();
                    shelf(app, ui, "Your playlists", playlists, Some(View::Playlists));
                    let artists: Vec<CardKind> = app
                        .home
                        .artists
                        .iter()
                        .take(12)
                        .cloned()
                        .map(CardKind::Coll)
                        .collect();
                    shelf(app, ui, "Your top artists", artists, None);
                    let radios: Vec<CardKind> = app
                        .home
                        .artists
                        .iter()
                        .take(8)
                        .cloned()
                        .map(CardKind::Radio)
                        .collect();
                    shelf(app, ui, "Your mixes", radios, None);
                    let top: Vec<CardKind> = app
                        .lists
                        .peek(&ListKey::Top)
                        .map(|l| {
                            l.tracks
                                .iter()
                                .enumerate()
                                .take(12)
                                .map(|(i, t)| CardKind::Track(ListKey::Top, i, t.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    shelf(app, ui, "On repeat", top, None);
                    let albums: Vec<CardKind> = app
                        .albums
                        .items
                        .iter()
                        .take(12)
                        .cloned()
                        .map(CardKind::Coll)
                        .collect();
                    shelf(app, ui, "Albums you saved", albums, Some(View::Albums));
                    if app.playlists.items.len() > 12 {
                        let more: Vec<CardKind> = app
                            .playlists
                            .items
                            .iter()
                            .skip(12)
                            .take(12)
                            .cloned()
                            .map(CardKind::Coll)
                            .collect();
                        shelf(
                            app,
                            ui,
                            "More of your playlists",
                            more,
                            Some(View::Playlists),
                        );
                    }
                    if !app.albums.started && app.backend.is_some() {
                        app.load_albums();
                    }
                    if app.backend.is_none() && !app.demo && app.playlists.items.is_empty() {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("Connect your Spotify library, or paste a public playlist link below. Local files work without signing in.")
                                .color(pal.weak),
                        );
                        if ui.add_enabled(!app.login.busy, egui::Button::new("Connect Spotify library")).clicked() { app.start_account_login(true); }
                        ui.add(egui::TextEdit::singleline(&mut app.free_import_path).hint_text("Paste a Spotify playlist, album or song link").desired_width(360.0));
                        if ui.add_enabled(!app.free_importing, egui::Button::new("Import link")).clicked() { app.import_free_library(); }
                        if ui.button("Play local files").clicked() { app.show_local(); }
                    } else if !app.home.loaded && app.backend.is_some() {
                        widgets::skeleton_cards(ui, 2, pal.text);
                    }
                    if let Some(e) = app.home.error.clone() {
                        ui.label(egui::RichText::new(e).small().color(pal.weak));
                    }
                });
        });
}

/// 2×4 grid of quick-access tiles (Liked Songs, Local Files, top playlists).
fn shortcuts(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    let mut items: Vec<(String, Option<String>, View, Option<ListKey>)> = vec![
        (
            "Liked Songs".into(),
            None,
            View::Liked,
            Some(ListKey::Liked),
        ),
        ("Local Files".into(), None, View::Local, None),
    ];
    items.extend(app.playlists.items.iter().take(6).map(|c| {
        (
            c.name.clone(),
            c.image.clone(),
            View::Collection(c.clone()),
            Some(ListKey::Collection(c.uri.clone())),
        )
    }));
    let w = ui.available_width();
    let cols = if w >= 900.0 {
        4
    } else if w >= 460.0 {
        2
    } else {
        1
    };
    let tile_w = (w - (cols - 1) as f32 * 8.0) / cols as f32;
    let rows = items.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(
        Vec2::new(
            w,
            rows as f32 * 64.0 + (rows.saturating_sub(1)) as f32 * 8.0,
        ),
        Sense::hover(),
    );
    for (i, (name, image, view, _key)) in items.into_iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let rect = Rect::from_min_size(
            area.min + Vec2::new(c as f32 * (tile_w + 8.0), r as f32 * 72.0),
            Vec2::new(tile_w, 64.0),
        );
        let id = ui.id().with(("shortcut", i));
        let resp = ui.interact(rect, id, Sense::click());
        let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
        ui.painter().rect_filled(
            rect,
            4.0,
            widgets::with_alpha(pal.text, if hovered { 0.17 } else { 0.09 }),
        );
        let art = Rect::from_min_size(rect.min, Vec2::splat(64.0));
        match &view {
            View::Liked => {
                widgets::gradient(
                    ui.painter(),
                    art,
                    4.0,
                    Color32::from_rgb(0x45, 0x0a, 0xf5),
                    Color32::from_rgb(0x8e, 0xb8, 0xe5),
                );
                widgets::draw(
                    ui,
                    Rect::from_center_size(art.center(), Vec2::splat(26.0)),
                    Icon::Heart,
                    Color32::WHITE,
                );
            }
            View::Local => {
                widgets::gradient(
                    ui.painter(),
                    art,
                    4.0,
                    Color32::from_rgb(0x0f, 0x6b, 0x45),
                    Color32::from_rgb(0x1e, 0xd7, 0x60),
                );
                widgets::draw(
                    ui,
                    Rect::from_center_size(art.center(), Vec2::splat(26.0)),
                    Icon::Local,
                    Color32::WHITE,
                );
            }
            _ => {
                let tex = image.as_deref().and_then(|u| app.art.get(u, THUMB_PX));
                widgets::image(ui.painter(), art, tex, 4.0, pal.panel, pal.weak);
                if hovered {
                    if let Some(c) = image.as_deref().and_then(|u| app.art.color(u)) {
                        app.home_hover_color = Some(c);
                    }
                }
            }
        }
        let text_w = rect.width() - 64.0 - 16.0 - if hovered { 48.0 } else { 0.0 };
        widgets::text(
            ui,
            egui::pos2(art.right() + 14.0, rect.center().y),
            Align2::LEFT_CENTER,
            &name,
            14.5,
            Weight::Bold,
            pal.text,
            text_w,
        );
        let mut play = false;
        // Like Spotify: the button stays (as pause) while this one is playing.
        let playing_here = app.view_playing(&view);
        if hovered || playing_here {
            let pb = Rect::from_center_size(
                egui::pos2(rect.right() - 28.0, rect.center().y),
                Vec2::splat(36.0),
            );
            let pr = ui.interact(pb, id.with("play"), Sense::click());
            widgets::paint_round_play(
                ui,
                pb,
                pr.hovered(),
                playing_here,
                pal.accent,
                pal.on_accent,
            );
            play = pr.clicked();
        }
        if play {
            app.toggle_play_view(&view);
        } else if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            app.navigate(view);
        }
    }
}

// ====================================================================== grids

fn page_title(app: &App, ui: &mut egui::Ui, title: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 64.0), Sense::hover());
    widgets::text(
        ui,
        r.left_center(),
        Align2::LEFT_CENTER,
        title,
        32.0,
        Weight::Black,
        app.palette.text,
        r.width(),
    );
}

fn grid_page(app: &mut App, ui: &mut egui::Ui, kind: CollectionKind) {
    let pal = app.palette;
    let (title, count, loading, error, more) = match kind {
        CollectionKind::Playlist => (
            "Playlists",
            app.playlists.items.len(),
            app.playlists.loading,
            app.playlists.error.clone(),
            app.playlists.next.is_some(),
        ),
        CollectionKind::Album => (
            "Albums",
            app.albums.items.len(),
            app.albums.loading,
            app.albums.error.clone(),
            app.albums.next.is_some(),
        ),
        CollectionKind::Artist => (
            "Artists",
            app.artists.items.len(),
            app.artists.loading,
            app.artists.error.clone(),
            app.artists.cursor.is_some(),
        ),
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(PAD as i8, 12))
        .show(ui, |ui| {
            page_title(app, ui, title);
            ui.horizontal(|ui| {
                for (label, view, k) in [
                    ("Playlists", View::Playlists, CollectionKind::Playlist),
                    ("Albums", View::Albums, CollectionKind::Album),
                    ("Artists", View::Artists, CollectionKind::Artist),
                ] {
                    if widgets::chip(ui, label, k == kind, &pal).clicked() {
                        app.navigate(view);
                    }
                }
                if loading {
                    ui.spinner();
                }
            });
            ui.add_space(12.0);
            if let Some(e) = &error {
                ui.label(egui::RichText::new(e).color(Color32::from_rgb(0xf1, 0x5e, 0x6c)));
            }
            if count == 0 && loading {
                widgets::skeleton_cards(ui, 2, pal.text);
            }
            if count == 0 {
                if !loading && error.is_none() {
                    let (icon, heading, body) = match kind {
                        CollectionKind::Playlist => (
                            Icon::Library,
                            "Your playlists will appear here",
                            "Playlists you make or follow in Spotify show up here.",
                        ),
                        CollectionKind::Album => (
                            Icon::Album,
                            "Albums you save will appear here",
                            "Save albums in any Spotify app and they'll show up here.",
                        ),
                        CollectionKind::Artist => (
                            Icon::User,
                            "Artists you follow will appear here",
                            "Follow artists in any Spotify app and they'll show up here.",
                        ),
                    };
                    let top = egui::pos2(ui.max_rect().center().x, ui.cursor().top() + 40.0);
                    let w = ui.available_width();
                    if widgets::empty_state(
                        ui,
                        top,
                        w,
                        icon,
                        heading,
                        body,
                        Some("Go to Home"),
                        &pal,
                    ) {
                        app.navigate(View::Home);
                    }
                }
                return;
            }
            let w = ui.available_width();
            let (n, cw) = card_width(w);
            let ch = cw + CARD_TEXT_H;
            let rows = count.div_ceil(n);
            let mut need_more = false;
            let mut act: Option<(CardKind, CardResult)> = None;
            ui.spacing_mut().item_spacing.y = 8.0;
            egui::ScrollArea::vertical()
                .id_salt(("grid", title))
                .auto_shrink([false, false])
                .show_rows(ui, ch, rows, |ui, range| {
                    if range.end + 2 >= rows && more {
                        need_more = true;
                    }
                    for r in range {
                        let (row, _) = ui.allocate_exact_size(Vec2::new(w, ch), Sense::hover());
                        for c in 0..n {
                            let i = r * n + c;
                            let item = match kind {
                                CollectionKind::Playlist => app.playlists.items.get(i).cloned(),
                                CollectionKind::Album => app.albums.items.get(i).cloned(),
                                CollectionKind::Artist => app.artists.items.get(i).cloned(),
                            };
                            let Some(item) = item else { break };
                            let rect = Rect::from_min_size(
                                egui::pos2(row.left() + c as f32 * (cw + 8.0), row.top()),
                                Vec2::new(cw, ch),
                            );
                            let k = CardKind::Coll(item);
                            let res = card(app, ui, rect, &k);
                            if res.open || res.play {
                                act = Some((k, res));
                            }
                        }
                    }
                });
            if let Some((k, res)) = act {
                handle_card(app, &k, res);
            }
            if need_more {
                match kind {
                    CollectionKind::Playlist => app.load_playlists(),
                    CollectionKind::Album => app.load_albums(),
                    CollectionKind::Artist => app.load_artists(),
                }
            }
        });
}

// ====================================================================== artist

fn artist_page(app: &mut App, ui: &mut egui::Ui, c: &Collection) {
    use crate::model::ReleaseGroup;
    let pal = app.palette;
    let key = ListKey::Collection(c.uri.clone());
    let (title, list_image, tracks, loading) = match app.lists.get(&key) {
        Some(l) => (
            l.title.clone(),
            l.image.clone().or(c.image.clone()),
            l.tracks.clone(),
            l.loading,
        ),
        None => (c.name.clone(), c.image.clone(), Vec::new(), true),
    };
    app.load_artist_info(&c.uri);
    let info = app
        .artist_info
        .get(&c.uri)
        .cloned()
        .flatten()
        .and_then(|r| r.ok());
    let releases = app.artist_albums.get(&c.uri).cloned().unwrap_or_default();
    // The artist's own picture first; an album cover only as a last resort.
    let portrait = info
        .as_ref()
        .and_then(|i| i.portrait.clone())
        .or(list_image);
    let image = portrait
        .clone()
        .or_else(|| releases.first().and_then(|r| r.coll.image.clone()));
    let color = image
        .as_deref()
        .and_then(|u| {
            let _ = app.art.get(u, LARGE_PX);
            app.art.color(u)
        })
        .unwrap_or(Color32::from_rgb(0x53, 0x53, 0x53));
    let need_meta: Vec<usize> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_stub())
        .map(|(i, _)| i)
        .collect();
    let ctx_key = key.context_key();
    let playing_here = app.snapshot.context_key.as_deref() == Some(ctx_key.as_str());
    let mut clicked = None;
    let mut play_all = false;
    let mut shuffle_click = false;
    let mut follow_click = false;
    let mut radio_click = false;
    let mut liked_click = false;
    // Page-local state: Popular expanded, discography filter, show-all grids.
    let st_id = egui::Id::new(("artist-state", &c.uri));
    let (mut more_popular, mut disc_filter, mut disc_all, mut fans_all): (bool, u8, bool, bool) =
        ui.ctx()
            .data_mut(|d| d.get_temp(st_id).unwrap_or((false, 0, false, false)));
    // Songs by this artist in your Liked Songs (when that list is loaded).
    let liked_count = app
        .lists
        .peek(&ListKey::Liked)
        .map(|l| {
            l.tracks
                .iter()
                .filter(|t| t.artist_uri.as_deref() == Some(c.uri.as_str()))
                .count()
        })
        .unwrap_or(0);
    egui::ScrollArea::vertical()
        .id_salt(("artist", &c.uri))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let w = ui.available_width();
            // ------------------------------------------------ header
            let banner = portrait.is_some();
            let head_h = if banner { 340.0 } else { 300.0 };
            let (head, _) = ui.allocate_exact_size(Vec2::new(w, head_h), Sense::hover());
            if banner {
                // Spotify's artist banner: the picture across the whole header,
                // darkened towards the bottom where the name sits.
                let tex = portrait
                    .as_deref()
                    .and_then(|u| app.art.get_sized(u, LARGE_PX));
                let clip = ui.painter().with_clip_rect(head);
                widgets::image_cover(&clip, head, tex, 8.0, color);
                widgets::gradient(
                    ui.painter(),
                    head,
                    8.0,
                    Color32::from_black_alpha(10),
                    Color32::from_black_alpha(170),
                );
            } else {
                widgets::gradient(
                    ui.painter(),
                    head,
                    8.0,
                    color,
                    widgets::mix(color, pal.bg, 0.7),
                );
                let pic = Rect::from_min_size(
                    egui::pos2(head.left() + PAD, head.bottom() - PAD - 220.0),
                    Vec2::splat(220.0),
                );
                widgets::shadow(ui.painter(), pic, 110.0);
                let tex = image.as_deref().and_then(|u| app.art.get(u, LARGE_PX));
                widgets::image(ui.painter(), pic, tex, 110.0, pal.panel, pal.weak);
            }
            let tx = if banner {
                head.left() + PAD
            } else {
                head.left() + PAD + 244.0
            };
            let tw = head.right() - tx - PAD;
            let size = title_size(ui, &title, tw).max(48.0);
            let name_r = widgets::text(
                ui,
                egui::pos2(tx, head.bottom() - 52.0),
                Align2::LEFT_BOTTOM,
                &title,
                size,
                Weight::Black,
                Color32::WHITE,
                tw,
            );
            // Badge line above the name.
            widgets::text(
                ui,
                egui::pos2(tx, name_r.top() - 6.0),
                Align2::LEFT_BOTTOM,
                "Artist",
                14.0,
                Weight::Semibold,
                Color32::WHITE,
                tw,
            );
            // Facts under the name: popularity and how much music there is.
            let own = releases
                .iter()
                .filter(|r| r.group != ReleaseGroup::AppearsOn)
                .count();
            let mut facts = Vec::new();
            if let Some(i) = &info {
                if i.popularity > 0 {
                    facts.push(format!("Popularity {}/100", i.popularity));
                }
            }
            if own > 0 {
                facts.push(format!("{own} releases"));
            }
            if app.is_following(&c.uri) {
                facts.push("You follow this artist".into());
            }
            widgets::text(
                ui,
                egui::pos2(tx, head.bottom() - 26.0),
                Align2::LEFT_CENTER,
                &facts.join(" • "),
                15.0,
                Weight::Regular,
                Color32::WHITE,
                tw,
            );

            // ------------------------------------------------ actions
            ui.add_space(-ui.spacing().item_spacing.y);
            let (act, _) = ui.allocate_exact_size(Vec2::new(w, ACTIONS_H), Sense::hover());
            widgets::gradient(
                ui.painter(),
                act,
                0.0,
                widgets::mix(color, pal.bg, 0.78),
                pal.bg,
            );
            let pb = Rect::from_center_size(
                egui::pos2(act.left() + PAD + 28.0, act.center().y),
                Vec2::splat(56.0),
            );
            let pr = ui.interact(pb, ui.id().with("artist-play"), Sense::click());
            let showing_pause = playing_here && app.snapshot.playing;
            widgets::paint_round_play(
                ui,
                pb,
                pr.hovered(),
                showing_pause,
                pal.accent,
                pal.on_accent,
            );
            if pr
                .on_hover_text(if showing_pause { "Pause" } else { "Play" })
                .clicked()
            {
                play_all = true;
            }
            // Shuffle, like Spotify's artist page.
            let sh = Rect::from_center_size(
                egui::pos2(pb.right() + 30.0, act.center().y),
                Vec2::splat(36.0),
            );
            let shr = ui.interact(sh, ui.id().with("artist-shuffle"), Sense::click());
            let on = app.snapshot.shuffle;
            widgets::draw(
                ui,
                Rect::from_center_size(sh.center(), Vec2::splat(28.0)),
                Icon::Shuffle,
                if on {
                    pal.accent
                } else if shr.hovered() {
                    pal.text
                } else {
                    pal.weak
                },
            );
            if on {
                ui.painter().circle_filled(
                    egui::pos2(sh.center().x, sh.bottom() + 2.0),
                    2.0,
                    pal.accent,
                );
            }
            if shr
                .on_hover_text(if on {
                    "Disable shuffle"
                } else {
                    "Enable shuffle"
                })
                .clicked()
            {
                shuffle_click = true;
            }
            let following = app.is_following(&c.uri);
            let label = if following { "Following" } else { "Follow" };
            let g = widgets::galley(ui, label, 14.0, Weight::Bold, pal.text, 200.0);
            let fb = Rect::from_min_size(
                egui::pos2(sh.right() + 16.0, act.center().y - 16.0),
                Vec2::new(g.size().x + 32.0, 32.0),
            );
            let fr = ui.interact(fb, ui.id().with("artist-follow"), Sense::click());
            let h = ui.ctx().animate_bool_with_time(fr.id, fr.hovered(), 0.12);
            ui.painter().rect_stroke(
                fb.expand(h),
                16.0,
                egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.5 + 0.5 * h)),
                egui::StrokeKind::Inside,
            );
            ui.painter()
                .galley(fb.center() - g.size() / 2.0, g, pal.text);
            if fr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                follow_click = true;
            }
            let more = Rect::from_center_size(
                egui::pos2(fb.right() + 32.0, act.center().y),
                Vec2::splat(36.0),
            );
            let mr = ui.interact(more, ui.id().with("artist-more"), Sense::click());
            widgets::draw(
                ui,
                Rect::from_center_size(more.center(), Vec2::splat(26.0)),
                Icon::More,
                if mr.hovered() { pal.text } else { pal.weak },
            );
            let mr = mr.on_hover_text(format!("More options for {title}"));
            egui::Popup::menu(&mr)
                .id(ui.id().with("artist-menu"))
                .show(|ui| {
                    widgets::menu_style(ui, 220.0);
                    if widgets::menu_item(
                        ui,
                        if following { "Unfollow" } else { "Follow" },
                        false,
                        &pal,
                    )
                    .clicked()
                    {
                        follow_click = true;
                        ui.close();
                    }
                    if widgets::menu_item(ui, "Go to artist radio", false, &pal).clicked() {
                        radio_click = true;
                        ui.close();
                    }
                    ui.separator();
                    if widgets::menu_item(ui, "Copy link to artist", false, &pal).clicked() {
                        ui.ctx().copy_text(format!(
                            "https://open.spotify.com/artist/{}",
                            crate::model::uri_id(&c.uri)
                        ));
                        ui.close();
                    }
                    if widgets::menu_item(ui, "Open in Spotify app", true, &pal).clicked() {
                        let _ = open::that_detached(&c.uri);
                        ui.close();
                    }
                });

            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(PAD as i8, 0))
                .show(ui, |ui| {
                    // ------------------------------------------------ popular
                    if !tracks.is_empty() || loading {
                        ui.label(
                            egui::RichText::new("Popular").font(widgets::font(24.0, Weight::Bold)),
                        );
                        ui.add_space(8.0);
                    }
                    if tracks.is_empty() && loading {
                        widgets::skeleton_rows(ui, 5, pal.text);
                    }
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let shown = if more_popular { 10 } else { 5 };
                    for (i, t) in tracks.iter().take(shown).enumerate() {
                        if let Some(a) = track_row(app, ui, t, i, false) {
                            clicked = Some((i, a));
                        }
                    }
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if tracks.len() > 5 {
                        ui.add_space(8.0);
                        let r = ui.add(
                            egui::Label::new(
                                egui::RichText::new(if more_popular {
                                    "Show less"
                                } else {
                                    "See more"
                                })
                                .font(widgets::font(14.0, Weight::Bold))
                                .color(pal.weak),
                            )
                            .sense(Sense::click()),
                        );
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            more_popular = !more_popular;
                        }
                    }
                    // ------------------------------------------------ liked songs
                    if liked_count > 0 {
                        ui.add_space(20.0);
                        ui.label(
                            egui::RichText::new("Liked Songs")
                                .font(widgets::font(24.0, Weight::Bold)),
                        );
                        ui.add_space(8.0);
                        let (row, r) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width().min(420.0), 64.0),
                            Sense::click(),
                        );
                        if r.hovered() {
                            ui.painter()
                                .rect_filled(row, 6.0, widgets::with_alpha(pal.text, 0.06));
                        }
                        let pic =
                            Rect::from_min_size(row.min + Vec2::new(4.0, 4.0), Vec2::splat(56.0));
                        let tex = image
                            .as_deref()
                            .and_then(|u| app.art.get(u, super::art::THUMB_PX));
                        widgets::image(ui.painter(), pic, tex, 28.0, pal.panel, pal.weak);
                        let badge = egui::pos2(pic.right() - 8.0, pic.bottom() - 8.0);
                        ui.painter().circle_filled(badge, 10.0, pal.accent);
                        widgets::draw(
                            ui,
                            Rect::from_center_size(badge, Vec2::splat(11.0)),
                            Icon::Heart,
                            Color32::WHITE,
                        );
                        widgets::text(
                            ui,
                            egui::pos2(pic.right() + 14.0, row.center().y - 9.0),
                            Align2::LEFT_CENTER,
                            &format!(
                                "You've liked {liked_count} song{}",
                                if liked_count == 1 { "" } else { "s" }
                            ),
                            15.0,
                            Weight::Bold,
                            pal.text,
                            row.width() - 80.0,
                        );
                        widgets::text(
                            ui,
                            egui::pos2(pic.right() + 14.0, row.center().y + 11.0),
                            Align2::LEFT_CENTER,
                            &format!("By {title}"),
                            13.0,
                            Weight::Regular,
                            pal.weak,
                            row.width() - 80.0,
                        );
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            liked_click = true;
                        }
                    }
                    ui.add_space(28.0);

                    // ------------------------------------------------ discography
                    let own: Vec<&crate::model::Release> = releases
                        .iter()
                        .filter(|r| r.group != ReleaseGroup::AppearsOn)
                        .collect();
                    if !own.is_empty() {
                        let mut popular: Vec<&crate::model::Release> = own.clone();
                        // Popular releases: the most played, newest breaking ties.
                        popular.sort_by_key(|r| {
                            (std::cmp::Reverse(r.popularity), std::cmp::Reverse(r.year))
                        });
                        let filters: Vec<(u8, &str, Vec<&crate::model::Release>)> = vec![
                            (0, "Popular releases", popular),
                            (
                                1,
                                "Albums",
                                own.iter()
                                    .copied()
                                    .filter(|r| r.group == ReleaseGroup::Album)
                                    .collect(),
                            ),
                            (
                                2,
                                "Singles and EPs",
                                own.iter()
                                    .copied()
                                    .filter(|r| r.group == ReleaseGroup::Single)
                                    .collect(),
                            ),
                            (
                                3,
                                "Compilations",
                                own.iter()
                                    .copied()
                                    .filter(|r| r.group == ReleaseGroup::Compilation)
                                    .collect(),
                            ),
                        ];
                        let w = ui.available_width();
                        let (headr, _) = ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::hover());
                        widgets::text(
                            ui,
                            headr.left_center(),
                            Align2::LEFT_CENTER,
                            "Discography",
                            24.0,
                            Weight::Bold,
                            pal.text,
                            w - 120.0,
                        );
                        let label = if disc_all { "Show less" } else { "Show all" };
                        let g = widgets::galley(ui, label, 14.0, Weight::Bold, pal.weak, 120.0);
                        let rect = Align2::RIGHT_CENTER.anchor_size(headr.right_center(), g.size());
                        let sr = ui.interact(rect, ui.id().with("disc-all"), Sense::click());
                        let col = if sr.hovered() { pal.text } else { pal.weak };
                        ui.painter().galley(rect.min, g, col);
                        if sr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            disc_all = !disc_all;
                        }
                        ui.horizontal(|ui| {
                            for (id, name, items) in &filters {
                                if items.is_empty() {
                                    continue;
                                }
                                if widgets::chip(ui, name, disc_filter == *id, &pal).clicked() {
                                    disc_filter = *id;
                                }
                            }
                        });
                        ui.add_space(8.0);
                        let chosen = filters
                            .iter()
                            .find(|(id, _, items)| *id == disc_filter && !items.is_empty())
                            .unwrap_or(&filters[0]);
                        let cards: Vec<CardKind> = chosen
                            .2
                            .iter()
                            .map(|r| CardKind::Coll(r.coll.clone()))
                            .collect();
                        card_grid(app, ui, &cards, if disc_all { 50 } else { 1 });
                        ui.add_space(16.0);
                    }
                    // ------------------------------------------------ fans also like
                    if let Some(info) = &info {
                        if !info.related.is_empty() {
                            let w = ui.available_width();
                            let (headr, _) =
                                ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::hover());
                            widgets::text(
                                ui,
                                headr.left_center(),
                                Align2::LEFT_CENTER,
                                "Fans also like",
                                24.0,
                                Weight::Bold,
                                pal.text,
                                w - 120.0,
                            );
                            let label = if fans_all { "Show less" } else { "Show all" };
                            let g = widgets::galley(ui, label, 14.0, Weight::Bold, pal.weak, 120.0);
                            let rect =
                                Align2::RIGHT_CENTER.anchor_size(headr.right_center(), g.size());
                            let sr = ui.interact(rect, ui.id().with("fans-all"), Sense::click());
                            let col = if sr.hovered() { pal.text } else { pal.weak };
                            ui.painter().galley(rect.min, g, col);
                            if sr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                fans_all = !fans_all;
                            }
                            let cards: Vec<CardKind> =
                                info.related.iter().cloned().map(CardKind::Coll).collect();
                            card_grid(app, ui, &cards, if fans_all { 10 } else { 1 });
                            ui.add_space(16.0);
                        }
                    }
                    // ------------------------------------------------ appears on
                    let appears: Vec<CardKind> = releases
                        .iter()
                        .filter(|r| r.group == ReleaseGroup::AppearsOn)
                        .map(|r| CardKind::Coll(r.coll.clone()))
                        .collect();
                    shelf(app, ui, "Appears On", appears, None);
                    // ------------------------------------------------ about
                    if let Some(info) = &info {
                        about_card(app, ui, &title, info);
                    }
                    ui.add_space(24.0);
                });
        });
    ui.ctx()
        .data_mut(|d| d.insert_temp(st_id, (more_popular, disc_filter, disc_all, fans_all)));
    if follow_click {
        app.toggle_follow(&c.uri);
    }
    if radio_click {
        app.start_radio(&c.uri, &title);
    }
    if liked_click {
        // Liked Songs, filtered to this artist.
        app.navigate(View::Liked);
        app.list_find = super::ListFind {
            ctx: ListKey::Liked.context_key(),
            open: true,
            query: title.clone(),
            ..Default::default()
        };
    }
    if let Some(&first) = need_meta.first() {
        app.ensure_meta(&key, first);
    }
    if shuffle_click {
        let on = !app.snapshot.shuffle;
        app.send(PlayerCmd::SetShuffle(on));
        if on && !playing_here {
            app.play_whole(&key, true);
        }
    }
    if play_all {
        if playing_here && app.snapshot.track.is_some() {
            app.send(if app.snapshot.playing {
                PlayerCmd::Pause
            } else {
                PlayerCmd::Play
            });
        } else {
            let shuffle = app.snapshot.shuffle;
            app.play_whole(&key, shuffle);
        }
    }
    if let Some((i, a)) = clicked {
        if let Some(t) = tracks.get(i).cloned() {
            let k = key.clone();
            apply_row_action(app, a, &t, move |app| app.play_list(&k, i));
        }
    }
}

/// Album page footer: release date, copyrights and "More by <artist>".
fn album_footer_ui(
    app: &mut App,
    ui: &mut egui::Ui,
    ai: &crate::model::AlbumInfo,
    more: &[CardKind],
) {
    let pal = app.palette;
    ui.label(
        egui::RichText::new(&ai.released)
            .font(widgets::font(14.0, Weight::Semibold))
            .color(pal.weak),
    );
    ui.add_space(6.0);
    for c in &ai.copyrights {
        // The bundled fonts have no ℗ glyph.
        ui.label(
            egui::RichText::new(c.replace('\u{2117}', "(P)"))
                .small()
                .color(pal.weak),
        );
    }
    ui.add_space(24.0);
    if !more.is_empty() {
        let artist = ai.artist_uri.clone().map(|u| Collection {
            kind: CollectionKind::Artist,
            uri: u,
            name: ai.artist_name.clone(),
            subtitle: "Artist".into(),
            image: None,
            total_tracks: 0,
            owned: false,
        });
        shelf(
            app,
            ui,
            &format!("More by {}", ai.artist_name),
            more.to_vec(),
            artist.map(View::Collection),
        );
    }
}

/// "Recommended — based on what's in this playlist", with Add buttons and Refresh.
fn recommended_ui(app: &mut App, ui: &mut egui::Ui, recs: &[TrackRef]) -> (Option<TrackRef>, bool) {
    let pal = app.palette;
    let mut add = None;
    let mut refresh = false;
    ui.label(egui::RichText::new("Recommended").font(widgets::font(24.0, Weight::Bold)));
    ui.label(egui::RichText::new("Based on what's in this playlist").color(pal.weak));
    ui.add_space(10.0);
    if recs.is_empty() {
        widgets::skeleton_rows(ui, 1, pal.text);
    }
    let w = ui.available_width();
    for (i, t) in recs.iter().enumerate() {
        let (row, r) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
        if r.hovered() {
            ui.painter()
                .rect_filled(row, 4.0, widgets::with_alpha(pal.text, 0.08));
        }
        let art = Rect::from_min_size(
            egui::pos2(row.left() + 8.0, row.center().y - 20.0),
            Vec2::splat(40.0),
        );
        let tex = t.image.as_deref().and_then(|u| app.art.get(u, THUMB_PX));
        widgets::image(ui.painter(), art, tex, 4.0, pal.panel, pal.weak);
        let tw = (w * 0.45).max(120.0);
        widgets::text(
            ui,
            egui::pos2(art.right() + 12.0, row.center().y - 9.0),
            Align2::LEFT_CENTER,
            &t.name,
            15.0,
            Weight::Regular,
            pal.text,
            tw,
        );
        widgets::text(
            ui,
            egui::pos2(art.right() + 12.0, row.center().y + 11.0),
            Align2::LEFT_CENTER,
            &t.artists,
            13.0,
            Weight::Regular,
            pal.weak,
            tw,
        );
        if w > 560.0 {
            widgets::text(
                ui,
                egui::pos2(row.left() + w * 0.55, row.center().y),
                Align2::LEFT_CENTER,
                &t.album,
                14.0,
                Weight::Regular,
                pal.weak,
                w * 0.25,
            );
        }
        let g = widgets::galley(ui, "Add", 14.0, Weight::Bold, pal.text, 80.0);
        let pill = Rect::from_center_size(
            egui::pos2(row.right() - 46.0, row.center().y),
            Vec2::new(g.size().x + 30.0, 32.0),
        );
        let pr = ui.interact(pill, ui.id().with(("rec-add", i)), Sense::click());
        let h = ui.ctx().animate_bool_with_time(pr.id, pr.hovered(), 0.12);
        ui.painter().rect_stroke(
            pill.expand(h),
            16.0,
            egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.5 + 0.5 * h)),
            egui::StrokeKind::Inside,
        );
        ui.painter()
            .galley(pill.center() - g.size() / 2.0, g, pal.text);
        if pr.on_hover_text("Add to this playlist").clicked() {
            add = Some(t.clone());
        }
        super::views::track_context_menu(app, &r, t);
        if r.double_clicked() {
            app.play_tracks(&t.name, vec![t.clone()], 0);
        }
    }
    ui.add_space(8.0);
    let rr = ui.add(
        egui::Label::new(
            egui::RichText::new("Refresh")
                .font(widgets::font(14.0, Weight::Bold))
                .color(pal.weak),
        )
        .sense(Sense::click()),
    );
    if rr
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Show other songs")
        .clicked()
    {
        refresh = true;
    }
    (add, refresh)
}

/// Cards in rows (up to `max_rows`), the same cards as a shelf.
fn card_grid(app: &mut App, ui: &mut egui::Ui, items: &[CardKind], max_rows: usize) {
    let w = ui.available_width();
    let (n, cw) = card_width(w);
    let ch = cw + CARD_TEXT_H;
    for chunk in items.chunks(n).take(max_rows) {
        let (row, _) = ui.allocate_exact_size(Vec2::new(w, ch), Sense::hover());
        for (i, item) in chunk.iter().enumerate() {
            let rect = Rect::from_min_size(
                egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()),
                Vec2::new(cw, ch),
            );
            let res = card(app, ui, rect, item);
            handle_card(app, item, res);
        }
        ui.add_space(8.0);
    }
}

/// Spotify's "About" card: the artist's picture with the bio over it.
fn about_card(app: &mut App, ui: &mut egui::Ui, name: &str, info: &crate::model::ArtistInfo) {
    let Some(bio) = info.bio.clone() else { return };
    let pal = app.palette;
    ui.add_space(16.0);
    ui.label(egui::RichText::new("About").font(widgets::font(24.0, Weight::Bold)));
    ui.add_space(8.0);
    let w = ui.available_width().min(760.0);
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, w * 0.5), Sense::click());
    let tex = info
        .portrait
        .as_deref()
        .and_then(|u| app.art.get_sized(u, 640));
    widgets::image_cover(ui.painter(), rect, tex, 8.0, pal.panel);
    widgets::gradient(
        ui.painter(),
        rect,
        8.0,
        Color32::from_black_alpha(20),
        Color32::from_black_alpha(220),
    );
    let h = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.15);
    if h > 0.0 {
        ui.painter()
            .rect_filled(rect, 8.0, Color32::from_black_alpha((40.0 * h) as u8));
    }
    let pad = 28.0;
    let pop = format!("Popularity {} / 100", info.popularity.clamp(0, 100));
    widgets::text(
        ui,
        rect.left_bottom() + Vec2::new(pad, -pad - 74.0),
        Align2::LEFT_BOTTOM,
        &pop,
        15.0,
        Weight::Bold,
        Color32::WHITE,
        w - 2.0 * pad,
    );
    let g = widgets::wrapped(
        ui,
        &bio,
        14.0,
        Weight::Regular,
        Color32::WHITE,
        w - 2.0 * pad,
    );
    // Show three lines, like Spotify; the rest is in the dialog.
    let clip = Rect::from_min_size(
        rect.left_bottom() + Vec2::new(pad, -pad - 60.0),
        Vec2::new(w - 2.0 * pad, 60.0),
    );
    ui.painter()
        .with_clip_rect(clip)
        .galley(clip.min, g, Color32::WHITE);
    if r.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Read more")
        .clicked()
    {
        app.about_open = Some((name.to_string(), bio));
    }
}

// ====================================================================== search

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum SearchTab {
    #[default]
    All,
    Songs,
    Artists,
    Albums,
    Playlists,
}

/// "Recent searches": cards for what you opened or played from search, each
/// with a small × to forget it, and "Clear recent searches".
fn recent_searches(app: &mut App, ui: &mut egui::Ui) {
    let items = app.config.ui.recent_searches.clone();
    if items.is_empty() {
        return;
    }
    let pal = app.palette;
    let w = ui.available_width();
    let (head, _) = ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::hover());
    widgets::text(
        ui,
        head.left_center(),
        Align2::LEFT_CENTER,
        "Recent searches",
        24.0,
        Weight::Bold,
        pal.text,
        w - 200.0,
    );
    let g = widgets::galley(
        ui,
        "Clear recent searches",
        14.0,
        Weight::Bold,
        pal.weak,
        220.0,
    );
    let cr = Rect::from_min_size(
        egui::pos2(
            head.right() - g.size().x,
            head.center().y - g.size().y / 2.0,
        ),
        g.size(),
    );
    let clear = ui.interact(cr, ui.id().with("recent-clear"), Sense::click());
    let color = if clear.hovered() { pal.text } else { pal.weak };
    ui.painter().galley(cr.min, g, color);
    if clear.hovered() {
        ui.painter()
            .hline(cr.x_range(), cr.bottom(), egui::Stroke::new(1.0, color));
    }
    let (n, cw) = card_width(w);
    let ch = cw + CARD_TEXT_H;
    let (row, _) = ui.allocate_exact_size(Vec2::new(w, ch), Sense::hover());
    let mut open = None;
    let mut forget = None;
    for (i, item) in items.iter().take(n).enumerate() {
        let rect = Rect::from_min_size(
            egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()),
            Vec2::new(cw, ch),
        );
        let kind = match item {
            crate::config::RecentSearch::Collection(c) => CardKind::Coll(c.clone()),
            crate::config::RecentSearch::Track(t) => {
                CardKind::Track(ListKey::Recent, 1000 + i, std::sync::Arc::new(t.clone()))
            }
        };
        let x =
            Rect::from_center_size(rect.right_top() + Vec2::new(-22.0, 22.0), Vec2::splat(26.0));
        let hovered = ui.rect_contains_pointer(rect);
        let res = card(app, ui, rect, &kind);
        // Registered after the card so it sits on top and gets the click.
        let xr = ui.interact(x, ui.id().with(("recent-x", i)), Sense::click());
        if hovered {
            ui.painter().circle_filled(
                x.center(),
                13.0,
                Color32::from_black_alpha(if xr.hovered() { 230 } else { 170 }),
            );
            widgets::draw(ui, x.shrink(7.0), Icon::Close, Color32::WHITE);
        }
        if xr.clicked() {
            forget = Some(item.uri().to_string());
        } else if res.open || res.play {
            open = Some((item.clone(), res.play));
        }
    }
    ui.add_space(24.0);
    if clear
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        app.forget_search(None);
    } else if let Some(u) = forget {
        app.forget_search(Some(&u));
    } else if let Some((item, play)) = open {
        match item {
            crate::config::RecentSearch::Collection(c) => {
                if play {
                    app.autoplay = Some(ListKey::Collection(c.uri.clone()));
                }
                // Not through navigate's search hook: keep the order as is.
                app.navigate(View::Collection(c));
                if play {
                    app.try_autoplay();
                }
            }
            crate::config::RecentSearch::Track(t) => {
                app.play_tracks("Recent searches", vec![std::sync::Arc::new(t)], 0);
            }
        }
    }
}

fn browse_tile(ui: &mut egui::Ui, rect: Rect, title: &str, color: Color32, icon: Icon) -> bool {
    let r = ui.interact(rect, ui.id().with(("browse", title)), Sense::click());
    let fill = if r.hovered() {
        widgets::mix(color, Color32::WHITE, 0.08)
    } else {
        color
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    widgets::text(
        ui,
        rect.min + Vec2::new(16.0, 26.0),
        Align2::LEFT_CENTER,
        title,
        22.0,
        Weight::Bold,
        Color32::WHITE,
        rect.width() - 32.0,
    );
    let ic = Rect::from_center_size(
        rect.right_bottom() - Vec2::new(44.0, 40.0),
        Vec2::splat(56.0),
    );
    widgets::draw(ui, ic, icon, widgets::with_alpha(Color32::WHITE, 0.85));
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn search(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    egui::ScrollArea::vertical()
        .id_salt("search")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(PAD as i8, 16))
                .show(ui, |ui| {
                    if app.backend.is_none() {
                        ui.label(egui::RichText::new("Search imported playlists and local music. Import Spotify account-data JSON in Settings > Storage.").color(pal.weak));
                        ui.add_space(12.0);
                    }
                    let Some(res) = app
                        .search
                        .results
                        .clone()
                        .filter(|_| !app.search.query.trim().is_empty())
                    else {
                        recent_searches(app, ui);
                        page_title(app, ui, "Browse all");
                        let tiles = [
                            (
                                "Liked Songs",
                                Color32::from_rgb(0x50, 0x38, 0xa0),
                                Icon::Heart,
                                View::Liked,
                            ),
                            (
                                "Local Files",
                                Color32::from_rgb(0x1a, 0x7a, 0x4c),
                                Icon::Local,
                                View::Local,
                            ),
                            (
                                "Playlists",
                                Color32::from_rgb(0xdc, 0x14, 0x8c),
                                Icon::Library,
                                View::Playlists,
                            ),
                            (
                                "Albums",
                                Color32::from_rgb(0x1e, 0x32, 0x64),
                                Icon::Album,
                                View::Albums,
                            ),
                            (
                                "Artists",
                                Color32::from_rgb(0xe8, 0x11, 0x5b),
                                Icon::User,
                                View::Artists,
                            ),
                            (
                                "Lyrics",
                                Color32::from_rgb(0xba, 0x5d, 0x07),
                                Icon::Mic,
                                View::Lyrics,
                            ),
                        ];
                        let w = ui.available_width();
                        let (n, cw) = card_width(w);
                        let th = (cw * 0.62).max(110.0);
                        for chunk in tiles.chunks(n) {
                            let (row, _) = ui.allocate_exact_size(Vec2::new(w, th), Sense::hover());
                            for (i, (t, c, icon, v)) in chunk.iter().enumerate() {
                                let rect = Rect::from_min_size(
                                    egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()),
                                    Vec2::new(cw, th),
                                );
                                if browse_tile(ui, rect, t, *c, *icon) {
                                    app.navigate(v.clone());
                                }
                            }
                            ui.add_space(8.0);
                        }
                        if app.search.loading {
                            ui.spinner();
                        }
                        return;
                    };
                    ui.horizontal(|ui| {
                        for (label, tab) in [
                            ("All", SearchTab::All),
                            ("Songs", SearchTab::Songs),
                            ("Artists", SearchTab::Artists),
                            ("Albums", SearchTab::Albums),
                            ("Playlists", SearchTab::Playlists),
                        ] {
                            if widgets::chip(ui, label, app.search_tab == tab, &pal).clicked() {
                                app.search_tab = tab;
                            }
                        }
                        if app.search.loading {
                            ui.spinner();
                        }
                    });
                    if let Some(e) = app.search.error.clone() {
                        ui.label(egui::RichText::new(e).color(Color32::from_rgb(0xf1, 0x5e, 0x6c)));
                    }
                    ui.add_space(16.0);
                    let name = format!("Search: {}", res.query);
                    let mut play_track: Option<usize> = None;
                    let mut row_act: Option<(usize, RowAction)> = None;
                    match app.search_tab {
                        SearchTab::All => {
                            let w = ui.available_width();
                            let wide = w > 760.0;
                            let left_w = if wide { (w * 0.42).min(460.0) } else { w };
                            let (area, _) = ui.allocate_exact_size(
                                Vec2::new(w, if wide { 300.0 } else { 300.0 + 280.0 }),
                                Sense::hover(),
                            );
                            // Top result.
                            let top_rect = Rect::from_min_size(area.min, Vec2::new(left_w, 300.0));
                            widgets::text(
                                ui,
                                top_rect.min + Vec2::new(0.0, 14.0),
                                Align2::LEFT_CENTER,
                                "Top result",
                                24.0,
                                Weight::Bold,
                                pal.text,
                                left_w,
                            );
                            let card_rect = Rect::from_min_max(
                                top_rect.min + Vec2::new(0.0, 40.0),
                                top_rect.max,
                            );
                            let top: Option<CardKind> = res
                                .tracks
                                .first()
                                .map(|t| CardKind::Track(ListKey::Recent, 0, t.clone()))
                                .or_else(|| res.artists.first().cloned().map(CardKind::Coll))
                                .or_else(|| res.albums.first().cloned().map(CardKind::Coll));
                            if let Some(top) = top {
                                let r = ui.interact(
                                    card_rect,
                                    ui.id().with("top-result"),
                                    Sense::click(),
                                );
                                let hov = r.hovered() || ui.rect_contains_pointer(card_rect);
                                ui.painter().rect_filled(
                                    card_rect,
                                    8.0,
                                    widgets::with_alpha(pal.text, if hov { 0.12 } else { 0.06 }),
                                );
                                let (img, title, sub, round) = match &top {
                                    CardKind::Track(_, _, t) => (
                                        t.image_large.clone().or(t.image.clone()),
                                        t.name.clone(),
                                        format!("Song • {}", t.artists),
                                        false,
                                    ),
                                    CardKind::Coll(c) | CardKind::Radio(c) => (
                                        c.image.clone(),
                                        c.name.clone(),
                                        if c.kind == CollectionKind::Artist {
                                            "Artist".into()
                                        } else {
                                            format!("Album • {}", c.subtitle)
                                        },
                                        c.kind == CollectionKind::Artist,
                                    ),
                                };
                                let art = Rect::from_min_size(
                                    card_rect.min + Vec2::new(20.0, 20.0),
                                    Vec2::splat(96.0),
                                );
                                let tex = img.as_deref().and_then(|u| app.art.get(u, LARGE_PX));
                                widgets::shadow(ui.painter(), art, if round { 48.0 } else { 6.0 });
                                widgets::image(
                                    ui.painter(),
                                    art,
                                    tex,
                                    if round { 48.0 } else { 6.0 },
                                    pal.panel,
                                    pal.weak,
                                );
                                widgets::text(
                                    ui,
                                    egui::pos2(art.left(), art.bottom() + 34.0),
                                    Align2::LEFT_CENTER,
                                    &title,
                                    30.0,
                                    Weight::Black,
                                    pal.text,
                                    card_rect.width() - 40.0,
                                );
                                widgets::text(
                                    ui,
                                    egui::pos2(art.left(), art.bottom() + 70.0),
                                    Align2::LEFT_CENTER,
                                    &sub,
                                    14.0,
                                    Weight::Regular,
                                    pal.weak,
                                    card_rect.width() - 40.0,
                                );
                                let mut played = false;
                                if hov {
                                    let pb = Rect::from_center_size(
                                        card_rect.right_bottom() - Vec2::new(44.0, 44.0),
                                        Vec2::splat(52.0),
                                    );
                                    let pr =
                                        ui.interact(pb, ui.id().with("top-play"), Sense::click());
                                    widgets::paint_round_play(
                                        ui,
                                        pb,
                                        pr.hovered(),
                                        false,
                                        pal.accent,
                                        pal.on_accent,
                                    );
                                    played = pr.clicked();
                                }
                                if played || r.clicked() {
                                    match &top {
                                        CardKind::Track(..) => play_track = Some(0),
                                        CardKind::Coll(c) | CardKind::Radio(c) => {
                                            if played {
                                                app.autoplay =
                                                    Some(ListKey::Collection(c.uri.clone()));
                                            }
                                            app.navigate(View::Collection(c.clone()));
                                        }
                                    }
                                }
                            }
                            // Songs.
                            let songs_rect = if wide {
                                Rect::from_min_max(
                                    egui::pos2(top_rect.right() + 24.0, area.top()),
                                    area.max,
                                )
                            } else {
                                Rect::from_min_max(
                                    egui::pos2(area.left(), top_rect.bottom() + 16.0),
                                    area.max,
                                )
                            };
                            widgets::text(
                                ui,
                                songs_rect.min + Vec2::new(0.0, 14.0),
                                Align2::LEFT_CENTER,
                                "Songs",
                                24.0,
                                Weight::Bold,
                                pal.text,
                                songs_rect.width(),
                            );
                            for (i, t) in res.tracks.iter().take(4).enumerate() {
                                let rect = Rect::from_min_size(
                                    songs_rect.min + Vec2::new(0.0, 40.0 + i as f32 * ROW_H),
                                    Vec2::new(songs_rect.width(), ROW_H),
                                );
                                if let Some(a) = track_row_at(
                                    app,
                                    ui,
                                    rect,
                                    t,
                                    i,
                                    false,
                                    ui.id().with(("search-song", i)),
                                ) {
                                    row_act = Some((i, a));
                                }
                            }
                            ui.add_space(28.0);
                            let artists: Vec<CardKind> =
                                res.artists.iter().cloned().map(CardKind::Coll).collect();
                            shelf(app, ui, "Artists", artists, None);
                            let albums: Vec<CardKind> =
                                res.albums.iter().cloned().map(CardKind::Coll).collect();
                            shelf(app, ui, "Albums", albums, None);
                            let playlists: Vec<CardKind> =
                                res.playlists.iter().cloned().map(CardKind::Coll).collect();
                            shelf(app, ui, "Playlists", playlists, None);
                        }
                        SearchTab::Songs => {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (i, t) in res.tracks.iter().enumerate() {
                                if let Some(a) = track_row(app, ui, t, i, true) {
                                    row_act = Some((i, a));
                                }
                            }
                        }
                        tab => {
                            let items = match tab {
                                SearchTab::Artists => &res.artists,
                                SearchTab::Albums => &res.albums,
                                _ => &res.playlists,
                            };
                            let w = ui.available_width();
                            let (n, cw) = card_width(w);
                            let ch = cw + CARD_TEXT_H;
                            let mut act = None;
                            for chunk in items.chunks(n) {
                                let (row, _) =
                                    ui.allocate_exact_size(Vec2::new(w, ch), Sense::hover());
                                for (i, c) in chunk.iter().enumerate() {
                                    let rect = Rect::from_min_size(
                                        egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()),
                                        Vec2::new(cw, ch),
                                    );
                                    let k = CardKind::Coll(c.clone());
                                    let res = card(app, ui, rect, &k);
                                    if res.open || res.play {
                                        act = Some((k, res));
                                    }
                                }
                                ui.add_space(8.0);
                            }
                            if let Some((k, r)) = act {
                                handle_card(app, &k, r);
                            }
                        }
                    }
                    if let Some(i) = play_track {
                        if let Some(t) = res.tracks.get(i) {
                            app.remember_search(crate::config::RecentSearch::Track((**t).clone()));
                        }
                        app.play_tracks(&name, res.tracks.clone(), i);
                    }
                    if let Some((i, a)) = row_act {
                        if let Some(t) = res.tracks.get(i).cloned() {
                            let tracks = res.tracks.clone();
                            let name = name.clone();
                            apply_row_action(app, a, &t, move |app| {
                                app.remember_search(crate::config::RecentSearch::Track(
                                    (*tracks[i]).clone(),
                                ));
                                app.play_tracks(&name, tracks, i)
                            });
                        }
                    }
                });
        });
}

// ====================================================================== lyrics

fn lyrics_page(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    let color = super::panels::current_color(app);
    let bg = color
        .map(|c| widgets::mix(c, Color32::BLACK, 0.12))
        .unwrap_or(Color32::from_rgb(0x53, 0x53, 0x53));
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 8.0, bg);
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(48, 28))
        .show(ui, |ui| {
            if let Some(t) = app.snapshot.track.clone() {
                let (head, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::hover());
                let art = Rect::from_min_size(head.min, Vec2::splat(48.0));
                let tex = t.image.as_deref().and_then(|u| app.art.get(u, THUMB_PX));
                widgets::image(ui.painter(), art, tex, 4.0, pal.panel, pal.weak);
                widgets::text(
                    ui,
                    egui::pos2(art.right() + 12.0, art.center().y - 9.0),
                    Align2::LEFT_CENTER,
                    t.display_name(),
                    16.0,
                    Weight::Bold,
                    Color32::WHITE,
                    head.width() - 80.0,
                );
                widgets::text(
                    ui,
                    egui::pos2(art.right() + 12.0, art.center().y + 11.0),
                    Align2::LEFT_CENTER,
                    &t.artists,
                    14.0,
                    Weight::Regular,
                    widgets::with_alpha(Color32::WHITE, 0.75),
                    head.width() - 80.0,
                );
                ui.add_space(16.0);
            }
            super::panels::lyrics_body(app, ui, true, Some(bg));
        });
    if app.lyrics.source.is_empty() {
        return;
    }
    widgets::text(
        ui,
        full.right_bottom() - Vec2::new(16.0, 12.0),
        Align2::RIGHT_BOTTOM,
        &format!("Lyrics provided by {}", app.lyrics.source),
        11.0,
        Weight::Regular,
        widgets::with_alpha(Color32::WHITE, 0.6),
        200.0,
    );
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn cards_fill_the_row_without_overflow() {
        for w in [300.0, 520.0, 900.0, 1400.0, 2400.0] {
            let (n, cw) = card_width(w);
            assert!((1..=8).contains(&n));
            assert!(cw >= 160.0 || n == 1, "w={w} n={n} cw={cw}");
            let used = n as f32 * cw + (n as f32 - 1.0) * 8.0;
            assert!((used - w).abs() < 0.01);
        }
    }

    #[test]
    fn columns_stay_inside_row() {
        let row = Rect::from_min_size(egui::pos2(10.0, 0.0), Vec2::new(900.0, ROW_H));
        let c = columns(row, true);
        assert!(c.album.is_some());
        assert!(c.idx.right() <= c.title.left());
        assert!(c.title.right() <= c.album.unwrap().left());
        assert!(c.album.unwrap().right() <= c.dur.left());
        assert!(row.contains_rect(c.dur));
        let narrow = columns(
            Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, ROW_H)),
            true,
        );
        assert!(narrow.album.is_none());
    }

    #[test]
    fn total_duration_formatting() {
        assert_eq!(fmt_total(3_725_000), "about 1 hr 2 min");
        assert_eq!(fmt_total(185_000), "3 min 5 sec");
    }
}

#[cfg(test)]
mod select_tests {
    #[test]
    fn click_toggle_and_range() {
        let mut multi = std::collections::BTreeSet::new();
        let mut anchor = None;
        // Rows shown reversed: row i is list index 9 - i.
        let real = |i: usize| (i < 10).then(|| 9 - i);
        super::select_click(&mut multi, &mut anchor, 1, false, false, &real);
        assert_eq!(multi.iter().copied().collect::<Vec<_>>(), [8]);
        super::select_click(&mut multi, &mut anchor, 3, false, true, &real);
        assert_eq!(multi.iter().copied().collect::<Vec<_>>(), [6, 7, 8]);
        super::select_click(&mut multi, &mut anchor, 2, true, false, &real);
        assert_eq!(multi.iter().copied().collect::<Vec<_>>(), [6, 8]);
        super::select_click(&mut multi, &mut anchor, 5, false, false, &real);
        assert_eq!(multi.iter().copied().collect::<Vec<_>>(), [4]);
    }
}
