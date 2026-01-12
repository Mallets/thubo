//! High-level sender API for transmitting messages over the network.
//!
//! This module provides the main [`Sender`] interface for sending messages
//! through Thubo's priority-aware transmission pipeline. Messages are
//! automatically batched, fragmented if needed, and transmitted with
//! configurable Quality of Service (QoS) guarantees.
//!
//! # Architecture
//!
//! The sender consists of two main components:
//!
//! - **[`Sender`]**: The user-facing handle for sending messages with priority and QoS control.
//! - **[`SenderTask`]**: A background task that manages network I/O, batching, and keep-alive.
#[cfg(feature = "stats")]
use std::sync::atomic::AtomicUsize;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use tokio::{io::AsyncWriteExt, select, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    buffers::Bytes,
    pipeline::{
        status,
        tx::{PipelineTxIn, PipelineTxOut, PullConfig, PushConfig, SendError},
    },
    protocol::{BatchSize, QoS},
    sync::AtomicDuration,
};

/// High-level sender for transmitting messages over the network.
///
/// The `Sender` provides a convenient interface for sending messages through
/// Thubo's multi-priority transmission pipeline. It handles message
/// serialization, batching, fragmentation, and QoS enforcement automatically.
///
/// # Cloning
///
/// `Sender` is cheaply cloneable (uses [`Arc`] internally). Multiple clones can
/// send messages concurrently from different tasks.
///
/// # Quality of Service
///
/// Each sender has configurable QoS settings that control:
/// - **Priority**: Message importance (8 levels from Control to Background)
/// - **Congestion Control**: Behavior when buffers are full (Block or Drop)
/// - **Express Mode**: Whether to batch messages or send immediately
///
/// # Examples
///
/// ```no_run
/// # use thubo::{Bytes, Priority, CongestionControl, QoS};
/// # use tokio::net::TcpStream;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
/// # let (_, writer) = stream.into_split();
/// # let (mut sender, sender_task) = thubo::sender(writer).build();
/// // Configure sender for high-priority, real-time messages
/// sender.qos(
///     QoS::default()
///         .with_priority(Priority::High)
///         .with_congestion_control(CongestionControl::Block)
///         .with_express(true),
/// );
/// let msg = Bytes::from(vec![1, 2, 3, 4]);
/// sender.send(&msg).await?;
///
/// // Check if the sender is experiencing congestion
/// if sender.is_congested() {
///     println!("Warning: sender is congested");
/// }
/// # Ok(())
/// # }
#[derive(Clone)]
pub struct Sender {
    /// Internal pipeline sender handle.
    inner: PipelineTxIn,
    /// Sender config
    config: PushConfig,
    /// Cancellation token for coordinating shutdown.
    token: CancellationToken,
}

impl Sender {
    /// Configures the Quality of Service settings for this sender.
    ///
    /// The QoS settings determine how messages are prioritized, batched, and
    /// handled when the pipeline is congested. This affects all subsequent
    /// [`send()`](`Sender::send`) calls.
    pub fn qos(&mut self, qos: QoS) -> &mut Self {
        self.config.qos = qos;
        self
    }

    /// Sets the overall deadline for the entire send operation.
    ///
    /// This is the maximum total time that [`send()`](`Sender::send`) will attempt to
    /// serialize and queue a message. If the message cannot be successfully queued within
    /// this time, the operation fails with [`SendError::Timeout`].
    ///
    /// The timeout is checked periodically based on the [`timeout_progress`](`Self::timeout_progress`)
    /// interval. When no progress can be made (e.g., batch pool is empty), the operation
    /// waits up to the progress timeout before checking if the overall deadline has been exceeded.
    ///
    /// Default: 10 seconds
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use thubo::Bytes;
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (_, writer) = stream.into_split();
    /// # let (mut sender, sender_task) = thubo::sender(writer).build();
    /// // Fail if message can't be sent within 5 seconds
    /// sender.timeout(Duration::from_secs(5));
    ///
    /// // Method chaining
    /// let msg = Bytes::from(vec![42; 42]);
    /// sender.timeout(Duration::from_secs(5)).send(&msg).await?;
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
        self.config.timeout = timeout;
        self
    }

    /// Sets the progress check interval when the send operation stalls.
    ///
    /// When the batch pool is temporarily empty (no batches available for serialization),
    /// [`send()`](`Sender::send`) will wait up to this duration for a batch to become available.
    /// After this timeout, it checks whether the overall [`timeout`](`Self::timeout`) has been
    /// exceeded. If not, it waits again for up to this duration.
    ///
    /// This creates a progress-checking loop:
    /// 1. Try to acquire a batch
    /// 2. If unavailable, wait up to `timeout_progress` duration
    /// 3. Check if overall `timeout` exceeded → return error if yes
    /// 4. Otherwise, repeat from step 1
    ///
    /// **Tuning guidance:**
    /// - **Too high** (e.g., 5s): Send operations become unresponsive to the overall timeout
    /// - **Too low** (e.g., 1ms): Causes busy-waiting and unnecessary CPU usage
    /// - **Recommended**: 100-500ms for most applications
    ///
    /// Default: 100ms
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use thubo::Bytes;
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (_, writer) = stream.into_split();
    /// # let (mut sender, sender_task) = thubo::sender(writer).build();
    ///
    /// // Check for progress every 100ms
    /// sender.timeout_progress(Duration::from_millis(100));
    ///
    /// // Typical configuration
    /// let msg = Bytes::from(vec![42; 42]);
    /// sender
    ///     .timeout(Duration::from_secs(5)) // Give up after 5 seconds
    ///     .timeout_progress(Duration::from_millis(100)) // Check progress every 100ms
    ///     .send(&msg)
    ///     .await?;
    ///
    /// # Ok(())
    /// # }
    /// ```
    pub fn timeout_progress(&mut self, wait: Duration) -> &mut Self {
        self.config.timeout_batch = wait;
        self
    }

    /// Checks if the sender's priority queue is currently congested.
    ///
    /// A queue is considered congested when it cannot keep up with incoming
    /// messages and has run out of available buffers. When congested:
    /// - Messages with [`CongestionControl::Drop`](`crate::CongestionControl::Drop`) are immediately dropped
    /// - Messages with [`CongestionControl::Block`](`crate::CongestionControl::Block`) wait (up to the configured
    ///   timeout)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use thubo::{CongestionControl, Priority, QoS};
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (_, writer) = stream.into_split();
    /// # let (mut sender, sender_task) = thubo::sender(writer).build();
    /// if sender.is_congested() {
    ///     // Switch to a lower priority or handle congestion
    ///     sender.qos(
    ///         QoS::default()
    ///             .with_priority(Priority::Background)
    ///             .with_congestion_control(CongestionControl::Drop),
    ///     );
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn is_congested(&self) -> bool {
        self.inner.status().any(status::congested(self.config.qos.priority()))
    }

    /// Checks if the sender has been closed.
    ///
    /// A sender is closed when [`stop()`](Sender::stop) has been called or the associated
    /// [`SenderTask`] has terminated. Once closed, all `send()` calls will fail.
    pub fn is_closed(&self) -> bool {
        // self.inner.status.is_disabled()
        false
    }

    /// Asynchronously sends a message through the transmission pipeline.
    ///
    /// The message is serialized, batched, and queued for transmission
    /// according to the configured QoS settings. Large messages are
    /// automatically fragmented across multiple batches if needed.
    ///
    /// # Errors
    ///
    /// - [`SendError::Dropped`]: Message was dropped due to congestion
    /// - [`SendError::Timeout`]: Timed out waiting for buffer space
    /// - [`SendError::EncodingFailed`]: Message serialization failed
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use thubo::{Bytes, SendError};
    /// # use std::time::Duration;
    /// # use tokio::net::TcpStream;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let stream = TcpStream::connect("127.0.0.1:8080").await?;
    /// # let (_, writer) = stream.into_split();
    /// # let (mut sender, sender_task) = thubo::sender(writer).build();
    /// #
    /// let msg = Bytes::from(vec![1, 2, 3, 4]);
    /// match sender.send(&msg).await {
    ///     Ok(()) => println!("Message sent successfully"),
    ///     Err(SendError::Dropped) => println!("Message dropped (congestion)"),
    ///     Err(SendError::Timeout) => println!("Timeout waiting for buffers"),
    ///     Err(SendError::EncodingFailed) => println!("Serialization failed"),
    ///     Err(SendError::Closed) => println!("Sender task is stopped"),
    ///     Err(SendError::Internal) => println!("Internal error"),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Performance note
    ///
    /// Passing a reference (`&Bytes`) is significantly more efficient than passing by value and can impact
    /// performance by up to few million msg/s in case of very small messages (e.g. 8 bytes payload). References avoid
    /// cloning reference counters and allow the message to be reused immediately after the call returns.
    ///
    /// However, **passing ownership is required** when using [`OnDrop`](`crate::collections::OnDrop`) or
    /// [`Boomerang`](`crate::collections::Boomerang`) wrappers for buffer lifecycle management (e.g., returning
    /// buffers to a pool or tracking when transmission completes).
    pub async fn send<T>(&mut self, msg: T) -> Result<(), SendError>
    where
        T: AsRef<Bytes>,
    {
        self.inner.push(msg.as_ref(), &self.config).await
    }

    /// Gracefully shuts down the sender and associated writer task.
    ///
    /// This method:
    /// 1. Disables the transmission pipeline (rejecting new messages)
    /// 2. Waits for in-flight serialization to complete
    /// 3. Signals the writer task to drain remaining batches and terminate
    ///
    /// After calling [`stop()`](Sender::stop), the sender is consumed and cannot be used again.
    /// The writer task will finish transmitting any buffered data before
    /// exiting.
    pub async fn stop(self) {
        self.inner.disable().await;
        self.token.cancel();
    }
}

// ================================================================================================
// SenderTask - Background I/O
// ================================================================================================

/// Statistics tracking for the writer task (only available with the `stats`
/// feature).
#[cfg(feature = "stats")]
struct SenderTaskStats {
    /// Number of batches successfully written to the network.
    batches_count: AtomicUsize,
    /// Total number of bytes successfully written to the network.
    bytes_count: AtomicUsize,
    /// Number of messages/batches dropped due to errors or congestion.
    dropped: AtomicUsize,
}

#[cfg(feature = "stats")]
impl SenderTaskStats {
    /// Creates a new statistics tracker with all counters initialized to zero.
    fn new() -> Self {
        Self {
            batches_count: AtomicUsize::new(0),
            bytes_count: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        }
    }
}

/// Shared state for the writer task, accessible from the `SenderTask` handle.
struct SenderTaskInner {
    /// Timeout for network write operations.
    write_timeout: AtomicDuration,
    /// Timeout before sending a keep alive.
    keep_alive: AtomicDuration,
    /// Max batch duration
    max_batch_age: AtomicDuration,
    /// Statistics tracking (only available with `stats` feature).
    #[cfg(feature = "stats")]
    stats: SenderTaskStats,
}

/// Snapshot of writer task statistics.
///
/// This struct is returned by [`SenderTask::get_stats`] when the `stats`
/// feature is enabled. It provides insight into the transmission performance
/// and health.
#[cfg(feature = "stats")]
#[non_exhaustive]
pub struct SenderStats {
    /// Number of batches successfully transmitted.
    pub batches: usize,
    /// Total bytes successfully transmitted.
    pub bytes: usize,
    /// Number of messages/batches dropped.
    pub dropped: usize,
}

/// Background task handle for managing network I/O and keep-alive messages.
///
/// The [`SenderTask`] runs in a separate async task and handles:
/// - Pulling serialized batches from the transmission pipeline
/// - Writing batches to the network with timeout protection
/// - Sending periodic keep-alive messages when idle
/// - Graceful shutdown and draining of remaining data
pub struct SenderTask<W> {
    /// Join handle for the background writer task.
    handle: JoinHandle<W>,
    /// Cancellation token for coordinating shutdown.
    token: CancellationToken,
    /// Shared state for configuration and statistics.
    inner: Arc<SenderTaskInner>,
}

impl<W> SenderTask<W> {
    /// Sets the timeout for network write operations.
    ///
    /// If a write operation takes longer than this timeout, it will be aborted
    /// and the writer task will terminate. This prevents hanging on slow or
    /// unresponsive network connections.
    ///
    /// # Default
    ///
    /// The default write timeout is 10 seconds.
    pub fn set_write_timeout(&self, timeout: Duration) {
        self.inner.write_timeout.store(timeout, Ordering::Relaxed);
    }

    /// Sets the interval for keep alive operations.
    ///
    /// The interval at which sending keep alive messages if no data is being sent.
    /// No keep alive messages are sent if user data is being transmitted in the interval.
    ///
    /// # Default
    ///
    /// The default keep alive interval is 3 seconds.
    pub fn set_keep_alive(&self, interval: Duration) {
        self.inner.keep_alive.store(interval, Ordering::Relaxed);
    }

    pub fn set_max_batch_age(&self, age: Duration) {
        self.inner.max_batch_age.store(age, Ordering::Relaxed);
    }

    /// Retrieves current transmission statistics.
    #[cfg(feature = "stats")]
    pub fn get_stats(&self) -> SenderStats {
        SenderStats {
            batches: self.inner.stats.batches_count.load(Ordering::Relaxed),
            bytes: self.inner.stats.bytes_count.load(Ordering::Relaxed),
            dropped: self.inner.stats.dropped.load(Ordering::Relaxed),
        }
    }

    /// Stops the writer task and returns a handle to await its completion.
    ///
    /// This method signals the writer task to terminate and returns a
    /// `JoinHandle` that can be awaited to retrieve the underlying writer
    /// once all data has been flushed.
    ///
    /// The writer task will:
    /// 1. Stop pulling new batches
    /// 2. Drain and transmit any remaining buffered data
    /// 3. Return the underlying writer
    pub fn stop(self) -> JoinHandle<W> {
        let Self {
            handle,
            token,
            inner: _,
        } = self;
        token.cancel();
        handle
    }
}

/// Builder for configuring and creating a [`Sender`] and [`SenderTask`].
///
/// This builder allows you to customize the reception pipeline parameters before
/// spawning the background task that handles network I/O and message reassembly.
pub struct SenderBuilder<R>
where
    R: AsyncWriteExt + Send + Sync + Unpin + 'static,
{
    qos: QoS,
    capacity: usize,
    batch_size: BatchSize,
    timeout: Duration,
    timeout_progress: Duration,
    timeout_batch: Duration,
    writer: R,
}

impl<R> SenderBuilder<R>
where
    R: AsyncWriteExt + Send + Sync + Unpin + 'static,
{
    /// Sets the QoS paramters.
    #[must_use]
    pub fn qos(mut self, qos: QoS) -> Self {
        self.qos = qos;
        self
    }

    /// Sets the maximum number of batches that can be buffered per priority level.
    ///
    /// Default: 16 batches per priority level
    ///
    /// Higher values allow for more buffering during traffic bursts but consume more memory.
    /// Lower values reduce memory usage but may cause messages to be dropped under congestion.
    #[must_use]
    pub fn capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Sets the batch size.
    ///
    /// Default: 65_535 bytes (max: 65_535 bytes)
    #[must_use]
    pub fn batch_size(mut self, batch_size: BatchSize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Sets the timeout for sending a complete message.
    ///
    /// This is the maximum time allowed for the entire message push operation,
    /// including waiting for batch availability, serialization, and queuing.
    /// If this timeout is exceeded, the send operation will fail with
    /// [`SendError::Timeout`](crate::SendError::Timeout).
    ///
    /// Default: 60 seconds
    ///
    /// # Parameters
    ///
    /// - `timeout`: Maximum duration to wait for sending a complete message
    ///
    /// # Notes
    ///
    /// This timeout applies to the entire push operation from the application's
    /// perspective. For finer-grained control over batch acquisition, use
    /// [`timeout_batch()`](Self::timeout_batch).
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the maximum age for a batch before forcing a flush.
    ///
    /// To improve throughput, messages are batched together before transmission.
    /// However, batching introduces latency - messages sit in a batch waiting
    /// for more messages to arrive. This timeout controls the maximum time a
    /// batch can remain unbatched (booked but not committed) before being
    /// forcibly flushed to the network.
    ///
    /// Default: 10 microseconds
    ///
    /// # Parameters
    ///
    /// - `timeout_batch`: Maximum age of a batch before forcing transmission
    ///
    /// # Behavior
    ///
    /// The transmission pipeline monitors batches that are actively being populated.
    /// When the batch age exceeds this timeout, the batch is flushed. Additionally,
    /// if no changes are detected between subsequent checks (i.e., the batch is no
    /// longer being actively written to), the batch is transmitted immediately
    /// regardless of its age. This ensures that batches don't sit idle waiting for
    /// more data that may never arrive.
    ///
    /// # Tuning Guidance
    ///
    /// - **Lower values** (e.g., 1-10μs): Reduce latency but may hurt throughput due to smaller batches being sent more
    ///   frequently
    /// - **Higher values** (e.g., 100μs-1ms): Improve throughput by allowing fuller batches, but increase latency for
    ///   messages waiting in the batch
    ///
    /// # Notes
    ///
    /// This setting only affects non-express messages. Messages sent with
    /// express mode bypass batching and are transmitted immediately regardless
    /// of this timeout.
    #[must_use]
    pub fn timeout_batch(mut self, timeout_batch: Duration) -> Self {
        self.timeout_batch = timeout_batch;
        self
    }

    /// Builds and returns a [`Sender`] and [`SenderTask`].
    ///
    /// This method creates the complete reception pipeline:
    /// - **Sender**: Handle for receiving messages in strict priority order
    /// - **SenderTask**: Background task that manages network I/O, defragmentation, and message routing
    ///
    /// # Background Task
    ///
    /// The returned [`SenderTask`] runs until:
    /// - The connection is closed (EOF or network error)
    /// - A read timeout occurs
    /// - The task is explicitly stopped via [`SenderTask::stop()`]
    #[must_use]
    pub fn build(self) -> (Sender, SenderTask<R>) {
        let Self {
            qos,
            capacity,
            batch_size,
            timeout,
            timeout_progress,
            timeout_batch: max_batch_age,
            writer,
        } = self;
        // Create the multi-priority transmission pipeline
        let (pipeline_in, pipeline_out) = crate::pipeline::tx::pipeline_tx(capacity, batch_size);

        // Initialize shared state for the writer task
        let token = CancellationToken::new();
        let c_token = token.clone();
        let inner = Arc::new(SenderTaskInner {
            write_timeout: AtomicDuration::new(Duration::from_secs(10)),
            max_batch_age: AtomicDuration::new(max_batch_age),
            keep_alive: AtomicDuration::new(Duration::from_secs(3)),
            #[cfg(feature = "stats")]
            stats: SenderTaskStats::new(),
        });
        let c_inner = inner.clone();

        // Spawn the background reader task
        let handle = tokio::spawn(write_task(pipeline_out, writer, c_inner, c_token));

        let sender = Sender {
            inner: pipeline_in,
            config: PushConfig {
                qos,
                timeout,
                timeout_batch: timeout_progress,
            },
            token: token.clone(),
        };
        let writer = SenderTask { handle, token, inner };
        (sender, writer)
    }
}

async fn write_task<W>(
    mut pipeline_out: PipelineTxOut,
    mut writer: W,
    inner: Arc<SenderTaskInner>,
    token: CancellationToken,
) -> W
where
    W: AsyncWriteExt + Send + Sync + Unpin + 'static,
{
    // Helper macro for writing data to the network with timeout protection
    macro_rules! write_batch {
        ($batch:expr) => {{
            let timeout = inner.write_timeout.load(Ordering::Relaxed);
            tokio::time::timeout(timeout, async {
                let mut written = 0;

                // Write the frame header (contains batch size and header)
                let frame = $batch.frame.as_slice();
                writer.write_all(frame).await?;
                written += frame.len();

                // Write each fragment slice in the batch (the actual message data)
                // This is no-op in case the message is not fragmented
                for s in $batch.fragment.slices() {
                    writer.write_all(s).await?;
                    written += s.len();
                }

                // Flush
                writer.flush().await?;

                // Return the batch buffer to the pool for reuse
                pipeline_out.refill($batch);
                Ok::<usize, std::io::Error>(written)
            })
            .await
        }};
    }

    // Main transmission loop
    loop {
        // Wait for a batch to become available or keep-alive timeout to expire
        let keep_alive = inner.keep_alive.load(Ordering::Relaxed);
        let config = PullConfig {
            max_batch_age: inner.max_batch_age.load(Ordering::Relaxed),
        };
        let res = select! {
            // Try to pull a batch with keep-alive timeout
            res = tokio::time::timeout(keep_alive, pipeline_out.pull(&config)) => match res {
                Ok(batch) => batch,
                // Send a keep-alive (0 bytes to read)
                Err(_) => match writer.write_all(&BatchSize::MIN.to_le_bytes()).await {
                    Ok(_) => continue,
                    Err(_) => break,
                },
            },
            // Check if shutdown was requested
            _ = token.cancelled() => {
                break;
            }
        };

        // Handle the pull result
        let batch = match res {
            // Batch is available - transmit it
            Some(batch) => batch,
            // Pipeline is closed - exit gracefully
            None => break,
        };

        // Write the batch to the network
        let Ok(Ok(_written)) = write_batch!(batch) else {
            break; // Network error - terminate
        };

        // Update statistics (only with `stats` feature)
        #[cfg(feature = "stats")]
        {
            inner.stats.batches_count.fetch_add(1, Ordering::Release);
            inner.stats.bytes_count.fetch_add(_written, Ordering::Release);
        }
    }

    // Graceful shutdown: drain and transmit remaining batches
    for batch in pipeline_out.drain().await.drain(..) {
        let Ok(Ok(_)) = write_batch!(batch) else {
            break; // Stop on first error during drain
        };
    }

    // Return the underlying writer
    writer
}

/// Creates a new sender for transmitting messages over an async writer.
///
/// This function sets up a complete transmission pipeline consisting of:
/// - A [`Sender`] handle for sending messages with priority and QoS control
/// - A [`SenderTask`] running in the background to manage network I/O
///
/// The writer task automatically handles:
/// - Pulling serialized batches from the transmission pipeline
/// - Writing batches to the network with configurable timeouts
/// - Sending keep-alive messages during idle periods
/// - Graceful shutdown and draining of buffered data
pub fn sender<W>(writer: W) -> SenderBuilder<W>
where
    W: AsyncWriteExt + Send + Sync + Unpin + 'static,
{
    SenderBuilder {
        qos: QoS::DEFAULT,
        capacity: 16,
        batch_size: BatchSize::MAX,
        timeout: Duration::from_secs(60),
        timeout_progress: Duration::from_secs(10),
        timeout_batch: Duration::from_micros(100),
        writer,
    }
}
