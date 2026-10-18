//! Dark (default) and light themes with a configurable accent colour, Spicetify
//! colour schemes (see `crate::themes`), and plugin overrides of single colours.
//! Palette changes are animated by lerping between palettes.

use egui::{Color32, CornerRadius, Stroke, Visuals};

use crate::config::{parse_hex_color, ThemeMode, UiConfig};
use crate::plugins::ThemeOverride;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    /// Window background behind the rounded panels.
    pub base: Color32,
    /// Panel background (library, main view, right panel).
    pub bg: Color32,
    pub panel: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub hover: Color32,
    /// Library (left) panel background.
    pub sidebar: Color32,
    /// Accent on hover / pressed.
    pub accent_hover: Color32,
    /// Icon/text colour on accent-filled buttons (black on Spotify green).
    pub on_accent: Color32,
    pub notification: Color32,
    pub notification_error: Color32,
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// Black or white, whichever reads better on `c`.
pub fn contrast_on(c: Color32) -> Color32 {
    if luminance(c) > 0.45 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

fn lerp_c(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

impl Palette {
    /// Blend between two palettes (`t` in 0..=1), for animated theme changes.
    pub fn lerp(&self, o: &Palette, t: f32) -> Palette {
        Palette {
            dark: if t < 0.5 { self.dark } else { o.dark },
            accent: lerp_c(self.accent, o.accent, t),
            base: lerp_c(self.base, o.base, t),
            bg: lerp_c(self.bg, o.bg, t),
            panel: lerp_c(self.panel, o.panel, t),
            text: lerp_c(self.text, o.text, t),
            weak: lerp_c(self.weak, o.weak, t),
            hover: lerp_c(self.hover, o.hover, t),
            sidebar: lerp_c(self.sidebar, o.sidebar, t),
            accent_hover: lerp_c(self.accent_hover, o.accent_hover, t),
            on_accent: lerp_c(self.on_accent, o.on_accent, t),
            notification: lerp_c(self.notification, o.notification, t),
            notification_error: lerp_c(self.notification_error, o.notification_error, t),
        }
    }
}

/// Map a Spicetify colour scheme onto Lumen's palette. Spotify's layout puts
/// the top bar, the gaps between panels and the player bar on one background,
/// which Spicetify calls `player`; `main` is the main view, `sidebar` the
/// library, `card` raised surfaces, `button`/`play-button` the accent.
pub fn from_scheme(s: &crate::themes::ColorScheme) -> Palette {
    let c = |k: &str| s.get(k).map(rgb);
    let bg = c("main").unwrap_or(Color32::from_rgb(0x12, 0x12, 0x12));
    let text = c("text").unwrap_or(Color32::WHITE);
    let dark = luminance(bg) < 0.5;
    let mix = |a: Color32, b: Color32, t: f32| lerp_c(a, b, t);
    // Some themes use `sidebar`/`card`/`player` as highlight colours rather
    // than backgrounds. Only treat a colour as a surface if text stays
    // readable on it and it's in the same light/dark family as `main`.
    let surface = |k: &str, max_delta: f32| {
        c(k).filter(|&x| {
            contrast(text, x) >= 4.0 && (luminance(x) - luminance(bg)).abs() <= max_delta
        })
    };
    let accent = c("play-button")
        .or_else(|| c("button"))
        .or_else(|| c("button-active"))
        .unwrap_or(Color32::from_rgb(0x1e, 0xd7, 0x60));
    let panel = surface("card", 0.12)
        .filter(|&p| p != bg)
        .unwrap_or_else(|| mix(bg, text, 0.07));
    let hover_src = c("selected-row").unwrap_or(text);
    // Keep Spotify's look of rounded panels on a darker backdrop even when a
    // scheme uses one colour for everything.
    let base = surface("player", 0.2).unwrap_or_else(|| mix(bg, Color32::BLACK, 0.6));
    let base = if base == bg {
        if dark {
            mix(bg, Color32::BLACK, 0.4)
        } else {
            mix(bg, Color32::BLACK, 0.08)
        }
    } else {
        base
    };
    let weak = c("subtext")
        .filter(|&w| contrast(w, bg) >= 2.5)
        .unwrap_or_else(|| mix(text, bg, 0.35));
    Palette {
        dark,
        accent,
        base,
        bg,
        panel,
        text,
        weak,
        hover: mix(bg, hover_src, 0.12),
        sidebar: surface("sidebar", 0.12).unwrap_or(bg),
        accent_hover: c("button-active").unwrap_or_else(|| mix(accent, Color32::WHITE, 0.15)),
        on_accent: contrast_on(accent),
        notification: c("notification")
            .filter(|&n| n != bg)
            .unwrap_or(Color32::from_rgb(0x2e, 0x77, 0xd0)),
        notification_error: c("notification-error")
            .filter(|&n| n != bg)
            .unwrap_or(Color32::from_rgb(0xe2, 0x21, 0x34)),
    }
}

/// WCAG contrast ratio between two colours (1..=21).
fn contrast(a: Color32, b: Color32) -> f32 {
    let lin = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = |c: Color32| 0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b());
    let (x, y) = (l(a), l(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn palette(
    cfg: &UiConfig,
    ov: &ThemeOverride,
    scheme: Option<&crate::themes::ColorScheme>,
) -> Palette {
    if let Some(s) = scheme {
        let mut p = from_scheme(s);
        // Plugins still win for the colours they set.
        if let Some(a) = ov.accent {
            p.accent = rgb(a);
            p.accent_hover = lerp_c(p.accent, Color32::WHITE, 0.15);
            p.on_accent = contrast_on(p.accent);
        }
        if let Some(t) = ov.text {
            p.text = rgb(t);
        }
        return p;
    }
    let dark = match ov.mode.as_deref() {
        Some("light") => false,
        Some("dark") => true,
        _ => cfg.theme == ThemeMode::Dark,
    };
    let accent = ov
        .accent
        .or_else(|| parse_hex_color(&cfg.accent))
        .map(rgb)
        .unwrap_or(Color32::from_rgb(0x1e, 0xd7, 0x60));
    let (base, bg, panel, text, weak, hover) = if dark {
        (
            Color32::BLACK,
            Color32::from_rgb(0x12, 0x12, 0x12),
            Color32::from_rgb(0x1f, 0x1f, 0x1f),
            Color32::WHITE,
            Color32::from_rgb(0xb3, 0xb3, 0xb3),
            Color32::from_rgb(0x2a, 0x2a, 0x2a),
        )
    } else {
        (
            Color32::from_rgb(0xe4, 0xe4, 0xe4),
            Color32::from_rgb(0xfb, 0xfb, 0xfb),
            Color32::from_rgb(0xef, 0xef, 0xef),
            Color32::from_rgb(0x12, 0x12, 0x12),
            Color32::from_rgb(0x5f, 0x5f, 0x5f),
            Color32::from_rgb(0xe0, 0xe0, 0xe0),
        )
    };
    Palette {
        dark,
        accent,
        base: if ov.background.is_some() {
            ov.background.map(rgb).unwrap_or(base).gamma_multiply(0.6)
        } else {
            base
        },
        bg: ov.background.map(rgb).unwrap_or(bg),
        panel: ov.panel.map(rgb).unwrap_or(panel),
        text: ov.text.map(rgb).unwrap_or(text),
        weak,
        hover,
        sidebar: ov.background.map(rgb).unwrap_or(bg),
        accent_hover: lerp_c(accent, Color32::WHITE, 0.15),
        on_accent: contrast_on(accent),
        notification: Color32::from_rgb(0x2e, 0x77, 0xd0),
        notification_error: Color32::from_rgb(0xe2, 0x21, 0x34),
    }
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.panel;
    v.faint_bg_color = p.panel;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.hover;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.slider_trailing_fill = true;
    let r = CornerRadius::same(6);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.inactive.weak_bg_fill = p.panel;
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.active.weak_bg_fill = p.accent.linear_multiply(0.6);
    // bg_fill is what egui paints scroll handles with: Spotify's are a light
    // translucent grey, brighter while hovered or dragged.
    v.widgets.inactive.bg_fill = lerp_c(p.bg, p.text, 0.3);
    v.widgets.hovered.bg_fill = lerp_c(p.bg, p.text, 0.45);
    v.widgets.active.bg_fill = lerp_c(p.bg, p.text, 0.6);
    v.window_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::NONE;
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(160),
    };
    v.menu_corner_radius = CornerRadius::same(6);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.hover);
    // Text boxes: white caret and a soft accent selection, like Spotify's inputs.
    v.text_cursor.stroke = Stroke::new(1.5, p.text);
    v.selection.bg_fill = lerp_c(p.panel, p.accent, 0.45);
    v.selection.stroke = Stroke::new(1.0, p.text);
    // No growing hover outlines on plain widgets; Spotify's controls only brighten.
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.expansion = 0.0;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.open.bg_stroke = Stroke::NONE;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        // Spotify's scrollbars: thin, floating over the content, only visible
        // while you're over the list, and slightly wider when grabbed.
        s.spacing.scroll = egui::style::ScrollStyle::floating();
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating_width = 3.0;
        s.spacing.scroll.handle_min_length = 32.0;
        s.spacing.scroll.bar_inner_margin = 2.0;
        s.spacing.scroll.bar_outer_margin = 2.0;
        s.spacing.scroll.dormant_handle_opacity = 0.0;
        s.spacing.scroll.dormant_background_opacity = 0.0;
        s.spacing.scroll.active_background_opacity = 0.0;
        s.spacing.scroll.interact_handle_opacity = 0.9;
        s.spacing.scroll.active_handle_opacity = 0.6;
        // egui fades scroll edges with the *panel* colour, which shows up as
        // dark bands over painted backgrounds (lyrics, gradients). Spotify
        // doesn't fade lists either.
        s.spacing.scroll.fade.strength = 0.0;
        s.spacing.tooltip_width = 320.0;
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_over_config() {
        let cfg = UiConfig::default();
        let p = palette(&cfg, &ThemeOverride::default(), None);
        assert!(p.dark);
        assert_eq!(p.accent, Color32::from_rgb(0x1e, 0xd7, 0x60));
        let ov = ThemeOverride {
            accent: Some([1, 2, 3]),
            mode: Some("light".into()),
            ..Default::default()
        };
        let p = palette(&cfg, &ov, None);
        assert!(!p.dark);
        assert_eq!(p.accent, Color32::from_rgb(1, 2, 3));
    }

    #[test]
    fn spicetify_scheme_maps_onto_palette() {
        let s = &crate::themes::parse_color_ini(
            "[Nord]\ntext = eceff4\nsubtext = d8dee9\nmain = 2e3440\nsidebar = 3b4252\n\
             player = 2e3440\ncard = 3b4252\nbutton = 88c0d0\nbutton-active = 8fbcbb\n",
        )[0];
        let p = from_scheme(s);
        assert!(p.dark);
        assert_eq!(p.bg, Color32::from_rgb(0x2e, 0x34, 0x40));
        assert_eq!(p.sidebar, Color32::from_rgb(0x3b, 0x42, 0x52));
        assert_eq!(p.accent, Color32::from_rgb(0x88, 0xc0, 0xd0));
        assert_eq!(p.on_accent, Color32::BLACK);
        let light =
            from_scheme(&crate::themes::parse_color_ini("[L]\nmain = fafafa\ntext = 111111\n")[0]);
        assert!(!light.dark);
        let mid = p.lerp(&light, 0.5);
        // A highlight-coloured "sidebar" (unreadable as a background) is ignored.
        let red = from_scheme(
            &crate::themes::parse_color_ini(
                "[G]\nmain = 282828\ntext = ebdbb2\nsidebar = cc241d\n",
            )[0],
        );
        assert_eq!(red.sidebar, red.bg);
        assert!(mid.bg.r() > p.bg.r() && mid.bg.r() < light.bg.r());
    }
}
