//! Thubo is a fast, priority-aware buffered channel for networked applications
//! that require deterministic message handling under load. It provides
//! high-performance, bidirectional communication over arbitrary network streams
//! while supporting strict priority scheduling as part of its Quality of
//! Service (QoS) model. Thubo maintains multiple internal priority queues. The
//! transmission pipeline fragments messages within each priority queue,
//! allowing the sender to interleave fragments from higher-priority messages
//! between fragments of lower-priority messages. This ensures that large,
//! low-priority messages never monopolize the channel and that high-priority
//! data experiences minimal latency even under heavy traffic. The result is
//! predictable responsiveness, reduced head-of-line blocking, and fine-grained
//! control over congestion behavior and reliability guarantees.
//!
//! # Architecture
//!
//! The receiver consists of two main components:
//!
//! - **[`Receiver`]**: The user-facing handle for receiving messages in priority order.
//! - **[`ReceiverTask`]**: A background task that manages network I/O, defragmentation, and buffering.
//!
//! # Message Ordering
//!
//! Messages are delivered in priority order (highest priority first). Within a
//! single priority level, messages are delivered in the order they were
//! received. The receiver maintains separate buffers for each priority level to
//! enable efficient priority-based delivery.
//!
//! # Examples
//!
//! ```no_run
//! use thubo::Bytes;
//! use tokio::net::TcpStream;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create a TCP connection
//! let stream = TcpStream::connect("127.0.0.1:8080").await?;
//! let (reader, writer) = stream.into_split();
//!
//! // Create receiver and reader task
//! let (mut receiver, reader_task) = thubo::receiver(reader).build();
//!
//! // Receive messages
//! loop {
//!     let (msg, qos) = receiver.recv().await?;
//!     println!("Received message with priority {:?}", qos.priority());
//! }
//!
//! # Ok(())
//! # }
//! ```

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::{io::AsyncReadExt, select, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    buffers::{Bytes, Chunk},
    pipeline::{
        self,
        rx::{PipelineRxIn, PipelineRxOut, PullConfig, PushConfig, RecvError, SendError},
    },
    protocol::{BatchSize, core::QoS},
    sync::AtomicDuration,
};

// ================================================================================================
// ReceiverTask - Background I/O and Defragmentation Management
// ================================================================================================

/// Statistics tracking for the reader task (only available with the `stats`
/// feature).
#[cfg(feature = "stats")]
struct ReceiverTaskStats {
    /// Number of batches successfully read from the network.
    batches_count: AtomicUsize,
    /// Total number of bytes successfully read from the network.
    bytes_count: AtomicUsize,
    /// Number of messages/batches dropped due to errors or congestion.
    dropped: AtomicUsize,
}

#[cfg(feature = "stats")]
impl ReceiverTaskStats {
    /// Creates a new statistics tracker with all counters initialized to zero.
    fn new() -> Self {
        Self {
            batches_count: AtomicUsize::new(0),
            bytes_count: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        }
    }
}

/// Shared state for the reader task, accessible from the `ReceiverTask` handle.
struct ReceiverTaskInner {
    /// Timeout for network read operations.
    timeout_read: AtomicDuration,
    /// Timeout for network read operations.
    timeout_drop: AtomicDuration,
    /// Timeout for network read operations.
    timeout_block: AtomicDuration,
    /// Timeout for network read operations.
    max_frag_size: AtomicUsize,
    /// Statistics tracking (only available with `stats` feature).
    #[cfg(feature = "stats")]
    stats: ReceiverTaskStats,
}

impl ReceiverTaskInner {
    fn push_config(&self) -> PushConfig {
        PushConfig {
            frag_max_size: self.max_frag_size.load(Ordering::Relaxed),
            timeout_drop: self.timeout_drop.load(Ordering::Relaxed),
            timeout_block: self.timeout_block.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of reader task statistics.
///
/// This struct is returned by [`ReceiverTask::get_stats`] when the `stats`
/// feature is enabled. It provides insight into the reception performance and
/// health.
#[cfg(feature = "stats")]
#[non_exhaustive]
pub struct ReceiverStats {
    /// Number of batches successfully received.
    pub batches: usize,
    /// Total bytes successfully received.
    pub bytes: usize,
    /// Number of messages/batches dropped.
    pub dropped: usize,
}

/// Background task handle for managing network I/O and message defragmentation.
///
/// The `ReceiverTask` runs in a separate async task and handles:
/// - Reading batches from the network with timeout protection
/// - Decoding and routing messages to priority queues
/// - Automatic defragmentation of large messages split across batches
/// - Keep-alive message processing
/// - Graceful shutdown
///
/// # Type Parameters
///
/// * `R` - The async reader type (typically `tokio::net::tcp::OwnedReadHalf`)
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
/// # use tokio::net::TcpStream;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
/// # let (reader, _) = stream.into_split();
/// # let (mut receiver, receiver_task) = thubo::receiver(reader).build();
///
/// // Configure the reader task
/// receiver_task.set_read_timeout(Duration::from_secs(5));
///
/// // Check statistics (requires `stats` feature)
/// #[cfg(feature = "stats")]
/// {
///     let stats = receiver_task.get_stats();
///     println!("Received {} batches, {} bytes", stats.batches, stats.bytes);
/// }
///
/// // Gracefully stop the task
/// let reader = receiver_task.stop().await?;
///
/// # Ok(())
/// # }
/// ```
pub struct ReceiverTask<R> {
    /// Join handle for the background reader task.
    handle: JoinHandle<R>,
    /// Cancellation token for coordinating shutdown.
    token: CancellationToken,
    /// Shared state for configuration and statistics.
    inner: Arc<ReceiverTaskInner>,
}

impl<R> ReceiverTask<R> {
    /// Sets the timeout for network read operations.
    ///
    /// If a read operation takes longer than this timeout, it will be aborted
    /// and the reader task will terminate. This prevents hanging on slow or
    /// unresponsive network connections.
    ///
    /// # Default
    ///
    /// The default read timeout is 10 seconds.
    pub fn set_read_timeout(&self, timeout: Duration) {
        self.inner.timeout_read.store(timeout, Ordering::Relaxed);
    }

    #[cfg(feature = "stats")]
    /// Retrieves current reception statistics.
    ///
    /// This method is only available when the `stats` feature is enabled.
    /// It provides a snapshot of the reader task's performance metrics.
    ///
    /// # Returns
    ///
    /// A [`ReceiverStats`] struct containing:
    /// - Number of batches received
    /// - Total bytes received
    /// - Number of messages/batches dropped
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// # let (mut receiver, receiver_task) = thubo::receiver(reader).build();
    ///
    /// #[cfg(feature = "stats")]
    /// {
    ///     let stats = receiver_task.get_stats();
    ///     println!(
    ///         "Batches: {}, Bytes: {}, Dropped: {}",
    ///         stats.batches, stats.bytes, stats.dropped
    ///     );
    /// }
    ///
    /// # Ok(())
    /// # }
    /// ```    
    pub fn get_stats(&self) -> ReceiverStats {
        ReceiverStats {
            batches: self.inner.stats.batches_count.load(Ordering::Relaxed),
            bytes: self.inner.stats.bytes_count.load(Ordering::Relaxed),
            dropped: self.inner.stats.dropped.load(Ordering::Relaxed),
        }
    }

    /// Stops the reader task and returns a handle to await its completion.
    ///
    /// This method signals the reader task to terminate and returns a
    /// `JoinHandle` that can be awaited to retrieve the underlying reader.
    ///
    /// The reader task will:
    /// 1. Stop reading new data from the network
    /// 2. Return the underlying reader
    ///
    /// # Returns
    ///
    /// A `JoinHandle<R>` that resolves to the underlying reader when the task
    /// completes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// # let (mut receiver, receiver_task) = thubo::receiver(reader).build();
    /// // Stop the reader task and wait for completion
    /// let join_handle = receiver_task.stop();
    /// let reader = join_handle.await?;
    /// println!("Reader task stopped, underlying reader recovered");
    /// # Ok(())
    /// # }
    /// ```
    pub fn stop(self) -> JoinHandle<R> {
        let Self { handle, token, inner } = self;
        token.cancel();
        // Set timeout to zero to immediately abort any pending reads
        inner.timeout_read.store(Duration::from_secs(0), Ordering::Relaxed);
        handle
    }
}

pub struct Receiver {
    /// Internal pipeline sender handle.
    inner: PipelineRxOut,
    /// Internal config
    config: PullConfig,
    /// Cancellation token for coordinating shutdown.
    token: CancellationToken,
}

impl Receiver {
    /// Receives the next message from the network in strict priority order.
    ///
    /// This method blocks until a message is available or an error occurs. Messages are
    /// delivered in strict priority order: Control (0) → RealTime (1) → ... → Background (7).
    /// Within each priority level, messages are delivered in FIFO order.
    ///
    /// # Returns
    ///
    /// * `Ok((Bytes, QoS))` - The received message and its QoS metadata (priority, congestion control)
    /// * `Err(RecvError::Timeout)` - No message received within the configured timeout
    /// * `Err(RecvError::DecodingFailed)` - Message data was corrupted or invalid
    /// * `Err(RecvError::Closed)` - Connection closed, no more messages will arrive
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// # let (mut receiver, task) = thubo::receiver(reader).build();
    /// // Receive messages in priority order
    /// loop {
    ///     match receiver.recv().await {
    ///         Ok((msg, qos)) => {
    ///             println!("Received message with priority {:?}", qos.priority());
    ///             // Process message...
    ///         }
    ///         Err(e) => {
    ///             eprintln!("Receive error: {}", e);
    ///             break;
    ///         }
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn recv(&mut self) -> Result<(Bytes, QoS), RecvError> {
        self.inner.pull(&self.config).await
    }

    /// Attempts to receive a message without blocking.
    ///
    /// This method checks if a message is immediately available in the priority queues.
    /// If a message is available, it returns it. Otherwise, it returns `None` immediately
    /// without waiting.
    ///
    /// This is useful for non-blocking receive patterns or when you want to poll for
    /// messages without being blocked by the timeout.
    ///
    /// # Returns
    ///
    /// * `Some((Bytes, QoS))` - A message was immediately available
    /// * `None` - No messages currently available (queues are empty)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// # let (mut receiver, task) = thubo::receiver(reader).build();
    /// // Non-blocking receive pattern
    /// loop {
    ///     if let Some((msg, qos)) = receiver.try_recv().await {
    ///         println!("Got message: {:?}", msg);
    ///     } else {
    ///         // Do other work while waiting for messages
    ///         tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn try_recv(&mut self) -> Option<(Bytes, QoS)> {
        self.inner.try_pull()
    }

    /// Stops the receiver and signals the background task to shut down.
    ///
    /// This method cancels the background [`ReceiverTask`], causing it to stop reading
    /// from the network. After calling this method, no more messages will be received.
    ///
    /// This is a graceful shutdown - the task will finish processing any in-flight
    /// operations before terminating.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// # let (receiver, task) = thubo::receiver(reader).build();
    /// // Receive some messages...
    /// // ...
    ///
    /// // Shut down gracefully
    /// receiver.stop().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stop(self) {
        self.token.cancel();
    }

    /// Checks if the receiver has been closed.
    ///
    /// Returns `true` if either:
    /// - [`stop()`](`Self::stop`) was called to shut down the receiver
    /// - The background task stopped due to connection loss or error
    ///
    /// Once closed, no more messages can be received.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// # let (mut receiver, task) = thubo::receiver(reader).build();
    /// if receiver.is_closed() {
    ///     println!("Receiver is closed, no more messages will arrive");
    /// } else {
    ///     println!("Receiver is active");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn is_closed(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Sets the timeout for waiting for messages.
    ///
    /// This timeout determines how long [`recv()`](`Self::recv`) will wait for a message
    /// to arrive before returning [`RecvError::Timeout`]. The timeout applies to each
    /// individual `recv()` call.
    ///
    /// Default: 10 seconds
    ///
    /// # Returns
    ///
    /// Returns `&mut self` for method chaining.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # use std::time::Duration;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// # let (mut receiver, task) = thubo::receiver(reader).build();
    /// // Set a 30-second timeout
    /// receiver.timeout(Duration::from_secs(30));
    ///
    /// // Method chaining
    /// let result = receiver.timeout(Duration::from_secs(5)).recv().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
        self.config.timeout = timeout;
        self
    }
}

/// Builder for configuring and creating a [`Receiver`] and [`ReceiverTask`].
///
/// This builder allows you to customize the reception pipeline parameters before
/// spawning the background task that handles network I/O and message reassembly.
///
/// # Parameters
///
/// - **capacity**: Maximum number of messages that can be buffered per priority level (default: 16)
/// - **max_frag_size**: Maximum size of a message fragment in bytes (default: 1GiB)
pub struct ReceiverBuilder<R>
where
    R: AsyncReadExt + Send + Sync + Unpin + 'static,
{
    capacity: usize,
    timeout: Duration,
    max_frag_size: usize,
    reader: R,
}

impl<R> ReceiverBuilder<R>
where
    R: AsyncReadExt + Send + Sync + Unpin + 'static,
{
    /// Sets the maximum number of messages that can be buffered per priority level.
    ///
    /// Default: 16 messages per priority level
    ///
    /// Higher values allow for more buffering during traffic bursts but consume more memory.
    /// Lower values reduce memory usage but may cause messages to be dropped under congestion.
    #[must_use]
    pub fn capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Sets the timeout for waiting for messages during receive operations.
    ///
    /// This timeout determines how long [`Receiver::recv()`] will wait for a message to
    /// arrive before returning [`RecvError::Timeout`]. The timeout applies to each
    /// individual receive call.
    ///
    /// This is the initial timeout value used when the receiver is built. It can be changed
    /// later using [`Receiver::timeout()`].
    ///
    /// Default: 10 seconds
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::net::TcpStream;
    /// # use std::time::Duration;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (reader, _) = stream.into_split();
    /// // Configure a 30-second timeout during construction
    /// let (receiver, task) = thubo::receiver(reader).timeout(Duration::from_secs(30)).build();
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the maximum size of a message fragment in bytes.
    ///
    /// Default: 65536 bytes (64KB)
    ///
    /// This should match the `max_frag_size` configured on the sender side. Messages larger
    /// than this size will be automatically fragmented by the sender and reassembled by the
    /// receiver. Smaller values reduce latency for small messages but increase overhead for
    /// large messages.
    #[must_use]
    pub fn max_frag_size(mut self, max_frag_size: usize) -> Self {
        self.max_frag_size = max_frag_size;
        self
    }

    /// Builds and returns a [`Receiver`] and [`ReceiverTask`].
    ///
    /// This method creates the complete reception pipeline:
    /// - **Receiver**: Handle for receiving messages in strict priority order
    /// - **ReceiverTask**: Background task that manages network I/O, defragmentation, and message routing
    ///
    /// # Background Task
    ///
    /// The returned [`ReceiverTask`] runs until:
    /// - The connection is closed (EOF or network error)
    /// - A read timeout occurs
    /// - The task is explicitly stopped via [`ReceiverTask::stop()`]
    #[must_use]
    pub fn build(self) -> (Receiver, ReceiverTask<R>) {
        let Self {
            capacity,
            max_frag_size,
            timeout,
            reader,
        } = self;

        // Initialize shared state for the reader task
        let token = CancellationToken::new();
        let c_token = token.clone();
        let inner = Arc::new(ReceiverTaskInner {
            timeout_read: AtomicDuration::new(Duration::from_secs(10)),
            timeout_drop: AtomicDuration::new(Duration::from_millis(500)),
            timeout_block: AtomicDuration::new(Duration::from_secs(60)),
            max_frag_size: AtomicUsize::new(max_frag_size),
            #[cfg(feature = "stats")]
            stats: ReceiverTaskStats::new(),
        });
        let c_inner = inner.clone();

        // Create the priority-aware receiver pipeline
        let (sender, receiver) = pipeline::rx::pipeline_rx(capacity);

        // Spawn the background reader task
        let handle = tokio::spawn(read_task(sender, reader, c_inner, c_token));

        let receiver = Receiver {
            inner: receiver,
            config: PullConfig { timeout },
            token: token.clone(),
        };
        let reader = ReceiverTask { handle, token, inner };
        (receiver, reader)
    }
}

async fn read_task<R>(
    mut sender: PipelineRxIn,
    mut reader: R,
    inner: Arc<ReceiverTaskInner>,
    token: CancellationToken,
) -> R
where
    R: AsyncReadExt + Send + Sync + Unpin + 'static,
{
    // Main reception loop
    loop {
        let read_timeout = inner.timeout_read.load(Ordering::Relaxed);

        // Helper macro for reading with timeout and cancellation support
        macro_rules! read_all {
            ($buff:expr) => {{
                let res = select! {
                    // Try to read with timeout
                    res = tokio::time::timeout(read_timeout, reader.read_exact($buff)) => res,
                    // Check if shutdown was requested
                    _ = token.cancelled() => break,
                };
                match res {
                    Ok(Ok(read)) if read == 0 => break, // EOF - connection closed
                    Ok(Err(_)) | Err(_) => break,       // Network error or timeout
                    _ => {}
                }
            }};
        }

        // Read the 2-byte length header (little-endian)
        let mut len = BatchSize::MIN.to_le_bytes();
        read_all!(&mut len);
        let len = BatchSize::from_le_bytes(len) as usize;
        if len == 0 {
            continue; // Empty batch - skip
        }

        // Allocate buffer with next power of two to reduce memory fragmentation
        let mut buf = vec![0u8; len.next_power_of_two()];
        // Read the batch data
        read_all!(&mut buf[..len]);

        // Update statistics
        #[cfg(feature = "stats")]
        {
            inner.stats.batches_count.fetch_add(1, Ordering::Relaxed);
            inner.stats.bytes_count.fetch_add(len, Ordering::Relaxed);
        }

        let res = sender
            .push(
                Bytes::single(unsafe {
                    // SAFETY: The invariants for `Chunk::new_unchecked` are satisfied:
                    // 1. start (0) <= end (len): trivially true since len >= 0
                    // 2. end (len) <= buf.len(): buf is allocated with size len.next_power_of_two() which is >= len,
                    //    ensuring the range [0..len] is within bounds
                    // 3. The data in buf[0..len] has been initialized by the read_all! macro above
                    Chunk::new_unchecked(Arc::new(buf), 0, len)
                }),
                &inner.push_config(),
            )
            .await;

        // Handle send result
        match res {
            Ok(true) => {
                // Successfully sent
            }
            Ok(false) => {
                // Message was dropped due to congestion
                #[cfg(feature = "stats")]
                inner.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => match e {
                SendError::InvalidSeqNum | SendError::InvalidFragId | SendError::CapacityLimit | SendError::Timeout => {
                    // Message was dropped due to errors
                    #[cfg(feature = "stats")]
                    inner.stats.dropped.fetch_add(1, Ordering::Relaxed);
                }
                SendError::DecodingFailed | SendError::ChannelClosed | SendError::InternalError => break,
            },
        }
    }

    // Return the underlying reader
    reader
}

/// Creates a new receiver for receiving messages from an async reader.
///
/// This function sets up a complete reception pipeline consisting of:
/// - A [`Receiver`] handle for receiving messages in priority order
/// - A [`ReceiverTask`] running in the background to manage network I/O
///
/// The reader task automatically handles:
/// - Reading batches from the network with configurable timeouts
/// - Decoding message headers and routing to priority queues
/// - Defragmenting large messages split across multiple batches
/// - Processing keep-alive messages
/// - Graceful shutdown
///
/// # Type Parameters
///
/// * `R` - An async reader implementing `AsyncReadExt + Send + Sync + Unpin + 'static`
///
/// # Returns
///
/// A tuple of:
/// - `Receiver` - The message receiving handle for priority-based message delivery
/// - `ReceiverTask<R>` - The background task handle for configuration and monitoring
///
/// # Protocol
///
/// Each batch is read with a 2-byte length header (little-endian `u16`)
/// followed by the batch data. The reader automatically handles:
/// - Frame messages (complete messages in a single batch)
/// - Fragment messages (parts of large messages split across batches)
/// - Keep-alive messages (discarded after processing)
///
/// # Defragmentation
///
/// Large messages are automatically reassembled from fragments. The reader
/// maintains separate defragmentation buffers for each priority level and
/// congestion control mode (16 total buffers: 8 priorities × 2 congestion
/// modes).
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use tokio::net::TcpStream;
///
/// // Create a TCP connection
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let stream = TcpStream::connect("127.0.0.1:8080").await?;
/// let (reader, writer) = stream.into_split();
///
/// // Create the receiver and reader task
/// let (mut receiver, reader_task) = thubo::receiver(reader).build();
///
/// // Configure the reader task
/// reader_task.set_read_timeout(Duration::from_secs(5));
///
/// // Receive messages
/// loop {
///     let (msg, qos) = receiver.recv().await?;
///     println!("Received message at priority {:?}", qos.priority());
/// }
/// # Ok(())
/// # }
/// ```
#[must_use]
pub fn receiver<R>(reader: R) -> ReceiverBuilder<R>
where
    R: AsyncReadExt + Send + Sync + Unpin + 'static,
{
    ReceiverBuilder {
        capacity: 16,
        max_frag_size: 1 << 30, // 1 GiB
        timeout: Duration::MAX,
        reader,
    }
}
