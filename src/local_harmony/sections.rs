use super::{spectrum::Chroma, KeySection, MAX_SECTIONS};
use crate::cancellation::Cancellation;

const MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
const CAMELOT: [u8; 12] = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];
const SWITCH_COST: f32 = 2.3;

struct Template {
    profile: Chroma,
    scale: Chroma,
}
fn templates() -> [Template; 24] {
    std::array::from_fn(|key| {
        let minor = key >= 12;
        let root = key % 12;
        let shape = if minor { MINOR } else { MAJOR };
        let mean = shape.iter().sum::<f32>() / 12.;
        let norm = shape.iter().map(|x| (x - mean).powi(2)).sum::<f32>().sqrt();
        let intervals = if minor {
            [0, 2, 3, 5, 7, 8, 10]
        } else {
            [0, 2, 4, 5, 7, 9, 11]
        };
        Template {
            profile: std::array::from_fn(|i| (shape[(i + 12 - root) % 12] - mean) / norm),
            scale: std::array::from_fn(|i| {
                f32::from(intervals.contains(&((i + 12 - root) % 12)) as u8)
            }),
        }
    })
}

fn chord(c: &Chroma) -> Option<usize> {
    let mut best = 0.;
    let mut second = 0.;
    let mut winner = None;
    for k in 0..24 {
        let t = k % 12;
        let third = (t + if k < 12 { 4 } else { 3 }) % 12;
        let fifth = (t + 7) % 12;
        let weight = c[t] + c[third] + c[fifth];
        if c[t].min(c[third]).min(c[fifth]) < 0.065 {
            continue;
        }
        if weight > best {
            second = best;
            best = weight;
            winner = Some(k);
        } else {
            second = f32::max(second, weight);
        }
    }
    winner.filter(|_| best >= 0.68 && best - second > 0.045)
}

/// Score every rotated Krumhansl–Kessler profile alongside scale coverage. A
/// dominant resolving to a heard tonic counts once at the actual chord transition.
fn scores(
    c: &Chroma,
    root: Option<usize>,
    previous: Option<usize>,
    keys: &[Template; 24],
) -> [f32; 24] {
    let spread = c
        .iter()
        .map(|v| (v - 1. / 12.).powi(2))
        .sum::<f32>()
        .sqrt()
        .max(1e-6);
    std::array::from_fn(|k| {
        let pattern = &keys[k];
        let mut coverage = 0.;
        let mut corr = 0.;
        for (i, &pitch) in c.iter().enumerate() {
            corr += pitch * pattern.profile[i];
            coverage += pitch * pattern.scale[i];
        }
        let tonic = if root == Some(k) { 0.16 } else { 0. };
        let cadence = if root == Some(k) && previous.is_some_and(|r| r % 12 == (k + 7) % 12) {
            0.85
        } else if root == Some(k) && previous.is_some_and(|r| r % 12 == (k + 5) % 12) {
            0.30
        } else {
            0.
        };
        1.8 * coverage + 0.34 * corr / spread + tonic + cadence
    })
}

fn best(scores: &[f32; 24]) -> usize {
    (1..24).fold(0, |winner, k| {
        if scores[k] > scores[winner] {
            k
        } else {
            winner
        }
    })
}

/// Find the highest-scoring sequence across one continuously tonal passage.
/// Every modulation pays the same transition penalty. Traceback assigns the boundary
/// to the first supported source interval, without adding confirmation time.
fn path(
    passage: &[Option<Chroma>],
    keys: &[Template; 24],
    cancel: &Cancellation,
) -> Result<Vec<u8>, String> {
    let mut trace = Vec::<[u8; 24]>::with_capacity(passage.len());
    let mut previous = [0.; 24];
    let mut prior_chord = None;
    for (i, c) in passage.iter().enumerate() {
        if i.is_multiple_of(128) {
            cancel.check()?;
        }
        let c = c.as_ref().expect("tonal passage");
        let root = chord(c);
        let evidence = scores(c, root, prior_chord, keys);
        prior_chord = root;
        let winner = best(&previous);
        let change = previous[winner] - SWITCH_COST;
        let mut parents = [0; 24];
        let next = std::array::from_fn(|k| {
            let parent = if previous[k] >= change { k } else { winner };
            parents[k] = parent as u8;
            previous[k].max(change) + evidence[k] - previous[winner]
        });
        trace.push(parents);
        previous = next;
    }
    let mut current = best(&previous) as u8;
    let mut labels = vec![0; trace.len()];
    for (label, parents) in labels.iter_mut().zip(trace).rev() {
        *label = current;
        current = parents[usize::from(current)];
    }
    Ok(labels)
}

fn confidence(
    passage: &[Option<Chroma>],
    key: usize,
    keys: &[Template; 24],
    cancel: &Cancellation,
) -> Result<u8, String> {
    if passage.len() < 6 {
        return Ok(0);
    }
    let mut pitches = [0.; 12];
    let mut accumulated = [0.; 24];
    let mut prior = None;
    let mut heard_tonic = false;
    for (i, c) in passage.iter().enumerate() {
        if i.is_multiple_of(128) {
            cancel.check()?;
        }
        let c = c.as_ref().expect("tonal passage");
        let root = chord(c);
        heard_tonic |= root == Some(key);
        for (sum, evidence) in accumulated.iter_mut().zip(scores(c, root, prior, keys)) {
            *sum += evidence;
        }
        prior = root;
        for (sum, pitch) in pitches.iter_mut().zip(c) {
            *sum += pitch;
        }
    }
    let count = passage.len() as f32;
    let distinct = pitches.iter().filter(|&&p| p >= 0.045 * count).count();
    let runner = accumulated
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != key)
        .map(|(_, v)| *v)
        .fold(f32::NEG_INFINITY, f32::max);
    let margin = (accumulated[key] - runner) / count;
    if distinct < 4
        || (!heard_tonic && distinct < 6)
        || margin < 0.025
        || accumulated[key] / count < 1.55
    {
        return Ok(0);
    }
    Ok((45. + margin * 155.).clamp(1., 99.).round() as u8)
}

fn append(sections: &mut Vec<KeySection>, section: KeySection) -> Result<(), String> {
    if let Some(last) = sections.last_mut() {
        if last.key == section.key && last.end_ms == section.start_ms {
            let weight_a = last.end_ms - last.start_ms;
            let weight_b = section.end_ms - section.start_ms;
            last.confidence = ((u64::from(last.confidence) * weight_a
                + u64::from(section.confidence) * weight_b)
                / (weight_a + weight_b)) as u8;
            last.end_ms = section.end_ms;
            return Ok(());
        }
    }
    if sections.len() >= MAX_SECTIONS {
        return Err("Local key profile exceeds 4096 intervals".into());
    }
    sections.push(section);
    Ok(())
}

pub(super) fn summarize(
    bins: &[Option<Chroma>],
    duration: u64,
    cancel: &Cancellation,
) -> Result<Vec<KeySection>, String> {
    let keys = templates();
    let mut out = Vec::new();
    let mut first = 0;
    let time = |i: usize| {
        if i == bins.len() {
            duration
        } else {
            (i as u64 * 500).min(duration)
        }
    };
    while first < bins.len() {
        cancel.check()?;
        let tonal = bins[first].is_some();
        let count = bins[first..]
            .iter()
            .take_while(|c| c.is_some() == tonal)
            .count();
        let end = first + count;
        if !tonal {
            append(
                &mut out,
                KeySection {
                    start_ms: time(first),
                    end_ms: time(end),
                    key: None,
                    confidence: 0,
                },
            )?;
        } else {
            let path = path(&bins[first..end], &keys, cancel)?;
            let mut start = 0;
            while start < path.len() {
                let label = path[start];
                let n = path[start..].iter().take_while(|v| **v == label).count();
                let k = usize::from(label);
                let confidence =
                    confidence(&bins[first + start..first + start + n], k, &keys, cancel)?;
                let key = (confidence > 0).then(|| {
                    if k >= 12 {
                        format!("{}A", CAMELOT[(k + 3) % 12])
                    } else {
                        format!("{}B", CAMELOT[k])
                    }
                });
                append(
                    &mut out,
                    KeySection {
                        start_ms: time(first + start),
                        end_ms: time(first + start + n),
                        key,
                        confidence,
                    },
                )?;
                start += n;
            }
        }
        first = end;
    }
    Ok(out)
}
