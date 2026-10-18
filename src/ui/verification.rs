//! On-demand, attended verification using platform web runtimes.
#[cfg(target_os = "linux")]
#[path = "verification_linux.rs"]
mod linux;
use super::App;
use crate::{events::ToastKind, verification::Request};
#[cfg(windows)]
use anyhow::Context as _;
use anyhow::Result;
#[cfg(target_os = "linux")]
pub(crate) use linux::run_helper as run_linux_helper;
use std::{
    sync::{atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

#[cfg_attr(not(windows), allow(dead_code))]
enum Notice {
    Ready,
    Token(String),
    PageNeeded,
    Failed,
}

#[cfg(any(windows, target_os = "linux", test))]
fn trusted_page(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("spotidownloader.com")
            && u.port_or_known_default() == Some(443)
            && u.username().is_empty()
            && u.password().is_none()
    })
}

#[cfg(any(windows, test))]
fn allowed_resource(raw: &str) -> bool {
    let raw = raw.strip_prefix("blob:").unwrap_or(raw);
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if url.scheme() == "data" || raw == "about:blank" {
        return true;
    }
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some(
                "spotidownloader.com"
                    | "www.spotidownloader.com"
                    | "api.spotidownloader.com"
                    | "challenges.cloudflare.com"
            )
        )
}

#[cfg(windows)]
fn restrict_resources(view: &wry::WebView) -> Result<()> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        WebResourceRequestedEventHandler,
    };
    use windows::core::{w, PWSTR};
    use wry::WebViewExtWindows as _;
    let native = view.webview();
    let environment = view.environment();
    let mut registration = 0;
    // COM objects belong to this UI thread; callback lifetimes are owned by the
    // WebView, and take_pwstr releases the string allocated by WebView2.
    unsafe {
        native.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        native.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let request = args.Request()?;
                    let mut raw = PWSTR::null();
                    request.Uri(&mut raw)?;
                    if !allowed_resource(&webview2_com::take_pwstr(raw)) {
                        let response = environment.CreateWebResourceResponse(
                            None,
                            403,
                            w!("Blocked"),
                            w!("Content-Type: text/plain"),
                        )?;
                        args.SetResponse(&response)?;
                    }
                }
                Ok(())
            })),
            &mut registration,
        )?;
    }
    Ok(())
}
#[cfg(any(windows, target_os = "linux", test))]
fn notice(origin: &str, body: &str) -> Option<Notice> {
    if !trusted_page(origin) || body.len() > 12000 {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    match json.get("kind")?.as_str()? {
        "ready" => Some(Notice::Ready),
        "page-needed" => Some(Notice::PageNeeded),
        "failed" => Some(Notice::Failed),
        "token" => crate::spotidownloader::normalize_token(json.get("token")?.as_str()?)
            .ok()
            .map(Notice::Token),
        _ => None,
    }
}

#[cfg(any(windows, target_os = "linux", test))]
fn initialization_script(uri: &str) -> String {
    let id = Some(crate::model::uri_id(uri))
        .filter(|id| id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or("76gtwXQM0Un36Lq1JzNh8I");
    let url = serde_json::to_string(&format!("https://open.spotify.com/track/{id}"))
        .expect("static URL serializes");
    include_str!("verification.js").replace("__LUMEN_TRACK_URL__", &url)
}

pub struct Panel {
    request: Request,
    started: Instant,
    rx: mpsc::Receiver<Notice>,
    #[cfg(windows)]
    webview: wry::WebView,
    #[cfg(target_os = "linux")]
    linux_view: linux::View,
    ready: bool,
    page_needed: bool,
    full_page: bool,
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.request.cancelled.store(true, Ordering::Relaxed);
        if let Some(reply) = self.request.complete.take() {
            let _ = reply.try_send(Err("Verification cancelled".into()));
        }
    }
}

#[cfg(windows)]
struct Parent(std::num::NonZeroIsize);
#[cfg(windows)]
impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        let handle = raw_window_handle::Win32WindowHandle::new(self.0);
        // The eframe window outlives this child. Panel is dropped at shutdown,
        // before eframe destroys its native window; no handle ownership is taken.
        Ok(unsafe {
            raw_window_handle::WindowHandle::borrow_raw(raw_window_handle::RawWindowHandle::Win32(
                handle,
            ))
        })
    }
}

impl Panel {
    #[cfg(windows)]
    fn new(app: &App, request: Request) -> Result<Self> {
        use wry::WebViewBuilderExtWindows as _;
        let parent = Parent(
            std::num::NonZeroIsize::new(app.native_hwnd.unwrap_or(0))
                .context("Native window is unavailable")?,
        );
        let profile = app.shared.paths.data_dir.join("download-webview");
        std::fs::create_dir_all(&profile)?;
        let mut context = wry::WebContext::new(Some(profile));
        let (tx, rx) = mpsc::channel();
        let ctx = app.ui_tx.ctx().clone();
        let webview = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_url("about:blank")
            .with_initialization_script(initialization_script(&request.track_uri))
            .with_background_color((18,18,18,255))
            .with_visible(false).with_autoplay(false).with_devtools(false)
            .with_general_autofill_enabled(false).with_clipboard(false)
            .with_default_context_menus(false).with_browser_accelerator_keys(false)
            .with_browser_extensions_enabled(false)
            .with_additional_browser_args("--disable-background-networking --disable-sync --disable-extensions --disable-default-apps")
            .with_navigation_handler(|url| trusted_page(&url) || url == "about:blank")
            .with_new_window_req_handler(|_,_| wry::NewWindowResponse::Deny)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_download_started_handler(|_,_| false)
            .with_ipc_handler(move |message| {
                if let Some(value) = notice(&message.uri().to_string(), message.body()) {
                    let _ = tx.send(value);
                    ctx.request_repaint();
                }
            }).build_as_child(&parent)
            .context("Could not open WebView2 verification")?;
        restrict_resources(&webview)?;
        webview.load_url("https://spotidownloader.com/en19")?;
        Ok(Self {
            request,
            started: Instant::now(),
            rx,
            webview,
            ready: false,
            page_needed: false,
            full_page: false,
        })
    }
    #[cfg(target_os = "linux")]
    fn new(app: &App, request: Request) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let view = linux::View::start(&request.track_uri, tx, app.ui_tx.ctx().clone())?;
        Ok(Self {
            request,
            started: Instant::now(),
            rx,
            linux_view: view,
            ready: false,
            page_needed: false,
            full_page: false,
        })
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    fn new(_app: &App, _request: Request) -> Result<Self> {
        anyhow::bail!("In-app verification currently requires Windows WebView2.");
    }

    fn reveal_page(&mut self) {
        self.full_page = true;
        #[cfg(target_os = "linux")]
        self.linux_view.show_page();
        #[cfg(windows)]
        {
            let _ = self
                .webview
                .evaluate_script("document.getElementById('lumen-verification-style')?.remove()");
            let _ = self.webview.set_visible(true);
        }
    }
}

impl App {
    pub fn start_download_verification(&mut self, request: Request) {
        if self.mini {
            self.set_mini(false);
        }
        let ctx = self.ui_tx.ctx();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        // A manual connection can already be open when the download job asks.
        if let Some(panel) = self.download_verification.as_mut() {
            if panel.request.complete.is_none() {
                panel.request = request;
            } else if let Some(reply) = request.complete {
                let _ = reply.try_send(Err("Already verifying".into()));
            }
            return;
        }
        match Panel::new(self, request) {
            Ok(panel) => self.download_verification = Some(panel),
            Err(_) => self.toast(ToastKind::Error, "Could not open download verification. Check the platform browser runtime (Windows WebView2 or Linux WebKitGTK), or use the manual token setting."),
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    pub fn connect_downloads(&mut self) {
        self.config.downloads.browser_connection = true;
        self.download_connection.set_enabled(true);
        self.save_config();
        let uri = self
            .snapshot
            .track
            .as_ref()
            .filter(|t| !self.private_session && t.uri.starts_with("spotify:track:"))
            .map(|t| t.uri.clone())
            .unwrap_or_default();
        self.start_download_verification(Request {
            track_uri: uri,
            cancelled: Default::default(),
            complete: None,
        });
    }

    pub fn show_download_verification(&mut self, ctx: &egui::Context) {
        let Some(mut panel) = self.download_verification.take() else {
            return;
        };
        if panel.request.cancelled.load(Ordering::Relaxed) {
            return;
        }
        let mut token = None;
        let mut failed = false;
        while let Ok(event) = panel.rx.try_recv() {
            match event {
                Notice::Ready => panel.ready = true,
                Notice::PageNeeded => panel.page_needed = true,
                Notice::Token(value) => token = Some(value),
                Notice::Failed => failed = true,
            }
        }
        if failed && token.is_none() {
            self.toast(ToastKind::Error, "The provider rejected verification. Try Connect again or use the manual token option.");
            return;
        }
        if let Some(token) = token {
            if self
                .shared
                .store
                .save(crate::spotidownloader::KEY, &token)
                .is_ok()
            {
                self.download_token_saved = true;
                self.download_token_input.clear();
                if let Some(reply) = panel.request.complete.take() {
                    let _ = reply.try_send(Ok(()));
                }
                self.toast(
                    ToastKind::Info,
                    "Download connection ready. Verification closed.",
                );
                if let (Some(downloader), Some(track)) = (&self.downloader, &self.snapshot.track) {
                    if !self.private_session && self.snapshot.playing {
                        downloader.request(track.clone());
                    }
                }
            } else {
                self.toast(
                    ToastKind::Error,
                    "Could not save the download connection locally.",
                );
            }
            return;
        }
        if panel.started.elapsed() > Duration::from_secs(210) {
            self.toast(
                ToastKind::Error,
                "Download verification timed out. Reconnect in Settings > Storage.",
            );
            return;
        }
        if !panel.ready && panel.started.elapsed() > Duration::from_secs(15) {
            panel.page_needed = true;
        }
        let mut cancel = false;
        #[cfg(windows)]
        let mut bounds = None;
        egui::Modal::new(egui::Id::new("download-verification"))
            .frame(
                egui::Frame::new()
                    .fill(self.palette.bg)
                    .corner_radius(8)
                    .inner_margin(18.0),
            )
            .show(ctx, |ui| {
                ui.set_width(if panel.full_page { 620.0 } else { 360.0 });
                ui.label(egui::RichText::new("Download verification").strong());
                ui.label(
                    egui::RichText::new("spotidownloader.com · Cloudflare")
                        .small()
                        .weak(),
                );
                #[cfg(target_os = "linux")]
                ui.label("Verification is open in Lumen's separate verification window.");
                let height = if cfg!(target_os = "linux") {
                    8.0
                } else if panel.full_page {
                    (ctx.content_rect().height() - 200.0).clamp(180.0, 520.0)
                } else {
                    150.0
                };
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    egui::Sense::hover(),
                );
                #[cfg(windows)]
                {
                    bounds = Some(rect);
                }
                #[cfg(not(windows))]
                let _ = rect;
                ui.label(if panel.page_needed {
                    "If verification is blank, open the full page below."
                } else {
                    "Complete verification if requested. This panel closes when ready."
                });
                ui.horizontal(|ui| {
                    if !panel.full_page && ui.button("Show page").clicked() {
                        panel.reveal_page();
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            return;
        }
        #[cfg(windows)]
        if let Some(rect) = bounds {
            let scale = ctx.pixels_per_point() as f64;
            let _ = panel.webview.set_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(
                    rect.left() as f64 * scale,
                    rect.top() as f64 * scale,
                )
                .into(),
                size: wry::dpi::PhysicalSize::new(
                    rect.width() as f64 * scale,
                    rect.height() as f64 * scale,
                )
                .into(),
            });
            if panel.ready {
                let _ = panel.webview.set_visible(true);
            }
        }
        self.download_verification = Some(panel);
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_provider_page_can_handoff_a_token() {
        let body = r#"{"kind":"token","token":"fixture-token"}"#;
        assert!(matches!(
            notice("https://spotidownloader.com/en19", body),
            Some(Notice::Token(_))
        ));
        for origin in [
            "https://evil.example",
            "https://spotidownloader.com.evil.example",
            "https://challenges.cloudflare.com",
            "http://spotidownloader.com",
            "https://spotidownloader.com:8080",
        ] {
            assert!(notice(origin, body).is_none());
        }
        assert!(notice("https://spotidownloader.com", &"x".repeat(12001)).is_none());
        assert!(notice(
            "https://spotidownloader.com",
            r#"{"kind":"token","token":""}"#
        )
        .is_none());
    }
    #[test]
    fn bootstrap_url_cannot_inject_script() {
        let script = initialization_script("spotify:track:</script>bad");
        assert!(!script.contains("</script>bad"));
        assert!(!script.contains("__LUMEN_TRACK_URL__"));
        assert!(script.contains("https://open.spotify.com/track/76gtwXQM0Un36Lq1JzNh8I"));
    }

    #[test]
    fn network_allowlist_keeps_verification_and_excludes_ads_and_local_network() {
        for url in [
            "https://spotidownloader.com/en19",
            "https://api.spotidownloader.com/session",
            "https://challenges.cloudflare.com/turnstile/v0/api.js",
            "blob:https://challenges.cloudflare.com/worker",
        ] {
            assert!(allowed_resource(url));
        }
        for url in [
            "https://ads.example/ad.js",
            "https://spotidownloader.com.evil.example/script",
            "http://api.spotidownloader.com/session",
            "https://127.0.0.1/",
            "file:///private",
            "https://spotidownloader.com:8080",
            "blob:blob:https://spotidownloader.com/worker",
        ] {
            assert!(!allowed_resource(url));
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires installed Windows WebView2; creates a hidden synthetic verification host"]
    fn native_webview_loads_and_hands_back_a_fixture_message() {
        use windows::{core::w, Win32::UI::WindowsAndMessaging::*};
        struct Window(windows::Win32::Foundation::HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        // Never shown; uses local fixture HTML, with no network or CAPTCHA.
        let window = Window(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Lumen verification fixture"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                400,
                240,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        });
        let parent = Parent(std::num::NonZeroIsize::new(window.0 .0 as isize).unwrap());
        let (tx, rx) = mpsc::channel();
        let view = wry::WebViewBuilder::new().with_html("<html><body>Synthetic fixture<script>window.ipc.postMessage('fixture-ready')</script></body></html>")
            .with_ipc_handler(move |message| { let _ = tx.send(message.body().clone()); })
            .build_as_child(&parent).unwrap();
        restrict_resources(&view).unwrap();
        let start = Instant::now();
        loop {
            if let Ok(message) = rx.try_recv() {
                assert_eq!(message, "fixture-ready");
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "WebView2 fixture timed out"
            );
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(view);
    }
}
