//! Theme marketplace: browse Spicetify themes, install them with one click and
//! switch colour schemes with a live hover preview.

use egui::{Align2, Color32, Rect, Sense, Vec2};

use super::widgets::{self, Icon, Weight};
use super::App;
use crate::themes::MarketTheme;

const PAD: f32 = 24.0;
const PREVIEW_PX: u32 = 384;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum MarketTab {
    #[default]
    All,
    Installed,
    Official,
}

enum Act {
    Open(String),
    Install(MarketTheme),
    Apply(String, String),
    Remove(String),
    Default,
    Refresh,
    Back,
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let mut act: Option<Act> = None;
    egui::ScrollArea::vertical()
        .id_salt("themes-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(PAD as i8, 16))
                .show(ui, |ui| {
                    let open = app
                        .market
                        .open
                        .as_ref()
                        .and_then(|id| app.market.items.iter().find(|t| &t.id == id).cloned());
                    match open {
                        Some(t) => detail(app, ui, &t, &mut act),
                        None => browse(app, ui, &mut act),
                    }
                });
        });
    match act {
        Some(Act::Open(id)) => app.market.open = Some(id),
        Some(Act::Back) => app.market.open = None,
        Some(Act::Install(t)) => app.install_theme(t, Some(String::new())),
        Some(Act::Apply(id, s)) => app.set_color_theme(&id, &s),
        Some(Act::Remove(id)) => app.remove_theme(&id),
        Some(Act::Default) => app.set_color_theme("", ""),
        Some(Act::Refresh) => app.load_market(true),
        None => {}
    }
}

fn heading(app: &App, ui: &mut egui::Ui, text: &str, size: f32) {
    let h = size * 1.6;
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    widgets::text(
        ui,
        r.left_center(),
        Align2::LEFT_CENTER,
        text,
        size,
        if size > 26.0 {
            Weight::Black
        } else {
            Weight::Bold
        },
        app.palette.text,
        r.width(),
    );
}

fn pill_button(
    ui: &mut egui::Ui,
    label: &str,
    fill: Color32,
    fg: Color32,
    id: egui::Id,
) -> egui::Response {
    let g = widgets::galley(ui, label, 14.0, Weight::Bold, fg, f32::INFINITY);
    let (rect, r) = ui.allocate_exact_size(Vec2::new(g.size().x + 32.0, 36.0), Sense::click());
    let h = ui.ctx().animate_bool_with_time(id, r.hovered(), 0.12);
    ui.painter().rect_filled(
        rect.expand(h * 1.5),
        18.0,
        widgets::mix(fill, Color32::WHITE, 0.1 * h),
    );
    ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Installed themes with their scheme chips (hover = preview, click = apply).
fn installed_section(app: &mut App, ui: &mut egui::Ui, act: &mut Option<Act>) {
    let pal = app.palette;
    let current = (
        app.config.ui.color_theme.clone(),
        app.config.ui.color_scheme.clone(),
    );
    let themes = app.themes.clone();
    // Which theme's colour schemes are shown under the cards.
    let open_id = egui::Id::new("themes-open");
    let open: String = ui
        .ctx()
        .data_mut(|d| d.get_temp::<String>(open_id))
        .unwrap_or_else(|| current.0.clone());

    // Cards: Spotify's default first, then your installed themes.
    let w = ui.available_width();
    let n = ((w + 12.0) / 188.0).floor().clamp(1.0, 6.0) as usize;
    let cw = (w - 12.0 * (n as f32 - 1.0)) / n as f32;
    let ch = cw * 0.52 + 66.0;
    let default_scheme: [[u8; 3]; 6] = [
        [0x12, 0x12, 0x12],
        [0x00, 0x00, 0x00],
        [0x1f, 0x1f, 0x1f],
        [0xff, 0xff, 0xff],
        [0xb3, 0xb3, 0xb3],
        [0x1e, 0xd7, 0x60],
    ];
    let mut cards: Vec<(Option<&crate::themes::InstalledTheme>, [[u8; 3]; 6])> =
        vec![(None, default_scheme)];
    for t in &themes {
        let s = t
            .schemes
            .iter()
            .find(|s| current.0 == t.id && s.name == current.1)
            .or(t.schemes.first());
        let c = |k: &str, d: [u8; 3]| s.and_then(|s| s.get(k)).unwrap_or(d);
        let main = c("main", [0x12, 0x12, 0x12]);
        cards.push((
            Some(t),
            [
                main,
                c("sidebar", c("player", main)),
                c("card", c("player", main)),
                c("text", [0xff, 0xff, 0xff]),
                c("subtext", [0xb3, 0xb3, 0xb3]),
                c("button", c("button-active", [0x1e, 0xd7, 0x60])),
            ],
        ));
    }
    for chunk in cards.chunks(n) {
        let (row, _) = ui.allocate_exact_size(Vec2::new(w, ch), Sense::hover());
        for (i, (theme, sw)) in chunk.iter().enumerate() {
            let rect = Rect::from_min_size(
                egui::pos2(row.left() + i as f32 * (cw + 12.0), row.top()),
                Vec2::new(cw, ch),
            );
            let id = egui::Id::new((
                "installed-theme-card",
                theme.map(|t| t.id.as_str()).unwrap_or(""),
            ));
            let r = ui.interact(rect, id, Sense::click());
            let in_use = match theme {
                None => current.0.is_empty(),
                Some(t) => current.0 == t.id,
            };
            let hov = ui
                .ctx()
                .animate_bool_with_time(id.with("hv"), r.hovered(), 0.12);
            let p = ui.painter();
            p.rect_filled(rect, 8.0, widgets::mix(pal.panel, pal.hover, hov));
            if in_use {
                p.rect_stroke(
                    rect,
                    8.0,
                    egui::Stroke::new(2.0, pal.accent),
                    egui::StrokeKind::Inside,
                );
            }
            // A tiny Spotify window painted in the theme's colours.
            let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
            let pv = Rect::from_min_size(
                rect.min + Vec2::new(10.0, 10.0),
                Vec2::new(cw - 20.0, cw * 0.52),
            );
            p.rect_filled(pv, 6.0, rgb(sw[1]));
            let side = Rect::from_min_size(
                pv.min + Vec2::new(5.0, 5.0),
                Vec2::new(pv.width() * 0.26, pv.height() - 24.0),
            );
            p.rect_filled(side, 4.0, rgb(sw[0]));
            let main = Rect::from_min_max(
                egui::pos2(side.right() + 5.0, pv.top() + 5.0),
                egui::pos2(pv.right() - 5.0, pv.bottom() - 19.0),
            );
            p.rect_filled(main, 4.0, rgb(sw[0]));
            for k in 0..3 {
                let y = side.top() + 8.0 + k as f32 * 9.0;
                p.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(side.left() + 6.0, y),
                        Vec2::new(side.width() - 14.0 - k as f32 * 6.0, 4.0),
                    ),
                    2.0,
                    rgb(sw[4]),
                );
            }
            p.rect_filled(
                Rect::from_min_size(
                    main.min + Vec2::new(8.0, 8.0),
                    Vec2::new(main.width() * 0.5, 7.0),
                ),
                3.0,
                rgb(sw[3]),
            );
            for k in 0..3 {
                let cx = main.left() + 8.0 + k as f32 * ((main.width() - 16.0) / 3.0);
                let card = Rect::from_min_size(
                    egui::pos2(cx, main.top() + 22.0),
                    Vec2::new(
                        (main.width() - 16.0) / 3.0 - 5.0,
                        (main.height() - 30.0).max(8.0),
                    ),
                );
                p.rect_filled(card, 3.0, rgb(sw[2]));
            }
            let bar_y = pv.bottom() - 10.0;
            p.circle_filled(egui::pos2(pv.center().x, bar_y), 5.0, rgb(sw[3]));
            p.rect_filled(
                Rect::from_center_size(
                    egui::pos2(pv.center().x + 30.0, bar_y),
                    Vec2::new(36.0, 3.0),
                ),
                2.0,
                rgb(sw[5]),
            );
            // Name and details.
            let (name, sub) = match theme {
                None => ("Spotify".to_string(), "Default".to_string()),
                Some(t) => {
                    let n = t.schemes.len();
                    let by = if t.author.is_empty() {
                        String::new()
                    } else {
                        format!("by {} · ", t.author)
                    };
                    (
                        t.name.clone(),
                        format!("{by}{n} scheme{}", if n == 1 { "" } else { "s" }),
                    )
                }
            };
            widgets::text(
                ui,
                egui::pos2(rect.left() + 12.0, pv.bottom() + 20.0),
                Align2::LEFT_CENTER,
                &name,
                15.0,
                Weight::Bold,
                pal.text,
                cw - (if in_use { 80.0 } else { 24.0 }),
            );
            widgets::text(
                ui,
                egui::pos2(rect.left() + 12.0, pv.bottom() + 41.0),
                Align2::LEFT_CENTER,
                &sub,
                12.5,
                Weight::Regular,
                pal.weak,
                cw - 24.0,
            );
            if in_use {
                let g = widgets::galley(ui, "In use", 11.0, Weight::Bold, pal.on_accent, 80.0);
                let pill = Rect::from_min_size(
                    egui::pos2(rect.right() - 12.0 - g.size().x - 14.0, pv.bottom() + 11.0),
                    Vec2::new(g.size().x + 14.0, 18.0),
                );
                ui.painter().rect_filled(pill, 9.0, pal.accent);
                ui.painter()
                    .galley(pill.center() - g.size() / 2.0, g, pal.on_accent);
            }
            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
            if let Some(t) = theme {
                let tid = t.id.clone();
                r.context_menu(|ui| {
                    widgets::menu_style(ui, 200.0);
                    if widgets::menu_item(ui, "Show colour schemes", false, &pal).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(open_id, tid.clone()));
                        ui.close();
                    }
                    if widgets::menu_item(ui, "Remove theme", false, &pal).clicked() {
                        *act = Some(Act::Remove(tid.clone()));
                        ui.close();
                    }
                });
            }
            if r.clicked() {
                match theme {
                    None => *act = Some(Act::Default),
                    Some(t) => {
                        ui.ctx().data_mut(|d| d.insert_temp(open_id, t.id.clone()));
                        // One click applies its first scheme if the theme isn't in use yet.
                        if current.0 != t.id {
                            if let Some(s) = t.schemes.first() {
                                *act = Some(Act::Apply(t.id.clone(), s.name.clone()));
                            }
                        }
                    }
                }
            }
        }
        ui.add_space(12.0);
    }

    // Colour schemes of the open theme.
    let Some(t) = themes.iter().find(|t| t.id == open) else {
        return;
    };
    if t.schemes.len() <= 1 {
        return;
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} colour schemes", t.name))
                .font(widgets::font(15.0, Weight::Bold))
                .color(pal.text),
        );
        ui.label(
            egui::RichText::new("Hover to preview, click to use")
                .small()
                .color(pal.weak),
        );
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
        for s in &t.schemes {
            let selected = current.0 == t.id && current.1 == s.name;
            let r = widgets::scheme_chip(
                ui,
                egui::Id::new(("scheme", &t.id, &s.name)),
                &s.name,
                &s.swatches(),
                selected,
                &pal,
            );
            if r.hovered() && !selected {
                app.theme_hover = Some((t.id.clone(), s.name.clone()));
            }
            if r.clicked() {
                *act = Some(Act::Apply(t.id.clone(), s.name.clone()));
            }
        }
    });
}

fn browse(app: &mut App, ui: &mut egui::Ui, act: &mut Option<Act>) {
    let pal = app.palette;
    heading(app, ui, "Themes", 32.0);
    ui.label(
        egui::RichText::new(
            "Colour themes from the Spicetify community. Install one with a click, then hover \
             its colour schemes to preview and click to apply. You can also drop a color.ini \
             file onto the window.",
        )
        .color(pal.weak),
    );
    ui.add_space(16.0);

    // ------------------------------------------------ your themes
    heading(app, ui, "Your themes", 22.0);
    installed_section(app, ui, act);
    ui.add_space(28.0);

    // ------------------------------------------------ marketplace controls
    heading(app, ui, "Marketplace", 22.0);
    ui.horizontal(|ui| {
        let (field, _) = ui.allocate_exact_size(Vec2::new(260.0, 36.0), Sense::hover());
        ui.painter().rect_filled(field, 18.0, pal.panel);
        widgets::draw(
            ui,
            Rect::from_center_size(
                egui::pos2(field.left() + 18.0, field.center().y),
                Vec2::splat(16.0),
            ),
            Icon::Search,
            pal.weak,
        );
        ui.put(
            Rect::from_min_max(
                field.min + Vec2::new(34.0, 6.0),
                field.max - Vec2::new(12.0, 6.0),
            ),
            egui::TextEdit::singleline(&mut app.market.query)
                .frame(egui::Frame::NONE)
                .hint_text(egui::RichText::new("Search themes").color(pal.weak))
                .vertical_align(egui::Align::Center),
        );
        ui.add_space(8.0);
        for (label, tab) in [
            ("All", MarketTab::All),
            ("Installed", MarketTab::Installed),
            ("Official", MarketTab::Official),
        ] {
            if widgets::chip(ui, label, app.market_tab == tab, &pal).clicked() {
                app.market_tab = tab;
            }
        }
        if widgets::icon_button(
            ui,
            Icon::Refresh,
            18.0,
            "Refresh from GitHub",
            pal.weak,
            pal.text,
        )
        .clicked()
        {
            *act = Some(Act::Refresh);
        }
        if app.market.loading {
            ui.spinner();
        }
    });
    ui.add_space(14.0);
    if let Some(e) = app.market.error.clone() {
        ui.label(
            egui::RichText::new(format!("Couldn't load themes: {e}")).color(pal.notification_error),
        );
        ui.add_space(8.0);
    }

    let q = app.market.query.trim().to_lowercase();
    let installed: std::collections::HashSet<String> =
        app.themes.iter().map(|t| t.id.clone()).collect();
    let items: Vec<MarketTheme> = app
        .market
        .items
        .iter()
        .filter(|t| match app.market_tab {
            MarketTab::All => true,
            MarketTab::Installed => installed.contains(&t.id),
            MarketTab::Official => t.official,
        })
        .filter(|t| {
            q.is_empty()
                || t.name.to_lowercase().contains(&q)
                || t.author.to_lowercase().contains(&q)
                || t.description.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    if items.is_empty() && !app.market.loading {
        ui.label(egui::RichText::new("No themes match.").color(pal.weak));
        return;
    }

    // ------------------------------------------------ grid
    let w = ui.available_width();
    let n = ((w + 16.0) / 296.0).floor().clamp(1.0, 5.0) as usize;
    let cw = (w - 16.0 * (n as f32 - 1.0)) / n as f32;
    let ch = cw * 0.58 + 92.0;
    for row in items.chunks(n) {
        let (line, _) = ui.allocate_exact_size(Vec2::new(w, ch), Sense::hover());
        for (i, t) in row.iter().enumerate() {
            let rect = Rect::from_min_size(
                egui::pos2(line.left() + i as f32 * (cw + 16.0), line.top()),
                Vec2::new(cw, ch),
            );
            card(app, ui, rect, t, installed.contains(&t.id), act);
        }
        ui.add_space(16.0);
    }
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Themes are made by the Spicetify community and loaded from GitHub. Lumen uses each \
             theme's colour schemes (color.ini); layout changes from its CSS don't apply.",
        )
        .small()
        .color(pal.weak),
    );
}

fn card(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: Rect,
    t: &MarketTheme,
    installed: bool,
    act: &mut Option<Act>,
) {
    let pal = app.palette;
    let id = egui::Id::new(("theme-card", &t.id));
    let r = ui.interact(rect, id, Sense::click());
    let h = ui
        .ctx()
        .animate_bool_with_time(id.with("h"), r.hovered(), 0.15);
    let p = ui.painter().clone();
    p.rect_filled(rect, 8.0, widgets::mix(pal.panel, pal.hover, h));
    let prev = Rect::from_min_size(
        rect.min + Vec2::splat(10.0),
        Vec2::new(rect.width() - 20.0, rect.width() * 0.58 - 12.0),
    );
    // Lift the preview a little on hover.
    let prev = prev.translate(Vec2::new(0.0, -2.0 * h));
    if h > 0.0 {
        widgets::shadow(&p, prev, 6.0);
    }
    let tex = if ui.is_rect_visible(rect) {
        t.preview
            .as_deref()
            .and_then(|u| app.art.get_sized(u, PREVIEW_PX))
    } else {
        None
    };
    widgets::image_cover(&p, prev, tex, 6.0, pal.hover);
    if tex.is_none() {
        widgets::draw_in(
            &p,
            Rect::from_center_size(prev.center(), Vec2::splat(32.0)),
            Icon::Browse,
            pal.weak,
        );
    }
    let tx = rect.left() + 14.0;
    let tw = rect.width() - 28.0;
    widgets::text(
        ui,
        egui::pos2(tx, prev.bottom() + 22.0),
        Align2::LEFT_CENTER,
        &t.name,
        16.0,
        Weight::Bold,
        pal.text,
        tw - 90.0,
    );
    let sub = if t.official {
        format!("Official · {}", t.author)
    } else if t.stars > 0 {
        format!("★ {} · {}", t.stars, t.author)
    } else {
        t.author.clone()
    };
    widgets::text(
        ui,
        egui::pos2(tx, prev.bottom() + 44.0),
        Align2::LEFT_CENTER,
        &sub,
        13.0,
        Weight::Regular,
        pal.weak,
        tw - 90.0,
    );
    if !t.description.is_empty() && t.description != t.name {
        widgets::text(
            ui,
            egui::pos2(tx, prev.bottom() + 64.0),
            Align2::LEFT_CENTER,
            &t.description,
            12.0,
            Weight::Regular,
            widgets::with_alpha(pal.weak, 0.8),
            tw,
        );
    }
    // Install / installed button (top-right of the text area).
    let b = Rect::from_min_size(
        egui::pos2(rect.right() - 92.0, prev.bottom() + 16.0),
        Vec2::new(80.0, 30.0),
    );
    let installing = app.market.installing.contains(&t.id);
    let br = ui.interact(b, id.with("btn"), Sense::click());
    let bh = ui
        .ctx()
        .animate_bool_with_time(id.with("bh"), br.hovered(), 0.12);
    if installing {
        p.rect_filled(b, 15.0, pal.hover);
        widgets::text(
            ui,
            b.center(),
            Align2::CENTER_CENTER,
            "Installing…",
            12.0,
            Weight::Semibold,
            pal.weak,
            b.width(),
        );
    } else if installed {
        p.rect_stroke(
            b,
            15.0,
            egui::Stroke::new(1.0, widgets::with_alpha(pal.weak, 0.6 + 0.4 * bh)),
            egui::StrokeKind::Inside,
        );
        widgets::text(
            ui,
            b.center(),
            Align2::CENTER_CENTER,
            "Installed",
            12.5,
            Weight::Semibold,
            pal.text,
            b.width(),
        );
    } else {
        p.rect_filled(
            b.expand(bh),
            15.0,
            widgets::mix(pal.accent, pal.accent_hover, bh),
        );
        widgets::text(
            ui,
            b.center(),
            Align2::CENTER_CENTER,
            "Install",
            13.0,
            Weight::Bold,
            pal.on_accent,
            b.width(),
        );
    }
    let br = br.on_hover_cursor(egui::CursorIcon::PointingHand);
    if br.clicked() {
        if installed {
            *act = Some(Act::Open(t.id.clone()));
        } else if !installing {
            *act = Some(Act::Install(t.clone()));
        }
    } else if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *act = Some(Act::Open(t.id.clone()));
    }
}

fn detail(app: &mut App, ui: &mut egui::Ui, t: &MarketTheme, act: &mut Option<Act>) {
    let pal = app.palette;
    ui.horizontal(|ui| {
        if widgets::icon_button(ui, Icon::Back, 18.0, "Back", pal.weak, pal.text).clicked() {
            *act = Some(Act::Back);
        }
        ui.label(egui::RichText::new("All themes").color(pal.weak));
    });
    ui.add_space(8.0);
    let w = ui.available_width().min(960.0);
    let (prev, _) = ui.allocate_exact_size(Vec2::new(w, w * 0.56), Sense::hover());
    widgets::shadow(ui.painter(), prev, 8.0);
    let tex = t
        .preview
        .as_deref()
        .and_then(|u| app.art.get_sized(u, 1024));
    widgets::image_cover(ui.painter(), prev, tex, 8.0, pal.panel);
    ui.add_space(16.0);
    heading(app, ui, &t.name, 32.0);
    ui.label(
        egui::RichText::new(if t.official {
            format!("Official Spicetify theme · by {}", t.author)
        } else {
            format!("by {} · ★ {}", t.author, t.stars)
        })
        .color(pal.weak),
    );
    if !t.description.is_empty() && t.description != t.name {
        ui.add_space(6.0);
        ui.label(&t.description);
    }
    ui.add_space(12.0);
    let installed = app.themes.iter().find(|x| x.id == t.id).cloned();
    ui.horizontal(|ui| {
        match &installed {
            None => {
                let busy = app.market.installing.contains(&t.id);
                let label = if busy {
                    "Installing…"
                } else {
                    "Install and apply"
                };
                if pill_button(
                    ui,
                    label,
                    pal.accent,
                    pal.on_accent,
                    egui::Id::new("detail-install"),
                )
                .clicked()
                    && !busy
                {
                    *act = Some(Act::Install(t.clone()));
                }
            }
            Some(it) => {
                if pill_button(
                    ui,
                    "Remove",
                    pal.panel,
                    pal.text,
                    egui::Id::new("detail-remove"),
                )
                .clicked()
                {
                    *act = Some(Act::Remove(it.id.clone()));
                }
            }
        }
        if pill_button(
            ui,
            "Open on GitHub",
            pal.panel,
            pal.text,
            egui::Id::new("detail-gh"),
        )
        .clicked()
        {
            let _ = open::that_detached(&t.repo);
        }
    });
    if let Some(it) = installed {
        ui.add_space(20.0);
        heading(app, ui, "Colour schemes", 20.0);
        let current = (
            app.config.ui.color_theme.clone(),
            app.config.ui.color_scheme.clone(),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            for s in &it.schemes {
                let selected = current.0 == it.id && current.1 == s.name;
                let r = widgets::scheme_chip(
                    ui,
                    egui::Id::new(("scheme", &it.id, &s.name)),
                    &s.name,
                    &s.swatches(),
                    selected,
                    &pal,
                );
                if r.hovered() && !selected {
                    app.theme_hover = Some((it.id.clone(), s.name.clone()));
                }
                if r.clicked() {
                    *act = Some(Act::Apply(it.id.clone(), s.name.clone()));
                }
            }
        });
    }
    ui.add_space(24.0);
}
