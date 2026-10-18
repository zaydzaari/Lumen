//! Settings page in Spotify's layout: one column of rows, each with a label
//! (and a short explanation) on the left and its control on the right. A
//! section bar at the top jumps to sections, and the search box filters rows.

use egui::{Align, Align2, Color32, Layout, Rect, RichText, Sense, Vec2};

use super::theme::Palette;
use super::widgets::{self, Icon, Weight};
use super::App;
use crate::cache::fmt_bytes;
use crate::config::{parse_hex_color, to_hex_color, ThemeMode};
use crate::events::{AppEvent, ToastKind};
use crate::player::PlayerCmd;

const MAX_W: f32 = 880.0;
const CONTROL_W: f32 = 300.0;

const SECTIONS: [(&str, &str); 9] = [
    ("account", "Account"),
    ("library", "Local files"),
    ("audio", "Audio"),
    ("display", "Display"),
    ("window", "Startup & window"),
    ("discord", "Discord"),
    ("storage", "Storage"),
    ("shortcuts", "Shortcuts"),
    ("plugins", "Plugins"),
];

/// Per-frame drawing context: palette, search query, scroll target.
struct Ctx {
    pal: Palette,
    query: String,
    scroll_to: Option<String>,
}

impl Ctx {
    fn searching(&self) -> bool {
        !self.query.is_empty()
    }
    fn matches(&self, a: &str, b: &str) -> bool {
        !self.searching()
            || a.to_lowercase().contains(&self.query)
            || b.to_lowercase().contains(&self.query)
    }
}

fn section(ui: &mut egui::Ui, c: &mut Ctx, id: &str, title: &str) {
    if c.searching() {
        return;
    }
    ui.add_space(36.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
    widgets::text(
        ui,
        r.left_center(),
        Align2::LEFT_CENTER,
        title,
        20.0,
        Weight::Bold,
        c.pal.text,
        r.width(),
    );
    if c.scroll_to.as_deref() == Some(id) {
        ui.scroll_to_rect_animation(
            r.expand2(Vec2::new(0.0, 12.0)),
            Some(Align::TOP),
            egui::style::ScrollAnimation::duration(0.35),
        );
        c.scroll_to = None;
    }
    ui.add_space(6.0);
}

/// A setting row. Returns `false` (and draws nothing) if filtered out.
fn row(
    ui: &mut egui::Ui,
    c: &Ctx,
    title: &str,
    desc: &str,
    control: impl FnOnce(&mut egui::Ui),
) -> bool {
    if !c.matches(title, desc) {
        return false;
    }
    let w = ui.available_width();
    let left_w = (w - CONTROL_W - 24.0).max(180.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let left = ui
            .allocate_ui_with_layout(Vec2::new(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(left_w);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(title)
                        .font(widgets::font(14.0, Weight::Regular))
                        .color(c.pal.text),
                );
                if !desc.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(desc)
                                .font(widgets::font(13.0, Weight::Regular))
                                .color(c.pal.weak),
                        )
                        .wrap(),
                    );
                }
                ui.add_space(8.0);
            })
            .response
            .rect;
        // The control column is exactly as tall as the text, centred on it.
        let h = left.height().max(48.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), h),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                control(ui)
            },
        );
    });
    true
}

/// Outlined pill button (Spotify's secondary buttons).
fn button(ui: &mut egui::Ui, label: &str, pal: &Palette) -> egui::Response {
    button_ex(ui, label, pal, false, true)
}

fn button_ex(
    ui: &mut egui::Ui,
    label: &str,
    pal: &Palette,
    primary: bool,
    enabled: bool,
) -> egui::Response {
    let fg = if primary {
        super::theme::contrast_on(pal.text)
    } else {
        pal.text
    };
    let g = widgets::galley(ui, label, 13.5, Weight::Bold, fg, 400.0);
    let (rect, r) = ui.allocate_exact_size(
        Vec2::new(g.size().x + 32.0, 32.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let h = ui
        .ctx()
        .animate_bool_with_time(r.id, r.hovered() && enabled, 0.12);
    let alpha = if enabled { 1.0 } else { 0.4 };
    if primary {
        ui.painter()
            .rect_filled(rect.expand(h), 16.0, widgets::with_alpha(pal.text, alpha));
    } else {
        ui.painter().rect_stroke(
            rect.expand(h),
            16.0,
            egui::Stroke::new(
                1.0,
                widgets::with_alpha(pal.weak, (0.55 + 0.45 * h) * alpha),
            ),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().galley(
        rect.center() - g.size() / 2.0,
        g,
        widgets::with_alpha(fg, alpha),
    );
    r.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::NotAllowed
    })
}

/// Filled text box.
fn input(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
    pal: &Palette,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::hover());
    let id = ui.id().with(("input", hint));
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter()
        .rect_filled(rect, 4.0, widgets::mix(pal.bg, pal.text, 0.08));
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(
            1.0,
            if focused {
                widgets::with_alpha(pal.text, 0.7)
            } else {
                widgets::with_alpha(pal.text, 0.12)
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.put(
        rect.shrink2(Vec2::new(10.0, 8.0)),
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(egui::Frame::NONE)
            .font(widgets::font(14.0, Weight::Regular))
            .vertical_align(Align::Center)
            .hint_text(RichText::new(hint).color(widgets::with_alpha(pal.weak, 0.7))),
    )
}

/// Spotify-style dropdown: dark box with the current value and a chevron.
fn dropdown<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut T,
    options: &[(T, &str)],
    pal: &Palette,
) -> bool {
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| *l)
        .unwrap_or("");
    let g = widgets::galley(ui, current, 14.0, Weight::Regular, pal.text, 260.0);
    let (rect, r) = ui.allocate_exact_size(
        Vec2::new((g.size().x + 52.0).max(150.0), 36.0),
        Sense::click(),
    );
    let h = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.12);
    ui.painter().rect_filled(
        rect,
        4.0,
        widgets::mix(widgets::mix(pal.bg, pal.text, 0.1), pal.text, 0.06 * h),
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 12.0, rect.center().y - g.size().y / 2.0),
        g,
        pal.text,
    );
    // Chevron
    let c = egui::pos2(rect.right() - 18.0, rect.center().y);
    ui.painter().line(
        vec![
            c + Vec2::new(-4.5, -2.0),
            c + Vec2::new(0.0, 2.5),
            c + Vec2::new(4.5, -2.0),
        ],
        egui::Stroke::new(1.6, pal.text),
    );
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut changed = false;
    egui::Popup::menu(&r)
        .id(ui.id().with(("dropdown", id)))
        .align(egui::RectAlign::BOTTOM_END)
        .gap(4.0)
        .show(|ui| {
            widgets::menu_style(ui, rect.width().max(180.0));
            for (v, label) in options {
                let selected = v == value;
                let resp = widgets::menu_item(ui, label, false, pal);
                if selected {
                    widgets::draw(
                        ui,
                        Rect::from_center_size(
                            egui::pos2(resp.rect.right() - 18.0, resp.rect.center().y),
                            Vec2::splat(14.0),
                        ),
                        Icon::Saved,
                        pal.accent,
                    );
                }
                if resp.clicked() {
                    *value = *v;
                    changed = true;
                    ui.close();
                }
            }
        });
    changed
}

/// Thin slider in the player-bar style, snapping to `step`.
fn slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    min: f32,
    max: f32,
    step: f32,
    label: &str,
    pal: &Palette,
) -> egui::Response {
    let mut v = *value - min;
    let r = super::player_bar::progress(ui, &mut v, max - min, 160.0, pal.accent, label);
    *value = (min + (v / step).round() * step).clamp(min, max);
    r
}

pub(super) fn keycaps(ui: &mut egui::Ui, combo: &str, pal: &Palette) {
    // Right-to-left layout: draw keys in reverse so they read left to right.
    for key in combo.split('+').rev() {
        let g = widgets::galley(ui, key.trim(), 12.5, Weight::Semibold, pal.text, 200.0);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(g.size().x.max(14.0) + 16.0, 28.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, 4.0, widgets::mix(pal.bg, pal.text, 0.1));
        ui.painter().rect_filled(
            Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.max),
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: 4,
                se: 4,
            },
            widgets::mix(pal.bg, pal.text, 0.18),
        );
        ui.painter().galley(
            rect.center() - g.size() / 2.0 - Vec2::new(0.0, 1.0),
            g,
            pal.text,
        );
    }
}

fn status_dot(ui: &mut egui::Ui, text: &str, ok: bool, pal: &Palette) {
    let g = widgets::galley(ui, text, 13.0, Weight::Regular, pal.weak, 260.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 18.0, 28.0), Sense::hover());
    ui.painter().circle_filled(
        egui::pos2(rect.left() + 5.0, rect.center().y),
        4.0,
        if ok {
            pal.accent
        } else {
            widgets::with_alpha(pal.weak, 0.6)
        },
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 16.0, rect.center().y - g.size().y / 2.0),
        g,
        pal.weak,
    );
}

const ACCENTS: [(&str, [u8; 3]); 8] = [
    ("Spotify green", [0x1e, 0xd7, 0x60]),
    ("Blue", [0x50, 0x9b, 0xf5]),
    ("Purple", [0xaf, 0x7b, 0xf6]),
    ("Pink", [0xf5, 0x73, 0xa0]),
    ("Red", [0xf1, 0x5e, 0x6c]),
    ("Orange", [0xff, 0x8c, 0x3a]),
    ("Yellow", [0xf5, 0xd1, 0x3c]),
    ("Teal", [0x2d, 0xd4, 0xbf]),
];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    let mut c = Ctx {
        pal,
        query: app.settings_query.trim().to_lowercase(),
        scroll_to: app.settings_scroll_to.take().or_else(|| {
            std::mem::take(&mut app.settings_scroll_to_discord).then(|| "discord".to_string())
        }),
    };

    // ------------------------------------------------ header: title, search, section bar
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Settings")
                .font(widgets::font(32.0, Weight::Black))
                .color(pal.text),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (field, _) = ui.allocate_exact_size(Vec2::new(240.0, 36.0), Sense::hover());
            ui.painter()
                .rect_filled(field, 18.0, widgets::mix(pal.bg, pal.text, 0.08));
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
                egui::TextEdit::singleline(&mut app.settings_query)
                    .frame(egui::Frame::NONE)
                    .vertical_align(Align::Center)
                    .hint_text(RichText::new("Search settings").color(pal.weak)),
            );
        });
    });
    ui.add_space(8.0);
    if !c.searching() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            for (id, label) in SECTIONS {
                if widgets::chip(ui, label, false, &pal).clicked() {
                    app.settings_scroll_to = Some(id.to_string());
                }
            }
        });
    }
    ui.add_space(4.0);
    let sep = ui.available_rect_before_wrap();
    ui.painter().hline(
        sep.x_range(),
        sep.top(),
        egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.08)),
    );

    egui::ScrollArea::vertical()
        .id_salt("settings-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(MAX_W.min(ui.available_width()));
            ui.add_space(4.0);
            account(app, ui, &mut c);
            local_files(app, ui, &mut c);
            audio(app, ui, &mut c);
            display(app, ui, &mut c);
            window(app, ui, &mut c);
            discord(app, ui, &mut c);
            storage(app, ui, &mut c);
            shortcuts(app, ui, &mut c);
            plugins(app, ui, &mut c);
            if !c.searching() {
                ui.add_space(40.0);
                ui.label(
                    RichText::new(format!(
                        "Lumen {} — an unofficial Spotify client, not affiliated with Spotify.",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .small()
                    .color(pal.weak),
                );
                ui.add_space(40.0);
            }
        });
}

fn account(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "account", "Account");
    if app.backend.is_some() {
        let name = app.username.clone();
        row(ui, c, &name, "Signed in with Spotify", |ui| {
            if button(ui, "Log out", &pal).clicked() {
                app.logout();
            }
            if button(ui, "Profile", &pal).clicked() {
                app.open_my_profile();
            }
            if button(ui, "Account ↗", &pal).clicked() {
                let _ = open::that_detached("https://www.spotify.com/account/overview/");
            }
        });
    } else {
        row(
            ui,
            c,
            "Not signed in",
            "Log in to play music from Spotify",
            |ui| {
                if button_ex(ui, "Log in", &pal, true, true).clicked() {
                    app.screen = super::Screen::Login;
                }
            },
        );
    }
    let connect = if app.connect_enabled {
        format!(
            "Your phone and other devices see Lumen as \"{}\"",
            app.config.device_name
        )
    } else {
        "Spotify Connect isn't available in this session".into()
    };
    row(ui, c, "Spotify Connect device name", &connect, |ui| {
        let r = input(ui, &mut app.config.device_name, "Lumen", 220.0, &pal);
        if r.lost_focus() {
            app.save_config();
            app.toast(ToastKind::Info, "Device name applies after restart");
        }
    });
    let session = app
        .backend
        .as_ref()
        .is_some_and(|b| b.api.session_browsing());
    let desc = if app.backend.is_some() && !session {
        "Your developer app is connected."
    } else {
        "Optional — everything works without it. Adds Spotify's own Recently played and Top lists to Home."
    };
    row(ui, c, "Spotify developer app (client ID)", desc, |ui| {
        if button_ex(
            ui,
            "Connect",
            &pal,
            true,
            !app.login.busy && !app.config.client_id.is_empty() && app.backend.is_some(),
        )
        .clicked()
        {
            app.save_config();
            app.connect_library();
        }
        let r = input(ui, &mut app.config.client_id, "Client ID", 220.0, &pal);
        if r.lost_focus() {
            app.save_config();
        }
    });
    if (session || app.config.client_id.is_empty()) && c.matches("developer dashboard redirect", "")
    {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Create one at").color(pal.weak).small());
            ui.hyperlink_to(
                RichText::new("developer.spotify.com/dashboard").small(),
                "https://developer.spotify.com/dashboard",
            );
            ui.label(RichText::new("with redirect URI").color(pal.weak).small());
            ui.label(RichText::new(app.config.redirect_uri()).monospace().small());
        });
    }
    if let Some(e) = &app.login.error {
        ui.colored_label(pal.notification_error, e);
    }
}

fn local_files(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "library", "Local files");
    let mut remove = None;
    for (i, folder) in app.config.local.folders.clone().iter().enumerate() {
        let busy = app.local_scanning;
        row(ui, c, &folder.display().to_string(), "Music folder", |ui| {
            if button_ex(ui, "Remove", &pal, false, !busy).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        app.config.local.folders.remove(i);
        app.save_config();
        app.rescan_local();
    }
    row(
        ui,
        c,
        "Add a music folder",
        "MP3, FLAC, Ogg, Opus, M4A and WAV — your own files only",
        |ui| {
            if button_ex(ui, "Add", &pal, true, !app.local_scanning).clicked() {
                let folder = std::path::PathBuf::from(app.local_folder_input.trim());
                if folder.is_dir() {
                    if !app.config.local.folders.contains(&folder) {
                        app.config.local.folders.push(folder);
                    }
                    app.local_folder_input.clear();
                    app.save_config();
                    app.rescan_local();
                } else {
                    app.toast(ToastKind::Error, "Choose an existing music folder");
                }
            }
            input(
                ui,
                &mut app.local_folder_input,
                "C:\\Users\\you\\Music",
                220.0,
                &pal,
            );
        },
    );
    let count = format!("{} songs found", app.local_tracks.len());
    row(ui, c, "Scan folders on startup", &count, |ui| {
        if app.local_scanning {
            ui.spinner();
        } else if button(ui, "Rescan now", &pal).clicked() {
            app.rescan_local();
        }
        ui.add_space(12.0);
        if ui
            .add(widgets::Switch::new(&mut app.config.local.scan_on_start, "").inline())
            .changed()
        {
            app.save_config();
        }
    });
}

fn audio(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "audio", "Audio quality");
    let before = app.config.audio.clone();
    row(
        ui,
        c,
        "Streaming quality",
        "Higher quality uses more data",
        |ui| {
            dropdown(
                ui,
                "bitrate",
                &mut app.config.audio.bitrate,
                &[
                    (96, "Low (96 kbps)"),
                    (160, "Normal (160 kbps)"),
                    (320, "Very high (320 kbps)"),
                ],
                &pal,
            );
        },
    );
    row(
        ui,
        c,
        "Normalize volume",
        "Set the same volume level for all songs",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.audio.normalisation, "").inline());
        },
    );
    if app.config.audio.normalisation {
        row(
            ui,
            c,
            "Volume level",
            "Adjust the volume for your environment. Loud may reduce audio quality",
            |ui| {
                let mut v = match app.config.audio.volume_level.as_str() {
                    "loud" => 2u8,
                    "quiet" => 0,
                    _ => 1,
                };
                if dropdown(
                    ui,
                    "volume-level",
                    &mut v,
                    &[(2, "Loud"), (1, "Normal"), (0, "Quiet")],
                    &pal,
                ) {
                    app.config.audio.volume_level = ["quiet", "normal", "loud"][v as usize].into();
                }
            },
        );
    }
    row(
        ui,
        c,
        "Autoplay",
        "Keep playing similar songs when your music ends",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.audio.autoplay, "").inline());
        },
    );
    row(
        ui,
        c,
        "Gapless playback",
        "No silence between songs that flow into each other",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.audio.gapless, "").inline());
        },
    );
    let mut after = app.config.audio.clone();
    after.autoplay = before.autoplay;
    if app.config.audio.autoplay != before.autoplay {
        app.save_config();
    }
    if after != before {
        app.save_config();
        app.send(PlayerCmd::ApplyAudio(crate::backend::audio_settings_of(
            &app.config,
        )));
    }
}

fn display(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "display", "Display");
    let before = app.config.ui.clone();
    let custom = !app.config.ui.color_theme.is_empty();
    let theme_desc = if custom {
        app.themes
            .iter()
            .find(|t| t.id == app.config.ui.color_theme)
            .map(|t| {
                format!(
                    "Using {} · {} from the theme marketplace",
                    t.name, app.config.ui.color_scheme
                )
            })
            .unwrap_or_else(|| "Using a custom theme".into())
    } else {
        "Dark, light, or one of hundreds of Spicetify colour themes".into()
    };
    row(ui, c, "Theme", &theme_desc, |ui| {
        if button(ui, "Browse themes", &pal).clicked() {
            app.navigate(super::View::Themes);
        }
        ui.add_space(8.0);
        let mut mode = if custom {
            None
        } else {
            Some(app.config.ui.theme)
        };
        let opts = [
            (Some(ThemeMode::Light), "Light"),
            (Some(ThemeMode::Dark), "Dark"),
            (None, "Custom theme"),
        ];
        let shown: Vec<(Option<ThemeMode>, &str)> = opts
            .iter()
            .copied()
            .filter(|(m, _)| m.is_some() || custom)
            .collect();
        if dropdown(ui, "theme", &mut mode, &shown, &pal) {
            if let Some(m) = mode {
                app.config.ui.theme = m;
                app.config.ui.color_theme.clear();
                app.config.ui.color_scheme.clear();
            }
        }
    });
    if !custom {
        row(
            ui,
            c,
            "Accent colour",
            "Buttons, progress bars and highlights",
            |ui| {
                let current = parse_hex_color(&app.config.ui.accent).unwrap_or([30, 215, 96]);
                let mut rgb = current;
                if ui
                    .color_edit_button_srgb(&mut rgb)
                    .on_hover_text("Custom colour")
                    .changed()
                {
                    app.config.ui.accent = to_hex_color(rgb);
                }
                ui.add_space(6.0);
                for (name, col) in ACCENTS.iter().rev() {
                    let (rect, r) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                    let selected = *col == current;
                    let h = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.1);
                    ui.painter().circle_filled(
                        rect.center(),
                        10.0 + 1.5 * h,
                        Color32::from_rgb(col[0], col[1], col[2]),
                    );
                    if selected {
                        ui.painter().circle_stroke(
                            rect.center(),
                            12.5,
                            egui::Stroke::new(2.0, pal.text),
                        );
                    }
                    if r.on_hover_text(*name)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        app.config.ui.accent = to_hex_color(*col);
                    }
                }
            },
        );
    }
    row(
        ui,
        c,
        "Animations",
        "Page fades, hover effects, sliding panels and theme cross-fades",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.ui.animations, "").inline());
        },
    );
    row(
        ui,
        c,
        "See what your friends are playing",
        "Show the Friend Activity button in the top bar",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.ui.show_friends, "").inline());
        },
    );
    row(
        ui,
        c,
        "Compact library",
        "Show Your Library as a strip of covers",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.ui.compact_library, "").inline());
        },
    );
    let zoom = format!("{:.0}%", app.config.ui.zoom * 100.0);
    row(ui, c, "Zoom", "Size of everything in the window", |ui| {
        ui.label(RichText::new(&zoom).color(pal.weak));
        let r = slider(ui, &mut app.config.ui.zoom, 0.7, 2.0, 0.05, "Zoom", &pal);
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            ui.ctx().set_zoom_factor(app.config.ui.zoom);
        }
    });
    if app.config.ui != before {
        app.save_config();
        app.apply_theme_now();
    }
}

fn window(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    section(ui, c, "window", "Startup and window behaviour");
    let before = app.config.ui.clone();
    row(ui, c, "Show tray icon", "Applies after restart", |ui| {
        ui.add(widgets::Switch::new(&mut app.config.ui.tray, "").inline());
    });
    let tray = app.config.ui.tray;
    row(
        ui,
        c,
        "Close button minimises to tray",
        "Keep playing in the background",
        |ui| {
            ui.add_enabled(
                tray,
                widgets::Switch::new(&mut app.config.ui.minimize_to_tray, "").inline(),
            );
        },
    );
    row(
        ui,
        c,
        "Spotify-style title bar",
        "Lumen draws its own window buttons. Applies after restart",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.ui.custom_title_bar, "").inline());
        },
    );
    row(
        ui,
        c,
        "Free unused memory",
        "Windows: give memory back when idle or minimised",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.ui.trim_memory, "").inline());
        },
    );
    if app.config.ui != before {
        app.save_config();
    }
}

fn discord(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "discord", "Discord");
    let before = app.config.discord.clone();
    let status = app.discord.status();
    let ok = status == "Connected";
    row(
        ui,
        c,
        "Show what you're listening to",
        "\"Listening to …\" on your Discord profile, with album art and progress",
        |ui| {
            ui.add(widgets::Switch::new(&mut app.config.discord.enabled, "").inline());
            ui.add_space(12.0);
            status_dot(ui, &status, ok, &pal);
        },
    );
    let enabled = app.config.discord.enabled;
    ui.add_enabled_ui(enabled, |ui| {
        row(
            ui,
            c,
            "Discord Application ID",
            "From discord.com/developers — its name is what Discord shows after \"Listening to\"",
            |ui| {
                input(
                    ui,
                    &mut app.config.discord.client_id,
                    "1234567890123456789",
                    240.0,
                    &pal,
                );
            },
        );
        row(
            ui,
            c,
            "Activity name",
            "Optional: overrides the application's name",
            |ui| {
                input(
                    ui,
                    &mut app.config.discord.activity_name,
                    "e.g. Spotify",
                    240.0,
                    &pal,
                );
            },
        );
        row(
            ui,
            c,
            "First line",
            "Placeholders: {title}, {artist}, {album}",
            |ui| {
                input(ui, &mut app.config.discord.details, "{title}", 240.0, &pal);
            },
        );
        row(
            ui,
            c,
            "Second line",
            "Placeholders: {title}, {artist}, {album}",
            |ui| {
                input(ui, &mut app.config.discord.state, "{artist}", 240.0, &pal);
            },
        );
        row(
            ui,
            c,
            "Album art tooltip",
            "Shown when hovering the cover in Discord",
            |ui| {
                input(
                    ui,
                    &mut app.config.discord.large_text,
                    "{album}",
                    240.0,
                    &pal,
                );
            },
        );
        row(ui, c, "Show album art", "", |ui| {
            ui.add(widgets::Switch::new(&mut app.config.discord.show_album_art, "").inline());
        });
        row(ui, c, "Show progress bar", "", |ui| {
            ui.add(widgets::Switch::new(&mut app.config.discord.show_progress, "").inline());
        });
        row(ui, c, "Show \"Open in Spotify\" button", "", |ui| {
            ui.add(widgets::Switch::new(&mut app.config.discord.show_button, "").inline());
        });
        row(ui, c, "Keep status while paused", "", |ui| {
            ui.add(widgets::Switch::new(&mut app.config.discord.show_when_paused, "").inline());
        });
    });
    if !c.searching() {
        egui::CollapsingHeader::new(RichText::new("How do I get an Application ID? (1 minute)").color(pal.weak))
            .default_open(app.config.discord.client_id.is_empty())
            .show(ui, |ui| {
                ui.label("1. Open the Discord Developer Portal and log in:");
                ui.hyperlink("https://discord.com/developers/applications");
                ui.label("2. Click \"New Application\". The name you choose is shown as \"Listening to <name>\" — e.g. \"Spotify\" or \"Lumen\".");
                ui.label("3. Copy the \"Application ID\" from General Information and paste it above.");
                ui.label("4. Keep the Discord desktop app running. Album art is sent automatically.");
            });
    }
    if app.config.discord != before {
        app.save_config();
        app.discord.set_config(app.config.discord.clone());
    }
}

fn storage(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "storage", "Storage");
    let usage = match app.cache_sizes {
        Some((audio, art, lyrics)) => format!(
            "In use: songs {} · covers {} · lyrics {}",
            fmt_bytes(audio),
            fmt_bytes(art),
            fmt_bytes(lyrics)
        ),
        None => "Measuring…".into(),
    };
    let mut independent = app.config.downloads.independent;
    row(ui, c, "Playback without Premium", "Download a requested song before playback. Saved songs play locally. Spotify streaming login is skipped on startup.", |ui| {
        if ui.add(widgets::Switch::new(&mut independent, "").inline()).changed() {
            app.config.downloads.independent = independent;
            app.send(PlayerCmd::Independent(independent));
            app.save_config();
        }
    });
    row(ui, c, "Import Spotify playlists", "Paste a Spotify track, album or public playlist link, or a path to extracted Playlist*.json / YourLibrary.json. Links use the download connection.", |ui| {
        ui.add(egui::TextEdit::singleline(&mut app.free_import_path).hint_text("Spotify link or JSON file path").desired_width(170.0));
        if button_ex(ui, "Import", &pal, false, !app.free_importing).clicked() { app.import_free_library(); }
    });
    row(ui, c, "Spotify account data", "Import works without a Spotify streaming login. Audio availability depends on the download provider. Importing does not download everything automatically.", |ui| {
        if button(ui, "Get my data", &pal).clicked() { let _ = open::that_detached("https://www.spotify.com/account/privacy/"); }
    });
    let mut automatic = app.config.downloads.automatic;
    row(ui,c,"Automatically save played songs","Uses your download connection or saved token; otherwise matches public sources. Completed files stay in your music folder for offline playback.",|ui| {
        if ui.add(widgets::Switch::new(&mut automatic,"").inline()).changed() {
            app.config.downloads.automatic=automatic;
            if let Some(downloader)=&app.downloader {downloader.set_enabled(automatic);}
            app.save_config();
        }
    });
    #[cfg(any(windows, target_os = "linux"))]
    {
        let connection_help = if app.download_verification.is_some() {
            "Complete verification in Lumen. Playback continues while waiting."
        } else if app.download_connection.enabled() {
            "Verification opens inside Lumen only when needed, then closes automatically."
        } else {
            "Connect inside Lumen without copying tokens. The browser runs only during verification."
        };
        row(ui, c, "SpotiDownloader connection", connection_help, |ui| {
            let verifying = app.download_verification.is_some();
            if button(ui, if verifying { "Verifying…" } else { "Connect" }, &pal).clicked()
                && !verifying
            {
                app.connect_downloads();
            }
            if button(ui, "Disconnect", &pal).clicked() {
                app.download_connection.set_enabled(false);
                app.config.downloads.browser_connection = false;
                app.download_verification.take();
                app.shared.store.delete(crate::spotidownloader::KEY);
                app.download_token_saved =
                    app.shared.store.load(crate::spotidownloader::KEY).is_some();
                app.save_config();
            }
        });
    }
    egui::CollapsingHeader::new("Advanced: manual download token").show(ui, |ui| {
    let token_help = if app.download_token_saved {
        "Token saved locally. Paste a replacement when it expires. Never stored in config.toml."
    } else {
        "Paste SpotiDownloader's Bearer token, then Save. Hover the field for instructions."
    };
    row(ui, c, "SpotiDownloader token", token_help, |ui| {
        let edit = egui::TextEdit::singleline(&mut app.download_token_input)
            .password(true)
            .hint_text(if app.download_token_saved {
                "Replace token…"
            } else {
                "Paste token…"
            })
            .desired_width(132.0);
        ui.add(edit).on_hover_text("Optional fallback: copy only the Authorization Bearer value from the site's successful /download request. The Connect button handles normal verification inside Lumen without copying a token. This is not your Spotify login token.");
        if button(ui, "Save", &pal).clicked() {
            let result = crate::spotidownloader::normalize_token(&app.download_token_input)
                .and_then(|token| app.shared.store.save(crate::spotidownloader::KEY, &token));
            match result {
                Ok(()) => {
                    app.download_token_input.clear();
                    app.download_token_saved = true;
                    app.toast(
                        ToastKind::Info,
                        "SpotiDownloader token saved locally. Applies to the next download.",
                    );
                }
                Err(_) => app.toast(
                    ToastKind::Error,
                    "Could not save token. Paste only its Bearer value and try again.",
                ),
            }
        }
        if button(ui, "Clear", &pal).clicked() {
            app.shared.store.delete(crate::spotidownloader::KEY);
            app.download_token_input.clear();
            app.download_token_saved = app.shared.store.load(crate::spotidownloader::KEY).is_some();
            if app.download_token_saved {
                app.toast(ToastKind::Error, "Could not remove the saved token.");
            } else {
                app.toast(
                    ToastKind::Info,
                    "SpotiDownloader token removed. Public-source matching is active.",
                );
            }
        }
    });
    });
    let folder = crate::downloads::folder(&app.config.downloads, &app.shared.paths);
    let folder_text = folder.display().to_string();
    row(ui, c, "Permanent music folder", &folder_text, |ui| {
        if button(ui, "Open folder", &pal).clicked() {
            if let Err(e) = open::that(&folder) {
                app.toast(
                    ToastKind::Error,
                    format!("Could not open downloads folder: {e}"),
                );
            }
        }
    });
    let status = format!(
        "{} saved · {}",
        app.download_tracks.len(),
        app.download_status
    );
    row(ui, c, "Download status", &status, |ui| {
        if button(ui, "Local files", &pal).clicked() {
            app.show_local();
        }
        if button(ui, "Retry song", &pal).clicked() {
            if let (Some(downloader), Some(track)) = (&app.downloader, &app.snapshot.track) {
                downloader.request(track.clone());
            }
        }
    });
    let mut enabled = app.config.cache.audio_cache;
    row(
        ui,
        c,
        "Streaming cache",
        "Encrypted streaming cache. Authentication can still need internet. Applies after restart",
        |ui| {
            if ui
                .add(widgets::Switch::new(&mut enabled, "").inline())
                .changed()
            {
                app.config.cache.audio_cache = enabled;
                app.save_config();
            }
        },
    );
    let mut gb = app.config.cache.audio_cache_mb as f32 / 1024.0;
    let size = format!("{gb:.2} GB").replace(".00", "");
    row(
        ui,
        c,
        "Cache size limit",
        "Oldest songs are removed when it's full. Applies after restart",
        |ui| {
            ui.label(RichText::new(&size).color(pal.weak));
            let r = slider(ui, &mut gb, 0.25, 20.0, 0.25, "Cache size", &pal);
            if r.changed() {
                app.config.cache.audio_cache_mb = (gb * 1024.0).round() as u64;
            }
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                app.save_config();
            }
        },
    );
    row(ui, c, "Clear cache", &usage, |ui| {
        if button(ui, "Clear covers & lyrics", &pal).clicked() {
            let _ = app.art.disk().clear();
            let _ = app.lyrics_cache.clear();
            app.art.clear_memory();
            app.toast(ToastKind::Info, "Covers and lyrics cache cleared");
            app.refresh_cache_sizes();
        }
        if button(ui, "Clear songs", &pal).clicked() {
            let paths = app.shared.paths.clone();
            let ui_tx = app.ui_tx.clone();
            app.shared.rt.spawn_blocking(move || {
                match crate::cache::clear_dir(&paths.audio_cache_dir()) {
                    Ok(()) => ui_tx.toast(ToastKind::Info, "Song cache cleared"),
                    Err(e) => {
                        ui_tx.toast(ToastKind::Error, format!("Could not clear cache: {e:#}"))
                    }
                }
                ui_tx.send(AppEvent::CacheSizes {
                    audio: crate::cache::dir_size(&paths.audio_cache_dir()),
                    art: crate::cache::dir_size(&paths.art_cache_dir()),
                    lyrics: crate::cache::dir_size(&paths.lyrics_cache_dir()),
                });
            });
        }
    });
    let logs = app.shared.paths.logs_dir();
    row(
        ui,
        c,
        "Logs and settings files",
        "For troubleshooting, or editing config.toml by hand",
        |ui| {
            if button(ui, "Open logs", &pal).clicked() {
                let _ = open::that_detached(&logs);
            }
            if button(ui, "Open config folder", &pal).clicked() {
                let _ = open::that_detached(&app.shared.paths.config_dir);
            }
        },
    );
}

fn shortcuts(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "shortcuts", "Keyboard shortcuts");
    let s = app.config.shortcuts.clone();
    for (label, key) in [
        ("Play / pause", s.play_pause.clone()),
        ("Next song", s.next.clone()),
        ("Previous song", s.previous.clone()),
        ("Search", s.search.clone()),
        ("Volume up", s.volume_up.clone()),
        ("Volume down", s.volume_down.clone()),
        ("Full screen", "F11".to_string()),
        ("Mini player", "Ctrl+Shift+M".to_string()),
        ("Like the current song", "Alt+Shift+B".to_string()),
        ("Go back", "Alt+Left".to_string()),
        ("Go forward", "Alt+Right".to_string()),
        ("Select song in a list", "Up / Down".to_string()),
        ("Play the selected song", "Enter".to_string()),
        ("Memory & FPS overlay", s.debug_overlay.clone()),
        ("Quit", s.quit.clone()),
    ] {
        row(ui, c, label, "", |ui| keycaps(ui, &key, &pal));
    }
    row(
        ui,
        c,
        "Change shortcuts",
        "Edit the [shortcuts] section of config.toml, then reload",
        |ui| {
            if button(ui, "Reload", &pal).clicked() {
                let loaded = crate::config::Config::load(&app.shared.paths);
                app.config.shortcuts = loaded.config.shortcuts;
                app.compile_shortcuts();
                app.toast(ToastKind::Info, "Shortcuts reloaded");
            }
            if button(ui, "Open config folder", &pal).clicked() {
                let _ = open::that_detached(&app.shared.paths.config_dir);
            }
        },
    );
}

fn plugins(app: &mut App, ui: &mut egui::Ui, c: &mut Ctx) {
    let pal = c.pal;
    section(ui, c, "plugins", "Plugins");
    let infos = app.plugins.infos().to_vec();
    let mut changed = false;
    for info in infos {
        let desc = if let Some(e) = &info.error {
            format!("Error: {e}")
        } else if info.enabled && info.commands > 0 {
            format!("{} command(s) in the profile menu", info.commands)
        } else {
            "Lua plugin".into()
        };
        row(ui, c, &info.name, &desc, |ui| {
            let mut enabled = !app.config.plugins.disabled.contains(&info.name);
            if ui
                .add(widgets::Switch::new(&mut enabled, "").inline())
                .changed()
            {
                if enabled {
                    app.config.plugins.disabled.retain(|n| n != &info.name);
                } else {
                    app.config.plugins.disabled.push(info.name.clone());
                }
                changed = true;
            }
        });
    }
    let dir = app.plugins.dir().display().to_string();
    row(ui, c, "Plugin folder", &dir, |ui| {
        if button(ui, "Reload", &pal).clicked() {
            changed = true;
        }
        if button(ui, "Open data folder", &pal).clicked() {
            let d = app.shared.paths.plugin_data_root();
            let _ = std::fs::create_dir_all(&d);
            let _ = open::that_detached(d);
        }
        if button(ui, "Open folder", &pal).clicked() {
            let _ = open::that_detached(app.plugins.dir());
        }
    });
    if changed {
        app.save_config();
        app.reload_plugins();
    }
}
