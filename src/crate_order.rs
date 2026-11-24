//! Prepare-only ordering. A fixed position protects an occurrence, even for repeated track IDs.
use drdjpro_ui_spike::analysis::keys::CamelotKey;

pub(crate) const MAX_ENTRIES: usize = 2048;

#[derive(Clone, Debug)]
pub(crate) struct OrderTrack {
    pub id: i32,
    pub name: String,
    pub bpm: Option<f64>,
    pub key: Option<CamelotKey>,
}

fn gap(a: &OrderTrack, b: &OrderTrack) -> (Option<f64>, Option<u8>) {
    let tempo = a.bpm.zip(b.bpm).and_then(|(x, y)| {
        (x.is_finite() && x > 0.0 && y.is_finite() && y > 0.0)
            .then(|| (x / y).log2().abs() * 1200.0)
    });
    let harmony = a.key.as_ref().zip(b.key.as_ref()).map(|(x, y)| {
        let d = x.number.abs_diff(y.number) % 12;
        d.min(12 - d) + u8::from(x.mode != y.mode && d != 0)
    });
    (tempo, harmony)
}

/// Difficulty is assessed on the source tempos; no implicit double-speed retiming is assumed.
pub(crate) fn difficulty(a: &OrderTrack, b: &OrderTrack) -> Option<String> {
    let (tempo, harmony) = gap(a, b);
    let mut reasons = Vec::new();
    match tempo {
        None => reasons.push("tempo unknown".to_owned()),
        Some(v) if v > 135.0 => reasons.push(format!(
            "tempo differs by {:.0}%",
            (2.0_f64.powf(v / 1200.0) - 1.0) * 100.0
        )),
        _ => {}
    }
    match harmony {
        None => reasons.push("key unknown".to_owned()),
        Some(v) if v > 1 => reasons.push("distant keys on the harmonic wheel".to_owned()),
        _ => {}
    }
    (!reasons.is_empty()).then(|| reasons.join(", "))
}

fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut x = *state;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Rank a bounded pool of permutations by their complete sequence, including transitions at
/// anchor boundaries. Use original occurrence indices so equal IDs never lose multiplicity.
pub(crate) fn variants(
    tracks: &[OrderTrack],
    fixed: &[bool],
    mut seed: u64,
) -> Result<Vec<Vec<i32>>, String> {
    if tracks.len() != fixed.len() || tracks.len() > MAX_ENTRIES {
        return Err("Invalid set length or anchor positions.".into());
    }
    let positions: Vec<usize> = (0..tracks.len()).filter(|&i| !fixed[i]).collect();
    let n = positions.len();
    if n < 2 {
        return Ok(Vec::new());
    }
    let original: Vec<_> = tracks.iter().map(|t| t.id).collect();
    let mut options: Vec<(f64, Vec<i32>)> = Vec::new();
    for pass in 0..20 {
        let mut free = positions.clone();
        match pass {
            0 => free.reverse(),
            1 => free.rotate_left(1),
            2 => free.rotate_right(1),
            3 => free.sort_by(|&a, &b| {
                tracks[a]
                    .bpm
                    .unwrap_or(0.)
                    .total_cmp(&tracks[b].bpm.unwrap_or(0.))
            }),
            4 => free.sort_by_key(|&a| tracks[a].key.as_ref().map(|k| k.number)),
            _ => {
                for i in (1..n).rev() {
                    let j = (next_random(&mut seed) % (i as u64 + 1)) as usize;
                    free.swap(i, j);
                }
            }
        }
        let mut order: Vec<usize> = (0..tracks.len()).collect();
        for (&index, src) in positions.iter().zip(free) {
            order[index] = src;
        }
        let ids: Vec<_> = order.iter().map(|&i| tracks[i].id).collect();
        if ids == original || options.iter().any(|(_, old)| old == &ids) {
            continue;
        }
        let cost = order
            .windows(2)
            .map(|w| {
                let (tempo, key) = gap(&tracks[w[0]], &tracks[w[1]]);
                tempo.unwrap_or(160.) / 100.0 + f64::from(key.unwrap_or(3))
            })
            .sum();
        options.push((cost, ids));
    }
    options.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(options.into_iter().take(3).map(|(_, ids)| ids).collect())
}

#[cfg(test)]
mod qualification {
    use super::*;
    #[test]
    fn r06_every_order_keeps_positions_one_seven_twelve_and_occurrences() {
        let tracks: Vec<_> = (1..=12)
            .map(|id| OrderTrack {
                id,
                name: id.to_string(),
                bpm: Some(if id == 4 { 165. } else { 120. }),
                key: CamelotKey::parse("8A"),
            })
            .collect();
        let fixed = std::array::from_fn::<_, 12, _>(|i| matches!(i, 0 | 6 | 11));
        let choices = variants(&tracks, &fixed, 4).unwrap();
        assert!(!choices.is_empty());
        for choice in choices {
            assert_eq!((choice[0], choice[6], choice[11]), (1, 7, 12));
            let mut sorted = choice;
            sorted.sort_unstable();
            assert_eq!(sorted, (1..=12).collect::<Vec<_>>());
        }
        assert!(variants(&tracks, &[true; 12], 4).unwrap().is_empty());
        assert!(difficulty(&tracks[2], &tracks[3])
            .unwrap()
            .contains("tempo"));
    }
}
