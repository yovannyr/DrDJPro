//! One installed fixed-format voice per deck. All ownership transfers happen between frames.
use rodio::Source;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, TryLockError,
    },
    time::Duration,
};

struct Transfer<T> {
    pending: Option<Box<T>>,
    returned: Option<Box<T>>,
}

struct Shared<T> {
    changed: AtomicBool,
    transfer: Mutex<Transfer<T>>,
}

pub(super) struct Input<T> {
    shared: Arc<Shared<T>>,
    channels: u16,
    rate: u32,
}

pub(super) struct Output<T> {
    shared: Arc<Shared<T>>,
    active: Option<Box<T>>,
    channel: u16,
    channels: u16,
    rate: u32,
}

pub(super) fn channel<T: Source + Send>(channels: u16, rate: u32) -> (Input<T>, Output<T>) {
    assert!(channels > 0 && rate > 0);
    let shared = Arc::new(Shared {
        changed: AtomicBool::new(false),
        transfer: Mutex::new(Transfer {
            pending: None,
            returned: None,
        }),
    });
    (
        Input {
            shared: Arc::clone(&shared),
            channels,
            rate,
        },
        Output {
            shared,
            active: None,
            channel: 0,
            channels,
            rate,
        },
    )
}

impl<T> Input<T> {
    fn publish(&self, incoming: Option<Box<T>>) {
        let discarded = {
            let mut transfer = self.shared.transfer.lock().unwrap();
            // Reserve the one return slot for this publication. Only the consumer fills it,
            // once, while clearing 'changed' under the same mutex.
            let returned = transfer.returned.take();
            let superseded = std::mem::replace(&mut transfer.pending, incoming);
            self.shared.changed.store(true, Ordering::Release);
            (returned, superseded)
        };
        drop(discarded);
    }

    pub(super) fn collect(&self) {
        let returned = self.shared.transfer.lock().unwrap().returned.take();
        drop(returned);
    }
}

impl<T: Source> Input<T> {
    pub(super) fn replace(&self, source: Option<T>) {
        if let Some(source) = &source {
            assert_eq!(source.channels(), self.channels);
            assert_eq!(source.sample_rate(), self.rate);
        }
        self.publish(source.map(Box::new));
    }
}

impl<T> Drop for Input<T> {
    fn drop(&mut self) {
        // An injected device graph can outlive the engine. Finish the current frame, then mute.
        self.publish(None);
    }
}

impl<T> Output<T> {
    fn receive(&mut self) {
        let mut transfer = match self.shared.transfer.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        // Both publication and consumption hold this mutex. The producer has taken the
        // previous return, so the swap below can never reclaim a voice in the callback.
        debug_assert!(transfer.returned.is_none());
        std::mem::swap(&mut self.active, &mut transfer.pending);
        transfer.returned = transfer.pending.take();
        self.shared.changed.store(false, Ordering::Release);
    }
}

impl<T: Source> Iterator for Output<T> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.channel == 0 && self.shared.changed.load(Ordering::Acquire) {
            self.receive();
        }
        let sample = self
            .active
            .as_mut()
            .and_then(|source| source.next())
            .unwrap_or(0.0);
        self.channel += 1;
        if self.channel == self.channels {
            self.channel = 0;
        }
        // Match the old per-deck mixer's sum starting at positive zero.
        Some(0.0 + sample)
    }
}

impl<T: Source> Source for Output<T> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl<T> Drop for Output<T> {
    fn drop(&mut self) {
        // Device teardown is outside next(). Release unclaimed sources even when the control
        // endpoint outlives the graph; SourceRetirement is the final field in each deck voice.
        let discarded = {
            let mut transfer = self.shared.transfer.lock().unwrap();
            (transfer.pending.take(), transfer.returned.take())
        };
        drop(discarded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_alloc::measured_heap;

    struct Voice {
        frame: [f32; 4],
        channel: usize,
        dropped: Arc<AtomicBool>,
    }

    impl Voice {
        fn new(frame: [f32; 4]) -> (Self, Arc<AtomicBool>) {
            let dropped = Arc::new(AtomicBool::new(false));
            (
                Self {
                    frame,
                    channel: 0,
                    dropped: Arc::clone(&dropped),
                },
                dropped,
            )
        }
    }
    impl Iterator for Voice {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            let value = self.frame[self.channel];
            self.channel = (self.channel + 1) % 4;
            Some(value)
        }
    }
    impl Source for Voice {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> u16 {
            4
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }
    impl Drop for Voice {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[test]
    fn replacement_and_eject_finish_frames_and_leave_heap_reclamation_to_control() {
        let (input, mut output) = channel(4, 48_000);
        assert_eq!((output.channels(), output.sample_rate()), (4, 48_000));
        let (skipped, skipped_drop) = Voice::new([9.; 4]);
        let (first, first_drop) = Voice::new([-0., 2., 3., 4.]);
        let (second, second_drop) = Voice::new([5., 6., 7., 8.]);
        input.replace(Some(skipped));
        input.replace(Some(first));
        assert!(skipped_drop.load(Ordering::Acquire));
        assert_eq!(output.next().unwrap().to_bits(), 0.0_f32.to_bits());
        input.replace(Some(second));
        let heap = measured_heap(|| {
            for expected in [2., 3., 4., 5.] {
                assert_eq!(output.next(), Some(expected));
            }
        });
        assert_eq!((heap.allocations, heap.deallocations), (0, 0));
        assert!(!first_drop.load(Ordering::Acquire));
        input.collect();
        assert!(first_drop.load(Ordering::Acquire));
        input.replace(None);
        let heap = measured_heap(|| {
            for expected in [6., 7., 8., 0., 0., 0., 0.] {
                assert_eq!(output.next(), Some(expected));
            }
        });
        assert_eq!((heap.allocations, heap.deallocations), (0, 0));
        assert!(!second_drop.load(Ordering::Acquire));
        input.collect();
        assert!(second_drop.load(Ordering::Acquire));
    }

    #[test]
    fn busy_publication_is_retried_at_the_next_frame_without_waiting() {
        let (input, mut output) = channel(4, 48_000);
        let (first, first_drop) = Voice::new([1., 2., 3., 4.]);
        input.replace(Some(first));
        let frame = |output: &mut Output<Voice>| std::array::from_fn(|_| output.next().unwrap());
        assert_eq!(frame(&mut output), [1., 2., 3., 4.]);
        let (second, second_drop) = Voice::new([5., 6., 7., 8.]);
        input.replace(Some(second));
        let guard = input.shared.transfer.lock().unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let while_locked = std::thread::scope(|scope| {
            scope.spawn(|| send.send(frame(&mut output)).unwrap());
            let frame = receive.recv_timeout(Duration::from_secs(3));
            drop(guard);
            frame
        });
        assert_eq!(while_locked.unwrap(), [1., 2., 3., 4.]);
        assert_eq!(frame(&mut output), [5., 6., 7., 8.]);
        // Publishing again also drains the previous return before reserving the slot anew.
        drop(input);
        assert!(first_drop.load(Ordering::Acquire));
        assert_eq!(frame(&mut output), [0.; 4]);
        assert!(!second_drop.load(Ordering::Acquire));
        drop(output);
        assert!(second_drop.load(Ordering::Acquire));
    }
}
