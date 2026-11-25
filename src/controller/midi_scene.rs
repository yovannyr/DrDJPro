use super::{set_planning::Lane, *};
use crate::{
    cancellation::Cancellation,
    midi_file::Note,
    midi_scene::{Event, Groove, Role, Scene},
    SceneWindow,
};
mod music;
mod scale;

enum Output {
    Loaded(Scene),
    Saved(PathBuf),
    Exported(PathBuf),
    Transcribed(Scene),
    Linked(Scene),
    Audio(crate::playback::PreviewClip),
    Loop(music::LoopAudio),
}
#[derive(Default)]
pub(super) struct State {
    view: Option<SceneWindow>,
    draft: Scene,
    undo: Option<Scene>,
    selected: Option<usize>,
    scale: scale::ScaleState,
    work: Lane<Output>,
    owner: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
    variants: Option<[Vec<Note>; 2]>,
    loop_control: Option<crate::playback::SceneLoopControl>,
    variant: u8,
}
fn role(i: i32) -> Option<Role> {
    match i {
        0 => Some(Role::Melody),
        1 => Some(Role::Accompaniment),
        2..=5 => Some(Role::Pad((i - 2) as u8)),
        _ => None,
    }
}
fn role_index(r: Role) -> i32 {
    match r {
        Role::Melody => 0,
        Role::Accompaniment => 1,
        Role::Pad(p) => i32::from(p) + 2,
    }
}
fn role_name(r: Role) -> String {
    match r {
        Role::Melody => "Melody".into(),
        Role::Accompaniment => "Accompaniment".into(),
        Role::Pad(i) => format!("Pad {}", i + 1),
    }
}
impl State {
    fn stop_audio(&mut self) {
        self.loop_control = None;
        if let Some((device, epoch)) = self.owner.take() {
            self.work.cancel();
            if let Some(e) = device.upgrade().filter(|e| e.preview_epoch() == epoch) {
                e.stop_tempo_preview();
            }
        }
    }
    fn replace(&mut self, draft: Scene, notice: &str) {
        if let Err(e) = draft.validate() {
            self.status(&e);
            return;
        }
        self.stop_audio();
        self.work.cancel();
        self.variants = None;
        self.undo = Some(std::mem::replace(&mut self.draft, draft));
        self.selected = self.selected.filter(|i| *i < self.draft.events.len());
        self.scale.reset(self.draft.key.as_deref());
        if let Some(v) = &self.view {
            let source = self.draft.source.as_ref();
            v.set_source_start(
                (source.map_or(0, |s| s.start_ms) as f64 / 1000.)
                    .to_string()
                    .into(),
            );
            v.set_source_length(
                (source.map_or(8000, |s| s.length_ms) as f64 / 1000.)
                    .to_string()
                    .into(),
            );
            v.set_source_kind(match source.map(|s| s.kind).unwrap_or_default() {
                crate::midi_scene::SourceKind::Original => 0,
                crate::midi_scene::SourceKind::Bass => 1,
                crate::midi_scene::SourceKind::Vocals => 2,
            });
        }
        self.publish();
        self.status(notice);
    }
    fn undo(&mut self) {
        if let Some(old) = self.undo.take() {
            self.replace(old, "Original scene restored.");
            self.undo = None;
            self.publish();
        }
    }
    fn status(&self, s: &str) {
        if let Some(v) = &self.view {
            v.set_status(s.into());
        }
    }
    fn publish(&self) {
        let Some(v) = &self.view else {
            return;
        };
        let t = self.draft.timing;
        v.set_bpm(t.bpm.to_string().into());
        v.set_numerator(t.numerator.into());
        v.set_denominator(t.denominator.into());
        v.set_bars(
            (u64::from(t.end_tick) * u64::from(t.denominator)
                / (u64::from(t.ppq) * 4 * u64::from(t.numerator)))
            .max(1) as i32,
        );
        v.set_key(self.draft.key.as_deref().unwrap_or("").into());
        v.set_mask(self.draft.groove.mask.into());
        v.set_intensity(self.draft.groove.intensity.into());
        v.set_max_ms(self.draft.groove.window_ms.into());
        v.set_source_label(
            self.draft
                .source
                .as_ref()
                .map(|s| {
                    format!(
                        "{} · {:.3}–{:.3} s",
                        s.path.display(),
                        s.start_ms as f64 / 1000.,
                        (s.start_ms + s.length_ms) as f64 / 1000.
                    )
                })
                .unwrap_or_default()
                .into(),
        );
        v.set_variants_ready(self.variants.is_some());
        v.set_can_undo(self.undo.is_some());
        v.set_selected(self.selected.map_or(-1, |i| i as i32));
        self.scale.publish(v);
        v.set_rows(ModelRc::new(VecModel::from(
            self.draft
                .rendered()
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    slint::SharedString::from(format!(
                        "{} · {} · note {} · tick {} (base {}) · {} ticks · velocity {}{}{}",
                        i + 1,
                        role_name(e.role),
                        e.note.pitch,
                        e.note.start,
                        self.draft.events[i].note.start,
                        e.note.duration,
                        e.note.velocity,
                        if e.uncertain { " · uncertain" } else { "" },
                        if e.approved {
                            " · approved"
                        } else {
                            " · draft"
                        }
                    ))
                })
                .collect::<Vec<_>>(),
        )));
        if let Some(e) = self.selected.and_then(|i| self.draft.events.get(i)) {
            v.set_role_index(role_index(e.role));
            v.set_start_tick(e.note.start.to_string().into());
            v.set_duration_tick(e.note.duration.to_string().into());
            v.set_pitch(e.note.pitch.into());
            v.set_velocity(e.note.velocity.into());
            v.set_approved(e.approved);
        }
    }
}
fn filename(v: &SceneWindow) -> Result<String, String> {
    let name = v.get_scene_name();
    let n = name.trim();
    if n.is_empty()
        || n.chars().count() > 64
        || n.ends_with('.')
        || n.chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || n == "."
        || n == ".."
    {
        return Err("Use a simple scene filename of up to 64 characters.".into());
    }
    Ok(n.to_string())
}
impl AudioController {
    pub(super) fn open_midi_scene(self: &Rc<Self>) -> Result<(), String> {
        if let Some(v) = &self.scene.borrow().view {
            return v.show().map_err(|e| e.to_string());
        }
        let v = SceneWindow::new().map_err(|e| e.to_string())?;
        self.wire_scene_scale(&v);
        self.wire_scene_music(&v);
        let w = Rc::downgrade(self);
        v.on_undo_scene(move || {
            if let Some(c) = w.upgrade() {
                c.scene.borrow_mut().undo();
            }
        });
        let w = Rc::downgrade(self);
        v.on_stop_scene(move || {
            if let Some(c) = w.upgrade() {
                c.scene.borrow_mut().stop_audio();
            }
        });
        let w = Rc::downgrade(self);
        v.on_choose_event(move |i| {
            if let Some(c) = w.upgrade() {
                let mut s = c.scene.borrow_mut();
                s.selected = usize::try_from(i)
                    .ok()
                    .filter(|i| *i < s.draft.events.len());
                s.publish();
            }
        });
        let w = Rc::downgrade(self);
        v.on_set_timing(move || {
            if let Some(c) = w.upgrade() {
                c.scene_timing();
            }
        });
        let w = Rc::downgrade(self);
        v.on_edit_event(move |mode| {
            if let Some(c) = w.upgrade() {
                c.edit_scene_event(mode);
            }
        });
        let w = Rc::downgrade(self);
        v.on_set_groove(move || {
            if let Some(c) = w.upgrade() {
                let mut s = c.scene.borrow_mut();
                if let Some(v) = &s.view {
                    let mut d = s.draft.clone();
                    d.groove = Groove {
                        mask: v.get_mask() as u8,
                        intensity: v.get_intensity() as u8,
                        window_ms: v.get_max_ms() as u16,
                    };
                    s.replace(d, "Groove applied to selected pad lanes.");
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_hear_groove(move || {
            if let Some(c) = w.upgrade() {
                let mut s = c.scene.borrow_mut();
                s.stop_audio();
                let e = c.engine.borrow();
                let result = s
                    .draft
                    .groove_pattern()
                    .and_then(|(p, b)| e.audition_phrase(&p, b));
                match result {
                    Ok(()) => {
                        s.owner = Some((Rc::downgrade(&e), e.preview_epoch()));
                        s.status("Sampler groove playing in headphones.");
                    }
                    Err(err) => s.status(&err),
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_file(move |op| {
            if let Some(c) = w.upgrade() {
                c.scene_file(op);
            }
        });
        let w = Rc::downgrade(self);
        v.on_dismiss(move || {
            if let Some(c) = w.upgrade() {
                c.close_scene();
            }
        });
        let w = Rc::downgrade(self);
        v.window().on_close_requested(move || {
            if let Some(c) = w.upgrade() {
                c.close_scene();
            }
            slint::CloseRequestResponse::HideWindow
        });
        v.show().map_err(|e| e.to_string())?;
        let mut s = self.scene.borrow_mut();
        s.view = Some(v);
        let key = s.draft.key.clone();
        s.scale.reset(key.as_deref());
        s.publish();
        Ok(())
    }
    pub(super) fn close_scene(&self) {
        let mut s = self.scene.borrow_mut();
        s.stop_audio();
        s.work.cancel();
        if let Some(v) = s.view.take() {
            let _ = v.hide();
        }
    }
    fn scene_timing(&self) {
        let mut s = self.scene.borrow_mut();
        let Some(v) = &s.view else {
            return;
        };
        let mut d = s.draft.clone();
        let Ok(bpm) = v.get_bpm().trim().parse::<f64>() else {
            s.status("Enter a BPM from 40 to 240.");
            return;
        };
        let n = v.get_numerator() as u8;
        let den = v.get_denominator() as u8;
        if den == 0 {
            return;
        }
        let key = v.get_key();
        if key.trim().is_empty() {
            d.key = None;
        } else {
            let Some(k) = crate::chord_passage::canonical_chord(key.trim()) else {
                s.status("Enter a major or minor key, for example C:maj or Dm.");
                return;
            };
            d.key = Some(k);
        }
        d.timing.bpm = bpm;
        d.timing.numerator = n;
        d.timing.denominator = den;
        d.timing.end_tick =
            u32::from(d.timing.ppq) * 4 * u32::from(n) * v.get_bars() as u32 / u32::from(den);
        d.tempo_confirmed = true;
        s.replace(d, "Scene timing and key confirmed.");
    }
    fn edit_scene_event(&self, op: i32) {
        let mut s = self.scene.borrow_mut();
        let Some(v) = &s.view else {
            return;
        };
        let mut d = s.draft.clone();
        if op == 2 {
            if let Some(i) = s.selected {
                d.events.remove(i);
            } else {
                return;
            }
        } else {
            let Some(r) = role(v.get_role_index()) else {
                return;
            };
            let (Ok(start), Ok(duration)) = (
                v.get_start_tick().trim().parse(),
                v.get_duration_tick().trim().parse(),
            ) else {
                s.status("Use whole MIDI ticks.");
                return;
            };
            let e = Event {
                role: r,
                note: Note {
                    start,
                    duration,
                    pitch: v.get_pitch() as u8,
                    velocity: v.get_velocity() as u8,
                    channel: match r {
                        Role::Melody => 0,
                        Role::Accompaniment => 1,
                        Role::Pad(_) => 9,
                    },
                },
                approved: v.get_approved(),
                uncertain: false,
            };
            if op == 0 {
                d.events.push(e);
            } else if op == 1 {
                if let Some(i) = s.selected {
                    d.events[i] = e;
                } else {
                    return;
                }
            } else {
                return;
            }
        }
        s.replace(d, "Scene events updated.");
    }
    fn scene_file(&self, mode: i32) {
        let mut s = self.scene.borrow_mut();
        if mode == 0 {
            s.replace(Scene::default(), "New scene.");
            return;
        }
        let Some(v) = &s.view else {
            return;
        };
        let name = match filename(v) {
            Ok(n) => n,
            Err(e) => {
                s.status(&e);
                return;
            }
        };
        let Some(picker) = self.library_folder_picker.borrow().clone() else {
            s.status("No folder picker available.");
            return;
        };
        let draft = s.draft.clone();
        s.stop_audio();
        s.work.submit(Box::new(move |c| {
            let dir = picker().ok_or("File operation cancelled.")?;
            c.check()?;
            let path = dir.join(format!("{name}.drdjscene.json"));
            if mode == 1 {
                Scene::read(&path, c).map(Output::Loaded)
            } else {
                draft.save(&path, c)?;
                Ok(Output::Saved(path))
            }
        }));
        s.status("Choose the folder containing your scene.");
    }
    pub(super) fn poll_scene(&self) {
        let mut s = self.scene.borrow_mut();
        let result = s.work.poll();
        let Some(v) = s.view.as_ref().map(ComponentHandle::clone_strong) else {
            return;
        };
        let e = self.engine.borrow();
        let route = e.preview_route_available() && self.pending_device_switch.borrow().is_none();
        v.set_route(route);
        if s.owner.as_ref().is_some_and(|(weak, epoch)| {
            !route
                || weak
                    .upgrade()
                    .is_none_or(|old| !Rc::ptr_eq(&old, &e) || old.preview_epoch() != *epoch)
        }) {
            s.stop_audio();
        }
        if let Some(result) = result {
            match result {
                Ok(Output::Loaded(d)) => s.replace(d, "Scene opened."),
                Ok(Output::Transcribed(d)) => s.replace(
                    d,
                    "Note draft ready. Inspect and approve the melody events.",
                ),
                Ok(Output::Linked(d)) => s.replace(d, "Source audio linked."),
                Ok(Output::Saved(p)) => s.status(&format!("Scene saved to {}", p.display())),
                Ok(Output::Exported(p)) => s.status(&format!("MIDI saved to {}", p.display())),
                Ok(Output::Audio(p)) if s.owner.is_some() => match e.play_sample_preview(&p) {
                    Ok(()) => {
                        s.owner = Some((Rc::downgrade(&e), e.preview_epoch()));
                        s.status("Playing in headphones.");
                    }
                    Err(err) => s.status(&err),
                },
                Ok(Output::Loop(p)) if s.owner.is_some() => {
                    match e.start_scene_loop(p.original, p.layers, p.rate, s.variant) {
                        Ok(handle) => {
                            s.owner = Some((Rc::downgrade(&e), e.preview_epoch()));
                            s.loop_control = Some(handle);
                            s.status("Loop playing. Switch accompaniment at the current musical position.");
                        }
                        Err(err) => s.status(&err),
                    }
                }
                Ok(_) => (),
                Err(err) => s.status(&err),
            }
        }
    }
}
