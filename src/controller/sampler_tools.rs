use super::*;
use crate::sampler::tools::{LayerMix, LayerPair, Tuning};
use crate::source_monitor::analysis::note_name;
use crate::{playback::PreviewClip, SamplerToolsWindow};
mod tonal;

#[derive(Default)]
pub(super) struct State {
    view: Option<SamplerToolsWindow>,
    slot: usize,
    asset: Option<[u8; 32]>,
    generation: Option<u64>,
    preview: Option<(std::rc::Weak<DualDeckEngine>, u64)>,
    scale: tonal::ScaleState,
}
impl AudioController {
    pub(super) fn wire_sampler_tools(self: &Rc<Self>, ui: &AppWindow) {
        let weak = Rc::downgrade(self);
        ui.on_open_sampler_tools(move || {
            if let Some(c) = weak.upgrade() {
                if let Err(e) = c.show_sampler_tools() {
                    c.sampler_status(e);
                }
            }
        });
    }
    fn show_sampler_tools(self: &Rc<Self>) -> Result<(), String> {
        if let Some(view) = &self.sampler_tools.borrow().view {
            return view.show().map_err(|e| e.to_string());
        }
        let view = SamplerToolsWindow::new().map_err(|e| e.to_string())?;
        self.wire_tonal_pads(&view);
        let weak = Rc::downgrade(self);
        view.on_select_pad(move |i| {
            if let Some(c) = weak.upgrade() {
                if (0..4).contains(&i) {
                    c.sampler_tools.borrow_mut().slot = i as usize;
                    c.refresh_sampler_tools();
                }
            }
        });
        let weak = Rc::downgrade(self);
        view.on_confirm(move |note, cents| {
            if let Some(c) = weak.upgrade() {
                c.apply_pad_tuning(Some((note, cents)));
            }
        });
        let weak = Rc::downgrade(self);
        view.on_reset_tuning(move || {
            if let Some(c) = weak.upgrade() {
                c.apply_pad_tuning(None);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_apply_layers(move |choose| {
            if let Some(c) = weak.upgrade() {
                let msg = c
                    .apply_pad_layers(choose)
                    .err()
                    .unwrap_or_else(|| "Preparing both layers…".into());
                c.sampler_tool_message(msg);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_reference(move |note| {
            if let Some(c) = weak.upgrade() {
                c.audition_pad_reference(note);
            }
        });
        let weak = Rc::downgrade(self);
        view.on_dismiss(move || {
            if let Some(c) = weak.upgrade() {
                c.close_sampler_tools();
            }
        });
        let weak = Rc::downgrade(self);
        view.window().on_close_requested(move || {
            if let Some(c) = weak.upgrade() {
                c.close_sampler_tools();
            }
            slint::CloseRequestResponse::HideWindow
        });
        view.show().map_err(|e| e.to_string())?;
        self.sampler_tools.borrow_mut().view = Some(view);
        self.refresh_sampler_tools();
        Ok(())
    }
    pub(super) fn close_sampler_tools(&self) {
        let mut s = self.sampler_tools.borrow_mut();
        s.scale = Default::default();
        if let Some((engine, epoch)) = s.preview.take() {
            if let Some(engine) = engine.upgrade() {
                if engine.preview_epoch() == epoch {
                    engine.stop_tempo_preview();
                }
            }
        }
        if let Some(v) = s.view.take() {
            let _ = v.hide();
        }
        s.asset = None;
    }
    pub(super) fn sampler_tool_message(&self, msg: String) {
        if let Some(v) = &self.sampler_tools.borrow().view {
            v.set_status(msg.into());
        }
    }
    fn refresh_sampler_tools(&self) {
        let mut tools = self.sampler_tools.borrow_mut();
        let slot = tools.slot;
        let s = self.sampler.borrow();
        let observed = s.observed[slot].filter(|_| s.shared.source_token(slot).is_some());
        tools.asset = observed.map(|o| o.digest);
        tools.generation = s.shared.source_token(slot);
        let Some(v) = &tools.view else {
            return;
        };
        let tuning = s.preferences.tuning[slot]
            .as_ref()
            .filter(|x| Some(x.digest) == tools.asset);
        let suggested = observed.and_then(|x| x.pitch);
        let pair = s.preferences.layers[slot].as_ref();
        let [a, b] = pair.map_or([LayerMix::default(); 2], |p| p.mixes);
        v.set_layer_name(
            pair.map_or_else(|| format!("Pad {}", slot + 1), |p| p.name.clone())
                .into(),
        );
        v.set_second_source(pair.map_or_else(
            || "No second sample".into(),
            |p| p.second_path.display().to_string().into(),
        ));
        v.set_delay_a(i32::from(a.delay_ms));
        v.set_delay_b(i32::from(b.delay_ms));
        v.set_level_a(i32::from(a.level));
        v.set_level_b(i32::from(b.level));
        v.set_pan_a(i32::from(a.pan));
        v.set_pan_b(i32::from(b.pan));
        v.set_slot(slot as i32);
        v.set_source(
            s.preferences.paths[slot]
                .as_ref()
                .map_or_else(|| "Empty pad".into(), |p| p.display().to_string().into()),
        );
        v.set_reading(suggested.map_or_else(
            || "Fundamental unknown · set the root manually".into(),
            |p| {
                format!(
                    "Detected {} · MIDI {} · {:+} ct · {}% confidence",
                    note_name(p.midi),
                    p.midi,
                    p.cents,
                    p.confidence
                )
                .into()
            },
        ));
        v.set_root_note(
            tuning
                .map(|x| x.root)
                .or(suggested.map(|x| x.midi))
                .unwrap_or(69),
        );
        v.set_correction(
            tuning
                .map(|x| x.correction)
                .or(suggested.map(|x| -x.cents))
                .unwrap_or(0),
        );
        v.set_ready(observed.is_some() && s.writable);
        v.set_pfl(self.engine.borrow().preview_route_available());
        v.set_status(tuning.map_or_else(
            || "Concert reference A4 = 440 Hz".into(),
            |x| {
                format!(
                    "Confirmed {} · playback {:+} ct",
                    note_name(x.root),
                    x.correction
                )
                .into()
            },
        ));
    }
    pub(super) fn poll_sampler_tools(&self) {
        self.poll_tonal_pads();
        let tools = self.sampler_tools.borrow();
        let Some(view) = &tools.view else {
            return;
        };
        let s = self.sampler.borrow();
        let current = s.observed[tools.slot]
            .filter(|_| s.shared.source_token(tools.slot).is_some())
            .map(|x| x.digest);
        view.set_pfl(self.engine.borrow().preview_route_available());
        if tools.asset != current || tools.generation != s.shared.source_token(tools.slot) {
            drop(s);
            drop(tools);
            self.refresh_sampler_tools();
        }
    }
    fn apply_pad_layers(&self, choose: bool) -> Result<(), String> {
        let tools = self.sampler_tools.borrow();
        let slot = tools.slot;
        let v = tools.view.as_ref().ok_or("Pad tools closed")?;
        let mut s = self.sampler.borrow_mut();
        if !s.writable
            || s.job.is_some()
            || !s.restore.is_empty()
            || s.shared.source_token(slot) != tools.generation
            || tools.asset.is_none()
        {
            return Err("Wait for the current pad to be ready".into());
        }
        let base = s.preferences.paths[slot]
            .clone()
            .ok_or("Assign a first sample to the pad")?;
        let current = s.preferences.layers[slot]
            .as_ref()
            .map(|p| p.second_path.clone());
        if current.is_none() && !choose {
            return Err("Choose a second sample first".into());
        }
        let name = v.get_layer_name().trim().to_string();
        let mixes = [
            LayerMix::new(v.get_delay_a(), v.get_level_a(), v.get_pan_a())?,
            LayerMix::new(v.get_delay_b(), v.get_level_b(), v.get_pan_b())?,
        ];
        let picker = self.picker.clone();
        s.job = Some(crate::sampler::Load::layers(
            s.shared.clone(),
            slot,
            base,
            s.preferences.tuning[slot].clone(),
            move || {
                let path = if choose { picker() } else { current };
                let Some(second_path) = path else {
                    return Ok(None);
                };
                let pair = LayerPair {
                    name,
                    mixes,
                    second_path,
                };
                pair.validate()?;
                Ok(Some(pair))
            },
        )?);
        Ok(())
    }
    fn apply_pad_tuning(&self, entry: Option<(i32, i32)>) {
        let result = (|| {
            let tools = self.sampler_tools.borrow();
            let slot = tools.slot;
            let mut s = self.sampler.borrow_mut();
            let obs = s.observed[slot]
                .filter(|o| {
                    Some(o.digest) == tools.asset
                        && s.shared.source_token(slot) == tools.generation
                        && tools.generation.is_some()
                })
                .ok_or("Pad source changed. Select its current sample again.")?;
            if !s.writable || s.job.is_some() || !s.restore.is_empty() {
                return Err("Finish restoring a writable sampler bank first".to_owned());
            }
            let tuning = entry.map(|(root, correction)| Tuning {
                root,
                correction,
                digest: obs.digest,
            });
            if let Some(t) = &tuning {
                t.validate()?;
            }
            s.shared
                .set_pitch(slot, tuning.as_ref().map_or(0, |t| t.correction))?;
            if s.preferences
                .tonal
                .as_ref()
                .is_some_and(|l| usize::from(l.slot) == slot)
            {
                s.shared.use_layout(None)?;
                s.preferences.tonal = None;
            }
            s.preferences.tuning[slot] = tuning;
            Ok(())
        })();
        match result {
            Ok(()) => self.refresh_sampler_tools(),
            Err(e) => self.sampler_tool_message(e),
        }
    }
    fn audition_pad_reference(&self, note: i32) {
        let engine = self.engine.borrow();
        if !engine.preview_route_available() {
            self.sampler_tool_message("Choose a separate headphone pair (3/4).".into());
            return;
        }
        match PreviewClip::pad_reference(note).and_then(|clip| engine.play_sample_preview(&clip)) {
            Ok(()) => {
                self.sampler_tools.borrow_mut().preview =
                    Some((Rc::downgrade(&engine), engine.preview_epoch()));
                self.sampler_tool_message(format!("PFL · {}", note_name(note)));
            }
            Err(e) => self.sampler_tool_message(e),
        }
    }
}
