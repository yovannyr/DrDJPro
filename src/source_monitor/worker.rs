use super::{analysis, Feed};
use crate::cancellation::Cancellation;
use std::{
    collections::VecDeque,
    sync::{atomic::Ordering, mpsc, Arc},
    time::{Duration, Instant},
};

pub(crate) struct Session {
    pub feed: Arc<Feed>,
    token: u64,
    stop: Cancellation,
    results: mpsc::Receiver<String>,
}
impl Session {
    pub(crate) fn start(feed: Arc<Feed>, source: u64) -> Result<Self, String> {
        let token = feed.select(source);
        let stop = Cancellation::default();
        let thread_stop = stop.clone();
        let thread_feed = Arc::clone(&feed);
        let (tx, results) = mpsc::sync_channel(1);
        let result = std::thread::Builder::new()
            .name("source-monitor".into())
            .spawn(move || {
                let mut recent = VecDeque::new();
                let mut previous = None;
                let mut rate = 0.;
                let mut last = Instant::now();
                let mut update = Instant::now();
                while !thread_stop.is_cancelled()
                    && thread_feed.selection.load(Ordering::Acquire) == token
                {
                    while let Some(block) = thread_feed.queue.pop() {
                        if thread_stop.is_cancelled()
                            || thread_feed.selection.load(Ordering::Acquire) != token
                        {
                            return;
                        }
                        if block.token != token {
                            continue;
                        }
                        let next_rate = f64::from(block.rate) / f64::from(block.stride);
                        if previous.is_some_and(|seq: u64| seq.wrapping_add(1) != block.seq)
                            || next_rate != rate
                        {
                            recent.clear();
                        }
                        previous = Some(block.seq);
                        rate = next_rate;
                        last = Instant::now();
                        // Select the stronger channel per block; phase-opposed stereo does not cancel.
                        let energies = block
                            .data
                            .iter()
                            .fold([0_f32; 2], |a, x| [a[0] + x[0] * x[0], a[1] + x[1] * x[1]]);
                        let ch = usize::from(energies[1] > energies[0]);
                        recent.extend(block.data.iter().map(|frame| frame[ch]));
                        let excess = recent.len().saturating_sub((rate * 8.25) as usize);
                        recent.drain(..excess);
                    }
                    if update.elapsed() >= Duration::from_millis(650) {
                        let msg = if last.elapsed() > Duration::from_secs(1) {
                            recent.clear();
                            previous = None;
                            "Silence · BPM unknown · key unknown · note unknown".to_owned()
                        } else if recent.is_empty() {
                            "Waiting for selected source".to_owned()
                        } else {
                            match analysis::describe(recent.make_contiguous(), rate, &thread_stop) {
                                Ok(v) => v,
                                Err(_) => break,
                            }
                        };
                        let _ = tx.try_send(msg);
                        update = Instant::now();
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
        if let Err(error) = result {
            feed.select(0);
            return Err(error.to_string());
        }
        Ok(Self {
            feed,
            token,
            stop,
            results,
        })
    }
    pub(crate) fn latest(&self) -> Option<String> {
        self.results.try_recv().ok()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop.request();
        let _ = self.feed.selection.compare_exchange(
            self.token,
            self.token & !3,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}
