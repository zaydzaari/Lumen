//! "Your listening stats": totals, top artists/songs/albums, when you listen,
//! and the last 30 days, from Lumen's own listening history.

use egui::{Align2, Color32, Rect, Sense, Vec2};

use super::art::{LARGE_PX, THUMB_PX};
use super::widgets::{self, Icon, Weight};
use super::App;
use crate::history::{Range, Stats};
use crate::model::CollectionKind;

const PAD: f32 = 24.0;

fn minutes(ms: u64) -> String {
    let m = ms / 60_000;
    if m >= 10_000 {
        format!("{:.1}k", m as f64 / 1000.0)
    } else {
        group(m)
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn heading(ui: &mut egui::Ui, text: &str, sub: &str, pal: &super::theme::Palette) {
    ui.add_space(28.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
    widgets::text(
        ui,
        r.left_center(),
        Align2::LEFT_CENTER,
        text,
        24.0,
        Weight::Bold,
        pal.text,
        r.width(),
    );
    if !sub.is_empty() {
        ui.label(egui::RichText::new(sub).color(pal.weak));
    }
    ui.add_space(10.0);
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    let Some(stats) = app.stats.clone() else {
        app.load_stats();
        ui.add_space(60.0);
        ui.vertical_centered(|ui| ui.spinner());
        return;
    };
    let mut play: Option<usize> = None;
    let mut open_artist: Option<(String, String)> = None;
    let mut open_album: Option<(String, String)> = None;
    let mut new_range = None;

    egui::ScrollArea::vertical()
        .id_salt("stats")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ------------------------------------------------ header
            let w = ui.available_width();
            let (head, _) = ui.allocate_exact_size(Vec2::new(w, 260.0), Sense::hover());
            let top = widgets::mix(pal.accent, Color32::from_rgb(0x50, 0x38, 0xa0), 0.55);
            widgets::gradient(ui.painter(), head, 8.0, top, widgets::mix(top, pal.bg, 0.75));
            let icon = Rect::from_min_size(egui::pos2(head.left() + PAD, head.bottom() - PAD - 180.0), Vec2::splat(180.0));
            widgets::shadow(ui.painter(), icon, 8.0);
            widgets::gradient(ui.painter(), icon, 8.0, widgets::mix(pal.accent, Color32::WHITE, 0.15), widgets::mix(pal.accent, Color32::BLACK, 0.35));
            draw_bars_icon(ui, icon, Color32::WHITE);
            let tx = icon.right() + 24.0;
            let tw = head.right() - tx - PAD;
            widgets::text(ui, egui::pos2(tx, icon.top() + 46.0), Align2::LEFT_BOTTOM, "Stats", 14.0, Weight::Semibold, Color32::WHITE, tw);
            let size = super::views::title_size(ui, "Your listening", tw);
            widgets::text(ui, egui::pos2(tx, icon.bottom() - 34.0), Align2::LEFT_BOTTOM, "Your listening", size, Weight::Black, Color32::WHITE, tw);
            let since = stats
                .first_ts
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|d| format!("Recorded by Lumen since {}", d.with_timezone(&chrono::Local).format("%-d %B %Y")))
                .unwrap_or_else(|| "Recorded by Lumen on this computer".into());
            widgets::text(ui, egui::pos2(tx, icon.bottom() - 4.0), Align2::LEFT_BOTTOM, &since, 14.0, Weight::Semibold, widgets::with_alpha(Color32::WHITE, 0.85), tw);

            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(PAD as i8, 0))
                .show(ui, |ui| {
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        for r in [Range::Month, Range::HalfYear, Range::All] {
                            if widgets::chip(ui, r.label(), stats.range == r, &pal).clicked() && stats.range != r {
                                new_range = Some(r);
                            }
                        }
                        if app.stats_loading {
                            ui.spinner();
                        }
                    });

                    if stats.plays == 0 {
                        ui.add_space(40.0);
                        let top = egui::pos2(ui.max_rect().center().x, ui.cursor().top());
                        widgets::empty_state(
                            ui,
                            top,
                            ui.available_width(),
                            Icon::Clock,
                            "No listening here yet",
                            "Play some music: songs count after 30 seconds, just like on Spotify.",
                            None,
                            &pal,
                        );
                        ui.add_space(240.0);
                        return;
                    }

                    // ------------------------------------------------ totals
                    ui.add_space(18.0);
                    tiles(ui, &stats, &pal);

                    // ------------------------------------------------ top artists
                    if !stats.top_artists.is_empty() {
                        heading(ui, "Top artists", "By time listened", &pal);
                        let n = ((ui.available_width() + 8.0) / 176.0).floor().clamp(2.0, 8.0) as usize;
                        let cw = (ui.available_width() - 8.0 * (n as f32 - 1.0)) / n as f32;
                        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), cw + 64.0), Sense::hover());
                        for (i, a) in stats.top_artists.iter().take(n).enumerate() {
                            let rect = Rect::from_min_size(egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()), Vec2::new(cw, cw + 64.0));
                            let id = ui.id().with(("stat-artist", i));
                            let r = ui.interact(rect, id, Sense::click());
                            let h = ui.ctx().animate_bool_with_time(id, r.hovered(), 0.15);
                            if h > 0.0 {
                                ui.painter().rect_filled(rect, 8.0, widgets::with_alpha(pal.text, 0.08 * h));
                            }
                            let pic = Rect::from_min_size(rect.min + Vec2::splat(12.0), Vec2::splat(cw - 24.0));
                            widgets::shadow(ui.painter(), pic, pic.width() / 2.0);
                            let tex = a.image.as_deref().and_then(|u| app.art.get(u, LARGE_PX));
                            widgets::image(ui.painter(), pic, tex, pic.width() / 2.0, pal.panel, pal.weak);
                            // Rank badge
                            let badge = egui::pos2(pic.left() + 16.0, pic.top() + 16.0);
                            ui.painter().circle_filled(badge, 15.0, Color32::from_black_alpha(170));
                            widgets::text(ui, badge, Align2::CENTER_CENTER, &(i + 1).to_string(), 14.0, Weight::Bold, Color32::WHITE, 30.0);
                            widgets::text(ui, egui::pos2(pic.left(), pic.bottom() + 18.0), Align2::LEFT_CENTER, &a.name, 15.0, Weight::Semibold, pal.text, pic.width());
                            widgets::text(ui, egui::pos2(pic.left(), pic.bottom() + 38.0), Align2::LEFT_CENTER, &format!("{} min · {} plays", minutes(a.ms), a.plays), 13.0, Weight::Regular, pal.weak, pic.width());
                            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                if let Some(u) = &a.uri {
                                    open_artist = Some((u.clone(), a.name.clone()));
                                }
                            }
                        }
                    }

                    // ------------------------------------------------ top songs
                    heading(ui, "Top songs", "Most played", &pal);
                    for (i, t) in stats.top_tracks.iter().take(10).enumerate() {
                        let (rect, r) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::click());
                        let id = ui.id().with(("stat-track", i));
                        let h = ui.ctx().animate_bool_with_time(id, r.hovered(), 0.1);
                        if h > 0.0 {
                            ui.painter().rect_filled(rect, 4.0, widgets::with_alpha(pal.text, 0.08 * h));
                        }
                        let playing = app.snapshot.track.as_ref().is_some_and(|c| c.uri == t.play.uri);
                        let idx = Rect::from_min_size(rect.min, Vec2::new(48.0, 56.0));
                        if r.hovered() {
                            widgets::draw(ui, Rect::from_center_size(idx.center(), Vec2::splat(14.0)), Icon::Play, pal.text);
                        } else {
                            widgets::text(ui, idx.center(), Align2::CENTER_CENTER, &(i + 1).to_string(), 15.0, Weight::Regular, if playing { pal.accent } else { pal.weak }, 40.0);
                        }
                        let art = Rect::from_min_size(egui::pos2(idx.right(), rect.top() + 8.0), Vec2::splat(40.0));
                        let tex = t.play.image.as_deref().and_then(|u| app.art.get(u, THUMB_PX));
                        widgets::image(ui.painter(), art, tex, 4.0, pal.panel, pal.weak);
                        let tw = rect.width() * 0.55 - 120.0;
                        widgets::text(ui, egui::pos2(art.right() + 12.0, rect.center().y - 9.0), Align2::LEFT_CENTER, &t.play.name, 15.0, Weight::Regular, if playing { pal.accent } else { pal.text }, tw);
                        widgets::text(ui, egui::pos2(art.right() + 12.0, rect.center().y + 11.0), Align2::LEFT_CENTER, &t.play.artists, 13.5, Weight::Regular, pal.weak, tw);
                        widgets::text(ui, egui::pos2(rect.left() + rect.width() * 0.6, rect.center().y), Align2::LEFT_CENTER, &t.play.album, 13.5, Weight::Regular, pal.weak, rect.width() * 0.22);
                        widgets::text(ui, egui::pos2(rect.right() - 16.0, rect.center().y), Align2::RIGHT_CENTER, &format!("{} plays", t.plays), 13.5, Weight::Regular, pal.weak, 120.0);
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            play = Some(i);
                        }
                    }

                    // ------------------------------------------------ top albums
                    if !stats.top_albums.is_empty() {
                        heading(ui, "Top albums", "", &pal);
                        let n = ((ui.available_width() + 8.0) / 176.0).floor().clamp(2.0, 8.0) as usize;
                        let cw = (ui.available_width() - 8.0 * (n as f32 - 1.0)) / n as f32;
                        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), cw + 64.0), Sense::hover());
                        for (i, a) in stats.top_albums.iter().take(n).enumerate() {
                            let rect = Rect::from_min_size(egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()), Vec2::new(cw, cw + 64.0));
                            let id = ui.id().with(("stat-album", i));
                            let r = ui.interact(rect, id, Sense::click());
                            let h = ui.ctx().animate_bool_with_time(id, r.hovered(), 0.15);
                            if h > 0.0 {
                                ui.painter().rect_filled(rect, 8.0, widgets::with_alpha(pal.text, 0.08 * h));
                            }
                            let pic = Rect::from_min_size(rect.min + Vec2::splat(12.0), Vec2::splat(cw - 24.0));
                            widgets::shadow(ui.painter(), pic, 6.0);
                            let tex = a.image.as_deref().and_then(|u| app.art.get(u, LARGE_PX));
                            widgets::image(ui.painter(), pic, tex, 6.0, pal.panel, pal.weak);
                            widgets::text(ui, egui::pos2(pic.left(), pic.bottom() + 18.0), Align2::LEFT_CENTER, &a.name, 15.0, Weight::Semibold, pal.text, pic.width());
                            widgets::text(ui, egui::pos2(pic.left(), pic.bottom() + 38.0), Align2::LEFT_CENTER, &format!("{} · {} plays", a.detail, a.plays), 13.0, Weight::Regular, pal.weak, pic.width());
                            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                if let Some(u) = &a.uri {
                                    open_album = Some((u.clone(), a.name.clone()));
                                }
                            }
                        }
                    }

                    // ------------------------------------------------ charts
                    heading(ui, "When you listen", "Minutes by time of day", &pal);
                    let labels: Vec<String> = (0..24).map(|h| if h % 6 == 0 { format!("{h}:00") } else { String::new() }).collect();
                    bar_chart(ui, &stats.by_hour, &labels, 150.0, &pal, |i| format!("{}:00 – {} min", i, stats.by_hour[i] / 60_000));
                    ui.add_space(12.0);
                    let days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
                    let wlabels: Vec<String> = days.iter().map(|d| d.to_string()).collect();
                    bar_chart(ui, &stats.by_weekday, &wlabels, 120.0, &pal, |i| format!("{} – {} min", days[i], stats.by_weekday[i] / 60_000));

                    heading(ui, "Last 30 days", &format!("{} day streak", stats.streak), &pal);
                    let dlabels: Vec<String> = (0..30).map(|i| match 29 - i {
                        0 => "Today".to_string(),
                        14 => "2 weeks ago".to_string(),
                        29 => "30 days ago".to_string(),
                        _ => String::new(),
                    }).collect();
                    bar_chart(ui, &stats.last_30_days, &dlabels, 150.0, &pal, |i| {
                        let ago = 29 - i;
                        let when = match ago { 0 => "Today".to_string(), 1 => "Yesterday".to_string(), n => format!("{n} days ago") };
                        format!("{when} – {} min", stats.last_30_days[i] / 60_000)
                    });
                    ui.add_space(20.0);
                    ui.label(
                        egui::RichText::new(
                            "Stats only include what you play in Lumen on this computer. A song counts after 30 seconds.",
                        )
                        .small()
                        .color(pal.weak),
                    );
                    ui.add_space(40.0);
                });
        });

    if let Some(r) = new_range {
        app.stats_range = r;
        app.load_stats();
    }
    if let Some(i) = play {
        let tracks: Vec<_> = stats.top_tracks.iter().map(|t| t.play.to_track()).collect();
        app.play_tracks("Your top songs", tracks, i);
    }
    if let Some((u, n)) = open_artist {
        app.open_uri(CollectionKind::Artist, &u, &n);
    }
    if let Some((u, n)) = open_album {
        app.open_uri(CollectionKind::Album, &u, &n);
    }
}

fn tiles(ui: &mut egui::Ui, s: &Stats, pal: &super::theme::Palette) {
    let items = [
        (minutes(s.ms), "minutes listened"),
        (group(s.plays as u64), "plays"),
        (group(s.tracks as u64), "different songs"),
        (group(s.artists as u64), "artists"),
        (s.streak.to_string(), "day streak"),
    ];
    let w = ui.available_width();
    let n = if w > 760.0 { 5 } else { 3 };
    let tw = (w - 12.0 * (n as f32 - 1.0)) / n as f32;
    for chunk in items.chunks(n) {
        let (row, _) = ui.allocate_exact_size(Vec2::new(w, 104.0), Sense::hover());
        for (i, (value, label)) in chunk.iter().enumerate() {
            let r = Rect::from_min_size(
                egui::pos2(row.left() + i as f32 * (tw + 12.0), row.top()),
                Vec2::new(tw, 104.0),
            );
            ui.painter()
                .rect_filled(r, 8.0, widgets::mix(pal.bg, pal.text, 0.06));
            widgets::text(
                ui,
                r.left_top() + Vec2::new(18.0, 20.0),
                Align2::LEFT_TOP,
                value,
                34.0,
                Weight::Black,
                if i == 0 { pal.accent } else { pal.text },
                tw - 36.0,
            );
            widgets::text(
                ui,
                r.left_bottom() + Vec2::new(18.0, -18.0),
                Align2::LEFT_BOTTOM,
                label,
                13.5,
                Weight::Semibold,
                pal.weak,
                tw - 36.0,
            );
        }
        ui.add_space(12.0);
    }
}

/// Simple bar chart: rounded bars, the tallest in the accent colour, hover
/// tooltip per bar, sparse labels underneath.
fn bar_chart(
    ui: &mut egui::Ui,
    values: &[u64],
    labels: &[String],
    height: f32,
    pal: &super::theme::Palette,
    tip: impl Fn(usize) -> String,
) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height + 22.0), Sense::hover());
    let chart = Rect::from_min_size(rect.min, Vec2::new(w, height));
    let n = values.len().max(1);
    let gap = (w / n as f32 * 0.22).clamp(2.0, 10.0);
    let bw = (w - gap * (n as f32 - 1.0)) / n as f32;
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    let top_i = values
        .iter()
        .enumerate()
        .max_by_key(|(_, v)| **v)
        .map(|(i, _)| i);
    // Baseline
    ui.painter().hline(
        chart.x_range(),
        chart.bottom(),
        egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.1)),
    );
    for (i, v) in values.iter().enumerate() {
        let x = chart.left() + i as f32 * (bw + gap);
        let h = (*v as f32 / max as f32 * (height - 8.0)).max(if *v > 0 { 3.0 } else { 0.0 });
        let bar = Rect::from_min_max(
            egui::pos2(x, chart.bottom() - h),
            egui::pos2(x + bw, chart.bottom()),
        );
        let hit = Rect::from_min_max(
            egui::pos2(x, chart.top()),
            egui::pos2(x + bw, chart.bottom()),
        );
        let r = ui.interact(
            hit,
            ui.id().with(("bar", rect.min.y as i32, i)),
            Sense::hover(),
        );
        let col = if Some(i) == top_i {
            pal.accent
        } else if r.hovered() {
            widgets::with_alpha(pal.text, 0.7)
        } else {
            widgets::with_alpha(pal.text, 0.35)
        };
        ui.painter().rect_filled(
            bar,
            egui::CornerRadius {
                nw: 3,
                ne: 3,
                sw: 0,
                se: 0,
            },
            col,
        );
        r.on_hover_text(tip(i));
        if let Some(l) = labels.get(i).filter(|l| !l.is_empty()) {
            widgets::text(
                ui,
                egui::pos2(x, chart.bottom() + 12.0),
                Align2::LEFT_CENTER,
                l,
                11.5,
                Weight::Regular,
                pal.weak,
                120.0,
            );
        }
    }
}

fn draw_bars_icon(ui: &egui::Ui, r: Rect, c: Color32) {
    let s = r.width();
    let base = r.bottom() - s * 0.25;
    for (i, h) in [0.25f32, 0.45, 0.35, 0.55].iter().enumerate() {
        let x = r.left() + s * (0.25 + i as f32 * 0.14);
        ui.painter().rect_filled(
            Rect::from_min_max(egui::pos2(x, base - s * h), egui::pos2(x + s * 0.09, base)),
            3.0,
            c,
        );
    }
}
