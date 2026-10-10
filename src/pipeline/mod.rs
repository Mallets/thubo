use crate::{Bytes, pipeline::tx::WriteBatch};

use std::{future::Future, io, time::Duration,sync::{
    LazyLock,
    atomic::{AtomicU16, Ordering},
}};

pub(super) mod ringbuf;
pub(crate) mod rx;
pub(crate) mod tx;

pub(super) static LOCAL_EPOCH: LazyLock<quanta::Instant> = LazyLock::new(quanta::Instant::now);

// Status
#[repr(transparent)]
pub(crate) struct Status(AtomicU16);

pub(crate) mod status {
    use crate::Priority;

    const CONGESTED: u16 = 1 << 0;
    const FLUSH: u16 = 1 << Priority::NUM;
    pub(super) const DISABLED: u16 = FLUSH << Priority::NUM;
    pub(super) const BACKOFF: u16 = DISABLED << 1;

    pub(crate) const fn congested(p: Priority) -> u16 {
        CONGESTED << p as u16
    }

    pub(crate) const fn flush(p: Priority) -> u16 {
        FLUSH << p as u16
    }

    pub(crate) const fn any(v: u16, f: u16) -> bool {
        v & f != 0
    }
}

impl Status {
    const fn new() -> Self {
        Self(AtomicU16::new(0))
    }

    fn set(&self, f: u16) -> u16 {
        self.0.fetch_or(f, Ordering::AcqRel)
    }

    fn unset(&self, f: u16) -> u16 {
        self.0.fetch_and(!f, Ordering::AcqRel)
    }

    pub(crate) fn any(&self, f: u16) -> bool {
        status::any(self.0.load(Ordering::Acquire), f)
    }
}


pub(crate) trait BatchWriter: Send + 'static {
    fn write_batch(
        &mut self,
        batch: WriteBatch,
        timeout: Duration,
    ) -> impl Future<Output = io::Result<(WriteBatch, usize)>> + Send;

    /// Writes multiple already-ready batches, ideally within a single
    /// vectored write. Batches stay in the given order. The default
    /// implementation writes them sequentially via [`BatchWriter::write_batch`].
    fn write_batches(
        &mut self,
        batches: Vec<WriteBatch>,
        timeout: Duration,
    ) -> impl Future<Output = io::Result<(Vec<WriteBatch>, usize)>> + Send {
        async move {
            let mut written = 0;
            let mut done = Vec::with_capacity(batches.len());
            for batch in batches {
                let (batch, n) = self.write_batch(batch, timeout).await?;
                written += n;
                done.push(batch);
            }
            Ok((done, written))
        }
    }

    fn keep_alive(&mut self, timeout: Duration) -> impl Future<Output = io::Result<()>> + Send;

    // Writers that must interrupt or reap in-flight data after an outer
    // write timeout override this.
    fn cancel(&mut self);
}

pub(crate) trait BatchReader: Send + 'static {
    // An empty buffer is a keep-alive. EOF is an UnexpectedEof error.
    fn read_batch(&mut self, timeout: Duration) -> impl Future<Output = io::Result<Bytes>> + Send;

    // Readers that must interrupt outstanding reads on cancellation
    // override this. The Tokio reader needs no cleanup.
    fn cancel(&mut self) {}
}
