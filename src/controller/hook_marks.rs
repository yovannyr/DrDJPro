use super::*;
use super::{local_keys::Version, set_planning::Lane};
use crate::{
    cancellation::Cancellation,
    library_store::hooks::{Mark, Op, Outcome},
    playback::PreviewClip,
    HookWindow,
};

#[derive(Default)]
pub(super) struct State {
    view: Option<HookWindow>,
    pin: Option<(i32, PathBuf)>,
    cues: Vec<Duration>,
    results: Vec<Mark>,
    selected: Option<usize>,
    query: String,
    pending: Option<Op>,
    rx: Option<Receiver<Result<Outcome, String>>>,
    audio: Lane<PreviewClip>,
    owner: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
}
impl State {
    fn stop(&mut self) {
        self.audio.cancel();
        if let Some((device, epoch)) = self.owner.take() {
            if let Some(e) = device.upgrade().filter(|e| e.preview_epoch() == epoch) {
                e.stop_tempo_preview();
            }
        }
    }
    fn mark(&self) -> Option<&Mark> {
        self.selected.and_then(|i| self.results.get(i))
    }
    fn busy(&self) -> bool {
        self.rx.is_some() || self.pending.is_some()
    }
    fn clear_selection(&mut self) {
        self.stop();
        self.selected = None;
        if let Some(v) = &self.view {
            v.set_selection(-1);
        }
    }
    fn queue(&mut self, req: Op) {
        if !self.busy() {
            self.pending = Some(req);
            if let Some(v) = &self.view {
                v.set_busy(true);
            }
        }
    }
}
fn model(rows: impl Iterator<Item = String>) -> ModelRc<slint::SharedString> {
    ModelRc::new(VecModel::from(rows.map(Into::into).collect::<Vec<_>>()))
}
fn secs(t: Duration) -> String {
    format!("{:.9}", t.as_secs_f64())
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}
fn clip(mark: &Mark, c: &Cancellation) -> Result<PreviewClip, String> {
    let before = Version::read(&mark.path, c)?;
    let (rate, count) = crate::playback::cue_trim::source_frames(&mark.path, c)?;
    let position = mark.at.as_nanos() * u128::from(rate) / 1_000_000_000;
    let frame = u64::try_from(position).map_err(|_| "Source frame outside supported range.")?;
    if frame >= count {
        return Err("The mark is outside the current audio file.".into());
    }
    let audio = crate::playback::cue_trim::audition(&mark.path, frame, c)?;
    if before != Version::read(&mark.path, c)? {
        return Err("Source changed during headphone preparation.".into());
    }
    Ok(audio)
}

impl AudioController {
    pub(super) fn open_hook_marks(self: &Rc<Self>) -> Result<(), String> {
        if let Some(view) = &self.hooks.borrow().view {
            return view.show().map_err(|e| e.to_string());
        }
        let view = HookWindow::new().map_err(|e| e.to_string())?;
        let weak = Rc::downgrade(self);
        view.on_open_practice(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.open_hook_practice() {
                    if let Some(v) = &c.hooks.borrow().view {
                        v.set_status(e.into());
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_use_library(move || {
            if let Some(c) = weak.upgrade() {
                c.use_hook_track();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_use_cue(move |i| {
            if let Some(c) = weak.upgrade() {
                let s = c.hooks.borrow();
                if let Some((v, p)) = s
                    .view
                    .as_ref()
                    .zip(usize::try_from(i).ok().and_then(|i| s.cues.get(i)))
                {
                    v.set_position(secs(*p).into());
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_search(move |q| {
            if let Some(c) = weak.upgrade() {
                let mut s = c.hooks.borrow_mut();
                if !s.busy() {
                    s.query = q.to_string();
                    s.clear_selection();
                    s.queue(Op::Search {
                        query: q.to_string(),
                        after: None,
                    });
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_next_page(move || {
            if let Some(c) = weak.upgrade() {
                let mut s = c.hooks.borrow_mut();
                if !s.busy() {
                    let query = s.query.clone();
                    let after = s.results.last().cloned();
                    s.clear_selection();
                    s.queue(Op::Search { query, after });
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_choose(move |i| {
            if let Some(c) = weak.upgrade() {
                c.choose_hook(i);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_save(move || {
            if let Some(c) = weak.upgrade() {
                c.store_hook(false);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_remove(move || {
            if let Some(c) = weak.upgrade() {
                c.store_hook(true);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_listen(move || {
            if let Some(c) = weak.upgrade() {
                c.audition_hook();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_stop(move || {
            if let Some(c) = weak.upgrade() {
                c.hooks.borrow_mut().stop();
            }
        });
        let weak = Rc::downgrade(self);
        view.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.close_hook_marks();
            }
        });
        let weak = Rc::downgrade(self);
        view.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                c.close_hook_marks();
            }
            slint::CloseRequestResponse::HideWindow
        });
        view.show().map_err(|e| e.to_string())?;
        self.hooks.borrow_mut().view = Some(view);
        self.use_hook_track();
        self.hooks.borrow_mut().queue(Op::Search {
            query: String::new(),
            after: None,
        });
        Ok(())
    }
    pub(super) fn close_hook_marks(&self) {
        let mut s = self.hooks.borrow_mut();
        s.stop();
        s.pending = None;
        s.rx = None;
        s.results.clear();
        s.pin = None;
        s.cues.clear();
        s.selected = None;
        s.query.clear();
        if let Some(v) = s.view.take() {
            let _ = v.hide();
        }
    }
    fn hook_pin(&self, s: &mut State, id: i32) -> bool {
        let lib = self.library.borrow();
        let Some(t) = lib.track_by_id(id) else {
            return false;
        };
        let Some(v) = &s.view else {
            return false;
        };
        s.pin = Some((id, t.path.clone()));
        v.set_title_label(if t.metadata.title.is_empty() {
            t.path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
                .into()
        } else {
            t.metadata.title.clone().into()
        });
        let cues = t
            .hot_cues
            .slots()
            .iter()
            .enumerate()
            .filter_map(|(i, c)| c.as_ref().map(|c| (i, c)))
            .collect::<Vec<_>>();
        s.cues = cues.iter().map(|(_, c)| c.position()).collect();
        v.set_cues(model(
            cues.iter()
                .map(|(i, c)| format!("Cue {} · {}", i + 1, c.name())),
        ));
        true
    }
    fn use_hook_track(&self) {
        let mut s = self.hooks.borrow_mut();
        if s.busy() {
            return;
        }
        s.clear_selection();
        let id = self.library.borrow().selected_id();
        if !self.hook_pin(&mut s, id) {
            s.pin = None;
            s.cues.clear();
            if let Some(v) = &s.view {
                v.set_cues(Default::default());
                v.set_title_label("Select a track in the library.".into());
            }
            return;
        }
        if let Some(v) = &s.view {
            v.set_term(Default::default());
            v.set_position("0".into());
            v.set_status("Enter a position or choose a saved cue.".into());
        }
    }
    fn choose_hook(&self, raw: i32) {
        let mut s = self.hooks.borrow_mut();
        if s.busy() {
            return;
        }
        let Some(index) = usize::try_from(raw).ok() else {
            return;
        };
        let Some(m) = s.results.get(index).cloned() else {
            return;
        };
        s.clear_selection();
        if !self.hook_pin(&mut s, m.track) || s.pin.as_ref().is_none_or(|(_, p)| *p != m.path) {
            if let Some(v) = &s.view {
                v.set_status("The catalog entry changed. Search again.".into());
            }
            return;
        }
        s.selected = Some(index);
        if let Some(v) = &s.view {
            v.set_selection(index as i32);
            v.set_position(secs(m.at).into());
            v.set_term(m.term.into());
        }
    }
    fn store_hook(&self, remove: bool) {
        let mut s = self.hooks.borrow_mut();
        if s.busy() {
            return;
        }
        let Some(v) = &s.view else {
            return;
        };
        let request = if remove {
            let Some(old) = s.mark() else {
                return;
            };
            Op::Delete(old.clone())
        } else {
            let Some((track, path)) = &s.pin else {
                v.set_status("Choose a track first.".into());
                return;
            };
            let Some(at) = v
                .get_position()
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(|s| Duration::try_from_secs_f64(s).ok())
            else {
                v.set_status("Enter non-negative seconds.".into());
                return;
            };
            Op::Save {
                original: s.mark().cloned(),
                track: *track,
                path: path.clone(),
                at,
                term: v.get_term().to_string(),
            }
        };
        s.stop();
        s.queue(request);
    }
    fn audition_hook(&self) {
        let mut s = self.hooks.borrow_mut();
        if s.busy() {
            return;
        }
        let Some(m) = s.mark().cloned() else {
            return;
        };
        s.stop();
        let engine = self.engine.borrow();
        if !engine.preview_route_available() || self.pending_device_switch.borrow().is_some() {
            return;
        }
        engine.stop_tempo_preview();
        s.owner = Some((Rc::downgrade(&engine), engine.preview_epoch()));
        s.audio.submit(Box::new(move |c| clip(&m, c)));
    }
    pub(super) fn poll_hook_marks(&self) {
        let mut s = self.hooks.borrow_mut();
        let audio = s.audio.poll();
        let Some(view) = s.view.as_ref().map(ComponentHandle::clone_strong) else {
            return;
        };
        let engine = self.engine.borrow();
        let route =
            engine.preview_route_available() && self.pending_device_switch.borrow().is_none();
        view.set_route(route);
        if s.owner.as_ref().is_some_and(|(weak, epoch)| {
            !route
                || weak
                    .upgrade()
                    .is_none_or(|old| !Rc::ptr_eq(&old, &engine) || old.preview_epoch() != *epoch)
        }) {
            s.stop();
        }
        if let (Some(audio), Some((_, epoch))) = (audio, s.owner.as_mut()) {
            match audio.and_then(|p| engine.play_sample_preview(&p)) {
                Ok(()) => {
                    *epoch = engine.preview_epoch();
                    view.set_status("Playing from the saved mark in headphones.".into());
                }
                Err(e) => {
                    s.owner = None;
                    view.set_status(e.into());
                }
            }
        }
        drop(engine);
        if let Some(rx) = &s.rx {
            let result = match rx.try_recv() {
                Ok(r) => Some(r),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("Library worker stopped.".into())),
            };
            if let Some(result) = result {
                s.rx = None;
                match result {
                    Ok(Outcome::Page(rows, more)) => {
                        s.results = rows;
                        view.set_rows(model(
                            s.results
                                .iter()
                                .map(|m| format!("{} · {} · {} s", m.title, m.term, secs(m.at))),
                        ));
                        view.set_more(more);
                        view.set_status(format!("{} marks on this page.", s.results.len()).into());
                    }
                    Ok(Outcome::Changed) => {
                        s.clear_selection();
                        let q = s.query.clone();
                        s.queue(Op::Search {
                            query: q,
                            after: None,
                        });
                        view.set_status("Saved.".into());
                    }
                    Err(e) => view.set_status(e.into()),
                }
            }
        }
        if s.pending.is_some() {
            if !self.storage_ready.get() || self.save_failed.get() {
                s.pending = None;
                view.set_status("Library storage is unavailable.".into());
            } else if self.saving.get() || self.dirty.get() {
                drop(s);
                self.save_library();
                return;
            } else if let Some(op) = s.pending.take() {
                match self
                    .persistence
                    .borrow()
                    .as_ref()
                    .ok_or("Library storage not ready.".to_owned())
                    .and_then(|w| w.hook_request(op))
                {
                    Ok(rx) => s.rx = Some(rx),
                    Err(e) => view.set_status(e.into()),
                }
            }
        }
        view.set_busy(s.busy());
    }
}

#[cfg(test)]
mod qualification {
    use super::*;
    #[test]
    fn r01_timestamped_audio_goes_only_to_private_output() {
        for (seconds, amplitude) in [(12, 8192_i16), (47, -4096_i16)] {
            let rate = 8000;
            let mut pcm = vec![0; (seconds + 1) * rate];
            pcm[seconds * rate..].fill(amplitude);
            let wav = crate::test_support::wav_samples(&pcm, rate as u32, 1);
            let marker = Mark {
                id: 1,
                track: 1,
                path: wav.path().to_owned(),
                title: "Hook".into(),
                term: "Word".into(),
                at: Duration::from_secs(seconds as u64),
            };
            let passage = clip(&marker, &Cancellation::default()).unwrap();
            for channels in [2, 4] {
                let (mixer, mut sink) = rodio::mixer::mixer(channels, 48_000);
                let engine = DualDeckEngine::from_mixer(mixer, channels, 48_000);
                let played = engine.play_sample_preview(&passage);
                if channels == 2 {
                    assert!(played.is_err());
                    continue;
                }
                played.unwrap();
                let mut frame = [0.; 4];
                for _ in 0..1024 {
                    for v in &mut frame {
                        *v = sink.next().unwrap();
                    }
                    assert_eq!(&frame[..2], &[0., 0.]);
                }
                let polarity = f32::from(amplitude).signum();
                assert!(frame[2] * polarity > 0.01 && frame[3] * polarity > 0.01);
            }
        }
    }
}
