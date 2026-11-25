use super::*;
use crate::sampler::phrase::{Patterns, Take};
use crate::PadPatternWindow;
use std::time::Instant;

#[cfg(test)]
mod acceptance;

#[derive(Default)]
pub(super) struct State {
    view: Option<PadPatternWindow>,
    book: Option<Patterns>,
    take: Option<Take>,
    target: bool,
    write: Option<(Receiver<Result<(), String>>, Patterns)>,
    running: Option<Patterns>,
    monitor: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
    listing_dirty: bool,
    notice: String,
}
impl State {
    fn stop_preview(&mut self) {
        if let Some((owner, epoch)) = self.monitor.take() {
            if let Some(e) = owner.upgrade().filter(|e| e.preview_epoch() == epoch) {
                e.stop_tempo_preview();
            }
        }
    }
}
impl AudioController {
    pub(super) fn wire_pad_patterns(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_pad_patterns(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_patterns() {
                    c.sampler_status(e);
                }
            }
        });
    }
    pub fn with_midi_time(&self, at: Instant, apply: impl FnOnce()) {
        let previous = self.midi_onset.replace(Some(at));
        apply();
        self.midi_onset.set(previous);
    }
    pub(super) fn capture_pad_hit(&self, pad: u8, hardware: bool) {
        let mut st = self.pad_patterns.borrow_mut();
        if st.write.is_some() {
            return;
        }
        if let Some(t) = &mut st.take {
            let at = if hardware {
                self.midi_onset.get().unwrap_or_else(Instant::now)
            } else {
                Instant::now()
            };
            let n = t.pattern.hits.len();
            t.hit(pad, at);
            if n != t.pattern.hits.len() {
                st.listing_dirty = true;
            }
        }
    }
    pub(super) fn finish_pad_recording(&self) {
        let mut st = self.pad_patterns.borrow_mut();
        if let Some(t) = &mut st.take {
            if t.end.is_none() {
                t.end = Some(Instant::now());
                st.notice = "Take stopped. Hear it, replace the target or discard.".into();
            }
        }
    }
    pub(super) fn reset_pad_patterns(&self) {
        let mut st = self.pad_patterns.borrow_mut();
        st.stop_preview();
        self.engine.borrow().sampler().pattern.stop();
        if let Some(v) = st.view.take() {
            let _ = v.hide();
        }
        *st = State::default();
    }
    pub(super) fn finish_pattern_write(&self) -> Option<String> {
        let (rx, _) = self.pad_patterns.borrow_mut().write.take()?;
        rx.recv().map_err(|e| e.to_string()).and_then(|r| r).err()
    }
    pub(super) fn restore_pad_patterns(&self, book: Result<Patterns, String>) {
        let mut st = self.pad_patterns.borrow_mut();
        match book {
            Ok(book) => {
                st.book = Some(book);
                st.notice = "Choose a target to record a new take.".into();
            }
            Err(e) => {
                st.book = None;
                st.notice = format!("Saved sampler patterns unavailable: {e}");
            }
        }
        st.listing_dirty = true;
    }
    fn close_pattern_view(&self) {
        self.finish_pad_recording();
        let mut st = self.pad_patterns.borrow_mut();
        st.stop_preview();
        if let Some(v) = st.view.take() {
            let _ = v.hide();
        }
    }
    fn open_patterns(self: &Rc<Self>) -> Result<(), String> {
        if let Some(v) = &self.pad_patterns.borrow().view {
            return v.show().map_err(|e| e.to_string());
        }
        let v = PadPatternWindow::new().map_err(|e| e.to_string())?;
        macro_rules! action{
            ($callback:ident, $method:ident $(,$a:ident)*)=>{{
                let weak=Rc::downgrade(self);
                v.$callback(move|$($a),*|{if let Some(c)=weak.upgrade(){
                    if let Err(e)=c.$method($($a),*){c.pad_patterns.borrow_mut().notice=e;}
                    c.poll_pad_patterns();
                }});
            }}
        }
        action!(on_record_take, record_pattern, fill, bpm);
        action!(on_stop_recording, end_pattern);
        action!(on_discard_take, discard_pattern);
        action!(on_accept_take, accept_pattern);
        action!(on_audition_take, audition_pattern);
        action!(on_stop_preview, stop_pattern_preview);
        action!(on_start_pattern, start_saved_pattern);
        action!(on_stop_pattern, stop_saved_pattern);
        action!(on_arm_fill, arm_pattern_fill);
        action!(on_cancel_fill, cancel_pattern_fill);
        action!(on_trigger_pad, trigger_pattern_pad, pad);
        action!(on_select_target, select_pattern_target, fill);
        let weak = Rc::downgrade(self);
        v.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.close_pattern_view();
            }
        });
        let weak = Rc::downgrade(self);
        v.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                c.close_pattern_view();
            }
            slint::CloseRequestResponse::HideWindow
        });
        v.show().map_err(|e| e.to_string())?;
        let mut st = self.pad_patterns.borrow_mut();
        st.view = Some(v);
        st.listing_dirty = true;
        drop(st);
        self.poll_pad_patterns();
        Ok(())
    }
    fn select_pattern_target(&self, fill: bool) -> Result<(), String> {
        let mut st = self.pad_patterns.borrow_mut();
        if st.take.is_none() {
            st.target = fill;
            st.listing_dirty = true;
        }
        Ok(())
    }
    fn trigger_pattern_pad(&self, pad: i32) -> Result<(), String> {
        self.sampler_action(pad, 0, false)?;
        self.publish_sampler();
        Ok(())
    }
    fn record_pattern(&self, fill: bool, bpm: i32) -> Result<(), String> {
        let mut st = self.pad_patterns.borrow_mut();
        if st.book.is_none() || !self.storage_ready.get() {
            return Err("Wait for the saved patterns to load.".into());
        }
        if st.take.is_some() || st.write.is_some() {
            return Err("Finish or discard the current take first.".into());
        }
        st.stop_preview();
        st.target = fill;
        st.take = Some(Take::new(
            fill,
            bpm.try_into().map_err(|_| "Invalid tempo.")?,
            Instant::now(),
        )?);
        st.notice = "Recording pad onsets…".into();
        st.listing_dirty = true;
        Ok(())
    }
    fn end_pattern(&self) -> Result<(), String> {
        self.finish_pad_recording();
        Ok(())
    }
    fn discard_pattern(&self) -> Result<(), String> {
        let mut st = self.pad_patterns.borrow_mut();
        if st.write.is_some() {
            return Err("Wait for the target to finish saving.".into());
        }
        st.stop_preview();
        st.take = None;
        st.notice = "Take discarded.".into();
        st.listing_dirty = true;
        Ok(())
    }
    fn accept_pattern(&self) -> Result<(), String> {
        let mut st = self.pad_patterns.borrow_mut();
        if st.write.is_some() {
            return Err("Saving the selected target.".into());
        }
        let t = st
            .take
            .as_ref()
            .filter(|t| t.end.is_some() && !t.pattern.hits.is_empty())
            .ok_or("Finish a non-empty take first.")?;
        let book = st.book.as_ref().ok_or("Pattern storage unavailable.")?;
        let mut next = book.clone();
        if t.fill {
            next.fill = t.pattern.clone();
        } else {
            next.base = t.pattern.clone();
        }
        next.validate()?;
        let rx = self
            .persistence
            .borrow()
            .as_ref()
            .ok_or("Pattern storage unavailable.")?
            .save_patterns(book.clone(), next.clone())?;
        st.stop_preview();
        st.write = Some((rx, next));
        st.notice = "Saving the captured onsets…".into();
        Ok(())
    }
    fn audition_pattern(&self) -> Result<(), String> {
        let mut st = self.pad_patterns.borrow_mut();
        st.stop_preview();
        let t = st
            .take
            .as_ref()
            .filter(|t| t.end.is_some())
            .ok_or("End the recording before auditioning.")?;
        let engine = self.engine.borrow();
        if self.pending_device_switch.borrow().is_some() {
            return Err("Audio output is changing.".into());
        }
        engine.audition_phrase(&t.pattern, if t.fill { 1 } else { 4 })?;
        st.monitor = Some((Rc::downgrade(&engine), engine.preview_epoch()));
        st.notice = "Playing take in headphones.".into();
        Ok(())
    }
    fn stop_pattern_preview(&self) -> Result<(), String> {
        self.pad_patterns.borrow_mut().stop_preview();
        Ok(())
    }
    fn start_saved_pattern(&self) -> Result<(), String> {
        let mut st = self.pad_patterns.borrow_mut();
        if self.engine.borrow().output_failed() || self.pending_device_switch.borrow().is_some() {
            return Err("Sampler output is unavailable.".into());
        }
        let book = st.book.as_ref().ok_or("No saved pattern available.")?;
        let bank = self.engine.borrow().sampler();
        bank.pattern.start(book, &bank)?;
        st.running = Some(book.clone());
        Ok(())
    }
    fn stop_saved_pattern(&self) -> Result<(), String> {
        self.engine.borrow().sampler().pattern.stop();
        self.pad_patterns.borrow_mut().running = None;
        Ok(())
    }
    fn arm_pattern_fill(&self) -> Result<(), String> {
        let st = self.pad_patterns.borrow();
        let book = st.running.as_ref().ok_or("Start the base pattern first.")?;
        let bank = self.engine.borrow().sampler();
        if book.fill.hits.is_empty() {
            return Err("Accept a fill, then start the base pattern.".into());
        }
        if book
            .fill
            .hits
            .iter()
            .any(|h| !bank.ready(usize::from(h.pad)))
        {
            return Err("A pad used by the fill is unavailable.".into());
        }
        bank.pattern.arm()
    }
    fn cancel_pattern_fill(&self) -> Result<(), String> {
        self.engine.borrow().sampler().pattern.cancel_fill();
        Ok(())
    }
    pub(super) fn poll_pad_patterns(&self) {
        let mut st = self.pad_patterns.borrow_mut();
        if let Some((rx, _)) = &st.write {
            let result = match rx.try_recv() {
                Ok(r) => Some(r),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Sampler pattern writer stopped.".into()))
                }
            };
            if let Some(r) = result {
                let (_, next) = st.write.take().expect("pending save");
                match r {
                    Ok(()) => {
                        st.book = Some(next);
                        st.take = None;
                        st.notice =
                            "Target saved. The next pattern start uses this version.".into();
                    }
                    Err(e) => {
                        st.notice =
                            format!("Could not save target: {e}. The take is still available.");
                    }
                }
                st.listing_dirty = true;
            }
        }
        if let Some(t) = &mut st.take {
            if t.end.is_none() && t.start.elapsed().as_micros() >= u128::from(t.duration_us()) {
                t.end = Some(t.start + Duration::from_micros(t.duration_us()));
                st.notice = "Recording reached the end of the pattern.".into();
            }
        }
        let engine = self.engine.borrow();
        let route =
            engine.preview_route_available() && self.pending_device_switch.borrow().is_none();
        if st.monitor.as_ref().is_some_and(|(w, epoch)| {
            !route
                || w.upgrade()
                    .is_none_or(|e| !Rc::ptr_eq(&e, &engine) || e.preview_epoch() != *epoch)
        }) {
            st.stop_preview();
        }
        let bank = engine.sampler();
        if !bank.pattern.playing() {
            st.running = None;
        }
        let Some(v) = st.view.as_ref().map(ComponentHandle::clone_strong) else {
            return;
        };
        v.set_target_index(i32::from(st.target));
        v.set_recording(st.take.as_ref().is_some_and(|t| t.end.is_none()));
        v.set_draft(st.take.is_some());
        v.set_saving(st.write.is_some());
        v.set_ready(st.book.is_some() && self.storage_ready.get());
        v.set_has_headphones(route);
        v.set_can_accept(
            st.write.is_none()
                && st
                    .take
                    .as_ref()
                    .is_some_and(|t| t.end.is_some() && !t.pattern.hits.is_empty()),
        );
        v.set_has_base(st.book.as_ref().is_some_and(|b| !b.base.hits.is_empty()));
        v.set_playing(bank.pattern.playing());
        v.set_pending(bank.pattern.pending());
        v.set_fill_ready(st.running.as_ref().is_some_and(|b| !b.fill.hits.is_empty()));
        let status = if !bank.pattern.playing() {
            "Pattern stopped".to_owned()
        } else {
            format!(
                "Bar {} of 4 · {}",
                bank.pattern.bar() % 4 + 1,
                if bank.pattern.filling() {
                    "playing fill"
                } else if bank.pattern.pending() {
                    "fill queued"
                } else {
                    "base pattern"
                }
            )
        };
        v.set_transport(status.into());
        v.set_status(st.notice.as_str().into());
        if st.listing_dirty {
            let pattern = st
                .take
                .as_ref()
                .map(|t| &t.pattern)
                .or_else(|| st.book.as_ref().map(|b| b.target(st.target)));
            let rows = pattern
                .into_iter()
                .flat_map(|p| p.hits.iter())
                .map(|h| {
                    format!("Pad {} · {:.3} ms", h.pad + 1, f64::from(h.micros) / 1000.).into()
                })
                .collect::<Vec<slint::SharedString>>();
            v.set_rows(ModelRc::new(VecModel::from(rows)));
            st.listing_dirty = false;
        }
    }
}
