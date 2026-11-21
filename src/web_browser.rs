//! Main-thread native web views. No IPC bridge, token scraping or deck access.
use crate::{web_policy, AppWindow, WebBrowserWindow};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc::{self, Receiver, SyncSender},
    time::{Duration, Instant},
};
enum Event {
    Started(String),
    Finished(String),
    Popup(String),
    Blocked,
}
#[derive(Clone, Copy)]
enum Action {
    Navigate,
    Back,
    Forward,
    Reload,
    Home,
}
trait Backend {
    fn load(&self, url: &str) -> Result<(), String>;
    fn command(&self, action: Action) -> Result<(), String>;
    fn resize(&self, window: &slint::Window) -> Result<(), String>;
}
#[cfg(target_os = "windows")]
struct Native(wry::WebView);
#[cfg(target_os = "windows")]
impl Backend for Native {
    fn load(&self, url: &str) -> Result<(), String> {
        self.0.load_url(url).map_err(|e| e.to_string())
    }
    fn command(&self, action: Action) -> Result<(), String> {
        self.0
            .evaluate_script(match action {
                Action::Back => "history.back()",
                Action::Forward => "history.forward()",
                Action::Reload => "location.reload()",
                _ => return Ok(()),
            })
            .map_err(|e| e.to_string())
    }
    fn resize(&self, window: &slint::Window) -> Result<(), String> {
        self.0.set_bounds(bounds(window)).map_err(|e| e.to_string())
    }
}
#[cfg(target_os = "windows")]
fn bounds(window: &slint::Window) -> wry::Rect {
    let size = window.size();
    let top = (42. * window.scale_factor()).round() as u32;
    let bottom = (32. * window.scale_factor()).round() as u32;
    wry::Rect {
        position: wry::dpi::PhysicalPosition::new(0, top as i32).into(),
        size: wry::dpi::PhysicalSize::new(
            size.width.max(1),
            size.height.saturating_sub(top + bottom).max(1),
        )
        .into(),
    }
}
#[cfg(target_os = "windows")]
fn native(
    view: &WebBrowserWindow,
    help: bool,
    tx: SyncSender<Event>,
) -> Result<Box<dyn Backend>, String> {
    use slint::winit_030::WinitWindowAccessor;
    use wry::WebViewBuilderExtWindows;
    let version =
        wry::webview_version().map_err(|e| format!("WebView2 runtime unavailable: {e}"))?;
    if !web_policy::private_runtime(&version) {
        return Err(
            "Update WebView2 Runtime to version102 or later for isolated private browsing".into(),
        );
    }
    let path = dirs::data_local_dir()
        .ok_or("Application data directory unavailable")?
        .join("DRDJPro")
        .join("web")
        .join(if help { "help" } else { "browse" });
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    let mut context = wry::WebContext::new(Some(path));
    let nav = tx.clone();
    let popup = tx.clone();
    let builder = wry::WebViewBuilder::new_with_web_context(&mut context)
        // Replace Wry's default arguments, which disable SmartScreen.
        .with_additional_browser_args("--autoplay-policy=user-gesture-required")
        .with_incognito(true)
        .with_autoplay(false)
        .with_clipboard(false)
        .with_user_agent("DRDJPro-Rust/0.1.0")
        .with_permission_handler(|_| wry::PermissionResponse::Deny)
        .with_download_started_handler(|_, _| false)
        .with_navigation_handler(move |url| {
            let allowed = web_policy::allowed(&url);
            if !allowed {
                let _ = nav.try_send(Event::Blocked);
            }
            allowed
        })
        .with_new_window_req_handler(move |url, _| {
            if web_policy::allowed(&url) {
                let _ = popup.try_send(Event::Popup(url));
            }
            wry::NewWindowResponse::Deny
        })
        .with_on_page_load_handler(move |event, url| {
            let url = web_policy::display_url(&url);
            let _ = tx.try_send(match event {
                wry::PageLoadEvent::Started => Event::Started(url),
                wry::PageLoadEvent::Finished => Event::Finished(url),
            });
        })
        .with_custom_protocol("drdjhelp".into(), |_, request| {
            let (mime, bytes, status) = web_policy::resource(&request.uri().to_string());
            wry::http::Response::builder()
                .status(status)
                .header("Content-Type", mime)
                .header("Content-Security-Policy", web_policy::CSP)
                .header("X-Content-Type-Options", "nosniff")
                .body(std::borrow::Cow::Borrowed(bytes))
                .expect("static help response headers")
        })
        .with_bounds(bounds(view.window()));
    let web = view
        .window()
        .with_winit_window(|window| builder.build_as_child(window))
        .ok_or("Winit native parent unavailable")?
        .map_err(|e| format!("WebView2 initialization failed: {e}"))?;
    Ok(Box::new(Native(web)))
}
#[cfg(not(target_os = "windows"))]
fn native(_: &WebBrowserWindow, _: bool, _: SyncSender<Event>) -> Result<Box<dyn Backend>, String> {
    Err("Native web backend is currently available on Windows only".into())
}

struct Session {
    backend: Option<Box<dyn Backend>>,
    pending: Option<(Instant, SyncSender<Event>)>,
    view: WebBrowserWindow,
    events: Receiver<Event>,
    closed: Rc<Cell<bool>>,
    help: bool,
    loading: Option<Instant>,
    size: (u32, u32, u32),
}
impl Session {
    fn act(&mut self, action: Action) {
        let Some(backend) = &self.backend else { return };
        let result = match action {
            Action::Navigate | Action::Home => {
                let target = if matches!(action, Action::Home) {
                    if self.help {
                        web_policy::HELP_URL.into()
                    } else {
                        web_policy::HOME_URL.into()
                    }
                } else {
                    self.view.get_address().to_string()
                };
                if !web_policy::allowed(&target) {
                    Err("Use an HTTPS address or bundled help".into())
                } else {
                    backend.load(&target)
                }
            }
            _ => backend.command(action),
        };
        match result {
            Ok(()) => {
                self.loading = Some(Instant::now());
                self.view.set_status(if self.help {
                    "Hilfe wird geladen …".into()
                } else {
                    "Loading; Beatport deck streaming is not configured".into()
                });
            }
            Err(e) => self.view.set_status(e.into()),
        }
    }
    fn poll(&mut self) {
        let Some(backend) = &self.backend else { return };
        for _ in 0..32 {
            let Ok(event) = self.events.try_recv() else {
                break;
            };
            match event {
                Event::Started(url) => {
                    self.view.set_address(url.into());
                    self.loading = Some(Instant::now());
                }
                Event::Finished(url) => {
                    self.view.set_address(url.into());
                    self.loading = None;
                    self.view.set_status(if self.help {
                        "Navigation abgeschlossen".into()
                    } else {
                        "Navigation ended; inspect the page for errors. Website audio is separate from DJ decks.".into()
                    });
                }
                Event::Popup(url) => {
                    if let Err(e) = backend.load(&url) {
                        self.view.set_status(e.into());
                    }
                }
                Event::Blocked => self.view.set_status(
                    "Navigation blocked: only HTTPS and bundled help are allowed".into(),
                ),
            }
        }
        if self
            .loading
            .is_some_and(|t| t.elapsed() > Duration::from_secs(30))
        {
            self.loading = None;
            self.view.set_status("Page timed out; check the network and reload. Native error pages may give more detail.".into());
        }
        let size = self.view.window().size();
        let key = (
            size.width,
            size.height,
            self.view.window().scale_factor().to_bits(),
        );
        if self.size != key {
            self.size = key;
            if let Err(e) = backend.resize(self.view.window()) {
                self.view.set_status(e.into());
            }
        }
    }
}
type Sessions = Rc<RefCell<[Option<Session>; 2]>>;
type Factory =
    Rc<dyn Fn(&WebBrowserWindow, bool, SyncSender<Event>) -> Result<Box<dyn Backend>, String>>;
/// Install explicit WEB/HELP actions; no runtime starts during app initialization.
pub fn wire(ui: &AppWindow) -> Timer {
    wire_with(ui, Rc::new(native), Rc::new(parent_ready)).0
}

fn parent_ready(view: &WebBrowserWindow) -> bool {
    #[cfg(target_os = "windows")]
    {
        use slint::winit_030::WinitWindowAccessor;
        view.window().has_winit_window()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = view;
        true
    }
}

fn raise(view: &WebBrowserWindow) {
    #[cfg(target_os = "windows")]
    {
        use slint::winit_030::WinitWindowAccessor;
        view.window().with_winit_window(|window| {
            if window.is_minimized() == Some(true) {
                window.set_minimized(false);
            }
            window.focus_window();
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = view;
}

fn wire_with(
    ui: &AppWindow,
    factory: Factory,
    ready: Rc<dyn Fn(&WebBrowserWindow) -> bool>,
) -> (Timer, Sessions) {
    let sessions: Sessions = Rc::new(RefCell::new([None, None]));
    let opening = Rc::clone(&sessions);
    let weak = ui.as_weak();
    ui.on_show_browser(move |help| {
        let index = usize::from(help);
        let existing = opening.borrow()[index]
            .as_ref()
            .filter(|s| !s.closed.get())
            .map(|s| s.view.clone_strong());
        if let Some(view) = existing {
            raise(&view);
            return;
        }
        if let Some(old) = opening.borrow_mut()[index].take() {
            let _ = old.view.hide();
        }
        let result = (|| -> Result<Session, String> {
            let view = WebBrowserWindow::new().map_err(|e| e.to_string())?;
            view.set_help_mode(help);
            let (tx, events) = mpsc::sync_channel(32);
            let closed = Rc::new(Cell::new(false));
            let flag = Rc::clone(&closed);
            view.on_dismiss(move || flag.set(true));
            let flag = Rc::clone(&closed);
            view.window().on_close_requested(move || {
                flag.set(true);
                slint::CloseRequestResponse::HideWindow
            });
            for action in [
                Action::Navigate,
                Action::Back,
                Action::Forward,
                Action::Reload,
                Action::Home,
            ] {
                let state = Rc::downgrade(&opening);
                let callback = move || {
                    if let Some(state) = state.upgrade() {
                        if let Some(s) = state.borrow_mut()[index].as_mut() {
                            s.act(action);
                        }
                    }
                };
                match action {
                    Action::Navigate => view.on_navigate(callback),
                    Action::Back => view.on_back(callback),
                    Action::Forward => view.on_forward(callback),
                    Action::Reload => view.on_reload(callback),
                    Action::Home => view.on_home(callback),
                }
            }
            view.show().map_err(|e| e.to_string())?;
            Ok(Session {
                pending: Some((Instant::now(), tx)),
                backend: None,
                view,
                events,
                closed,
                help,
                loading: None,
                size: (0, 0, 0),
            })
        })();
        match result {
            Ok(session) => opening.borrow_mut()[index] = Some(session),
            Err(e) => {
                if let Some(ui) = weak.upgrade() {
                    ui.set_status(format!("Browser: {e}").into());
                }
            }
        }
    });
    let closing = Rc::clone(&sessions);
    ui.on_close_web_browsers(move || {
        for slot in closing.borrow_mut().iter_mut() {
            if let Some(session) = slot.take() {
                let _ = session.view.hide();
            }
        }
    });
    let ticking = Rc::clone(&sessions);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        for index in 0..2 {
            tick_session(&ticking, index, &factory, ready.as_ref());
        }
    });
    (timer, sessions)
}

fn tick_session(
    sessions: &Sessions,
    index: usize,
    factory: &Factory,
    ready: &dyn Fn(&WebBrowserWindow) -> bool,
) {
    let init = {
        let mut slots = sessions.borrow_mut();
        let slot = &mut slots[index];
        let Some(s) = slot else { return };
        if s.closed.get() {
            let _ = s.view.hide();
            *slot = None;
            return;
        }
        let Some((since, _)) = &s.pending else {
            s.poll();
            return;
        };
        // show() may complete before Winit creates the secondary HWND.
        if !ready(&s.view) {
            if since.elapsed() > Duration::from_secs(10) {
                s.pending = None;
                s.view.set_status(
                    "Das Fenster konnte nicht geöffnet werden. Bitte schließen und erneut öffnen."
                        .into(),
                );
                s.view.set_startup_failed(true);
            }
            return;
        }
        s.pending
            .take()
            .map(|(_, tx)| (s.view.clone_strong(), s.help, tx, s.closed.clone()))
    };
    if let Some((view, help, tx, closed)) = init {
        // WebView2 can dispatch window messages during initialization.
        // Do not hold a sessions borrow or attach to a dismissed window.
        let result = factory(&view, help, tx);
        if let Some(s) = sessions.borrow_mut()[index]
            .as_mut()
            .filter(|s| Rc::ptr_eq(&s.closed, &closed) && !closed.get())
        {
            match result {
                Ok(backend) => {
                    s.backend = Some(backend);
                    s.act(Action::Home);
                }
                Err(e) => {
                    s.view.set_status(e.into());
                    s.view.set_startup_failed(true);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests_open;
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) struct Fake(pub(super) Rc<RefCell<Vec<String>>>);
    impl Backend for Fake {
        fn load(&self, url: &str) -> Result<(), String> {
            self.0.borrow_mut().push(url.into());
            Ok(())
        }
        fn command(&self, a: Action) -> Result<(), String> {
            self.0.borrow_mut().push(
                match a {
                    Action::Back => "back",
                    Action::Forward => "forward",
                    _ => "reload",
                }
                .into(),
            );
            Ok(())
        }
        fn resize(&self, _: &slint::Window) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn browser_actions_are_lazy_independent_validated_and_close_without_playback_changes() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.set_left_playing(true);
        ui.set_left_position(42.);
        let log = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::clone(&log);
        let (_timer, state) = wire_with(
            &ui,
            Rc::new(move |_, _, _| Ok(Box::new(Fake(Rc::clone(&calls))))),
            Rc::new(|_| true),
        );
        assert!(log.borrow().is_empty());
        ui.invoke_show_browser(false);
        ui.invoke_show_browser(true);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(51));
        slint::platform::update_timers_and_animations();
        assert_eq!(
            log.borrow().as_slice(),
            [web_policy::HOME_URL, web_policy::HELP_URL]
        );
        let view = state.borrow()[0].as_ref().unwrap().view.clone_strong();
        view.set_address("file:///C:/secret".into());
        view.invoke_navigate();
        assert_eq!(log.borrow().len(), 2);
        for i in 0..50 {
            view.set_address(format!("https://example.org/{i}").into());
            view.invoke_navigate();
        }
        view.invoke_back();
        view.invoke_forward();
        view.invoke_reload();
        assert_eq!(log.borrow().len(), 55);
        assert_eq!(state.borrow()[1].as_ref().unwrap().view.get_address(), "");
        view.invoke_dismiss();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(51));
        slint::platform::update_timers_and_animations();
        assert!(state.borrow()[0].is_none());
        assert!(state.borrow()[1].is_some());
        ui.invoke_close_web_browsers();
        assert!(state.borrow().iter().all(Option::is_none));
        assert!(ui.get_left_playing());
        assert_eq!(ui.get_left_position(), 42.);
    }
    #[test]
    fn browser_events_update_navigation_status_and_route_popups_without_playback_changes() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.set_left_playing(true);
        ui.set_left_position(42.);
        let log = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::clone(&log);
        let sender: Rc<RefCell<Option<SyncSender<Event>>>> = Rc::new(RefCell::new(None));
        let capture = Rc::clone(&sender);
        let (_timer, state) = wire_with(
            &ui,
            Rc::new(move |_, _, tx| {
                *capture.borrow_mut() = Some(tx);
                Ok(Box::new(Fake(Rc::clone(&calls))))
            }),
            Rc::new(|_| true),
        );
        ui.invoke_show_browser(false);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(51));
        slint::platform::update_timers_and_animations();
        let view = state.borrow()[0].as_ref().unwrap().view.clone_strong();
        let send = |event| {
            assert!(sender.borrow().as_ref().unwrap().try_send(event).is_ok());
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(51));
            slint::platform::update_timers_and_animations();
        };
        send(Event::Started("https://example.org/loading".into()));
        assert_eq!(view.get_address(), "https://example.org/loading");
        assert!(state.borrow()[0].as_ref().unwrap().loading.is_some());
        send(Event::Finished("https://example.org/ended".into()));
        assert_eq!(view.get_address(), "https://example.org/ended");
        assert!(state.borrow()[0].as_ref().unwrap().loading.is_none());
        assert!(view
            .get_status()
            .contains("Navigation ended; inspect the page for errors"));
        send(Event::Popup(
            "https://example.org/popup?state=fixture".into(),
        ));
        assert_eq!(
            log.borrow().last().unwrap(),
            "https://example.org/popup?state=fixture"
        );
        let loads = log.borrow().len();
        send(Event::Blocked);
        assert!(view.get_status().contains("Navigation blocked"));
        assert_eq!(log.borrow().len(), loads);
        assert!(ui.get_left_playing());
        assert_eq!(ui.get_left_position(), 42.);
        ui.invoke_close_web_browsers();
    }
}
