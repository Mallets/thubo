use std::sync::Arc;

use super::common::{RingBuffer, RingBufferReader, RingBufferWriter, update};
use crate::{
    pipeline::ringbuf::common::{Pull, WriterPeek},
    protocol::Priority,
};

pub(crate) fn ringbuffer_mpsc<T>(
    capacity: usize,
) -> ([RingBufferWriterMPSC<T>; Priority::NUM], RingBufferReaderMPSC<T>) {
    let rb = Arc::new(RingBuffer::new(capacity));
    let writers: [RingBufferWriterMPSC<T>; Priority::NUM] = Priority::ALL
        .into_iter()
        .map(|priority| RingBufferWriterMPSC {
            inner: RingBufferWriter::new(rb.clone(), update::m),
            priority,
        })
        .collect::<Vec<RingBufferWriterMPSC<T>>>()
        .try_into()
        .unwrap_or_else(|_| panic!("Priorities num do not match"));
    (writers, RingBufferReaderMPSC(RingBufferReader::new(rb, update::s)))
}

pub(crate) struct RingBufferWriterMPSC<T> {
    inner: RingBufferWriter<T>,
    priority: Priority,
}

impl<T> RingBufferWriterMPSC<T> {
    #[cfg(test)]
    pub(crate) fn push(&mut self, t: T) -> Option<T> {
        self.inner.push(self.priority, t)
    }

    pub(crate) fn book(&mut self, t: T) -> Result<WriterPeek<'_, T>, T> {
        self.inner.book(self.priority, t)
    }

    pub(crate) fn peek(&mut self) -> Option<WriterPeek<'_, T>> {
        self.inner.peek(self.priority)
    }
}

#[repr(transparent)]
pub(crate) struct RingBufferReaderMPSC<T>(RingBufferReader<T>);

impl<T> RingBufferReaderMPSC<T> {
    pub(crate) fn pull(&mut self) -> Option<T> {
        self.0.pull()
    }

    pub(crate) fn pull_priority(&mut self, p: Priority) -> Pull<T> {
        self.0.pull_priority(p)
    }

    #[cfg(test)]
    pub(crate) fn peek(&mut self) -> Option<&T> {
        self.0.peek()
    }

    #[cfg(test)]
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

        let (mut writers, mut rx) = ringbuffer_mpsc::<usize>(16);

        let p = std::thread::spawn(move || {
            let mut current: usize = 0;
            for tx in writers.iter_mut() {
                while current < (1 + tx.priority as usize) * N {
                    if tx.push(current).is_none() {
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

        let (mut writers, rx) = ringbuffer_mpsc::<DropCounter>(N);
        for tx in writers.iter_mut() {
            for _ in 0..N {
                assert!(tx.push(DropCounter::new()).is_none());
            }
            assert!(tx.push(DropCounter::new()).is_some());
        }

        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            N * Priority::NUM,
            "There should be as many counters as ringbuffer capacity"
        );

        // Drop both reader and writer
        drop(writers);
        drop(rx);

        assert_eq!(
            COUNTER.load(Ordering::SeqCst),
            0,
            "All the drop counters should have been dropped"
        );
    }
}
