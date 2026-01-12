use std::sync::Arc;

use super::common::{RingBuffer, RingBufferReader, RingBufferWriter, update};
use crate::protocol::Priority;

pub(crate) fn ringbuffer_spsc<T>(capacity: usize) -> (RingBufferWriterSPSC<T>, RingBufferReaderSPSC<T>) {
    let rb = Arc::new(RingBuffer::new(capacity));
    (
        RingBufferWriterSPSC(RingBufferWriter::new(rb.clone(), update::s)),
        RingBufferReaderSPSC(RingBufferReader::new(rb, update::s)),
    )
}

#[repr(transparent)]
pub(crate) struct RingBufferWriterSPSC<T>(RingBufferWriter<T>);

impl<T> RingBufferWriterSPSC<T> {
    pub(crate) fn push(&mut self, p: Priority, t: T) -> Option<T> {
        self.0.push(p, t)
    }
}

#[repr(transparent)]
pub(crate) struct RingBufferReaderSPSC<T>(RingBufferReader<T>);

impl<T> RingBufferReaderSPSC<T> {
    pub(crate) fn pull(&mut self) -> Option<T> {
        self.0.pull()
    }

    #[cfg(test)]
    pub(crate) fn peek(&mut self) -> Option<&T> {
        self.0.peek()
    }

    pub(crate) fn peek_mut(&mut self) -> Option<&mut T> {
        self.0.peek_mut()
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

        let (mut tx, mut rx) = ringbuffer_spsc::<usize>(16);

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
            while current < Priority::NUM * N {
                if let Some(c) = rx.peek() {
                    assert_eq!(*c, current);
                    let c = rx.peek_mut().unwrap();
                    assert_eq!(*c, current);
                    let c = rx.pull().unwrap();
                    assert_eq!(c, current);
                    current = current.wrapping_add(1);
                } else {
                    std::thread::yield_now();
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

        let (mut tx, rx) = ringbuffer_spsc::<DropCounter>(N);
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
        drop(rx);

        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            0,
            "All the drop counters should have been dropped"
        );
    }
}
