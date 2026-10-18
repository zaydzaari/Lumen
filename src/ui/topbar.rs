//! Top bar (Spotify layout): logo, ⋯ menu, back/forward, home + search pill with a
//! browse button (centred), profile avatar menu, and — with the custom title bar —
//! window drag, double-click maximise, caption buttons and edge resizing.

use egui::{Align2, Color32, Rect, Sense, Vec2};

use super::widgets::{self, Icon, Weight};
use super::{App, Screen, View, SEARCH_ID};

const H: f32 = 64.0;

fn circle_button(
    ui: &mut egui::Ui,
    rect: Rect,
    icon: Icon,
    tip: &str,
    app: &App,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let pal = app.palette;
    let r = ui.interact(
        rect,
        ui.id().with(tip),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let fill = if r.hovered() && enabled {
        widgets::mix(pal.panel, pal.text, 0.08)
    } else {
        pal.panel
    };
    ui.painter()
        .circle_filled(rect.center(), rect.width() / 2.0, fill);
    let color = if !enabled {
        widgets::with_alpha(pal.weak, 0.4)
    } else if active || r.hovered() {
        pal.text
    } else {
        pal.weak
    };
    widgets::draw(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(rect.width() * 0.5)),
        icon,
        color,
    );
    r.on_hover_text(tip)
}

/// Whether Lumen draws its own title bar (window buttons, drag, resize).
pub fn custom_chrome(app: &App) -> bool {
    app.config.ui.custom_title_bar && !cfg!(target_os = "macos")
}

/// Windows-style caption buttons (minimise, maximise/restore, close).
fn window_buttons(ui: &mut egui::Ui, app: &App, right: f32, top: f32) -> f32 {
    let pal = app.palette;
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let w = 46.0;
    let h = 32.0;
    let mut x = right - w;
    for (icon, tip, cmd) in [
        (Icon::Close, "Close", egui::ViewportCommand::Close),
        (
            if maximized {
                Icon::WinRestore
            } else {
                Icon::WinMaximize
            },
            if maximized { "Restore" } else { "Maximise" },
            egui::ViewportCommand::Maximized(!maximized),
        ),
        (
            Icon::WinMinimize,
            "Minimise",
            egui::ViewportCommand::Minimized(true),
        ),
    ] {
        let rect = Rect::from_min_size(egui::pos2(x, top), Vec2::new(w, h));
        let r = ui.interact(rect, ui.id().with(("caption", tip)), Sense::click());
        if r.hovered() {
            let fill = if icon == Icon::Close {
                Color32::from_rgb(0xc4, 0x2b, 0x1c)
            } else {
                widgets::with_alpha(pal.text, 0.1)
            };
            ui.painter().rect_filled(rect, 0.0, fill);
        }
        let size = if icon == Icon::Close { 14.0 } else { 18.0 };
        widgets::draw(
            ui,
            Rect::from_center_size(rect.center(), Vec2::splat(size)),
            icon,
            if r.hovered() {
                Color32::WHITE
            } else {
                pal.weak
            },
        );
        if r.on_hover_text(tip).clicked() {
            ui.ctx().send_viewport_cmd(cmd);
        }
        x -= w;
    }
    x + w
}

/// The "⋯" main menu (Spotify's app menu).
fn main_menu(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let pal = app.palette;
    let r = ui.interact(rect, ui.id().with("main-menu"), Sense::click());
    widgets::draw(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(20.0)),
        Icon::More,
        if r.hovered() { pal.text } else { pal.weak },
    );
    let r = r.on_hover_text("Menu");
    egui::Popup::menu(&r)
        .id(ui.id().with("main-menu-popup"))
        .align(egui::RectAlign::BOTTOM_START)
        .gap(4.0)
        .show(|ui| {
            super::widgets::menu_style(ui, 220.0);
            for (label, view) in [
                ("Home", View::Home),
                ("Search", View::Search),
                ("Liked Songs", View::Liked),
                ("Local Files", View::Local),
            ] {
                if ui.button(label).clicked() {
                    app.navigate(view);
                    ui.close();
                }
            }
            ui.menu_button("Your Library", |ui| {
                for (label, view) in [
                    ("Playlists", View::Playlists),
                    ("Albums", View::Albums),
                    ("Artists", View::Artists),
                ] {
                    if ui.button(label).clicked() {
                        app.navigate(view);
                        ui.close();
                    }
                }
            });
            ui.separator();
            if ui.button("Lyrics").clicked() {
                app.navigate(View::Lyrics);
                app.request_lyrics();
                ui.close();
            }
            if ui.button("Queue").clicked() {
                app.right = super::RightPanel::Queue;
                ui.close();
            }
            if ui.button("Now playing view").clicked() {
                app.right = super::RightPanel::NowPlaying;
                ui.close();
            }
            ui.separator();
            if app.backend.is_some() && ui.button("Refresh library").clicked() {
                app.refresh_library();
                ui.close();
            }
            if ui.button("Keyboard shortcuts   Ctrl+/").clicked() {
                app.shortcuts_open = true;
                ui.close();
            }
            if ui.button("Listening stats").clicked() {
                app.navigate(View::Stats);
                ui.close();
            }
            if app.backend.is_some() && ui.button("Create playlist").clicked() {
                app.create_playlist(Vec::new());
                ui.close();
            }
            if ui.button("Themes").clicked() {
                app.navigate(View::Themes);
                ui.close();
            }
            if ui.button("Settings").clicked() {
                app.navigate(View::Settings);
                ui.close();
            }
            if ui.button("Memory & FPS overlay (F12)").clicked() {
                app.toggle_debug();
                ui.close();
            }
            ui.separator();
            if ui.button("Quit Lumen").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                ui.close();
            }
        });
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let pal = app.palette;
    let full = ui.max_rect();
    let cy = full.center().y;
    let chrome = custom_chrome(app);

    // Window drag / double-click-to-maximise on the empty parts of the bar.
    // Registered first so every widget drawn later sits on top of it.
    if chrome {
        let drag = ui.interact(full, ui.id().with("title-drag"), Sense::click_and_drag());
        if drag.double_clicked() {
            let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        } else if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    // ------------------------------------------------ left: logo, menu, history
    let logo = Rect::from_center_size(egui::pos2(full.left() + 24.0, cy), Vec2::splat(30.0));
    ui.painter().circle_filled(logo.center(), 15.0, pal.accent);
    widgets::draw(
        ui,
        Rect::from_center_size(logo.center() + Vec2::new(1.0, 0.0), Vec2::splat(14.0)),
        Icon::Play,
        pal.on_accent,
    );
    let menu = Rect::from_center_size(egui::pos2(logo.right() + 22.0, cy), Vec2::splat(32.0));
    main_menu(app, ui, menu);
    let back = Rect::from_center_size(egui::pos2(menu.right() + 24.0, cy), Vec2::splat(32.0));
    let can_back = !app.back.is_empty();
    if circle_button(ui, back, Icon::Back, "Go back", app, false, can_back).clicked() {
        app.go_back();
    }
    let fwd = back.translate(Vec2::new(40.0, 0.0));
    let can_fwd = !app.forward.is_empty();
    if circle_button(ui, fwd, Icon::Forward, "Go forward", app, false, can_fwd).clicked() {
        app.go_forward();
    }

    // ------------------------------------------------ right: window buttons + avatar
    let mut right = full.right();
    if chrome {
        right = window_buttons(ui, app, full.right() + 8.0, full.top()) - 8.0;
    }
    let avatar = Rect::from_center_size(egui::pos2(right - 26.0, cy), Vec2::splat(44.0));
    profile(app, ui, avatar);
    // Theme marketplace (where Spotify shows "What's New" / Friend Activity).
    let themes = Rect::from_center_size(egui::pos2(avatar.left() - 22.0, cy), Vec2::splat(32.0));
    let on_themes = app.view == View::Themes;
    if circle_button(ui, themes, Icon::Palette, "Themes", app, on_themes, true).clicked() {
        if on_themes {
            app.go_back();
        } else {
            app.navigate(View::Themes);
        }
    }
    // Friend Activity, where Spotify has it.
    let friends = if app.config.ui.show_friends {
        Rect::from_center_size(egui::pos2(themes.left() - 22.0, cy), Vec2::splat(32.0))
    } else {
        themes
    };
    let on_friends = app.right == super::RightPanel::Friends;
    if app.config.ui.show_friends
        && circle_button(
            ui,
            friends,
            Icon::Friends,
            "Friend Activity",
            app,
            on_friends,
            true,
        )
        .clicked()
    {
        app.right = if on_friends {
            super::RightPanel::None
        } else {
            app.load_friends(true);
            super::RightPanel::Friends
        };
    }
    let avatar = avatar.union(themes).union(friends);

    // ------------------------------------------------ centre: home + search
    let left_edge = fwd.right() + 24.0;
    let right_edge = avatar.left() - 24.0;
    let search_w = (right_edge - left_edge - 64.0).clamp(160.0, 474.0);
    let group_w = 48.0 + 8.0 + search_w;
    let gx =
        (full.center().x - group_w / 2.0).clamp(left_edge, (right_edge - group_w).max(left_edge));
    let home = Rect::from_min_size(egui::pos2(gx, cy - 24.0), Vec2::splat(48.0));
    let on_home = app.view == View::Home;
    if circle_button(
        ui,
        home,
        if on_home {
            Icon::HomeFilled
        } else {
            Icon::Home
        },
        "Home",
        app,
        on_home,
        true,
    )
    .clicked()
    {
        app.navigate(View::Home);
    }
    let pill = Rect::from_min_size(
        egui::pos2(home.right() + 8.0, cy - 24.0),
        Vec2::new(search_w, 48.0),
    );
    let id = egui::Id::new(SEARCH_ID);
    let focused = ui.memory(|m| m.has_focus(id));
    let hovered = ui.rect_contains_pointer(pill);
    ui.painter().rect_filled(
        pill,
        24.0,
        if hovered || focused {
            widgets::mix(pal.panel, pal.text, 0.06)
        } else {
            pal.panel
        },
    );
    if focused {
        ui.painter().rect_stroke(
            pill,
            24.0,
            egui::Stroke::new(2.0, pal.text),
            egui::StrokeKind::Inside,
        );
    } else if hovered {
        ui.painter().rect_stroke(
            pill,
            24.0,
            egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.15)),
            egui::StrokeKind::Inside,
        );
    }
    widgets::draw(
        ui,
        Rect::from_center_size(egui::pos2(pill.left() + 26.0, cy), Vec2::splat(22.0)),
        Icon::Search,
        if focused || hovered {
            pal.text
        } else {
            pal.weak
        },
    );
    // Browse button at the right end of the pill (opens "Browse all").
    let browse = Rect::from_center_size(egui::pos2(pill.right() - 28.0, cy), Vec2::splat(32.0));
    ui.painter().vline(
        browse.left() - 6.0,
        (cy - 12.0)..=(cy + 12.0),
        egui::Stroke::new(1.0, widgets::with_alpha(pal.text, 0.3)),
    );
    let br = ui.interact(browse, ui.id().with("browse"), Sense::click());
    widgets::draw(
        ui,
        Rect::from_center_size(browse.center(), Vec2::splat(22.0)),
        Icon::Browse,
        if br.hovered() || (app.view == View::Search && app.search.query.is_empty()) {
            pal.text
        } else {
            pal.weak
        },
    );
    if br.on_hover_text("Browse").clicked() {
        app.search.query.clear();
        app.search.results = None;
        app.search.sent.clear();
        app.navigate(View::Search);
    }
    let edit_rect = Rect::from_min_max(
        egui::pos2(pill.left() + 48.0, pill.top() + 6.0),
        egui::pos2(
            browse.left()
                - (if app.search.query.is_empty() {
                    12.0
                } else {
                    40.0
                }),
            pill.bottom() - 6.0,
        ),
    );
    let edit = ui.put(
        edit_rect,
        egui::TextEdit::singleline(&mut app.search.query)
            .id(id)
            .frame(egui::Frame::NONE)
            .hint_text(egui::RichText::new("What do you want to play?").color(pal.weak))
            .text_color(pal.text)
            .font(widgets::font(16.0, Weight::Regular))
            .vertical_align(egui::Align::Center)
            .desired_width(edit_rect.width()),
    );
    if edit.changed() {
        app.search.edited_at = Some(std::time::Instant::now());
        if app.view != View::Search {
            app.navigate(View::Search);
        }
    }
    if edit.gained_focus() && app.view != View::Search {
        app.navigate(View::Search);
    }
    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        app.search.edited_at = None;
        app.run_search();
    }
    if !app.search.query.is_empty() {
        let x = Rect::from_center_size(egui::pos2(browse.left() - 26.0, cy), Vec2::splat(28.0));
        let r = ui.interact(x, ui.id().with("clear-search"), Sense::click());
        widgets::draw(
            ui,
            Rect::from_center_size(x.center(), Vec2::splat(16.0)),
            Icon::Close,
            if r.hovered() { pal.text } else { pal.weak },
        );
        if r.on_hover_text("Clear search").clicked() {
            app.search.query.clear();
            app.search.sent.clear();
            app.search.results = None;
        }
    }
}

/// Invisible resize handles along the window edges (custom title bar only).
pub fn resize_handles(ctx: &egui::Context) {
    use egui::viewport::ResizeDirection as D;
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
    if maximized || fullscreen {
        return;
    }
    let screen = ctx.content_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let m = 5.0;
    let (l, r) = (pos.x - screen.left() < m, screen.right() - pos.x < m);
    let (t, b) = (pos.y - screen.top() < m, screen.bottom() - pos.y < m);
    let dir = match (l, r, t, b) {
        (true, _, true, _) => Some(D::NorthWest),
        (_, true, true, _) => Some(D::NorthEast),
        (true, _, _, true) => Some(D::SouthWest),
        (_, true, _, true) => Some(D::SouthEast),
        (true, _, _, _) => Some(D::West),
        (_, true, _, _) => Some(D::East),
        (_, _, true, _) => Some(D::North),
        (_, _, _, true) => Some(D::South),
        _ => None,
    };
    let Some(dir) = dir else { return };
    let icon = match dir {
        D::North | D::South => egui::CursorIcon::ResizeVertical,
        D::East | D::West => egui::CursorIcon::ResizeHorizontal,
        D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
    };
    ctx.set_cursor_icon(icon);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

/// Initials for the avatar fallback ("Zayd Zaari" → "Z").
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into())
}

fn profile(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let pal = app.palette;
    let id = ui.id().with("profile");
    let r = ui.interact(rect, id, Sense::click());
    let outer = if r.hovered() {
        widgets::mix(pal.panel, pal.text, 0.1)
    } else {
        pal.panel
    };
    ui.painter()
        .circle_filled(rect.center(), rect.width() / 2.0, outer);
    let inner = Rect::from_center_size(rect.center(), Vec2::splat(32.0));
    let tex = app
        .avatar
        .clone()
        .and_then(|u| app.art.get(&u, super::art::THUMB_PX));
    match tex {
        Some(t) => widgets::image(ui.painter(), inner, Some(t), 16.0, pal.hover, pal.weak),
        None => {
            ui.painter()
                .circle_filled(inner.center(), 16.0, Color32::from_rgb(0xf5, 0x73, 0x3c));
            let name = if app.username.is_empty() {
                "?"
            } else {
                &app.username
            };
            widgets::text(
                ui,
                inner.center(),
                Align2::CENTER_CENTER,
                &initial(name),
                15.0,
                Weight::Bold,
                Color32::BLACK,
                30.0,
            );
        }
    }
    if app.private_session {
        // Spotify shows a blue lock badge on the avatar during a private session.
        let c = rect.right_top() + Vec2::new(-5.0, 6.0);
        ui.painter()
            .circle_filled(c, 8.0, Color32::from_rgb(0x50, 0x9b, 0xf5));
        widgets::draw(
            ui,
            Rect::from_center_size(c, Vec2::splat(10.0)),
            Icon::Lock,
            Color32::BLACK,
        );
    }
    let r = r.on_hover_text(if app.private_session {
        "Private session"
    } else if app.username.is_empty() {
        "Profile"
    } else {
        &app.username
    });
    egui::Popup::menu(&r)
        .id(id.with("menu"))
        .align(egui::RectAlign::BOTTOM_END)
        .gap(6.0)
        .show(|ui| {
            super::widgets::menu_style(ui, 240.0);
            if app.backend.is_some() {
                if widgets::menu_item(ui, "Account", true, &pal).clicked() {
                    let _ = open::that_detached("https://www.spotify.com/account/overview/");
                    ui.close();
                }
                if app.user_id.is_some() && widgets::menu_item(ui, "Profile", false, &pal).clicked()
                {
                    app.open_my_profile();
                    ui.close();
                }
                if widgets::menu_item(ui, "Listening stats", false, &pal).clicked() {
                    app.navigate(View::Stats);
                    ui.close();
                }
                let label = if app.private_session {
                    "Private session  ✓"
                } else {
                    "Private session"
                };
                if widgets::menu_item(ui, label, false, &pal).clicked() {
                    let on = !app.private_session;
                    app.set_private_session(on);
                    ui.close();
                }
                if widgets::menu_item(ui, "Support", true, &pal).clicked() {
                    let _ = open::that_detached("https://support.spotify.com/");
                    ui.close();
                }
                ui.separator();
            }
            if widgets::menu_item(ui, "Themes", false, &pal).clicked() {
                app.navigate(View::Themes);
                ui.close();
            }
            if widgets::menu_item(ui, "Settings", false, &pal).clicked() {
                app.navigate(View::Settings);
                ui.close();
            }
            let commands = app.plugins.commands();
            if !commands.is_empty() {
                ui.menu_button("Plugins", |ui| {
                    for (p, c, label) in commands {
                        if ui.button(label).clicked() {
                            app.run_plugin_command(p, c);
                            ui.close();
                        }
                    }
                });
            }
            if widgets::menu_item(ui, "Discord status", false, &pal).clicked() {
                app.navigate(View::Settings);
                app.settings_scroll_to_discord = true;
                ui.close();
            }
            ui.separator();
            if app.backend.is_some() {
                if widgets::menu_item(ui, "Log out", false, &pal).clicked() {
                    app.logout();
                    ui.close();
                }
            } else if widgets::menu_item(ui, "Log in to Spotify", false, &pal).clicked() {
                app.screen = Screen::Login;
                ui.close();
            }
        });
}

pub const HEIGHT: f32 = H;

#[cfg(test)]
mod tests {
    #[test]
    fn initials() {
        assert_eq!(super::initial("zayd Zaari"), "Z");
        assert_eq!(super::initial("  ébène"), "É");
        assert_eq!(super::initial(""), "?");
    }
}
