use std::{
    io::{self, IoSlice},
    time::Duration,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    Bytes,
    pipeline::{BatchReader, BatchWriter, tx::WriteBatch},
    protocol::BatchSize,
};

pub(crate) struct TokioWriter<W>(pub(crate) W);

impl<W: AsyncWriteExt + Send + Unpin + 'static> TokioWriter<W> {
    /// Linux limits a single writev to IOV_MAX (1024) iovecs.
    const MAX_IOV: usize = 1_024;

    /// Writes every slice with vectored writes, looping over partial writes.
    /// Writers without vectored support (e.g. TLS wrappers) transparently
    /// degrade to one write per slice.
    async fn write_all_slices(&mut self, slices: &mut [IoSlice<'_>]) -> io::Result<usize> {
        let total: usize = slices.iter().map(|s| s.len()).sum();
        let mut written = 0;
        let mut remaining: &mut [IoSlice<'_>] = slices;
        while !remaining.is_empty() {
            // The kernel accepts at most IOV_MAX iovecs per writev call.
            let end = remaining.len().min(Self::MAX_IOV);
            let mut window: &mut [IoSlice<'_>] = &mut remaining[..end];
            while !window.is_empty() {
                let n = self.0.write_vectored(window).await?;
                if n == 0 {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                IoSlice::advance_slices(&mut window, n);
                written += n;
            }
            remaining = &mut remaining[end..];
        }
        debug_assert_eq!(written, total);
        Ok(written)
    }

    /// Gathers the frame and fragment slices of every batch into one
    /// vectored write list: the length prefix, headers and payload chunks
    /// cross the socket in a single `writev(2)` instead of one syscall
    /// per slice.
    fn gather(batches: &[WriteBatch]) -> Vec<IoSlice<'_>> {
        let mut slices = Vec::new();
        for batch in batches {
            let frame = batch.frame.as_slice();
            if !frame.is_empty() {
                slices.push(IoSlice::new(frame));
            }
            for slice in batch.fragment.slices() {
                if !slice.is_empty() {
                    slices.push(IoSlice::new(slice));
                }
            }
        }
        slices
    }
}

impl<W: AsyncWriteExt + Send + Unpin + 'static> BatchWriter for TokioWriter<W> {
    async fn write_batch(&mut self, batch: WriteBatch, timeout: Duration) -> io::Result<(WriteBatch, usize)> {
        let (mut batches, written) = self.write_batches(vec![batch], timeout).await?;
        Ok((batches.pop().expect("one batch in, one batch out"), written))
    }

    async fn write_batches(
        &mut self,
        batches: Vec<WriteBatch>,
        _timeout: Duration,
    ) -> io::Result<(Vec<WriteBatch>, usize)> {
        let mut slices = Self::gather(&batches);
        let written = self.write_all_slices(&mut slices).await?;
        self.0.flush().await?;
        Ok((batches, written))
    }

    async fn keep_alive(&mut self, _timeout: Duration) -> io::Result<()> {
        self.0.write_all(&BatchSize::MIN.to_le_bytes()).await
    }

    fn cancel(&mut self) {
        // Tokio writers are cancellation safe.
    }
}

pub(crate) struct TokioReader<R>(pub(crate) R);

impl<R: AsyncReadExt + Send + Unpin + 'static> BatchReader for TokioReader<R> {
    async fn read_batch(&mut self, timeout: Duration) -> io::Result<Bytes> {
        let mut header = BatchSize::MIN.to_le_bytes();
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
