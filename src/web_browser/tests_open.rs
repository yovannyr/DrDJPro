use super::tests::Fake;
use super::*;

fn tick() {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(51));
    slint::platform::update_timers_and_animations();
}

#[test]
fn repeated_open_waits_for_native_windows_then_loads_each_home_once() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let ready = Rc::new(Cell::new(false));
    let builds = Rc::new(Cell::new(0));
    let pages = Rc::new(RefCell::new(Vec::new()));
    let (_timer, _sessions) = wire_with(
        &ui,
        Rc::new({
            let builds = builds.clone();
            let pages = pages.clone();
            move |_, _, _| {
                builds.set(builds.get() + 1);
                Ok(Box::new(Fake(pages.clone())))
            }
        }),
        Rc::new({
            let ready = ready.clone();
            move |_| ready.get()
        }),
    );

    ui.invoke_show_browser(true);
    tick();
    ui.invoke_show_browser(true);
    ui.invoke_show_browser(false);
    tick();
    assert_eq!(builds.get(), 0);
    assert!(pages.borrow().is_empty());

    ready.set(true);
    tick();
    ui.invoke_show_browser(true);
    ui.invoke_show_browser(false);
    tick();
    assert_eq!(builds.get(), 2);
    let mut loaded = pages.borrow().clone();
    loaded.sort();
    assert_eq!(loaded, [web_policy::HELP_URL, web_policy::HOME_URL]);
    ui.invoke_close_web_browsers();
}

#[test]
fn dismiss_before_native_parent_exists_cancels_only_that_window() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let ready = Rc::new(Cell::new(false));
    let builds = Rc::new(RefCell::new(Vec::new()));
    let (_timer, sessions) = wire_with(
        &ui,
        Rc::new({
            let builds = builds.clone();
            move |_, help, _| {
                builds.borrow_mut().push(help);
                Err("fixture runtime error".into())
            }
        }),
        Rc::new({
            let ready = ready.clone();
            move |_| ready.get()
        }),
    );
    ui.invoke_show_browser(true);
    ui.invoke_show_browser(false);
    let help = sessions.borrow()[1].as_ref().unwrap().view.clone_strong();
    help.invoke_dismiss();
    ready.set(true);
    tick();
    assert!(sessions.borrow()[1].is_none());
    assert_eq!(*builds.borrow(), [false]);

    ui.invoke_show_browser(true);
    tick();
    let help = sessions.borrow()[1].as_ref().unwrap().view.clone_strong();
    assert!(help.window().is_visible());
    assert!(help.get_startup_failed());
    assert!(help.get_status().contains("fixture runtime error"));
    tick();
    assert_eq!(*builds.borrow(), [false, true]);
    ui.invoke_close_web_browsers();
}

#[test]
fn replacement_opened_during_factory_initialization_gets_its_own_backend() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let obsolete = Rc::new(RefCell::new(Vec::new()));
    let replacement = Rc::new(RefCell::new(Vec::new()));
    let (_timer, _sessions) = wire_with(
        &ui,
        Rc::new({
            let weak = ui.as_weak();
            let first = Cell::new(true);
            let obsolete = obsolete.clone();
            let replacement = replacement.clone();
            move |_, _, _| {
                if first.replace(false) {
                    let ui = weak.upgrade().unwrap();
                    ui.invoke_close_web_browsers();
                    ui.invoke_show_browser(true);
                    Ok(Box::new(Fake(obsolete.clone())))
                } else {
                    Ok(Box::new(Fake(replacement.clone())))
                }
            }
        }),
        Rc::new(|_| true),
    );
    ui.invoke_show_browser(true);
    tick();
    tick();
    assert!(obsolete.borrow().is_empty());
    assert_eq!(*replacement.borrow(), [web_policy::HELP_URL]);
    ui.invoke_close_web_browsers();
}

#[test]
fn timeout_waiting_for_parent_leaves_error_visible_until_dismissed() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    let (_timer, sessions) = wire_with(
        &ui,
        Rc::new(|_, _, _| panic!("factory requires a native parent window")),
        Rc::new(|_| false),
    );
    ui.invoke_show_browser(true);
    let help = {
        let mut slots = sessions.borrow_mut();
        let session = slots[1].as_mut().unwrap();
        session.pending.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(11);
        session.view.clone_strong()
    };
    tick();
    assert!(help.window().is_visible());
    assert!(help.get_startup_failed());
    assert!(help.get_status().contains("erneut öffnen"));
    help.invoke_dismiss();
    tick();
    assert!(sessions.borrow()[1].is_none());
}
