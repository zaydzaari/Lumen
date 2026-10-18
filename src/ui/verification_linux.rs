//! A short-lived GTK helper keeps WebKitGTK's main loop separate from eframe
//! and the optional tray's GTK thread. The same executable runs on X11/Wayland.
use super::{initialization_script, notice, trusted_page, Notice};
use anyhow::{Context as _, Result};
use std::{
    io::{BufRead as _, Write as _},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
};

pub(super) struct View {
    child: Child,
    input: ChildStdin,
}
impl View {
    pub(super) fn start(uri: &str, tx: mpsc::Sender<Notice>, ctx: egui::Context) -> Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--download-verification")
            .arg(uri)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Could not start the Linux verification window")?;
        let output = child
            .stdout
            .take()
            .context("Verification output unavailable")?;
        let input = child
            .stdin
            .take()
            .context("Verification input unavailable")?;
        std::thread::spawn(move || {
            use std::io::Read as _;
            let mut output = std::io::BufReader::new(output);
            loop {
                let mut line = String::new();
                // Bound each message before allocation; never log session material.
                let count = (&mut output).take(12001).read_line(&mut line);
                if !matches!(count, Ok(n) if n > 0 && n <= 12000 && line.ends_with('\n')) {
                    break;
                }
                // Only the child validates web IPC origin. This private pipe cannot
                // receive messages from a website or an unrelated browser process.
                if let Some(value) = notice("https://spotidownloader.com", line.trim()) {
                    let done = matches!(value, Notice::Token(_) | Notice::Failed);
                    if tx.send(value).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                    if done {
                        return;
                    }
                }
            }
            let _ = tx.send(Notice::Failed);
            ctx.request_repaint();
        });
        Ok(Self { child, input })
    }
    pub(super) fn show_page(&mut self) {
        let _ = self.input.write_all(b"show-page\n");
    }
}
impl Drop for View {
    fn drop(&mut self) {
        let _ = self.input.write_all(b"close\n");
        // Only this isolated helper is owned/stopped here; no user's browser is touched.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn filter_rules() -> Vec<u8> {
    // WebKit content filters use a restricted regex dialect without alternation.
    let mut rules =
        vec![serde_json::json!({"trigger":{"url-filter":".*"},"action":{"type":"block"}})];
    for domain in [
        "spotidownloader.com",
        "www.spotidownloader.com",
        "api.spotidownloader.com",
        "challenges.cloudflare.com",
    ] {
        let host = domain.replace('.', "\\.");
        for prefix in ["^https://", "^blob:https://"] {
            rules.push(serde_json::json!({"trigger":{"url-filter":format!("{prefix}{host}(:443)?/")},"action":{"type":"ignore-previous-rules"}}));
        }
    }
    for pattern in ["^about:blank$", "^data:"] {
        rules.push(serde_json::json!({"trigger":{"url-filter":pattern},"action":{"type":"ignore-previous-rules"}}));
    }
    serde_json::to_vec(&rules).expect("static filter serializes")
}

fn restrict_resources(view: &webkit2gtk::WebView, path: &std::path::Path) -> Result<()> {
    use gtk::glib::{self, translate::ToGlibPtr as _};
    use std::{cell::RefCell, rc::Rc};
    use webkit2gtk::{ffi, WebViewExt as _};
    struct State {
        manager: webkit2gtk::UserContentManager,
        main_loop: glib::MainLoop,
        result: Rc<RefCell<Option<bool>>>,
    }
    unsafe extern "C" fn saved(
        source: *mut glib::gobject_ffi::GObject,
        result: *mut gtk::gio::ffi::GAsyncResult,
        data: glib::ffi::gpointer,
    ) {
        // WebKit owns the asynchronous callback; its boxed state is consumed once.
        let state = Box::from_raw(data as *mut State);
        let store = source as *mut ffi::WebKitUserContentFilterStore;
        let mut error = std::ptr::null_mut();
        let filter = ffi::webkit_user_content_filter_store_save_finish(store, result, &mut error);
        let ok = !filter.is_null();
        if ok {
            ffi::webkit_user_content_manager_add_filter(state.manager.to_glib_none().0, filter);
            ffi::webkit_user_content_filter_unref(filter);
        }
        if !error.is_null() {
            glib::ffi::g_error_free(error);
        }
        *state.result.borrow_mut() = Some(ok);
        state.main_loop.quit();
    }
    let manager = view
        .user_content_manager()
        .context("WebKit content manager unavailable")?;
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
    let main_loop = glib::MainLoop::new(None, false);
    let result = Rc::new(RefCell::new(None));
    let state = Box::new(State {
        manager,
        main_loop: main_loop.clone(),
        result: result.clone(),
    });
    let bytes = glib::Bytes::from_owned(filter_rules());
    // The Rust wrapper omits content-filter APIs. These are the corresponding
    // stable WebKitGTK APIs; all calls and callbacks stay on the GTK main thread.
    unsafe {
        let store = ffi::webkit_user_content_filter_store_new(path.as_ptr());
        anyhow::ensure!(
            !store.is_null(),
            "Could not create verification network filter"
        );
        ffi::webkit_user_content_filter_store_save(
            store,
            c"lumen-verification".as_ptr(),
            bytes.to_glib_none().0,
            std::ptr::null_mut(),
            Some(saved),
            Box::into_raw(state) as *mut _,
        );
        main_loop.run();
        glib::gobject_ffi::g_object_unref(store as *mut _);
    }
    anyhow::ensure!(
        *result.borrow() == Some(true),
        "Could not install verification network filter"
    );
    Ok(())
}

fn emit(body: &str) {
    let mut output = std::io::stdout().lock();
    let _ = writeln!(output, "{body}");
    let _ = output.flush();
}

pub(crate) fn run_helper() -> Result<()> {
    use gtk::{glib, prelude::*};
    use webkit2gtk::{SettingsExt as _, WebViewExt as _};
    use wry::{WebViewBuilderExtUnix as _, WebViewExtUnix as _};
    gtk::init().context("Could not initialize GTK; a graphical Linux session is required")?;
    let uri = std::env::args().nth(2).unwrap_or_default();
    let paths = crate::config::Paths::resolve()?;
    let profile = paths.data_dir.join("download-webview");
    std::fs::create_dir_all(&profile)?;
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&profile, std::fs::Permissions::from_mode(0o700))?;
    let mut context = wry::WebContext::new(Some(profile.clone()));
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Lumen — Download verification");
    window.set_default_size(420, 270);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.set_margin_top(12);
    column.set_margin_bottom(12);
    column.pack_start(
        &gtk::Label::new(Some("spotidownloader.com · Cloudflare")),
        false,
        false,
        0,
    );
    let browser = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.pack_start(&browser, true, true, 0);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let show = gtk::Button::with_label("Show page");
    let cancel = gtk::Button::with_label("Cancel");
    controls.pack_start(&show, false, false, 0);
    controls.pack_end(&cancel, false, false, 0);
    column.pack_end(&controls, false, false, 0);
    window.add(&column);
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        glib::Propagation::Proceed
    });
    cancel.connect_clicked(|_| gtk::main_quit());
    let view = std::rc::Rc::new(
        wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_url("about:blank")
            .with_initialization_script(initialization_script(&uri))
            .with_devtools(false)
            .with_autoplay(false)
            .with_clipboard(false)
            .with_navigation_handler(|url| trusted_page(&url) || url == "about:blank")
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_ipc_handler(|message| {
                if let Some(value) = notice(&message.uri().to_string(), message.body()) {
                    emit(message.body());
                    if matches!(value, Notice::Token(_) | Notice::Failed) {
                        gtk::main_quit();
                    }
                }
            })
            .build_gtk(&browser)
            .context("Could not create WebKitGTK verification")?,
    );
    let native = view.webview();
    native.connect_context_menu(|_, _, _, _| true);
    if let Some(settings) = webkit2gtk::WebViewExt::settings(&native) {
        settings.set_enable_developer_extras(false);
        settings.set_enable_media(false);
        settings.set_enable_media_stream(false);
        settings.set_enable_page_cache(false);
    }
    restrict_resources(&native, &profile.join("filters"))?;
    let reveal = {
        let view = view.clone();
        let window = window.clone();
        move || {
            let _ = view
                .evaluate_script("document.getElementById('lumen-verification-style')?.remove()");
            window.resize(640, 600);
        }
    };
    let reveal = std::rc::Rc::new(reveal);
    let button_reveal = reveal.clone();
    show.connect_clicked(move |_| button_reveal());
    let (commands, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            match line.as_deref() {
                Ok("show-page") => {
                    if commands.send(true).is_err() {
                        return;
                    }
                }
                _ => {
                    let _ = commands.send(false);
                    return;
                }
            }
        }
        let _ = commands.send(false);
    });
    let started = std::time::Instant::now();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        for show in received.try_iter() {
            if show {
                reveal();
            } else {
                gtk::main_quit();
                return glib::ControlFlow::Break;
            }
        }
        if started.elapsed() > std::time::Duration::from_secs(210) {
            gtk::main_quit();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    view.load_url("https://spotidownloader.com/en19")?;
    window.show_all();
    gtk::main();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a graphical WebKitGTK session; loads only synthetic local HTML"]
    fn native_webkit_fixture() {
        use gtk::{glib, prelude::*};
        use std::{cell::Cell, rc::Rc};
        use webkit2gtk::WebViewExt as _;
        use wry::{WebViewBuilderExtUnix as _, WebViewExtUnix as _};
        gtk::init().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let main_loop = glib::MainLoop::new(None, false);
        let received = Rc::new(Cell::new(false));
        let (done, callback_loop) = (received.clone(), main_loop.clone());
        let view = wry::WebViewBuilder::new()
            .with_url("about:blank")
            .with_ipc_handler(move |message| {
                if message.body() == "fixture-ready" {
                    done.set(true);
                    callback_loop.quit();
                }
            })
            .build_gtk(&window)
            .unwrap();
        restrict_resources(&view.webview(), folder.path()).unwrap();
        let deadline_loop = main_loop.clone();
        let deadline =
            glib::timeout_add_local_once(std::time::Duration::from_secs(15), move || {
                deadline_loop.quit()
            });
        view.webview().load_html("<html><body>Local fixture<script>window.ipc.postMessage('fixture-ready')</script></body></html>", Some("https://spotidownloader.com/fixture"));
        main_loop.run();
        if received.get() {
            deadline.remove();
        }
        assert!(
            received.get(),
            "WebKitGTK did not deliver the synthetic IPC fixture"
        );
        drop(view);
        unsafe {
            window.destroy();
        }
    }

    #[test]
    fn network_filter_is_valid_and_default_deny() {
        let rules: serde_json::Value = serde_json::from_slice(&filter_rules()).unwrap();
        assert_eq!(rules[0]["action"]["type"], "block");
        assert_eq!(rules[1]["action"]["type"], "ignore-previous-rules");
        assert!(rules[1]["trigger"]["url-filter"]
            .as_str()
            .unwrap()
            .ends_with("(:443)?/"));
    }
}
