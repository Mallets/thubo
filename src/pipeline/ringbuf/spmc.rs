use std::sync::Arc;

use super::common::{RingBuffer, RingBufferReader, RingBufferWriter, update};
use crate::{pipeline::ringbuf::common::Pull, protocol::Priority};

pub(crate) fn ringbuffer_spmc<T>(
    capacity: usize,
) -> (RingBufferWriterSPMC<T>, [RingBufferReaderSPMC<T>; Priority::NUM]) {
    let rb = Arc::new(RingBuffer::new(capacity));
    let readers: [RingBufferReaderSPMC<T>; Priority::NUM] = Priority::ALL
        .into_iter()
        .map(|priority| RingBufferReaderSPMC {
            inner: RingBufferReader::new(rb.clone(), update::m),
            priority,
        })
        .collect::<Vec<RingBufferReaderSPMC<T>>>()
        .try_into()
        .unwrap_or_else(|_| panic!("Priorities num do not match"));
    (RingBufferWriterSPMC(RingBufferWriter::new(rb, update::s)), readers)
}

#[repr(transparent)]
pub(crate) struct RingBufferWriterSPMC<T>(RingBufferWriter<T>);

impl<T> RingBufferWriterSPMC<T> {
    pub(crate) fn push(&mut self, p: Priority, t: T) -> Option<T> {
        self.0.push(p, t)
    }
}

pub(crate) struct RingBufferReaderSPMC<T> {
    inner: RingBufferReader<T>,
    priority: Priority,
}

impl<T> RingBufferReaderSPMC<T> {
    pub(crate) fn pull(&mut self) -> Option<T> {
        match self.inner.pull_priority(self.priority) {
            Pull::Some(t) => Some(t),
            Pull::Booked | Pull::None => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn peek(&mut self) -> Option<&T> {
        self.inner.peek_priority(self.priority)
    }

    #[cfg(test)]
    pub(crate) fn peek_mut(&mut self) -> Option<&mut T> {
        self.inner.peek_priority_mut(self.priority)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    // Elements arrive in order
    #[test]
    fn it_works() {
        const N: usize = 1_000_000;

        let (mut tx, mut readers) = ringbuffer_spmc::<usize>(16);

        let p = std::thread::spawn(move || {
            let mut current: usize = 0;
            for p in Priority::ALL {
                while current < (1 + p as usize) * N {
                    if tx.push(p, current).is_none() {
                        current = current.wrapping_add(1);
                    } else {
                        std::thread::yield_now();
                    }
                }
            }
        });

        let c = std::thread::spawn(move || {
            let mut current: usize = 0;
            for rx in readers.iter_mut() {
                let mut tmp = 0;
                while tmp < N {
                    if let Some(c) = rx.peek() {
                        assert_eq!(*c, current);
                        let c = rx.peek_mut().unwrap();
                        assert_eq!(*c, current);
                        let c = rx.pull().unwrap();
                        assert_eq!(c, current);
                        tmp = tmp.wrapping_add(1);
                        current = current.wrapping_add(1);
                    } else {
                        std::thread::yield_now();
                    }
                }
            }
        });

        p.join().unwrap();
        c.join().unwrap();
    }

    // Memory drop check
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct DropCounter;

    impl DropCounter {
        fn new() -> Self {
            COUNTER.fetch_add(1, Ordering::SeqCst);
            Self
        }
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            COUNTER.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn memcheck() {
        const N: usize = 128;

        let (mut tx, readers) = ringbuffer_spmc::<DropCounter>(N);
        for p in Priority::ALL {
            for _ in 0..N {
                assert!(tx.push(p, DropCounter::new()).is_none());
            }
            assert!(tx.push(p, DropCounter::new()).is_some());
        }

        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            N * Priority::NUM,
            "There should be as many counters as ringbuffer capacity"
        );

        // Drop both reader and writer
        drop(tx);
        drop(readers);

        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            0,
            "All the drop counters should have been dropped"
        );
    }
}
