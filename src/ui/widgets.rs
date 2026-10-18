//! Design kit: bundled fonts, vector icons, truncated text, gradients, artwork
//! and the round buttons used across the Spotify-style shell.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, FontFamily, FontId, Pos2, Rect, Response, Sense, Stroke, Vec2};

// ------------------------------------------------------------------ fonts

/// Font weights available through [`font`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Semibold,
    Bold,
    Black,
}

pub fn font(size: f32, w: Weight) -> FontId {
    let family = match w {
        Weight::Regular => FontFamily::Proportional,
        Weight::Semibold => FontFamily::Name("semibold".into()),
        Weight::Bold => FontFamily::Name("bold".into()),
        Weight::Black => FontFamily::Name("black".into()),
    };
    FontId::new(size, family)
}

/// Install Figtree (Latin) with Noto Kufi Arabic as fallback, keeping egui's
/// emoji/symbol fonts at the end of every family. Both fonts are SIL OFL 1.1.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let data: [(&str, &'static [u8]); 5] = [
        (
            "figtree",
            include_bytes!("../../assets/fonts/Figtree-Regular.ttf"),
        ),
        (
            "figtree-semibold",
            include_bytes!("../../assets/fonts/Figtree-SemiBold.ttf"),
        ),
        (
            "figtree-bold",
            include_bytes!("../../assets/fonts/Figtree-Bold.ttf"),
        ),
        (
            "figtree-black",
            include_bytes!("../../assets/fonts/Figtree-ExtraBold.ttf"),
        ),
        (
            "kufi",
            include_bytes!("../../assets/fonts/NotoKufiArabic.ttf"),
        ),
    ];
    for (name, bytes) in data {
        fonts.font_data.insert(
            name.to_owned(),
            Arc::new(egui::FontData::from_static(bytes)),
        );
    }
    let fallbacks: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let family = |primary: &str| {
        let mut v = vec![primary.to_owned(), "kufi".to_owned()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts
        .families
        .insert(FontFamily::Proportional, family("figtree"));
    for (fam, file) in [
        ("semibold", "figtree-semibold"),
        ("bold", "figtree-bold"),
        ("black", "figtree-black"),
    ] {
        fonts
            .families
            .insert(FontFamily::Name(fam.into()), family(file));
    }
    ctx.set_fonts(fonts);
}

// ------------------------------------------------------------------ text

/// Lay out single-line text truncated with "…" to `max_w`.
pub fn galley(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    w: Weight,
    color: Color32,
    max_w: f32,
) -> Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(
        super::bidi::visual_line(text).into_owned(),
        TextFormat {
            font_id: font(size, w),
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    ui.painter().layout_job(job)
}

/// Paint truncated text anchored at `pos`. Returns the painted rect.
#[allow(clippy::too_many_arguments)]
pub fn text(
    ui: &egui::Ui,
    pos: Pos2,
    anchor: Align2,
    s: &str,
    size: f32,
    w: Weight,
    color: Color32,
    max_w: f32,
) -> Rect {
    let g = galley(ui, s, size, w, color, max_w);
    let rect = anchor.anchor_size(pos, g.size());
    ui.painter().galley(rect.min, g, color);
    rect
}

/// Multi-line wrapped text (used for lyrics and descriptions).
pub fn wrapped(
    ui: &egui::Ui,
    s: &str,
    size: f32,
    w: Weight,
    color: Color32,
    max_w: f32,
) -> Arc<egui::Galley> {
    // Right-to-left text: break lines first, then reorder each line.
    let text = if super::bidi::has_rtl(s) {
        super::bidi::wrap_visual(s, max_w, |t| {
            ui.painter()
                .layout_no_wrap(t.to_owned(), font(size, w), color)
                .size()
                .x
        })
    } else {
        s.to_owned()
    };
    let mut job = LayoutJob::single_section(
        text,
        TextFormat {
            font_id: font(size, w),
            color,
            line_height: Some(size * 1.3),
            ..Default::default()
        },
    );
    job.wrap = TextWrapping::wrap_at_width(max_w.max(1.0));
    ui.painter().layout_job(job)
}

// ------------------------------------------------------------------ colour

pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Linear blend between two colours (t = 0 → a, 1 → b).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_srgba_unmultiplied();
    let [br, bg, bb, ba] = b.to_srgba_unmultiplied();
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(ar, br), l(ag, bg), l(ab, bb), l(aa, ba))
}

/// Vertical gradient inside `rect` with the top corners rounded by `radius`.
pub fn gradient(painter: &egui::Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    if rect.height() <= 0.0 || rect.width() <= 0.0 {
        return;
    }
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    // Outline: rounded top corners (arcs), square bottom corners.
    let mut pts: Vec<Pos2> = Vec::with_capacity(24);
    let steps = 6;
    for i in 0..=steps {
        let a = std::f32::consts::PI + (i as f32 / steps as f32) * std::f32::consts::FRAC_PI_2;
        pts.push(Pos2::new(
            rect.left() + r + r * a.cos(),
            rect.top() + r + r * a.sin(),
        ));
    }
    for i in 0..=steps {
        let a =
            -std::f32::consts::FRAC_PI_2 + (i as f32 / steps as f32) * std::f32::consts::FRAC_PI_2;
        pts.push(Pos2::new(
            rect.right() - r + r * a.cos(),
            rect.top() + r + r * a.sin(),
        ));
    }
    pts.push(rect.right_bottom());
    pts.push(rect.left_bottom());
    let color_at = |y: f32| mix(top, bottom, (y - rect.top()) / rect.height());
    let mut mesh = egui::Mesh::default();
    let c = rect.center();
    mesh.colored_vertex(c, color_at(c.y));
    for p in &pts {
        mesh.colored_vertex(*p, color_at(p.y));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(egui::Shape::mesh(mesh));
}

// ------------------------------------------------------------------ artwork

/// Paint a texture (or a placeholder) into `rect` with rounded corners.
pub fn image(
    painter: &egui::Painter,
    rect: Rect,
    tex: Option<egui::TextureId>,
    radius: f32,
    placeholder: Color32,
    icon_color: Color32,
) {
    match tex {
        Some(t) => {
            let a = super::art::fade_alpha(painter.ctx(), t);
            if a < 1.0 {
                painter.rect_filled(rect, radius, placeholder);
            }
            painter.add(
                egui::epaint::RectShape::filled(rect, radius, Color32::WHITE.gamma_multiply(a))
                    .with_texture(t, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))),
            );
        }
        None => {
            painter.rect_filled(rect, radius, placeholder);
            let s = (rect.width().min(rect.height()) * 0.4).max(12.0);
            draw_in(
                painter,
                Rect::from_center_size(rect.center(), Vec2::splat(s)),
                Icon::Note,
                icon_color,
            );
        }
    }
}

pub fn shadow(painter: &egui::Painter, rect: Rect, radius: f32) {
    let s = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    painter.add(s.as_shape(rect, radius));
}

// ------------------------------------------------------------------ icons

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Home,
    HomeFilled,
    Search,
    Library,
    Local,
    Heart,
    Back,
    Forward,
    Album,
    Playing,
    Play,
    Pause,
    Next,
    Prev,
    Shuffle,
    Repeat,
    RepeatOne,
    Mic,
    Queue,
    Devices,
    Volume(u8),
    Plus,
    /// Filled circle with a black tick ("in Liked Songs").
    Saved,
    Clock,
    More,
    Close,
    Note,
    User,
    Browse,
    Refresh,
    Palette,
    WinMinimize,
    WinMaximize,
    WinRestore,
    /// Phone / tablet (Connect devices).
    Phone,
    /// Pinned item marker.
    Pin,
    /// Opens outside the app (box with an arrow).
    External,
    /// Enter full screen (four outward corners).
    Expand,
    /// Leave full screen (four inward corners).
    Collapse,
    /// Padlock (private session).
    Lock,
    /// Two people (Friend Activity).
    Friends,
    /// Mini player (window with a small window in the corner).
    MiniPlayer,
}

/// Draw an icon designed on a 24×24 grid into `rect`.
pub fn draw(ui: &egui::Ui, rect: Rect, icon: Icon, color: Color32) {
    draw_in(ui.painter(), rect, icon, color);
}

pub fn draw_in(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 24.0;
    let o = rect.center() - Vec2::splat(12.0 * s);
    let pt = |x: f32, y: f32| o + Vec2::new(x * s, y * s);
    let w = (1.9 * s).max(1.2);
    let stroke = Stroke::new(w, color);
    let line = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            pts.iter().map(|&(x, y)| pt(x, y)).collect(),
            stroke,
        ));
    };
    let fill = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::convex_polygon(
            pts.iter().map(|&(x, y)| pt(x, y)).collect(),
            color,
            Stroke::NONE,
        ));
    };
    let circle = |x: f32, y: f32, r: f32| p.circle_stroke(pt(x, y), r * s, stroke);
    let dot = |x: f32, y: f32, r: f32| p.circle_filled(pt(x, y), r * s, color);
    match icon {
        Icon::Home => {
            line(&[(3., 11.), (12., 3.5), (21., 11.)]);
            line(&[(5.5, 9.5), (5.5, 20.5), (18.5, 20.5), (18.5, 9.5)]);
            line(&[(10., 20.5), (10., 14.5), (14., 14.5), (14., 20.5)]);
        }
        Icon::HomeFilled => {
            fill(&[
                (12., 3.),
                (20.5, 10.5),
                (20.5, 21.),
                (3.5, 21.),
                (3.5, 10.5),
            ]);
        }
        Icon::Search => {
            circle(10.5, 10.5, 6.5);
            line(&[(15.5, 15.5), (20.5, 20.5)]);
        }
        Icon::Library => {
            line(&[(4., 3.5), (4., 20.5)]);
            line(&[(9., 3.5), (9., 20.5)]);
            line(&[(13.5, 4.5), (19.5, 20.)]);
        }
        Icon::Local => {
            line(&[
                (3., 6.),
                (9., 6.),
                (11., 8.5),
                (21., 8.5),
                (21., 19.5),
                (3., 19.5),
                (3., 6.),
            ]);
        }
        Icon::Heart => {
            // One closed outline (classic parametric heart) starting at the
            // bottom tip. egui fills polygons as a fan from the first vertex
            // and the heart is star-shaped around its tip, so this renders
            // as a single anti-aliased shape — no seams with translucent
            // colours (overlapping circles showed lines).
            let n = 48;
            let pts: Vec<Pos2> = (0..n)
                .map(|i| {
                    let t = std::f32::consts::PI + i as f32 / n as f32 * std::f32::consts::TAU;
                    let x = 16.0 * t.sin().powi(3);
                    let y = 13.0 * t.cos()
                        - 5.0 * (2.0 * t).cos()
                        - 2.0 * (3.0 * t).cos()
                        - (4.0 * t).cos();
                    pt(12.0 + x * 0.55, 10.6 - y * 0.55)
                })
                .collect();
            p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Back => line(&[(15., 4.5), (7.5, 12.), (15., 19.5)]),
        Icon::Forward => line(&[(9., 4.5), (16.5, 12.), (9., 19.5)]),
        Icon::Album => {
            circle(12., 12., 8.5);
            circle(12., 12., 2.5);
        }
        Icon::Playing => {
            p.rect_stroke(
                Rect::from_min_max(pt(3.5, 4.5), pt(20.5, 19.5)),
                2.0 * s,
                stroke,
                egui::StrokeKind::Inside,
            );
            fill(&[(13., 7.5), (17.5, 7.5), (17.5, 16.5), (13., 16.5)]);
        }
        Icon::Play => fill(&[(7., 4.), (20., 12.), (7., 20.)]),
        Icon::Pause => {
            fill(&[(6., 4.), (10., 4.), (10., 20.), (6., 20.)]);
            fill(&[(14., 4.), (18., 4.), (18., 20.), (14., 20.)]);
        }
        Icon::Next => {
            fill(&[(5., 4.5), (16., 12.), (5., 19.5)]);
            fill(&[(16.5, 4.5), (19., 4.5), (19., 19.5), (16.5, 19.5)]);
        }
        Icon::Prev => {
            fill(&[(19., 4.5), (8., 12.), (19., 19.5)]);
            fill(&[(5., 4.5), (7.5, 4.5), (7.5, 19.5), (5., 19.5)]);
        }
        Icon::Shuffle => {
            line(&[(3., 7.), (7., 7.), (15., 17.), (20., 17.)]);
            line(&[(3., 17.), (7., 17.), (15., 7.), (20., 7.)]);
            line(&[(17.5, 4.5), (20.5, 7.), (17.5, 9.5)]);
            line(&[(17.5, 14.5), (20.5, 17.), (17.5, 19.5)]);
        }
        Icon::Repeat | Icon::RepeatOne => {
            line(&[(5., 13.), (5., 8.), (19., 8.)]);
            line(&[(16., 5.), (19., 8.), (16., 11.)]);
            line(&[(19., 11.), (19., 16.), (5., 16.)]);
            line(&[(8., 13.), (5., 16.), (8., 19.)]);
            if icon == Icon::RepeatOne {
                p.circle_filled(pt(12., 12.), 4.2 * s, color);
                let bg = Color32::BLACK;
                p.line_segment([pt(12., 10.), pt(12., 14.2)], Stroke::new(1.6 * s, bg));
            }
        }
        Icon::Mic => {
            p.rect_stroke(
                Rect::from_min_max(pt(9., 3.), pt(15., 14.)),
                3.0 * s,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[
                (6., 11.),
                (6., 12.),
                (8., 16.),
                (12., 17.),
                (16., 16.),
                (18., 12.),
                (18., 11.),
            ]);
            line(&[(12., 17.), (12., 21.)]);
        }
        Icon::Queue => {
            line(&[(3., 6.), (21., 6.)]);
            line(&[(3., 12.), (13., 12.)]);
            line(&[(3., 18.), (13., 18.)]);
            fill(&[(16., 10.), (21., 15.), (16., 20.)]);
        }
        Icon::Devices => {
            p.rect_stroke(
                Rect::from_min_max(pt(4., 3.5), pt(14., 20.5)),
                1.5 * s,
                stroke,
                egui::StrokeKind::Middle,
            );
            circle(9., 14.5, 2.2);
            line(&[(17., 6.), (20., 6.), (20., 20.5), (17., 20.5)]);
        }
        Icon::Volume(level) => {
            fill(&[
                (3., 9.),
                (7., 9.),
                (12., 4.5),
                (12., 19.5),
                (7., 15.),
                (3., 15.),
            ]);
            if level == 0 {
                line(&[(15., 9.), (21., 15.)]);
                line(&[(21., 9.), (15., 15.)]);
            } else {
                line(&[(15., 9.5), (16.2, 12.), (15., 14.5)]);
                if level > 1 {
                    line(&[(17.5, 7.), (19.5, 12.), (17.5, 17.)]);
                }
            }
        }
        Icon::Plus => {
            line(&[(12., 4.), (12., 20.)]);
            line(&[(4., 12.), (20., 12.)]);
        }
        Icon::Saved => {
            dot(12., 12., 10.);
            p.add(egui::Shape::line(
                vec![pt(7.2, 12.3), pt(10.6, 15.6), pt(16.8, 8.9)],
                Stroke::new(w * 1.1, Color32::BLACK),
            ));
        }
        Icon::Clock => {
            circle(12., 12., 8.5);
            line(&[(12., 7.), (12., 12.), (15.5, 14.)]);
        }
        Icon::More => {
            dot(5., 12., 1.8);
            dot(12., 12., 1.8);
            dot(19., 12., 1.8);
        }
        Icon::Close => {
            line(&[(6., 6.), (18., 18.)]);
            line(&[(18., 6.), (6., 18.)]);
        }
        Icon::Note => {
            line(&[(9., 18.), (9., 5.), (19., 3.), (19., 16.)]);
            p.circle_filled(pt(6.5, 18.), 2.8 * s, color);
            p.circle_filled(pt(16.5, 16.), 2.8 * s, color);
        }
        Icon::User => {
            circle(12., 8., 4.);
            line(&[
                (4.5, 20.5),
                (6., 16.),
                (12., 13.5),
                (18., 16.),
                (19.5, 20.5),
            ]);
        }
        Icon::Browse => {
            p.rect_stroke(
                Rect::from_min_max(pt(3.5, 5.), pt(20.5, 19.5)),
                2.0 * s,
                stroke,
                egui::StrokeKind::Middle,
            );
            line(&[
                (3.5, 13.5),
                (8.5, 13.5),
                (10., 16.),
                (14., 16.),
                (15.5, 13.5),
                (20.5, 13.5),
            ]);
        }
        Icon::Palette => {
            // Painter's palette: a blob with a thumb hole and paint dots.
            let pts: Vec<(f32, f32)> = (0..=40)
                .map(|i| {
                    let a = 0.9 + i as f32 / 40.0 * 5.6;
                    (12.0 + 8.6 * a.cos(), 12.0 + 8.6 * a.sin())
                })
                .collect();
            let mut path = pts.clone();
            path.push((15.0, 15.5));
            path.push((16.5, 18.2));
            path.push(pts[0]);
            line(&path);
            dot(8.2, 10.6, 1.4);
            dot(11.3, 7.4, 1.4);
            dot(15.3, 8.2, 1.4);
            dot(8.6, 14.8, 1.4);
        }
        Icon::Refresh => {
            let pts: Vec<(f32, f32)> = (0..=24)
                .map(|i| {
                    let a = -0.6 + i as f32 / 24.0 * 5.2;
                    (12.0 + 7.5 * a.cos(), 12.0 + 7.5 * a.sin())
                })
                .collect();
            line(&pts);
            let end = pts[pts.len() - 1];
            line(&[(end.0 - 3.5, end.1 - 1.0), end, (end.0 + 0.5, end.1 + 3.5)]);
        }
        Icon::WinMinimize => line(&[(7., 12.), (17., 12.)]),
        Icon::WinMaximize => {
            p.rect_stroke(
                Rect::from_min_max(pt(7., 7.), pt(17., 17.)),
                0.0,
                Stroke::new(1.2 * s.max(0.8), color),
                egui::StrokeKind::Middle,
            );
        }
        Icon::Phone => {
            p.rect_stroke(
                Rect::from_min_max(pt(7.0, 2.5), pt(17.0, 21.5)),
                2.0 * s,
                stroke,
                egui::StrokeKind::Middle,
            );
            dot(12.0, 18.0, 1.1);
        }
        Icon::Pin => {
            fill(&[(13.5, 2.5), (21.5, 10.5), (17.5, 12.0), (12.0, 6.5)]);
            fill(&[(12.0, 6.5), (17.5, 12.0), (12.5, 17.0), (7.0, 11.5)]);
            line(&[(9.5, 14.5), (3.5, 20.5)]);
        }
        Icon::External => {
            line(&[(10., 5.), (5., 5.), (5., 19.), (19., 19.), (19., 14.)]);
            line(&[(12., 12.), (19., 5.)]);
            line(&[(14., 5.), (19., 5.), (19., 10.)]);
        }
        Icon::Expand => {
            line(&[(4., 9.), (4., 4.), (9., 4.)]);
            line(&[(15., 4.), (20., 4.), (20., 9.)]);
            line(&[(20., 15.), (20., 20.), (15., 20.)]);
            line(&[(9., 20.), (4., 20.), (4., 15.)]);
        }
        Icon::Friends => {
            circle(9., 8., 3.5);
            line(&[(2.5, 20.), (4., 15.5), (9., 13.5), (14., 15.5), (15.5, 20.)]);
            circle(17., 7., 2.8);
            line(&[(16.5, 12.5), (19.5, 13.5), (21.5, 18.)]);
        }
        Icon::Lock => {
            fill(&[(5., 11.), (19., 11.), (19., 21.), (5., 21.)]);
            line(&[
                (8., 11.),
                (8., 7.5),
                (9.5, 4.5),
                (12., 3.5),
                (14.5, 4.5),
                (16., 7.5),
                (16., 11.),
            ]);
        }
        Icon::MiniPlayer => {
            line(&[(3., 5.), (21., 5.), (21., 19.), (3., 19.), (3., 5.)]);
            fill(&[(12., 12.), (19., 12.), (19., 17.), (12., 17.)]);
        }
        Icon::Collapse => {
            line(&[(4., 9.), (9., 9.), (9., 4.)]);
            line(&[(15., 4.), (15., 9.), (20., 9.)]);
            line(&[(20., 15.), (15., 15.), (15., 20.)]);
            line(&[(9., 20.), (9., 15.), (4., 15.)]);
        }
        Icon::WinRestore => {
            let st = Stroke::new(1.2 * s.max(0.8), color);
            p.rect_stroke(
                Rect::from_min_max(pt(7., 9.), pt(15., 17.)),
                0.0,
                st,
                egui::StrokeKind::Middle,
            );
            p.line_segment([pt(9., 9.), pt(9., 7.)], st);
            p.line_segment([pt(9., 7.), pt(17., 7.)], st);
            p.line_segment([pt(17., 7.), pt(17., 15.)], st);
            p.line_segment([pt(17., 15.), pt(15., 15.)], st);
        }
    }
}

// ------------------------------------------------------------------ buttons

/// Borderless icon button that brightens on hover (Spotify control style).
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    tip: &str,
    color: Color32,
    hover: Color32,
) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size + 12.0), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tip));
    let c = if r.hovered() { hover } else { color };
    let scale = if r.is_pointer_button_down_on() {
        0.94
    } else {
        1.0
    };
    draw(
        ui,
        Rect::from_center_size(rect.center(), Vec2::splat(size * scale)),
        icon,
        c,
    );
    r.on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Toggle icon (shuffle/repeat): accent colour plus a small dot when active.
#[allow(clippy::too_many_arguments)]
pub fn toggle_icon(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    tip: &str,
    active: bool,
    accent: Color32,
    weak: Color32,
    text: Color32,
) -> Response {
    let r = icon_button(
        ui,
        icon,
        size,
        tip,
        if active { accent } else { weak },
        if active { accent } else { text },
    );
    if active {
        ui.painter()
            .circle_filled(r.rect.center_bottom() - Vec2::new(0.0, 2.0), 2.0, accent);
    }
    r
}

/// Filled circle with a play/pause glyph (green "big play" and white bar button).
pub fn round_play(
    ui: &mut egui::Ui,
    diameter: f32,
    playing: bool,
    fill: Color32,
    glyph: Color32,
    tip: &str,
) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    paint_round_play(ui, rect, r.hovered(), playing, fill, glyph);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tip));
    r.on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn paint_round_play(
    ui: &egui::Ui,
    rect: Rect,
    hovered: bool,
    playing: bool,
    fill: Color32,
    glyph: Color32,
) {
    let grow = if hovered { 1.04 } else { 1.0 };
    let d = rect.width() * grow;
    let c = rect.center();
    let fill = if hovered {
        mix(fill, Color32::WHITE, 0.12)
    } else {
        fill
    };
    ui.painter().circle_filled(c, d / 2.0, fill);
    let g = d * 0.42;
    let icon_rect = Rect::from_center_size(
        c + if playing {
            Vec2::ZERO
        } else {
            Vec2::new(d * 0.03, 0.0)
        },
        Vec2::splat(g),
    );
    draw(
        ui,
        icon_rect,
        if playing { Icon::Pause } else { Icon::Play },
        glyph,
    );
}

/// Pill-shaped chip (filters like "Playlists", "Albums").
pub fn chip(ui: &mut egui::Ui, label: &str, active: bool, pal: &super::theme::Palette) -> Response {
    let g = galley(
        ui,
        label,
        13.5,
        Weight::Semibold,
        if active {
            super::theme::contrast_on(pal.text)
        } else {
            pal.text
        },
        400.0,
    );
    let size = Vec2::new(g.size().x + 26.0, 32.0);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let fill = if active {
        pal.text
    } else if r.hovered() {
        with_alpha(pal.text, 0.16)
    } else {
        with_alpha(pal.text, 0.08)
    };
    ui.painter().rect_filled(rect, 16.0, fill);
    ui.painter().galley(
        rect.center() - g.size() / 2.0,
        g,
        if active {
            super::theme::contrast_on(pal.text)
        } else {
            pal.text
        },
    );
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Spotify-style switch. Full-width row (label left, switch right) by default;
/// `.inline()` puts the switch before the label for use inside horizontal rows.
pub struct Switch<'a> {
    on: &'a mut bool,
    label: egui::WidgetText,
    inline: bool,
}

impl<'a> Switch<'a> {
    pub fn new(on: &'a mut bool, label: impl Into<egui::WidgetText>) -> Self {
        Self {
            on,
            label: label.into(),
            inline: false,
        }
    }

    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }
}

impl egui::Widget for Switch<'_> {
    fn ui(self, ui: &mut egui::Ui) -> Response {
        let sw = Vec2::new(36.0, 20.0);
        let galley = self.label.into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            ui.available_width() - sw.x - 16.0,
            egui::TextStyle::Body,
        );
        let height = galley.size().y.max(sw.y) + 8.0;
        let width = if self.inline {
            sw.x + 10.0 + galley.size().x
        } else {
            ui.available_width()
        };
        let (rect, mut r) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        if r.clicked() {
            *self.on = !*self.on;
            r.mark_changed();
        }
        let enabled = ui.is_enabled();
        r.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, *self.on, galley.text())
        });
        let (switch_rect, text_pos) = if self.inline {
            let s = Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - sw.y / 2.0), sw);
            (
                s,
                egui::pos2(s.right() + 10.0, rect.center().y - galley.size().y / 2.0),
            )
        } else {
            let s = Rect::from_min_size(
                egui::pos2(rect.right() - sw.x, rect.center().y - sw.y / 2.0),
                sw,
            );
            (
                s,
                egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
            )
        };
        let t = ui.ctx().animate_bool_with_time(r.id, *self.on, 0.12);
        // The theme's accent (hyperlink colour) and a neutral grey for "off".
        let accent = ui.visuals().hyperlink_color;
        let off = ui.visuals().widgets.inactive.weak_bg_fill;
        let track = mix(mix(off, Color32::from_gray(0x72), 0.6), accent, t);
        let track = if enabled {
            track
        } else {
            with_alpha(track, 0.4)
        };
        ui.painter().rect_filled(switch_rect, 10.0, track);
        let knob_x = egui::lerp(
            (switch_rect.left() + 10.0)..=(switch_rect.right() - 10.0),
            t,
        );
        let knob_r = if r.hovered() { 7.5 } else { 7.0 };
        ui.painter().circle_filled(
            egui::pos2(knob_x, switch_rect.center().y),
            knob_r,
            if t > 0.5 {
                Color32::BLACK
            } else {
                Color32::WHITE
            },
        );
        let text_color = if enabled {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().galley(text_pos, galley, text_color);
        r.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

#[allow(clippy::too_many_arguments)]
/// Spotify's "add to Liked Songs" button: an outlined "+" circle, or a filled
/// accent circle with a tick once the track is saved. `d` is the circle diameter.
pub fn like_button(
    ui: &mut egui::Ui,
    center: Pos2,
    d: f32,
    id: egui::Id,
    saved: bool,
    accent: Color32,
    weak: Color32,
    strong: Color32,
) -> egui::Response {
    let rect = Rect::from_center_size(center, Vec2::splat(d + 8.0));
    let r = ui.interact(rect, id, egui::Sense::click());
    let hovered = r.hovered();
    // Small pop when the state flips.
    let t = ui.ctx().animate_bool_with_time(id.with("pop"), saved, 0.18);
    let pop = 1.0 + 0.25 * (t * std::f32::consts::PI).sin();
    if saved {
        let c = if hovered {
            mix(accent, Color32::WHITE, 0.15)
        } else {
            accent
        };
        draw(
            ui,
            Rect::from_center_size(center, Vec2::splat(d * 1.1 * pop)),
            Icon::Saved,
            c,
        );
    } else {
        let c = if hovered { strong } else { weak };
        ui.painter().circle_stroke(
            center,
            d / 2.0 * pop,
            Stroke::new((d / 11.0).clamp(1.3, 1.8), c),
        );
        draw(
            ui,
            Rect::from_center_size(center, Vec2::splat(d * 0.6)),
            Icon::Plus,
            c,
        );
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if saved {
            "Remove from Liked Songs"
        } else {
            "Save to your Liked Songs"
        })
}

/// Centred empty-state block (icon bubble, heading, explanation, optional pill
/// button) painted at `top_center`. Returns true when the button is clicked.
#[allow(clippy::too_many_arguments)]
pub fn empty_state(
    ui: &mut egui::Ui,
    top_center: Pos2,
    max_w: f32,
    icon: Icon,
    title: &str,
    body: &str,
    button: Option<&str>,
    pal: &super::theme::Palette,
) -> bool {
    let p = ui.painter().clone();
    let bubble = top_center + Vec2::new(0.0, 36.0);
    p.circle_filled(bubble, 36.0, pal.hover);
    draw_in(
        &p,
        Rect::from_center_size(bubble, Vec2::splat(30.0)),
        icon,
        pal.text,
    );
    text(
        ui,
        top_center + Vec2::new(0.0, 96.0),
        egui::Align2::CENTER_TOP,
        title,
        22.0,
        Weight::Bold,
        pal.text,
        max_w,
    );
    text(
        ui,
        top_center + Vec2::new(0.0, 130.0),
        egui::Align2::CENTER_TOP,
        body,
        14.0,
        Weight::Regular,
        pal.weak,
        max_w,
    );
    let Some(label) = button else { return false };
    let g = galley(ui, label, 14.0, Weight::Bold, Color32::BLACK, f32::INFINITY);
    let b = Rect::from_center_size(
        top_center + Vec2::new(0.0, 182.0),
        Vec2::new(g.size().x + 40.0, 40.0),
    );
    let r = ui.interact(b, ui.id().with(("empty-btn", label)), egui::Sense::click());
    let grow = if r.hovered() { 1.5 } else { 0.0 };
    p.rect_filled(b.expand(grow), 20.0, pal.text);
    p.galley(b.center() - g.size() / 2.0, g, Color32::BLACK);
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Roomier menu items, like Spotify's context menus.
pub fn menu_style(ui: &mut egui::Ui, min_width: f32) {
    ui.set_min_width(min_width);
    ui.spacing_mut().button_padding = Vec2::new(12.0, 8.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.style_mut().override_font_id = Some(font(14.0, Weight::Regular));
}

/// Draw a texture of size `tex_size` filling `rect` without distortion
/// (centre-cropped horizontally, top-aligned vertically, like CSS
/// `object-fit: cover; object-position: top`).
pub fn image_cover(
    painter: &egui::Painter,
    rect: Rect,
    tex: Option<(egui::TextureId, Vec2)>,
    radius: f32,
    placeholder: Color32,
) {
    let Some((t, size)) = tex.filter(|(_, s)| s.x > 0.0 && s.y > 0.0) else {
        painter.rect_filled(rect, radius, placeholder);
        return;
    };
    let (ta, ra) = (size.x / size.y, rect.width() / rect.height().max(1.0));
    let uv = if ta > ra {
        let w = ra / ta;
        Rect::from_min_max(Pos2::new(0.5 - w / 2.0, 0.0), Pos2::new(0.5 + w / 2.0, 1.0))
    } else {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, ta / ra))
    };
    let a = super::art::fade_alpha(painter.ctx(), t);
    if a < 1.0 {
        painter.rect_filled(rect, radius, placeholder);
    }
    painter.add(
        egui::epaint::RectShape::filled(rect, radius, Color32::WHITE.gamma_multiply(a))
            .with_texture(t, uv),
    );
}

/// Pill with a scheme's colour swatches and its name. `selected` draws an
/// accent ring.
pub fn scheme_chip(
    ui: &mut egui::Ui,
    id: egui::Id,
    name: &str,
    swatches: &[[u8; 3]],
    selected: bool,
    pal: &super::theme::Palette,
) -> egui::Response {
    let g = galley(ui, name, 13.0, Weight::Semibold, pal.text, 220.0);
    let dots = swatches.len().min(5) as f32;
    let w = 14.0 + dots * 12.0 + 6.0 + g.size().x + 14.0;
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, 34.0), egui::Sense::click());
    let hov = ui.ctx().animate_bool_with_time(id, r.hovered(), 0.12);
    let fill = mix(pal.panel, pal.hover, hov);
    let p = ui.painter();
    p.rect_filled(rect, 17.0, fill);
    if selected {
        p.rect_stroke(
            rect,
            17.0,
            Stroke::new(2.0, pal.accent),
            egui::StrokeKind::Inside,
        );
    }
    let mut x = rect.left() + 14.0 + 6.0;
    for c in swatches.iter().take(5) {
        let center = Pos2::new(x, rect.center().y);
        p.circle_filled(center, 8.0, Color32::from_rgb(c[0], c[1], c[2]));
        p.circle_stroke(center, 8.0, Stroke::new(1.5, fill));
        p.circle_stroke(center, 6.8, Stroke::new(0.8, with_alpha(pal.text, 0.18)));
        x += 12.0;
    }
    p.galley(
        Pos2::new(x + 6.0, rect.center().y - g.size().y / 2.0),
        g,
        pal.text,
    );
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Spotify's "now playing" equaliser: four bars bouncing at different rates.
/// Repaints at ~12 fps while visible; `animate = false` draws still bars.
pub fn eq_bars(ui: &egui::Ui, rect: Rect, color: Color32, animate: bool) {
    let t = if animate {
        ui.input(|i| i.time) as f32
    } else {
        0.0
    };
    let s = rect.width().min(rect.height()) / 24.0;
    let o = rect.center() - Vec2::splat(12.0 * s);
    let base = o.y + 20.0 * s;
    for (k, (x, speed, phase)) in [
        (5.0, 7.3, 0.0),
        (10.0, 9.1, 1.7),
        (15.0, 6.2, 3.1),
        (20.0, 8.4, 4.4),
    ]
    .iter()
    .enumerate()
    {
        let h = if animate {
            let v = ((t * speed + phase).sin() * 0.5 + 0.5)
                * ((t * speed * 0.37 + k as f32).sin() * 0.25 + 0.75);
            3.0 + 13.0 * v
        } else {
            [9.0, 15.0, 6.0, 11.0][k]
        };
        let bar = Rect::from_min_max(
            Pos2::new(o.x + (x - 1.4) * s, base - h * s),
            Pos2::new(o.x + (x + 1.4) * s, base),
        );
        ui.painter().rect_filled(bar, 1.0 * s, color);
    }
    if animate && ui.is_rect_visible(rect) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(80));
    }
}

/// Spotify-style menu row: label on the left, optional "opens outside" icon
/// on the right, full-width hover highlight.
pub fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    external: bool,
    pal: &super::theme::Palette,
) -> Response {
    // Menus size to their widest row; keep rows a fixed, Spotify-like width.
    let w = ui.min_rect().width().max(240.0);
    let (rect, r) = ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(rect, 3.0, pal.hover);
    }
    text(
        ui,
        Pos2::new(rect.left() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        14.0,
        Weight::Regular,
        pal.text,
        w - 48.0,
    );
    if external {
        draw(
            ui,
            Rect::from_center_size(
                Pos2::new(rect.right() - 18.0, rect.center().y),
                Vec2::splat(15.0),
            ),
            Icon::External,
            pal.text,
        );
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Shimmer alpha for skeleton placeholders (a soft pulse, like Spotify's loading state).
fn shimmer(ui: &egui::Ui) -> f32 {
    let t = ui.input(|i| i.time) as f32;
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));
    0.06 + 0.035 * (t * 2.6).sin().abs()
}

/// Placeholder song rows while a list loads: cover square and two text bars.
pub fn skeleton_rows(ui: &mut egui::Ui, n: usize, text: Color32) {
    let a = shimmer(ui);
    let w = ui.available_width();
    for i in 0..n {
        let (row, _) = ui.allocate_exact_size(Vec2::new(w, 56.0), egui::Sense::hover());
        let p = ui.painter();
        let c = with_alpha(text, a);
        p.rect_filled(
            Rect::from_center_size(
                egui::pos2(row.left() + 24.0, row.center().y),
                Vec2::new(12.0, 10.0),
            ),
            3.0,
            c,
        );
        let art = Rect::from_min_size(
            egui::pos2(row.left() + 48.0, row.center().y - 20.0),
            Vec2::splat(40.0),
        );
        p.rect_filled(art, 4.0, c);
        let tw = (w * 0.28) * (0.7 + 0.3 * ((i * 37 % 10) as f32 / 10.0));
        p.rect_filled(
            Rect::from_min_size(
                egui::pos2(art.right() + 12.0, art.top() + 6.0),
                Vec2::new(tw, 11.0),
            ),
            5.0,
            c,
        );
        p.rect_filled(
            Rect::from_min_size(
                egui::pos2(art.right() + 12.0, art.top() + 25.0),
                Vec2::new(tw * 0.6, 9.0),
            ),
            4.0,
            c,
        );
        p.rect_filled(
            Rect::from_min_size(
                egui::pos2(row.right() - 60.0, row.center().y - 5.0),
                Vec2::new(36.0, 10.0),
            ),
            4.0,
            c,
        );
    }
}

/// Placeholder card shelf while Home or a grid loads.
pub fn skeleton_cards(ui: &mut egui::Ui, rows: usize, text: Color32) {
    let a = shimmer(ui);
    let w = ui.available_width();
    let n = ((w + 8.0) / 176.0).floor().clamp(1.0, 8.0) as usize;
    let cw = (w - (n as f32 - 1.0) * 8.0) / n as f32;
    for _ in 0..rows {
        let (bar, _) = ui.allocate_exact_size(Vec2::new(w, 40.0), egui::Sense::hover());
        ui.painter().rect_filled(
            Rect::from_min_size(bar.min + Vec2::new(0.0, 10.0), Vec2::new(200.0, 18.0)),
            6.0,
            with_alpha(text, a),
        );
        let (row, _) = ui.allocate_exact_size(Vec2::new(w, cw + 58.0), egui::Sense::hover());
        for i in 0..n {
            let r = Rect::from_min_size(
                egui::pos2(row.left() + i as f32 * (cw + 8.0), row.top()),
                Vec2::new(cw, cw + 58.0),
            )
            .shrink(12.0);
            let p = ui.painter();
            let c = with_alpha(text, a);
            p.rect_filled(Rect::from_min_size(r.min, Vec2::splat(r.width())), 6.0, c);
            p.rect_filled(
                Rect::from_min_size(
                    egui::pos2(r.left(), r.top() + r.width() + 12.0),
                    Vec2::new(r.width() * 0.8, 11.0),
                ),
                5.0,
                c,
            );
            p.rect_filled(
                Rect::from_min_size(
                    egui::pos2(r.left(), r.top() + r.width() + 32.0),
                    Vec2::new(r.width() * 0.5, 9.0),
                ),
                4.0,
                c,
            );
        }
        ui.add_space(16.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_helpers() {
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 0.0), Color32::BLACK);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 1.0), Color32::WHITE);
        let half = with_alpha(Color32::WHITE, 0.5);
        assert_eq!(half.to_srgba_unmultiplied()[3], 127);
    }
}
