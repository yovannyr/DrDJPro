use super::*;
use crate::source_monitor::{self as audio, worker::Session};

#[derive(Default)]
pub(super) struct State {
    view: Option<crate::SourceMonitorWindow>,
    source: u64,
    worker: Option<Session>,
}

impl AudioController {
    pub(super) fn wire_source_monitor(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_source_monitor(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(error) = c.show_source_monitor() {
                    if let Some(ui) = c.ui.upgrade() {
                        ui.set_status(error.into());
                    }
                }
            }
        });
    }
    fn show_source_monitor(self: &Rc<Self>) -> Result<(), String> {
        if let Some(view) = &self.source_monitor.borrow().view {
            return view.show().map_err(|e| e.to_string());
        }
        let view = crate::SourceMonitorWindow::new().map_err(|e| e.to_string())?;
        let weak = Rc::downgrade(self);
        view.on_select_source(move |index| {
            if let Some(c) = weak.upgrade() {
                c.select_monitor_source(index);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.close_source_monitor();
            }
        });
        let weak = Rc::downgrade(self);
        view.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                c.close_source_monitor();
            }
            slint::CloseRequestResponse::HideWindow
        });
        view.show().map_err(|e| e.to_string())?;
        self.source_monitor.borrow_mut().view = Some(view);
        self.select_monitor_source(2);
        Ok(())
    }
    pub(super) fn close_source_monitor(&self) {
        let mut state = self.source_monitor.borrow_mut();
        state.worker.take();
        if let Some(view) = state.view.take() {
            let _ = view.hide();
        }
    }
    fn select_monitor_source(&self, index: i32) {
        let source = match index {
            0 => audio::DECK_A,
            1 => audio::DECK_B,
            2 => audio::MASTER,
            _ => return,
        };
        let feed = self.engine.borrow().source_monitor.clone();
        let mut state = self.source_monitor.borrow_mut();
        state.worker.take();
        state.source = source;
        let result = Session::start(feed, source);
        if let Some(view) = &state.view {
            view.set_selected(index);
            view.set_reading(
                result
                    .as_ref()
                    .map_or_else(|e| e.clone(), |_| "Waiting for selected source".to_owned())
                    .into(),
            );
        }
        state.worker = result.ok();
    }
    pub(super) fn poll_source_monitor(&self) {
        let state = self.source_monitor.borrow();
        let (Some(view), Some(worker)) = (&state.view, &state.worker) else {
            return;
        };
        if !Arc::ptr_eq(&worker.feed, &self.engine.borrow().source_monitor) {
            let index = state.source as i32 - 1;
            drop(state);
            self.select_monitor_source(index);
        } else if let Some(reading) = worker.latest() {
            view.set_reading(reading.into());
        }
    }
}
