//! "Your Library" panel: collapsed artwork rail (default) or expanded list with
//! filter chips. Long lists are virtualised.

use egui::{Align2, Color32, Rect, Sense, Vec2};

use super::widgets::{self, Icon, Weight};
use super::{App, View};
use crate::model::{Collection, CollectionKind};

const ROW_EXPANDED: f32 = 64.0;
const ROW_COMPACT: f32 = 56.0;

/// Which collection type the expanded library shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LibraryFilter {
    #[default]
    Playlists,
    Albums,
    Artists,
}

enum Item {
    Liked,
    Local,
    Coll(Collection),
}

fn pinned_tile(ui: &egui::Ui, rect: Rect, liked: bool) {
    let (top, bottom) = if liked {
        (
            Color32::from_rgb(0x45, 0x0a, 0xf5),
            Color32::from_rgb(0xc4, 0xef, 0xd9),
        )
    } else {
        (
            Color32::from_rgb(0x0f, 0x6b, 0x45),
            Color32::from_rgb(0x1e, 0xd7, 0x60),
        )
    };
    // Diagonal-ish gradient approximated by a vertical one with rounded corners.
    ui.painter().rect_filled(rect, 4.0, top);
    widgets::gradient(
        ui.painter(),
        Rect::from_min_max(rect.min, rect.max),
        4.0,
        top,
        widgets::mix(top, bottom, 0.55),
    );
    widgets::draw(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(rect.width() * 0.42)),
        if liked { Icon::Heart } else { Icon::Local },
        Color32::WHITE,
    );
}

fn subtitle(c: &Collection) -> String {
    let kind = match c.kind {
        CollectionKind::Playlist => "Playlist",
        CollectionKind::Album => "Album",
        CollectionKind::Artist => return "Artist".into(),
    };
    let who = c.subtitle.trim_start_matches("by ").trim();
    if who.is_empty() {
        kind.into()
    } else {
        format!("{kind} • {who}")
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let compact = app.config.ui.compact_library || app.auto_compact;
    let pal = app.palette;

    // ------------------------------------------------ header
    let head = ui
        .allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover())
        .0;
    let toggle_rect = if compact {
        Rect::from_center_size(head.center(), Vec2::splat(40.0))
    } else {
        Rect::from_min_size(head.min, Vec2::new(head.width() - 140.0, 40.0))
    };
    let tr = ui.interact(toggle_rect, ui.id().with("lib-toggle"), Sense::click());
    let col = if tr.hovered() { pal.text } else { pal.weak };
    let icon_rect = if compact {
        Rect::from_center_size(head.center(), Vec2::splat(24.0))
    } else {
        Rect::from_center_size(
            egui::pos2(head.left() + 18.0, head.center().y),
            Vec2::splat(24.0),
        )
    };
    widgets::draw(ui, icon_rect, Icon::Library, col);
    if !compact {
        widgets::text(
            ui,
            egui::pos2(icon_rect.right() + 12.0, head.center().y),
            Align2::LEFT_CENTER,
            "Your Library",
            16.0,
            Weight::Bold,
            col,
            head.width() - 100.0,
        );
        // Refresh button on the right.
        let rr = Rect::from_center_size(
            egui::pos2(head.right() - 20.0, head.center().y),
            Vec2::splat(32.0),
        );
        let r = ui.interact(rr, ui.id().with("lib-refresh"), Sense::click());
        if r.hovered() {
            ui.painter().circle_filled(rr.center(), 16.0, pal.panel);
        }
        widgets::draw(
            ui,
            Rect::from_center_size(rr.center(), Vec2::splat(16.0)),
            Icon::Refresh,
            if r.hovered() { pal.text } else { pal.weak },
        );
        if r.on_hover_text("Refresh your library").clicked() {
            app.refresh_library();
        }
        // Spotify's "+ Create" pill.
        let g = widgets::galley(ui, "Create", 14.0, Weight::Bold, pal.text, 200.0);
        let cr = Rect::from_min_max(
            egui::pos2(
                rr.left() - 12.0 - (g.size().x + 44.0),
                head.center().y - 16.0,
            ),
            egui::pos2(rr.left() - 12.0, head.center().y + 16.0),
        );
        let r = ui.interact(cr, ui.id().with("lib-create"), Sense::click());
        let h = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.12);
        ui.painter()
            .rect_filled(cr, 16.0, widgets::mix(pal.panel, pal.hover, h));
        widgets::draw(
            ui,
            Rect::from_center_size(
                egui::pos2(cr.left() + 18.0, cr.center().y),
                Vec2::splat(14.0),
            ),
            Icon::Plus,
            pal.text,
        );
        ui.painter().galley(
            egui::pos2(cr.left() + 32.0, cr.center().y - g.size().y / 2.0),
            g,
            pal.text,
        );
        if r.on_hover_text("Create a playlist")
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            app.create_playlist(Vec::new());
        }
    }
    if tr
        .on_hover_text(if compact {
            "Expand Your Library"
        } else {
            "Collapse Your Library"
        })
        .clicked()
    {
        if app.auto_compact {
            // Not enough room: make some by closing the right-hand panel.
            app.right = super::RightPanel::None;
            app.config.ui.compact_library = false;
        } else {
            app.config.ui.compact_library = !compact;
        }
        app.save_config();
    }

    // ------------------------------------------------ filter chips
    if !compact {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (label, f) in [
                ("Playlists", LibraryFilter::Playlists),
                ("Albums", LibraryFilter::Albums),
                ("Artists", LibraryFilter::Artists),
            ] {
                if widgets::chip(ui, label, app.library_filter == f, &pal).clicked() {
                    app.library_filter = f;
                    match f {
                        LibraryFilter::Albums if !app.albums.started => app.load_albums(),
                        LibraryFilter::Artists if !app.artists.started => app.load_artists(),
                        _ => {}
                    }
                }
            }
        });
        ui.add_space(4.0);
        // Search in Your Library + sort label (Spotify style).
        let (row, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
        let icon_rect = Rect::from_center_size(
            egui::pos2(row.left() + 18.0, row.center().y),
            Vec2::splat(32.0),
        );
        let ir = ui.interact(icon_rect, ui.id().with("lib-search"), Sense::click());
        if app.library_search_open {
            let field = Rect::from_min_max(
                egui::pos2(row.left() + 4.0, row.top() + 2.0),
                egui::pos2(row.right() - 90.0, row.bottom() - 2.0),
            );
            ui.painter().rect_filled(field, 4.0, pal.panel);
            widgets::draw(
                ui,
                Rect::from_center_size(
                    egui::pos2(field.left() + 16.0, field.center().y),
                    Vec2::splat(16.0),
                ),
                Icon::Search,
                pal.weak,
            );
            let edit = ui.put(
                Rect::from_min_max(
                    field.min + Vec2::new(32.0, 4.0),
                    field.max - Vec2::new(6.0, 4.0),
                ),
                egui::TextEdit::singleline(&mut app.library_query)
                    .id(egui::Id::new("lib-search-field"))
                    .frame(egui::Frame::NONE)
                    .hint_text(egui::RichText::new("Search in Your Library").color(pal.weak))
                    .font(widgets::font(13.0, Weight::Regular))
                    .vertical_align(egui::Align::Center),
            );
            if app.library_focus {
                edit.request_focus();
                app.library_focus = false;
            }
            if edit.lost_focus() && app.library_query.is_empty() {
                app.library_search_open = false;
            }
        } else {
            if ir.hovered() {
                ui.painter()
                    .circle_filled(icon_rect.center(), 16.0, pal.panel);
            }
            widgets::draw(
                ui,
                Rect::from_center_size(icon_rect.center(), Vec2::splat(16.0)),
                Icon::Search,
                if ir.hovered() { pal.text } else { pal.weak },
            );
            if ir.on_hover_text("Search in Your Library").clicked() {
                app.library_search_open = true;
                app.library_focus = true;
            }
        }
        // Sort menu (Spotify: Recents ▾).
        let label = match app.config.ui.library_sort.as_str() {
            "alphabetical" => "Alphabetical",
            "creator" => "Creator",
            "custom" => "Custom order",
            _ => "Recents",
        };
        let g = widgets::galley(ui, label, 13.0, Weight::Regular, pal.weak, 120.0);
        let sr = Rect::from_min_max(
            egui::pos2(row.right() - 8.0 - g.size().x - 16.0, row.center().y - 12.0),
            egui::pos2(row.right() - 4.0, row.center().y + 12.0),
        );
        let resp = ui.interact(sr, ui.id().with("lib-sort"), Sense::click());
        let c = if resp.hovered() { pal.text } else { pal.weak };
        ui.painter().galley(
            egui::pos2(sr.left(), sr.center().y - g.size().y / 2.0),
            g,
            c,
        );
        let cx = egui::pos2(sr.right() - 6.0, sr.center().y);
        ui.painter().line(
            vec![
                cx + Vec2::new(-3.5, -1.5),
                cx + Vec2::new(0.0, 2.0),
                cx + Vec2::new(3.5, -1.5),
            ],
            egui::Stroke::new(1.4, c),
        );
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::Popup::menu(&resp)
            .id(ui.id().with("lib-sort-menu"))
            .align(egui::RectAlign::BOTTOM_END)
            .show(|ui| {
                widgets::menu_style(ui, 200.0);
                ui.label(egui::RichText::new("Sort by").small().color(pal.weak));
                for (key, name) in [
                    ("recents", "Recents"),
                    ("alphabetical", "Alphabetical"),
                    ("creator", "Creator"),
                    ("custom", "Custom order"),
                ] {
                    let r = widgets::menu_item(ui, name, false, &pal);
                    if app.config.ui.library_sort == key {
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
                        app.config.ui.library_sort = key.into();
                        app.save_config();
                        ui.close();
                    }
                }
                ui.separator();
                ui.label(egui::RichText::new("View as").small().color(pal.weak));
                for (dense, name) in [(true, "Compact"), (false, "List")] {
                    let r = widgets::menu_item(ui, name, false, &pal);
                    if app.config.ui.library_dense == dense {
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
                        app.config.ui.library_dense = dense;
                        app.save_config();
                        ui.close();
                    }
                }
            });
        ui.add_space(4.0);
    } else {
        ui.add_space(4.0);
    }

    // ------------------------------------------------ list
    let (paged_items, more) = match app.library_filter {
        LibraryFilter::Playlists => (&app.playlists.items, app.playlists.next.is_some()),
        LibraryFilter::Albums => (&app.albums.items, app.albums.next.is_some()),
        LibraryFilter::Artists => (&app.artists.items, app.artists.cursor.is_some()),
    };
    let collections = if compact {
        &app.playlists.items
    } else {
        paged_items
    };
    let query = app.library_query.trim().to_lowercase();
    let filtering = !compact && !query.is_empty();
    // Display order: pinned first, then the chosen sort.
    let order = library_order(collections, &app.config.ui);
    // Indices (into `order`) of matching items while searching.
    let matches: Vec<usize> = if filtering {
        order
            .iter()
            .enumerate()
            .filter(|(_, &i)| {
                let c = &collections[i];
                c.name.to_lowercase().contains(&query) || c.subtitle.to_lowercase().contains(&query)
            })
            .map(|(n, _)| n)
            .collect()
    } else {
        Vec::new()
    };
    let pinned = if filtering { 0 } else { 2 };
    let count = if filtering {
        matches.len()
    } else {
        pinned + collections.len()
    };
    // Spotify's "View as: Compact": one text line per item.
    let dense = !compact && app.config.ui.library_dense;
    let row_h = if compact {
        ROW_COMPACT
    } else if dense {
        34.0
    } else {
        ROW_EXPANDED
    };
    let current_ctx = app.snapshot.context_key.clone();
    let mut open: Option<View> = None;
    let mut play: Option<View> = None;
    let mut prefetch: Option<crate::model::Collection> = None;
    let mut dropped: Option<(super::DragItem, crate::model::Collection, bool)> = None;
    let mut menu_action: Option<LibAction> = None;
    let mut need_more = false;
    let loading = app.playlists.loading || app.albums.loading || app.artists.loading;
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt("library-list")
        .auto_shrink([false, false])
        .show_rows(ui, row_h, count, |ui, range| {
            if !filtering
                && range.end + 6 >= count
                && (if compact {
                    app.playlists.next.is_some()
                } else {
                    more
                })
            {
                need_more = true;
            }
            for i in range {
                let item = match i {
                    0 if !filtering => Item::Liked,
                    1 if !filtering => Item::Local,
                    n => {
                        let n = if filtering { matches[n] + pinned } else { n };
                        let list = if compact {
                            &app.playlists.items
                        } else {
                            match app.library_filter {
                                LibraryFilter::Playlists => &app.playlists.items,
                                LibraryFilter::Albums => &app.albums.items,
                                LibraryFilter::Artists => &app.artists.items,
                            }
                        };
                        match order.get(n - pinned).and_then(|&i| list.get(i)) {
                            Some(c) => Item::Coll(c.clone()),
                            None => continue,
                        }
                    }
                };
                let (rect, r) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), row_h),
                    Sense::click_and_drag(),
                );
                let (name, sub, view, ctx_key) = match &item {
                    Item::Liked => (
                        "Liked Songs".to_string(),
                        "Playlist • Your favourites".to_string(),
                        View::Liked,
                        "liked".to_string(),
                    ),
                    Item::Local => (
                        "Local Files".to_string(),
                        format!("{} songs on this computer", app.local_tracks.len()),
                        View::Local,
                        "local".to_string(),
                    ),
                    Item::Coll(c) => (
                        c.name.clone(),
                        subtitle(c),
                        View::Collection(c.clone()),
                        c.uri.clone(),
                    ),
                };
                let selected = app.view == view;
                let playing_here = current_ctx.as_deref() == Some(ctx_key.as_str());
                let hv = ui
                    .ctx()
                    .animate_bool_with_time(r.id.with("hv"), r.hovered(), 0.12);
                if selected {
                    ui.painter().rect_filled(rect, 6.0, pal.panel);
                } else if hv > 0.0 {
                    ui.painter()
                        .rect_filled(rect, 6.0, widgets::with_alpha(pal.text, 0.06 * hv));
                }
                // Hovering a moment starts loading it, so the click feels instant.
                if hv >= 1.0 {
                    if let Item::Coll(c) = &item {
                        prefetch = Some(c.clone());
                    }
                }
                let art_size = if dense { 0.0 } else { 48.0 };
                let art = if compact {
                    Rect::from_center_size(rect.center(), Vec2::splat(art_size))
                } else {
                    Rect::from_min_size(
                        egui::pos2(rect.left() + 8.0, rect.center().y - art_size / 2.0),
                        Vec2::splat(art_size),
                    )
                };
                if ui.is_rect_visible(rect) && dense {
                    let x = rect.left() + 12.0;
                    let g = widgets::galley(
                        ui,
                        &name,
                        14.0,
                        Weight::Semibold,
                        if playing_here { pal.accent } else { pal.text },
                        (rect.width() - 60.0) * 0.65,
                    );
                    let gw = g.size().x;
                    ui.painter().galley(
                        egui::pos2(x, rect.center().y - g.size().y / 2.0),
                        g,
                        pal.text,
                    );
                    let kind = sub.split(" • ").next().unwrap_or("");
                    widgets::text(
                        ui,
                        egui::pos2(x + gw + 6.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        &format!("• {kind}"),
                        13.0,
                        Weight::Regular,
                        pal.weak,
                        (rect.right() - x - gw - 36.0).max(0.0),
                    );
                    if playing_here && app.snapshot.playing {
                        widgets::eq_bars(
                            ui,
                            Rect::from_center_size(
                                egui::pos2(rect.right() - 16.0, rect.center().y),
                                Vec2::splat(14.0),
                            ),
                            pal.accent,
                            app.config.ui.animations,
                        );
                    }
                } else if ui.is_rect_visible(rect) {
                    match &item {
                        Item::Liked => pinned_tile(ui, art, true),
                        Item::Local => pinned_tile(ui, art, false),
                        Item::Coll(c) => {
                            let round = if c.kind == CollectionKind::Artist {
                                24.0
                            } else {
                                4.0
                            };
                            let tex = c
                                .image
                                .as_deref()
                                .and_then(|u| app.art.get(u, super::art::THUMB_PX));
                            widgets::image(ui.painter(), art, tex, round, pal.panel, pal.weak);
                        }
                    }
                    if !compact {
                        let x = art.right() + 12.0;
                        let w = rect.right() - x - 30.0;
                        widgets::text(
                            ui,
                            egui::pos2(x, rect.center().y - 10.0),
                            Align2::LEFT_CENTER,
                            &name,
                            15.0,
                            Weight::Semibold,
                            if playing_here { pal.accent } else { pal.text },
                            w,
                        );
                        widgets::text(
                            ui,
                            egui::pos2(x, rect.center().y + 11.0),
                            Align2::LEFT_CENTER,
                            &sub,
                            13.0,
                            Weight::Regular,
                            pal.weak,
                            w,
                        );
                        if playing_here && app.snapshot.playing {
                            widgets::eq_bars(
                                ui,
                                Rect::from_center_size(
                                    egui::pos2(rect.right() - 16.0, rect.center().y),
                                    Vec2::splat(16.0),
                                ),
                                pal.accent,
                                app.config.ui.animations,
                            );
                        }
                    } else if playing_here && app.snapshot.playing {
                        ui.painter().circle_filled(
                            egui::pos2(art.right() + 3.0, art.top() + 3.0),
                            4.0,
                            pal.accent,
                        );
                    }
                }
                // Pin marker (Spotify's small green pin before the subtitle).
                if let Item::Coll(c) = &item {
                    if !compact
                        && !dense
                        && app.config.ui.pinned.contains(&c.uri)
                        && ui.is_rect_visible(rect)
                    {
                        let x = art.right() + 12.0;
                        widgets::draw(
                            ui,
                            Rect::from_center_size(
                                egui::pos2(x + 5.0, rect.center().y + 11.0),
                                Vec2::splat(11.0),
                            ),
                            Icon::Pin,
                            pal.accent,
                        );
                    }
                }
                // Drag & drop: drag rows to reorder, drop songs onto playlists.
                if let Item::Coll(c) = &item {
                    if r.drag_started() {
                        r.dnd_set_drag_payload(super::DragItem::Library(c.clone()));
                    }
                    if let Some(p) = r.dnd_hover_payload::<super::DragItem>() {
                        match &*p {
                            super::DragItem::Library(d) if d.uri != c.uri => {
                                let below = ui
                                    .input(|i| i.pointer.hover_pos())
                                    .is_some_and(|p| p.y > rect.center().y);
                                let y = if below { rect.bottom() } else { rect.top() };
                                ui.painter().hline(
                                    rect.x_range(),
                                    y,
                                    egui::Stroke::new(2.0, pal.accent),
                                );
                            }
                            super::DragItem::Tracks { .. } if c.owned => {
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0,
                                    egui::Stroke::new(2.0, pal.accent),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            _ => {}
                        }
                    }
                    if let Some(p) = r.dnd_release_payload::<super::DragItem>() {
                        let below = ui
                            .input(|i| i.pointer.interact_pos())
                            .is_some_and(|p| p.y > rect.center().y);
                        dropped = Some(((*p).clone(), c.clone(), below));
                    }
                }
                let r = r.on_hover_ui(|ui| {
                    ui.label(
                        egui::RichText::new(&name).font(widgets::font(14.0, Weight::Semibold)),
                    );
                    ui.label(egui::RichText::new(&sub).small().color(pal.weak));
                });
                if let Item::Coll(c) = &item {
                    r.context_menu(|ui| {
                        widgets::menu_style(ui, 240.0);
                        library_menu(app, ui, c, &mut menu_action);
                    });
                }
                // Spotify's play button over the cover while hovering the row.
                let mut art_clicked = false;
                // In the narrow cover rail the cover *is* the row: a click there must
                // open it (double-click plays), so the play button is list-view only.
                if !compact && !dense && (r.hovered() || ui.rect_contains_pointer(art)) {
                    // Only the play icon itself plays; the rest of the row (cover included) opens.
                    let ar = ui.interact(
                        Rect::from_center_size(art.center(), Vec2::splat(28.0)),
                        r.id.with("art-play"),
                        Sense::click(),
                    );
                    let playing = playing_here && app.snapshot.playing;
                    ui.painter().rect_filled(
                        art,
                        if matches!(&item, Item::Coll(c) if c.kind == CollectionKind::Artist) {
                            24.0
                        } else {
                            4.0
                        },
                        Color32::from_black_alpha(if ar.hovered() { 150 } else { 110 }),
                    );
                    widgets::draw(
                        ui,
                        Rect::from_center_size(
                            art.center(),
                            Vec2::splat(if ar.hovered() { 22.0 } else { 20.0 }),
                        ),
                        if playing { Icon::Pause } else { Icon::Play },
                        Color32::WHITE,
                    );
                    let ar = ar
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(if playing {
                            format!("Pause {name}")
                        } else {
                            format!("Play {name}")
                        });
                    if ar.clicked() {
                        art_clicked = true;
                        play = Some(view.clone());
                    }
                }
                if r.double_clicked() {
                    // Double-click plays it, like Spotify.
                    play = Some(view.clone());
                }
                if !art_clicked && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    open = Some(view);
                }
            }
        });
    if loading && count <= pinned {
        ui.spinner();
    }
    if need_more {
        match (compact, app.library_filter) {
            (true, _) | (false, LibraryFilter::Playlists) => app.load_playlists(),
            (false, LibraryFilter::Albums) => app.load_albums(),
            (false, LibraryFilter::Artists) => app.load_artists(),
        }
    }
    if let Some(c) = prefetch {
        app.prefetch(&c);
    }
    if let Some((item, target, below)) = dropped {
        match item {
            super::DragItem::Library(d) => move_in_library(app, &d.uri, &target.uri, below),
            super::DragItem::Tracks { tracks, .. } if target.owned => {
                let uris: Vec<String> = tracks.iter().map(|t| t.uri.clone()).collect();
                app.add_to_playlist(&target.uri, &target.name, uris);
            }
            super::DragItem::Tracks { .. } => {
                app.toast(
                    crate::events::ToastKind::Error,
                    "You can only add songs to your own playlists",
                );
            }
        }
    }
    if let Some(a) = menu_action {
        apply_lib_action(app, a);
    }
    if let Some(v) = open {
        app.navigate(v);
    }
    if let Some(v) = play {
        app.toggle_play_view(&v);
    }
}

pub(super) fn apply_lib_action(app: &mut App, a: LibAction) {
    {
        match a {
            LibAction::Library(c) => app.toggle_in_library(&c),
            LibAction::Play(c) => app.play_collection(&c),
            LibAction::Queue(c) => app.queue_collection(&c),
            LibAction::Pin(uri) => app.toggle_pin(&uri),
            LibAction::Edit(c) => app.edit_playlist_details(&c),
            LibAction::Delete(c) => {
                if c.owned {
                    app.confirm_delete = Some(c);
                } else {
                    app.delete_playlist(&c.uri);
                }
            }
            LibAction::Radio(c) => app.start_radio(&c.uri, &c.name),
        }
    }
}

pub(super) enum LibAction {
    /// Add to / remove from Your Library.
    Library(crate::model::Collection),
    Play(crate::model::Collection),
    Queue(crate::model::Collection),
    Pin(String),
    Edit(crate::model::Collection),
    Delete(crate::model::Collection),
    Radio(crate::model::Collection),
}

/// Right-click menu of a Your Library row, like Spotify's.
pub(super) fn library_menu(
    app: &App,
    ui: &mut egui::Ui,
    c: &crate::model::Collection,
    act: &mut Option<LibAction>,
) {
    let pal = app.palette;
    let mut item = |ui: &mut egui::Ui, label: &str, a: LibAction| {
        if widgets::menu_item(ui, label, false, &pal).clicked() {
            *act = Some(a);
            ui.close();
        }
    };
    if c.kind != CollectionKind::Artist {
        item(ui, "Play", LibAction::Play(c.clone()));
        item(ui, "Add to queue", LibAction::Queue(c.clone()));
    }
    let is_pinned = app.config.ui.pinned.contains(&c.uri);
    let pin_label = match c.kind {
        CollectionKind::Playlist => {
            if is_pinned {
                "Unpin playlist"
            } else {
                "Pin playlist"
            }
        }
        CollectionKind::Album => {
            if is_pinned {
                "Unpin album"
            } else {
                "Pin album"
            }
        }
        CollectionKind::Artist => {
            if is_pinned {
                "Unpin artist"
            } else {
                "Pin artist"
            }
        }
    };
    let saved = app.in_library(c);
    if saved {
        item(ui, pin_label, LibAction::Pin(c.uri.clone()));
    }
    let radio = match c.kind {
        CollectionKind::Playlist => "Go to playlist radio",
        CollectionKind::Album => "Go to album radio",
        CollectionKind::Artist => "Go to artist radio",
    };
    item(ui, radio, LibAction::Radio(c.clone()));
    ui.separator();
    if c.kind == CollectionKind::Playlist && c.owned {
        item(ui, "Edit details", LibAction::Edit(c.clone()));
        item(ui, "Delete", LibAction::Delete(c.clone()));
    } else {
        let label = match (c.kind, saved) {
            (CollectionKind::Artist, true) => "Unfollow",
            (CollectionKind::Artist, false) => "Follow",
            (_, true) => "Remove from Your Library",
            (_, false) => "Add to Your Library",
        };
        item(ui, label, LibAction::Library(c.clone()));
    }
    ui.separator();
    let kind = match c.kind {
        CollectionKind::Playlist => "playlist",
        CollectionKind::Album => "album",
        CollectionKind::Artist => "artist",
    };
    if widgets::menu_item(ui, &format!("Copy link to {kind}"), false, &pal).clicked() {
        ui.ctx().copy_text(format!(
            "https://open.spotify.com/{kind}/{}",
            crate::model::uri_id(&c.uri)
        ));
        ui.close();
    }
    if widgets::menu_item(ui, "Open in Spotify app", true, &pal).clicked() {
        let _ = open::that_detached(&c.uri);
        ui.close();
    }
}

/// Library display order: pinned items first (in pin order), then the
/// selected sort. Returns indices into `items`.
pub fn library_order(
    items: &[crate::model::Collection],
    cfg: &crate::config::UiConfig,
) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..items.len()).collect();
    match cfg.library_sort.as_str() {
        "alphabetical" => idx.sort_by_key(|&i| items[i].name.to_lowercase()),
        "creator" => idx.sort_by_key(|&i| {
            (
                items[i].subtitle.to_lowercase(),
                items[i].name.to_lowercase(),
            )
        }),
        "custom" => {
            let pos = |u: &str| {
                cfg.library_order
                    .iter()
                    .position(|o| o == u)
                    .unwrap_or(usize::MAX)
            };
            idx.sort_by_key(|&i| pos(&items[i].uri));
        }
        _ => {}
    }
    let pin = |u: &str| cfg.pinned.iter().position(|p| p == u);
    idx.sort_by_key(|&i| pin(&items[i].uri).unwrap_or(usize::MAX));
    idx
}

/// Move `uri` before/after `target` (switches the library to custom order).
fn move_in_library(app: &mut App, uri: &str, target: &str, after: bool) {
    if uri == target {
        return;
    }
    // Start the custom order from what's on screen now.
    let current: Vec<String> = library_order(&app.playlists.items, &app.config.ui)
        .into_iter()
        .map(|i| app.playlists.items[i].uri.clone())
        .collect();
    let mut order: Vec<String> = current.into_iter().filter(|u| u != uri).collect();
    let at = order
        .iter()
        .position(|u| u == target)
        .map_or(order.len(), |p| if after { p + 1 } else { p });
    order.insert(at, uri.to_string());
    // Pinned items stay pinned; dragging a pinned item reorders the pins.
    let pins = &mut app.config.ui.pinned;
    if pins.iter().any(|p| p == uri) {
        let mut new_pins: Vec<String> =
            order.iter().filter(|u| pins.contains(u)).cloned().collect();
        new_pins.retain(|p| !p.is_empty());
        *pins = new_pins;
    }
    app.config.ui.library_order = order;
    app.config.ui.library_sort = "custom".into();
    app.save_config();
}
