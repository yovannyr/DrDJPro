use super::*;
use crate::{
    midi_scene::{Source, SourceKind},
    playback::PreviewClip,
};
use drdjpro_ui_spike::{media::stems::VoiceCategory, prepared_stems::PreparedStems};
pub(super) struct LoopAudio {
    pub(super) original: Arc<Vec<f32>>,
    pub(super) layers: [Arc<Vec<f32>>; 2],
    pub(super) rate: u32,
}

fn verified(source: &Source, c: &Cancellation) -> Result<(), String> {
    if crate::local_harmony::fingerprint(&source.path, c)? != source.digest {
        return Err("The linked source has changed. Link it again.".into());
    }
    Ok(())
}
fn read_source(source: &Source, c: &Cancellation) -> Result<(Vec<f32>, u32), String> {
    verified(source, c)?;
    let (pcm, rate) = match source.kind {
        SourceKind::Original => {
            let frames = usize::try_from(source.length_ms * 48)
                .map_err(|_| "Source duration exceeds the supported range.")?;
            let data = crate::playback::chord_preview::source_stereo(
                &source.path,
                source.start_ms,
                frames,
                c,
            )?;
            (data, 48000)
        }
        kind => {
            let mut file =
                crate::media_input::open_regular(&source.path).map_err(|e| e.to_string())?;
            let stems =
                PreparedStems::cached(&mut file, || c.is_cancelled()).map_err(|e| e.to_string())?;
            let mut data = Vec::with_capacity((source.length_ms * 44100 / 1000 * 2) as usize);
            stems
                .visit_window(
                    if kind == SourceKind::Bass {
                        VoiceCategory::Bass
                    } else {
                        VoiceCategory::Vocals
                    },
                    source.start_ms,
                    source.length_ms,
                    || c.is_cancelled(),
                    |chunk| {
                        data.extend_from_slice(chunk);
                        Ok(())
                    },
                )
                .map_err(|e| e.to_string())?;
            (data, 44100)
        }
    };
    if pcm.is_empty() {
        return Err("No audio inside that source interval.".into());
    }
    verified(source, c)?;
    Ok((pcm, rate))
}
fn synthesized(
    scene: &Scene,
    notes: &[Note],
    rate: u32,
    c: &Cancellation,
) -> Result<Vec<f32>, String> {
    scene.validate()?;
    let total = (scene.seconds() * f64::from(rate)).round() as usize;
    let mut out = vec![0f32; total * 2];
    let frames = |tick: u32| {
        (f64::from(tick) * 60. * f64::from(rate) / (scene.timing.bpm * f64::from(scene.timing.ppq)))
            .round() as usize
    };
    let mut budget = total.saturating_mul(32);
    for n in notes {
        c.check()?;
        let start = frames(n.start).min(total);
        let end = frames(n.start.saturating_add(n.duration)).min(total);
        let len = end.saturating_sub(start);
        budget = budget
            .checked_sub(len)
            .ok_or("Scene exceeds the headphone voice budget.")?;
        let hz = 440. * 2_f64.powf((f64::from(n.pitch) - 69.) / 12.);
        for (i, stereo) in out[start * 2..end * 2]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .enumerate()
        {
            if i.is_multiple_of(4096) {
                c.check()?;
            }
            let ramp = (i as f64 / (f64::from(rate) * 0.005))
                .min((len - 1 - i) as f64 / (f64::from(rate) * 0.012))
                .min(1.);
            let phase = std::f64::consts::TAU * hz * i as f64 / f64::from(rate);
            let v =
                ((phase.sin() + 0.15 * (2. * phase).sin()) * ramp * 0.13 * f64::from(n.velocity)
                    / 127.) as f32;
            stereo[0] += v;
            stereo[1] += v;
        }
    }
    for sample in &mut out {
        *sample = sample.clamp(-0.85, 0.85);
    }
    Ok(out)
}
fn selected_notes(s: &Scene, role: Role) -> Vec<Note> {
    s.rendered()
        .into_iter()
        .filter(|e| e.role == role && e.approved)
        .map(|e| e.note)
        .collect()
}
fn make_loop(scene: &Scene, layers: [Vec<Note>; 2], c: &Cancellation) -> Result<LoopAudio, String> {
    let source = scene
        .source
        .as_ref()
        .ok_or("Link the original vocal loop first.")?;
    let (mut original, rate) = read_source(source, c)?;
    let len = (scene.seconds() * f64::from(rate)).round() as usize * 2;
    if len * 3 * std::mem::size_of::<f32>() > 64 * 1024 * 1024 {
        return Err("Four-bar loop exceeds the headphone memory budget.".into());
    }
    original.resize(len, 0.);
    let [a, b] = layers;
    let left = synthesized(scene, &a, rate, c)?;
    let right = synthesized(scene, &b, rate, c)?;
    verified(source, c)?;
    Ok(LoopAudio {
        original: Arc::new(original),
        layers: [Arc::new(left), Arc::new(right)],
        rate,
    })
}
impl AudioController {
    pub(super) fn wire_scene_music(self: &Rc<Self>, v: &SceneWindow) {
        let w = Rc::downgrade(self);
        v.on_link_source(move || {
            if let Some(c) = w.upgrade() {
                c.link_scene_source(false);
            }
        });
        let w = Rc::downgrade(self);
        v.on_transcribe(move || {
            if let Some(c) = w.upgrade() {
                c.link_scene_source(true);
            }
        });
        let w = Rc::downgrade(self);
        v.on_original(move || {
            if let Some(c) = w.upgrade() {
                let src = c.scene.borrow().draft.source.clone();
                if let Some(source) = src {
                    if let Err(e) = c.start_scene_audio(Box::new(move |cancel| {
                        let (audio, rate) = read_source(&source, cancel)?;
                        PreviewClip::pad_sequence(audio, rate, cancel).map(Output::Audio)
                    })) {
                        c.scene.borrow().status(&e);
                    }
                } else {
                    c.scene.borrow().status("Link a source passage first.");
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_hear_notes(move || {
            if let Some(c) = w.upgrade() {
                let draft = c.scene.borrow().draft.clone();
                if let Err(e) = c.audition_scene_notes(&draft) {
                    c.scene.borrow().status(&e);
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_generate_variants(move || {
            if let Some(c) = w.upgrade() {
                c.make_scene_variants();
            }
        });
        let w = Rc::downgrade(self);
        v.on_hear_variant(move |i| {
            if let Some(c) = w.upgrade() {
                if let Err(e) = c.switch_scene_variant(i) {
                    c.scene.borrow().status(&e);
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_accept_variant(move |i| {
            if let Some(c) = w.upgrade() {
                let mut s = c.scene.borrow_mut();
                if let Some(notes) = usize::try_from(i)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| s.variants.as_ref()?.get(i))
                    .cloned()
                {
                    let mut scene = s.draft.clone();
                    scene.events.retain(|e| e.role != Role::Accompaniment);
                    scene.events.extend(notes.into_iter().map(|mut note| {
                        note.channel = 1;
                        Event {
                            note,
                            role: Role::Accompaniment,
                            approved: true,
                            uncertain: false,
                        }
                    }));
                    s.replace(scene, "Accompaniment adopted as a separate track.");
                }
            }
        });
        let w = Rc::downgrade(self);
        v.on_export_role(move |i| {
            if let Some(c) = w.upgrade() {
                c.export_scene_notes(i);
            }
        });
    }
    fn start_scene_audio(
        &self,
        work: super::super::set_planning::Task<Output>,
    ) -> Result<(), String> {
        let e = self.engine.borrow();
        if !e.preview_route_available() || self.pending_device_switch.borrow().is_some() {
            return Err("Select a separate headphone output first.".into());
        }
        let mut s = self.scene.borrow_mut();
        s.stop_audio();
        e.stop_tempo_preview();
        s.owner = Some((Rc::downgrade(&e), e.preview_epoch()));
        s.work.submit(work);
        s.status("Preparing headphone audio…");
        Ok(())
    }
    pub(super) fn audition_scene_notes(&self, draft: &Scene) -> Result<(), String> {
        let draft = draft.clone();
        let notes = draft
            .rendered()
            .into_iter()
            .filter(|e| e.approved && !matches!(e.role, Role::Pad(_)))
            .map(|e| e.note)
            .collect::<Vec<_>>();
        if notes.is_empty() {
            return Err("Approve a tonal event to audition it.".into());
        }
        self.start_scene_audio(Box::new(move |c| {
            PreviewClip::pad_sequence(synthesized(&draft, &notes, 48000, c)?, 48000, c)
                .map(Output::Audio)
        }))
    }
    fn link_scene_source(&self, transcribe: bool) {
        let mut s = self.scene.borrow_mut();
        let Some(v) = &s.view else {
            return;
        };
        let lib = self.library.borrow();
        let Some(t) = lib.track_by_id(lib.selected_id()) else {
            s.status("Choose an audio track in the main library.");
            return;
        };
        let parse = |v: &str| {
            v.trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && (0.0..=604800.).contains(v))
                .map(|v| (v * 1000.).round() as u64)
        };
        let (Some(start_ms), Some(length_ms @ 1..=60000)) =
            (parse(&v.get_source_start()), parse(&v.get_source_length()))
        else {
            s.status("Choose a non-negative start and up to 60 seconds of source audio.");
            return;
        };
        let kind = match v.get_source_kind() {
            0 => SourceKind::Original,
            1 => SourceKind::Bass,
            2 => SourceKind::Vocals,
            _ => return,
        };
        let path = t.path.clone();
        let mut scene = s.draft.clone();
        drop(lib);
        if transcribe && !scene.tempo_confirmed {
            s.status("Confirm the scene tempo before transcribing notes onto its grid.");
            return;
        }
        s.stop_audio();
        s.work.submit(Box::new(move |c| {
            let digest = crate::local_harmony::fingerprint(&path, c)?;
            let source = Source {
                path,
                start_ms,
                length_ms,
                digest,
                kind,
            };
            if transcribe {
                let t = scene.timing;
                let notes = if kind == SourceKind::Original {
                    crate::scene_transcription::transcribe(
                        &source.path,
                        start_ms,
                        length_ms,
                        t.bpm,
                        t.ppq,
                        c,
                    )?
                } else {
                    let (pcm, rate) = read_source(&source, c)?;
                    let mut mono = Vec::with_capacity(pcm.len() / 2);
                    for block in pcm.chunks((rate as usize / 4).max(1) * 2) {
                        c.check()?;
                        let mut energy = [0f64; 2];
                        for frame in block.as_chunks::<2>().0 {
                            for side in 0..2 {
                                energy[side] += f64::from(frame[side]).powi(2);
                            }
                        }
                        let channel = usize::from(energy[1] > energy[0]);
                        mono.extend(block.as_chunks::<2>().0.iter().map(|frame| frame[channel]));
                    }
                    crate::scene_transcription::transcribe_mono(&mono, rate, t.bpm, t.ppq, c)?
                };
                let source_ticks =
                    (length_ms as f64 * t.bpm * f64::from(t.ppq) / 60000.).round() as u32;
                scene.timing.end_tick = scene.timing.end_tick.max(source_ticks);
                scene.events.retain(|e| e.role != Role::Melody);
                for (note, uncertain) in notes {
                    scene.events.push(Event {
                        role: Role::Melody,
                        note,
                        approved: false,
                        uncertain,
                    });
                }
                verified(&source, c)?;
            } else {
                // Linking commits the same source interval that audio preview and transcription read.
                drop(read_source(&source, c)?);
            }
            scene.source = Some(source);
            scene.validate()?;
            Ok(if transcribe {
                Output::Transcribed(scene)
            } else {
                Output::Linked(scene)
            })
        }));
        s.status(if transcribe {
            "Transcribing the source passage…"
        } else {
            "Linking the source passage…"
        });
    }
    fn make_scene_variants(&self) {
        let mut s = self.scene.borrow_mut();
        let d = &s.draft;
        if !d.tempo_confirmed || d.key.is_none() {
            s.status("Confirm the key, tempo and four-bar time signature first.");
            return;
        }
        let key = d.key.as_deref().unwrap_or("");
        let Some(triad) = crate::chord_passage::midi_triad(key) else {
            s.status("Confirm a major or minor key.");
            return;
        };
        let music = selected_notes(d, Role::Melody);
        match crate::scene_bridge::variants(
            &music,
            d.timing,
            triad[0] % 12,
            triad[1] - triad[0] == 3,
        ) {
            Ok(variants) => {
                s.stop_audio();
                s.variants = Some(variants);
                s.publish();
                s.status("Bass and chord variants ready. Hear them against the linked audio.");
            }
            Err(e) => s.status(&e),
        }
    }
    fn switch_scene_variant(&self, i: i32) -> Result<(), String> {
        let variant = u8::try_from(i)
            .ok()
            .filter(|n| *n <= 2)
            .ok_or("Unknown accompaniment variant.")?;
        let mut s = self.scene.borrow_mut();
        s.variant = variant;
        if let Some(handle) = &s.loop_control {
            return handle.select(variant);
        }
        let layers = s
            .variants
            .clone()
            .ok_or("Generate accompaniment variants first.")?;
        let scene = s.draft.clone();
        drop(s);
        self.start_scene_audio(Box::new(move |c| {
            make_loop(&scene, layers, c).map(Output::Loop)
        }))
    }
    fn export_scene_notes(&self, i: i32) {
        let Some(role @ (Role::Melody | Role::Accompaniment)) = role(i) else {
            return;
        };
        let mut s = self.scene.borrow_mut();
        let d = s.draft.clone();
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
        let notes = selected_notes(&d, role);
        if role == Role::Accompaniment && notes.is_empty() {
            s.status("Choose a generated accompaniment before exporting.");
            return;
        }
        let Some(picker) = self.library_folder_picker.borrow().clone() else {
            s.status("No folder picker available.");
            return;
        };
        s.stop_audio();
        s.work.submit(Box::new(move |c| {
            let folder = picker().ok_or("MIDI export cancelled.")?;
            c.check()?;
            let path = folder.join(format!("{name}-{}.mid", role_name(role)));
            crate::midi_file::write(&path, d.timing, &notes, c)?;
            Ok(Output::Exported(path))
        }));
        s.status("Choose an export folder.");
    }
}

#[cfg(test)]
mod acceptance {
    use super::*;

    #[test]
    fn m03_link_selected_audio_commits_only_complete_passages() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let (mixer, _output) = rodio::mixer::mixer(4, 48000);
        let engine = Rc::new(DualDeckEngine::from_mixer(mixer, 4, 48000));
        let c = AudioController::wire(&ui, engine, Arc::new(|| None), Arc::new(|| None));
        let wav = crate::test_support::wav_samples(&[0; 1600], 8000, 1);
        let path = wav.path().canonicalize().unwrap();
        {
            let mut library = c.library.borrow_mut();
            library.insert([path.clone()]);
            let id = library.track_id_for_path(&path).unwrap();
            library.select(id);
        }
        c.open_midi_scene().unwrap();
        let view = c.scene.borrow().view.as_ref().unwrap().clone_strong();
        let link = |length: &str| {
            view.set_source_start("0.1".into());
            view.set_source_length(length.into());
            view.invoke_link_source();
            let limit = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                c.poll_scene();
                let status = view.get_status();
                if status.contains("ends before") || status.contains("Source audio linked") {
                    return status;
                }
                assert!(std::time::Instant::now() < limit, "{status}");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        assert!(link("0.2").contains("ends before"));
        assert!(c.scene.borrow().draft.source.is_none());
        assert!(link("0.1").contains("Source audio linked"));
        assert_eq!(
            c.scene.borrow().draft.source.as_ref().unwrap().length_ms,
            100
        );
        c.close_scene();
    }
}
