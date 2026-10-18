//! System tray icon with Play/Pause, Next, Previous, Show and Quit.
//!
//! On Windows/macOS the tray lives on the main (event-loop) thread; on Linux
//! `tray-icon` needs GTK, so it runs on its own GTK thread.

use crate::events::{AppEvent, UiTx};
use crate::player::PlayerCmd;

/// RGBA pixels for a small round "play" icon in the accent colour.
pub fn icon_rgba(size: u32, accent: [u8; 3]) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    let r = size as f32 / 2.0 - 0.5;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 - c, y as f32 - c);
            let d = (fx * fx + fy * fy).sqrt();
            let i = ((y * size + x) * 4) as usize;
            if d <= r {
                // Triangle pointing right, centred.
                let tx = fx + r * 0.12;
                let inside_tri =
                    tx > -r * 0.3 && tx < r * 0.45 && fy.abs() < (r * 0.45 - tx) * 0.85;
                let rgb = if inside_tri { [255, 255, 255] } else { accent };
                let alpha = ((r - d).clamp(0.0, 1.0) * 255.0) as u8;
                px[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], alpha]);
            }
        }
    }
    px
}

#[cfg(feature = "tray")]
mod imp {
    use super::*;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    pub struct Tray {
        #[cfg(not(target_os = "linux"))]
        _icon: TrayIcon,
    }

    fn build(accent: [u8; 3]) -> Result<(TrayIcon, [tray_icon::menu::MenuId; 5]), String> {
        let menu = Menu::new();
        let play = MenuItem::new("Play / Pause", true, None);
        let next = MenuItem::new("Next", true, None);
        let prev = MenuItem::new("Previous", true, None);
        let show = MenuItem::new("Show Lumen", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[
            &play,
            &next,
            &prev,
            &PredefinedMenuItem::separator(),
            &show,
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let icon = Icon::from_rgba(icon_rgba(32, accent), 32, 32).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Lumen")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;
        Ok((
            tray,
            [
                play.id().clone(),
                next.id().clone(),
                prev.id().clone(),
                show.id().clone(),
                quit.id().clone(),
            ],
        ))
    }

    fn install_handlers(ui: UiTx, ids: [tray_icon::menu::MenuId; 5]) {
        let ui2 = ui.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let [play, next, prev, show, quit] = &ids;
            let id = e.id();
            if id == play {
                ui.send(AppEvent::Remote(PlayerCmd::TogglePlay));
            } else if id == next {
                ui.send(AppEvent::Remote(PlayerCmd::Next));
            } else if id == prev {
                ui.send(AppEvent::Remote(PlayerCmd::Previous));
            } else if id == show {
                ui.send(AppEvent::TrayShow);
            } else if id == quit {
                ui.send(AppEvent::TrayQuit);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                ui2.send(AppEvent::TrayShow);
            }
        }));
    }

    impl Tray {
        pub fn new(ui: UiTx, accent: [u8; 3]) -> Option<Self> {
            #[cfg(target_os = "linux")]
            {
                std::thread::Builder::new()
                    .name("tray-gtk".into())
                    .spawn(move || {
                        if let Err(e) = gtk::init() {
                            tracing::warn!("tray unavailable (GTK init failed): {e}");
                            return;
                        }
                        match build(accent) {
                            Ok((tray, ids)) => {
                                install_handlers(ui, ids);
                                gtk::main();
                                drop(tray);
                            }
                            Err(e) => tracing::warn!("tray unavailable: {e}"),
                        }
                    })
                    .ok()?;
                Some(Self {})
            }
            #[cfg(not(target_os = "linux"))]
            {
                match build(accent) {
                    Ok((tray, ids)) => {
                        install_handlers(ui, ids);
                        Some(Self { _icon: tray })
                    }
                    Err(e) => {
                        tracing::warn!("tray unavailable: {e}");
                        None
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "tray"))]
mod imp {
    use super::*;
    pub struct Tray;
    impl Tray {
        pub fn new(_ui: UiTx, _accent: [u8; 3]) -> Option<Self> {
            None
        }
    }
}

pub use imp::Tray;

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_expected_size_and_transparent_corners() {
        let px = super::icon_rgba(32, [30, 215, 96]);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "corner must be transparent");
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(px[centre + 3], 255);
    }
}
