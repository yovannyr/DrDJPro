use super::*;

#[derive(Clone)]
pub(super) struct Cell {
    pub event: ChordEvent,
    pub confirmed: bool,
}

fn covering(events: &[ChordEvent], at: u64) -> Option<usize> {
    let index = events.partition_point(|e| e.end_ms <= at);
    events
        .get(index)
        .filter(|e| e.start_ms <= at)
        .map(|_| index)
}

pub(super) fn layer_cells(
    events: &[ChordEvent],
    saved: &[ChordEvent],
    start: u64,
    end: u64,
) -> Vec<Cell> {
    let mut bounds = vec![start, end];
    for e in events.iter().chain(saved) {
        if start < e.end_ms && e.start_ms < end {
            bounds.extend([e.start_ms.max(start), e.end_ms.min(end)]);
        }
    }
    bounds.sort_unstable();
    bounds.dedup();
    let mut cells: Vec<Cell> = Vec::new();
    let mut previous = None;
    for pair in bounds.windows(2) {
        let left = pair[0];
        let right = pair[1];
        let source = covering(saved, left)
            .map(|index| (true, index))
            .or_else(|| covering(events, left).map(|index| (false, index)));
        let Some((manual, index)) = source else {
            previous = None;
            continue;
        };
        if source == previous {
            if let Some(last) = cells.last_mut() {
                last.event.end_ms = right;
            }
            continue;
        }
        let mut e = if manual {
            saved[index].clone()
        } else {
            events[index].clone()
        };
        e.start_ms = left;
        e.end_ms = right;
        cells.push(Cell {
            event: e,
            confirmed: manual,
        });
        previous = source;
    }
    cells
}

#[derive(Clone)]
pub(super) struct Row {
    pub begin_us: u64,
    pub end_us: u64,
    pub cells: [Option<usize>; 2],
}

pub(super) fn rows(layers: &[Layer; 2], timing: PairTiming) -> Vec<Row> {
    let end = timing.length_ms * 1000;
    let mut bounds = vec![0, end];
    let intervals = std::array::from_fn::<_, 2, _>(|side| {
        let at = |ms| {
            (timing.mix_ms(side, ms) * 1000.)
                .round()
                .clamp(0., end as f64) as u64
        };
        layers[side]
            .cells
            .iter()
            .map(|c| {
                let segment = (at(c.event.start_ms), at(c.event.end_ms));
                bounds.extend([segment.0, segment.1]);
                segment
            })
            .collect::<Vec<_>>()
    });
    bounds.sort_unstable();
    bounds.dedup();
    bounds
        .windows(2)
        .map(|b| {
            let cells = std::array::from_fn(|side| {
                let cells = &intervals[side];
                let index = cells.partition_point(|(_, to)| *to <= b[0]);
                cells
                    .get(index)
                    .filter(|(from, _)| *from <= b[0])
                    .map(|_| index)
            });
            Row {
                begin_us: b[0],
                end_us: b[1],
                cells,
            }
        })
        .collect()
}

pub(super) fn settings(s: crate::ChordPairSettings) -> Result<PairTiming, String> {
    let number = |raw: &str| {
        raw.trim()
            .parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| "Enter finite decimal numbers for the comparison.".to_string())
    };
    let ms = |raw: &str| -> Result<u64, String> {
        let sec = number(raw)?;
        if !(0.0..=604_800.).contains(&sec) {
            return Err("Enter a source position between zero and seven days.".into());
        }
        Ok((sec * 1000.).round() as u64)
    };
    let offset = number(&s.offset_b)?;
    if offset.abs() > 16. {
        return Err("B offset must be within the 16-second comparison window.".into());
    }
    let pitch = |raw: &str| {
        raw.trim()
            .parse::<i32>()
            .map_err(|_| "Enter pitch in whole semitones.".to_string())
    };
    let time = PairTiming {
        starts_ms: [ms(&s.start_a)?, ms(&s.start_b)?],
        length_ms: ms(&s.length)?,
        offset_b_ms: (offset * 1000.).round() as i64,
        tempos: [number(&s.tempo_a)?, number(&s.tempo_b)?],
        pitches: [pitch(&s.pitch_a)?, pitch(&s.pitch_b)?],
    };
    time.validate()?;
    Ok(time)
}

pub(super) struct ModelRows {
    pub pair: Arc<Snapshot>,
    pub grids: [Option<crate::beatgrid::BeatGrid>; 2],
    pub notify: slint::ModelNotify,
}
impl Model for ModelRows {
    type Data = crate::ChordPairRow;
    fn row_count(&self) -> usize {
        self.pair.rows.len()
    }
    fn row_data(&self, index: usize) -> Option<Self::Data> {
        let row = self.pair.rows.get(index)?;
        let text = |side: usize| {
            let Some(cell) = row.cells[side].map(|i| &self.pair.layers[side].cells[i]) else {
                return slint::SharedString::from("No audio");
            };
            let event = &cell.event;
            let source = event.chord.as_deref().unwrap_or("Unknown");
            let mut label = source.to_string();
            let pitch = self.pair.timing.pitches[side];
            if pitch != 0 {
                if let Some(heard) = event
                    .chord
                    .as_ref()
                    .and_then(|c| chord_passage::transpose_chord(c, pitch))
                {
                    label = format!("{source} → {heard} heard");
                }
            }
            let grid = self.grids[side]
                .map(|g| {
                    let (bar, beat) = g.bar_beat_at_ms(event.start_ms);
                    format!(", bar {bar} beat {beat}")
                })
                .unwrap_or_default();
            format!(
                "{} · {}% · {} · {:.3}–{:.3} s{}",
                label,
                event.confidence,
                if cell.confirmed {
                    "confirmed"
                } else {
                    "estimate"
                },
                event.start_ms as f64 / 1000.,
                event.end_ms as f64 / 1000.,
                grid
            )
            .into()
        };
        Some(crate::ChordPairRow {
            time: format!(
                "{:.3}–{:.3} s",
                row.begin_us as f64 / 1e6,
                row.end_us as f64 / 1e6
            )
            .into(),
            a: text(0),
            b: text(1),
        })
    }
    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.notify
    }
}
