//! Linux-only, owned-buffer TCP transport for throughput experiments.
//!
//! Enable the `io-uring` feature. Socket setup uses `std::net`; Thubo scheduling
//! remains on Tokio. Each I/O task owns its ring directly; AsyncFd integrates
//! completion readiness without a blocking thread or per-batch actor handoff.
//! RX reads ahead into
//! 128 KiB buffers and returns immutable batch views, retaining storage until all
//! consumers release it. This is not multishot, SQPOLL, or zero-copy send.
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! let socket = std::net::TcpStream::connect("127.0.0.1:9999")?;
//! let (mut sender, task) = thubo::uring::sender(socket)?.build_uring();
//! sender.send(thubo::Bytes::from("hello")).await.unwrap();
//! sender.stop().await;
//! drop(task.stop().await.unwrap());
//! # Ok(())
//! # }
//! ```
//!
//! Do not construct multiple writers or multiple readers for the same socket.
//! A reader and writer may share it via `TcpStream::try_clone()`. Dropping a
//! transport shuts down its socket direction; cancelling a read also shuts down
//! that direction. Read deadlines are enforced by a Tokio timer in `read_batch`;
//! write deadlines are enforced by the caller's Tokio timeout. Cancellation moves
//! in-flight storage into a cleanup task which reaps the outstanding completion
//! (issuing `AsyncCancel` if needed) before releasing it. Ring setup errors are
//! returned, never hidden by fallback. If completion cannot be established
//! (including runtime teardown), pending storage is intentionally leaked rather
//! than freed unsafely.
use std::{
    io::{self, Read},
    net::{Shutdown, TcpStream},
    os::fd::AsRawFd,
    time::Duration,
};

use io_uring::{IoUring, opcode, squeue, types};
use tokio::io::unix::AsyncFd;

use crate::{
    Bytes, ReceiverBuilder, SenderBuilder,
    api::{receiver_builder, sender_builder},
    pipeline::tx::WriteBatch,
    protocol::{BATCH_HEADER_BYTES, BatchSize, MAX_BATCH_DEFAULT},
    transport::{BatchReader, BatchWriter},
};

/// Owned-batch TCP writer. Construct with [`sender`].
pub struct Writer(Option<Driver>);

/// Owned-buffer TCP reader. Construct with [`receiver`].
pub struct Reader {
    driver: Option<Driver>,
    buffered: Bytes,
    /// Largest accepted batch length; larger frames are rejected as protocol
    /// violations. Buffering is chunk-based, so this never sizes an allocation.
    pub(crate) max_batch: usize,
}

/// Creates an io_uring sender builder for an already connected socket.
/// Construct inside a Tokio runtime with I/O enabled. Ring creation is fallible.
pub fn sender(socket: TcpStream) -> io::Result<SenderBuilder<Writer>> {
    Ok(sender_builder(Writer(Some(Driver::new(socket, Shutdown::Write)?))))
}

/// Creates an io_uring receiver builder for an already connected socket.
pub fn receiver(socket: TcpStream) -> io::Result<ReceiverBuilder<Reader>> {
    Ok(receiver_builder(Reader {
        driver: Some(Driver::new(socket, Shutdown::Read)?),
        buffered: Bytes::new(),
        max_batch: MAX_BATCH_DEFAULT,
    }))
}

async fn read_buffered(driver: &mut Driver, buffered: &mut Bytes, max_batch: usize) -> io::Result<Bytes> {
    loop {
        if buffered.len() >= BATCH_HEADER_BYTES {
            let mut header = [0; BATCH_HEADER_BYTES];
            buffered.reader().read_exact(&mut header)?;
            let len = BatchSize::from_le_bytes(header) as usize;
            if len > max_batch {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "batch length exceeds receiver limit",
                ));
            }
            if buffered.len() >= len + BATCH_HEADER_BYTES {
                let batch = buffered
                    .view(BATCH_HEADER_BYTES..len + BATCH_HEADER_BYTES)
                    .expect("complete batch");
                *buffered = buffered.view(len + BATCH_HEADER_BYTES..).expect("remaining stream");
                return Ok(batch);
            }
        }
        let bytes = driver.recv(vec![0; 128 * 1024]).await?;
        buffered.push(bytes.into());
    }
}

fn close(driver: &mut Option<Driver>) {
    if let Some(mut driver) = driver.take() {
        let _ = driver.socket.shutdown(driver.direction);
        if driver.in_flight {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    // Driver::Drop is the conservative fallback if this fails or
                    // the runtime aborts cleanup. Submitted storage stays owned.
                    let _ = driver.drain_pending().await;
                });
            }
            // Without a runtime Driver::Drop retains any kernel-referenced memory.
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        close(&mut self.0);
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        close(&mut self.driver);
    }
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "io_uring transport stopped")
}

impl BatchWriter for Writer {
    async fn write_batch(&mut self, batch: WriteBatch, _timeout: Duration) -> io::Result<(WriteBatch, usize)> {
        let (mut batches, written) = self.write_batches(vec![batch], Duration::ZERO).await?;
        Ok((batches.pop().expect("single batch returns single batch"), written))
    }

    async fn write_batches(
        &mut self,
        batches: Vec<WriteBatch>,
        _timeout: Duration,
    ) -> io::Result<(Vec<WriteBatch>, usize)> {
        match self.0.as_mut().ok_or_else(closed)?.send(Data::Batches(batches)).await? {
            (Data::Batches(batches), written) => Ok((batches, written)),
            _ => unreachable!("send returns its original data"),
        }
    }

    async fn keep_alive(&mut self, _timeout: Duration) -> io::Result<()> {
        self.0
            .as_mut()
            .ok_or_else(closed)?
            .send(Data::Buffer(vec![0; BATCH_HEADER_BYTES]))
            .await
            .map(|_| ())
    }

    fn cancel(&mut self) {
        close(&mut self.0);
    }
}

impl BatchReader for Reader {
    async fn read_batch(&mut self, timeout: Duration) -> io::Result<Bytes> {
        // Removing mpsc also removes its cooperative scheduling points.
        tokio::task::coop::consume_budget().await;
        // The caller-facing deadline is enforced here with a Tokio timer, so the
        // kernel does not arm and cancel an hrtimer for every operation.
        let result = tokio::time::timeout(
            timeout,
            read_buffered(
                self.driver.as_mut().ok_or_else(closed)?,
                &mut self.buffered,
                self.max_batch,
            ),
        )
        .await;
        match result {
            Ok(result) => result,
            // The deadline expired while an operation may still be in flight.
            // Interrupt the direction and reap its completion before returning.
            Err(_) => {
                self.cancel();
                Err(io::ErrorKind::TimedOut.into())
            }
        }
    }

    fn cancel(&mut self) {
        close(&mut self.driver);
    }
}

enum Data {
    Batches(Vec<WriteBatch>),
    Buffer(Vec<u8>),
}

// All kernel-referenced memory lives here, including descriptor storage. Box
// keeps the msghdr address stable.
struct Pending {
    data: Data,
    iovecs: Vec<libc::iovec>,
    message: libc::msghdr,
}

// SAFETY: all raw pointers refer to allocations owned by this Pending. Only
// its exclusive owning driver accesses it, and Box/Vec addresses remain stable
// when a Tokio task migrates between workers. Kernel access is completion-gated.
unsafe impl Send for Pending {}

impl Pending {
    fn new(data: Data) -> Box<Self> {
        // SAFETY: all-zero msghdr is valid; unused fields are null/zero.
        let message = unsafe { std::mem::zeroed() };
        Box::new(Self {
            data,
            iovecs: Vec::new(),
            message,
        })
    }

    fn prepare_send(&mut self) {
        let mut append = |slice: &[u8]| {
            if !slice.is_empty() {
                self.iovecs.push(libc::iovec {
                    iov_base: slice.as_ptr().cast_mut().cast(),
                    iov_len: slice.len(),
                });
            }
        };
        match &self.data {
            Data::Batches(batches) => {
                for batch in batches {
                    append(batch.frame.as_slice());
                    for slice in batch.fragment.slices() {
                        append(slice);
                    }
                }
            }
            Data::Buffer(buffer) => append(buffer),
        }
    }
}

struct Driver {
    ring: AsyncFd<IoUring>,
    socket: TcpStream,
    direction: Shutdown,
    pending: Option<Box<Pending>>,
    spare: Option<Box<Pending>>,
    in_flight: bool,
    outstanding: usize,
}

impl Drop for Driver {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(self.direction);
        if self.in_flight {
            // Closing a ring is NOT proof that all kernel buffer references have
            // disappeared. On an unrecoverable submission/wait error (or panic),
            // leak the one pending operation rather than risk use-after-free.
            // Normal errors, timeouts, and cancellations reap CQEs and do not leak.
            if let Some(pending) = self.pending.take() {
                std::mem::forget(pending);
            }
        }
    }
}

impl Driver {
    fn new(socket: TcpStream, direction: Shutdown) -> io::Result<Self> {
        tokio::runtime::Handle::try_current()
            .map_err(|_| io::Error::other("io_uring transport requires a Tokio runtime"))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            ring: AsyncFd::new(IoUring::new(8)?)?,
            socket,
            direction,
            pending: None,
            spare: None,
            in_flight: false,
            outstanding: 0,
        })
    }

    async fn drain_pending(&mut self) -> io::Result<()> {
        if !self.in_flight {
            return Ok(());
        }
        // Guarantee the outstanding operation's CQE, whether it is queued,
        // executing, or already complete but unreaped. AsyncCancel posts a CQE
        // for the targeted user_data in every case.
        let cancel = opcode::AsyncCancel::new(1).build().user_data(2);
        if unsafe { self.ring.get_mut().submission().push(&cancel) }.is_err() {
            return Err(io::Error::other("io_uring submission queue full"));
        }
        // Also submits any entries left by an interrupted operation.
        loop {
            match self.ring.get_mut().submit() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => {
                    result?;
                    break;
                }
            }
        }
        loop {
            for cqe in self.ring.get_mut().completion() {
                if cqe.user_data() == 1 {
                    // The submitted operation's completion is now observed;
                    // only now may its storage be reclaimed. Other CQEs (the
                    // cancel itself) reference no user memory.
                    self.outstanding = 0;
                    self.in_flight = false;
                    return Ok(());
                }
            }
            let mut readiness = self.ring.readable().await?;
            readiness.clear_ready();
        }
    }

    fn install(&mut self, data: Data) {
        self.pending = Some(match self.spare.take() {
            Some(mut pending) => {
                pending.data = data;
                pending.iovecs.clear();
                pending
            }
            None => Pending::new(data),
        });
    }

    fn retire(&mut self) -> Data {
        let mut pending = self.pending.take().unwrap();
        let data = std::mem::replace(&mut pending.data, Data::Buffer(Vec::new()));
        self.spare = Some(pending);
        data
    }

    /// Runs one socket operation, reaping its CQE. On a wait failure, pending
    /// remains retained and Drop conservatively leaks it. All pointers in entry
    /// refer to that owned pending allocation.
    async fn complete(&mut self, entry: squeue::Entry) -> io::Result<usize> {
        tokio::task::coop::consume_budget().await;
        self.pending
            .as_mut()
            .expect("owned operation installed before submission");
        let entry = entry.user_data(1);
        // SAFETY: pending owns all referenced memory at stable addresses. It is
        // never mutated or reclaimed until the CQE arrives. Its exceptional
        // Drop path leaks it if completion cannot be established.
        if unsafe { self.ring.get_mut().submission().push(&entry) }.is_err() {
            return Err(io::Error::other("io_uring submission queue full"));
        }
        self.in_flight = true;
        self.outstanding = 1;
        let mut result = None;
        loop {
            match self.ring.get_mut().submit() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => {
                    result?;
                    break;
                }
            }
        }
        while self.outstanding != 0 {
            for cqe in self.ring.get_mut().completion() {
                self.outstanding -= 1;
                // A single operation is in flight; its user_data is 1.
                debug_assert_eq!(cqe.user_data(), 1);
                result = Some(cqe.result());
            }
            if self.outstanding != 0 {
                let mut readiness = self.ring.readable().await?;
                readiness.clear_ready();
            }
        }
        self.in_flight = false;
        match result {
            Some(n) if n >= 0 => Ok(n as usize),
            Some(n) => Err(io::Error::from_raw_os_error(-n)),
            None => Err(io::Error::other("missing io_uring completion")),
        }
    }

    async fn send(&mut self, data: Data) -> io::Result<(Data, usize)> {
        if self.in_flight {
            // Starting a new operation would install storage over memory the
            // kernel may still reference. Callers must cancel and drain first.
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "io_uring operation already in flight",
            ));
        }
        self.install(data);
        self.pending.as_mut().unwrap().prepare_send();
        let mut first = 0;
        let mut written = 0;
        loop {
            let pending = self.pending.as_mut().unwrap();
            if first == pending.iovecs.len() {
                break;
            }
            pending.message.msg_iov = pending.iovecs[first..].as_mut_ptr();
            pending.message.msg_iovlen = (pending.iovecs.len() - first).min(1024); // Linux UIO_MAXIOV.
            let entry = opcode::SendMsg::new(types::Fd(self.socket.as_raw_fd()), &pending.message)
                .flags(libc::MSG_NOSIGNAL as u32)
                .build();
            let n = self.complete(entry).await?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            written += n;
            advance(&mut self.pending.as_mut().unwrap().iovecs, &mut first, n);
        }
        Ok((self.retire(), written))
    }

    async fn recv(&mut self, buffer: Vec<u8>) -> io::Result<Vec<u8>> {
        if self.in_flight {
            // Same hazard as send: never overwrite kernel-referenced storage.
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "io_uring operation already in flight",
            ));
        }
        self.install(Data::Buffer(buffer));
        let Data::Buffer(buffer) = &mut self.pending.as_mut().unwrap().data else {
            unreachable!("read owns a Vec");
        };
        let entry = opcode::Recv::new(
            types::Fd(self.socket.as_raw_fd()),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
        .build();
        let n = self.complete(entry).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        match self.retire() {
            Data::Buffer(mut buffer) => {
                buffer.truncate(n);
                Ok(buffer)
            }
            Data::Batches(_) => unreachable!("read returns the original owned Vec"),
        }
    }
}

fn advance(iovecs: &mut [libc::iovec], first: &mut usize, mut sent: usize) {
    while sent > 0 {
        let iovec = &mut iovecs[*first];
        if sent >= iovec.iov_len {
            sent -= iovec.iov_len;
            *first += 1;
        } else {
            // SAFETY: sent is strictly within this retained slice.
            iovec.iov_base = unsafe { iovec.iov_base.cast::<u8>().add(sent).cast() };
            iovec.iov_len -= sent;
            sent = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_read_retains_storage_until_all_completions_are_reaped() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let _peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        let mut driver = match Driver::new(socket, Shutdown::Read) {
            Ok(driver) => driver,
            Err(error) => {
                assert!(std::env::var_os("THUBO_REQUIRE_IO_URING").is_none(), "{error}");
                eprintln!("SKIP: ring unavailable: {error}");
                return;
            }
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(10), driver.recv(vec![0; 128]))
                .await
                .is_err()
        );
        assert!(driver.in_flight);
        assert_eq!(driver.outstanding, 1);
        assert!(driver.pending.is_some());
        driver.socket.shutdown(Shutdown::Read).unwrap();
        tokio::time::timeout(Duration::from_secs(1), driver.drain_pending())
            .await
            .unwrap()
            .unwrap();
        assert!(!driver.in_flight);
        assert_eq!(driver.outstanding, 0);
        // Only now can this allocation safely be reclaimed.
        assert!(driver.pending.take().is_some());
    }

    #[test]
    fn partial_scatter_gather_progress() {
        let bytes = [0u8; 12];
        let mut iovecs = [
            libc::iovec {
                iov_base: bytes.as_ptr().cast_mut().cast(),
                iov_len: 4,
            },
            libc::iovec {
                iov_base: bytes[4..].as_ptr().cast_mut().cast(),
                iov_len: 8,
            },
        ];
        let mut first = 0;
        advance(&mut iovecs, &mut first, 6);
        assert_eq!(first, 1);
        assert_eq!(iovecs[1].iov_len, 6);
        assert_eq!(iovecs[1].iov_base, bytes[6..].as_ptr().cast_mut().cast());
        advance(&mut iovecs, &mut first, 6);
        assert_eq!(first, 2);
    }
}
