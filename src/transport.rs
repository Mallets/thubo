//! Internal owned-batch boundary shared by readiness and completion transports.
use std::{future::Future, io, time::Duration};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{Bytes, pipeline::tx::WriteBatch, protocol::BatchSize};

pub(crate) trait BatchWriter: Send + 'static {
    fn write_batch(
        &mut self,
        batch: WriteBatch,
        timeout: Duration,
    ) -> impl Future<Output = io::Result<(WriteBatch, usize)>> + Send;

    /// Writes multiple already-ready batches, ideally within a single
    /// operation. Batches stay in the given order. The default implementation
    /// writes them sequentially via [`BatchWriter::write_batch`].
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

    // Direct completion transports must reap I/O cancelled by an outer timeout.
    fn cancel(&mut self) {}
}

pub(crate) trait BatchReader: Send + 'static {
    // An empty buffer is a keep-alive. EOF is an UnexpectedEof error.
    fn read_batch(&mut self, timeout: Duration) -> impl Future<Output = io::Result<Bytes>> + Send;

    // Completion transports must interrupt outstanding reads on cancellation.
    fn cancel(&mut self) {}
}

pub(crate) struct TokioWriter<W>(pub(crate) W);
pub(crate) struct TokioReader<R>(pub(crate) R);

impl<W: AsyncWriteExt + Send + Unpin + 'static> BatchWriter for TokioWriter<W> {
    async fn write_batch(&mut self, batch: WriteBatch, _timeout: Duration) -> io::Result<(WriteBatch, usize)> {
        let frame = batch.frame.as_slice();
        self.0.write_all(frame).await?;
        let mut written = frame.len();
        for slice in batch.fragment.slices() {
            self.0.write_all(slice).await?;
            written += slice.len();
        }
        self.0.flush().await?;
        Ok((batch, written))
    }

    async fn keep_alive(&mut self, _timeout: Duration) -> io::Result<()> {
        self.0.write_all(&BatchSize::MIN.to_le_bytes()).await
    }
}

impl<R: AsyncReadExt + Send + Unpin + 'static> BatchReader for TokioReader<R> {
    async fn read_batch(&mut self, timeout: Duration) -> io::Result<Bytes> {
        let mut header = [0; 2];
        tokio::time::timeout(timeout, self.0.read_exact(&mut header)).await??;
        let len = BatchSize::from_le_bytes(header) as usize;
        if len == 0 {
            return Ok(Bytes::new());
        }
        let mut buf = vec![0; len.next_power_of_two()];
        tokio::time::timeout(timeout, self.0.read_exact(&mut buf[..len])).await??;
        buf.truncate(len);
        Ok(Bytes::from(buf))
    }
}
